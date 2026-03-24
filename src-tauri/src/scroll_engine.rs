//! Scroll Engine — intercepts discrete mouse scroll events and replaces them
//! with smooth, interpolated pixel-based scroll events using CoreGraphics.
//!
//! Architecture:
//!   Event Tap Thread    →  captures raw scroll events via CGEventTap, suppresses originals
//!   CVDisplayLink       →  fires at display refresh rate, emits interpolated scroll events
//!   Shared state        →  `ScrollSettings` (user config) + `ScrollAnimation` (active anim)
//!
//! Safety:
//!   - Synthetic events tagged via `eventSourceUserData` (field 42) with a sentinel
//!   - Synthetic events posted to `kCGSessionEventTap` (downstream of our HID-level tap)
//!   - `std::sync::Mutex` used for macOS priority inheritance support
//!   - CVDisplayLink callback runs on a high-priority CoreVideo thread, synced to display
//!
//! Lock ordering (must always be acquired in this order to prevent deadlocks):
//!   1. settings
//!   2. animation

use core_foundation::runloop::kCFRunLoopCommonModes;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ─── CoreGraphics FFI ────────────────────────────────────────────────

mod cg {
    use std::ffi::c_void;

    pub type EventTapProxy = *const c_void;
    pub type EventRef = *const c_void;
    pub type MachPortRef = *const c_void;
    pub type EventSourceRef = *const c_void;
    pub type RunLoopRef = *const c_void;

    pub type EventTapCallBack = unsafe extern "C" fn(
        proxy: EventTapProxy,
        event_type: u32,
        event: EventRef,
        user_info: *mut c_void,
    ) -> EventRef;

    // Event tap locations (pipeline: HID → Session → AnnotatedSession)
    pub const HID_EVENT_TAP: u32 = 0;
    pub const SESSION_EVENT_TAP: u32 = 1;

    pub const HEAD_INSERT: u32 = 0;
    pub const TAP_OPTION_DEFAULT: u32 = 0;

    pub const SCROLL_WHEEL: u32 = 22;
    pub const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
    pub const PIXEL_UNITS: u32 = 1;

    pub const FIELD_IS_CONTINUOUS: u32 = 88;
    pub const FIELD_DELTA_Y: u32 = 11;
    pub const FIELD_DELTA_X: u32 = 12;
    pub const FIELD_USER_DATA: u32 = 42;

    pub const SOURCE_STATE_PRIVATE: i32 = -1;

    /// Sentinel to tag our synthetic events ("SMSC" in ASCII).
    pub const SYNTHETIC_MARKER: i64 = 0x534D_5343;

    extern "C" {
        pub fn CGEventTapCreate(
            tap: u32, place: u32, options: u32,
            events_of_interest: u64,
            callback: EventTapCallBack,
            user_info: *mut c_void,
        ) -> MachPortRef;
        pub fn CGEventTapEnable(tap: MachPortRef, enable: bool);

        pub fn CGEventSourceCreate(state_id: i32) -> EventSourceRef;
        pub fn CGEventSourceSetUserData(source: EventSourceRef, data: i64);

        pub fn CGEventCreateScrollWheelEvent2(
            source: EventSourceRef, units: u32, wheel_count: u32,
            wheel1: i32, wheel2: i32, wheel3: i32,
        ) -> EventRef;
        pub fn CGEventPost(tap: u32, event: EventRef);
        pub fn CGEventGetIntegerValueField(event: EventRef, field: u32) -> i64;

        pub fn CFMachPortCreateRunLoopSource(
            allocator: *const c_void, port: MachPortRef, order: i64,
        ) -> *const c_void;
        pub fn CFRunLoopAddSource(rl: RunLoopRef, source: *const c_void, mode: *const c_void);
        pub fn CFRunLoopGetCurrent() -> RunLoopRef;
        pub fn CFRunLoopRun();
        pub fn CFRunLoopStop(rl: RunLoopRef);
        pub fn CFRelease(cf: *const c_void);

        pub fn AXIsProcessTrusted() -> bool;
    }
}

// ─── CoreVideo FFI (CVDisplayLink) ───────────────────────────────────

mod cv {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct CVDisplayLink { _opaque: [u8; 0] }
    pub type CVDisplayLinkRef = *mut CVDisplayLink;
    pub type CVReturn = i32;
    pub type CVOptionFlags = u64;

    #[repr(C)]
    pub struct CVSMPTETime {
        pub subframes: i16, pub subframe_divisor: i16,
        pub counter: u32, pub time_type: u32, pub flags: u32,
        pub hours: i16, pub minutes: i16, pub seconds: i16, pub frames: i16,
    }

    #[repr(C)]
    pub struct CVTimeStamp {
        pub version: u32, pub video_time_scale: i32,
        pub video_time: i64, pub host_time: u64,
        pub rate_scalar: f64, pub video_refresh_period: i64,
        pub smpte_time: CVSMPTETime,
        pub flags: u64, pub reserved: u64,
    }

    pub type CVDisplayLinkOutputCallback = unsafe extern "C" fn(
        display_link: CVDisplayLinkRef,
        in_now: *const CVTimeStamp,
        in_output_time: *const CVTimeStamp,
        flags_in: CVOptionFlags,
        flags_out: *mut CVOptionFlags,
        context: *mut c_void,
    ) -> CVReturn;

    pub const K_CV_RETURN_SUCCESS: CVReturn = 0;

    extern "C" {
        pub fn CVDisplayLinkCreateWithActiveCGDisplays(out: *mut CVDisplayLinkRef) -> CVReturn;
        pub fn CVDisplayLinkSetOutputCallback(
            dl: CVDisplayLinkRef, cb: CVDisplayLinkOutputCallback, ctx: *mut c_void,
        ) -> CVReturn;
        pub fn CVDisplayLinkStart(dl: CVDisplayLinkRef) -> CVReturn;
        pub fn CVDisplayLinkStop(dl: CVDisplayLinkRef) -> CVReturn;
        pub fn CVDisplayLinkRelease(dl: CVDisplayLinkRef);
    }
}

// ─── Public API ──────────────────────────────────────────────────────

pub fn has_accessibility_permission() -> bool {
    unsafe { cg::AXIsProcessTrusted() }
}

// ─── Settings ────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScrollSettings {
    pub enabled: bool,
    pub scroll_speed: f64,
    pub acceleration: f64,
    pub animation_duration: f64,
    pub inertia: bool,
    pub inertia_decay: f64,
    pub easing: EasingCurve,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum EasingCurve {
    Linear,
    EaseOut,
    EaseInOut,
}

impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            scroll_speed: 3.0,
            acceleration: 0.5,
            animation_duration: 300.0,
            inertia: true,
            inertia_decay: 0.92,
            easing: EasingCurve::EaseOut,
        }
    }
}

impl EasingCurve {
    fn apply(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseOut => 1.0 - (1.0 - t).powi(3),
            Self::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

// ─── Animation state ─────────────────────────────────────────────────

struct ScrollAnimation {
    target: (f64, f64),
    emitted: (f64, f64),
    remainder: (f64, f64), // sub-pixel rounding remainder carried between frames
    start: Instant,
    duration_ms: f64,
    easing: EasingCurve,
}

impl ScrollAnimation {
    fn tick(&mut self) -> (f64, f64) {
        // Guard against zero/negative duration → prevents NaN/Inf
        let duration = self.duration_ms.max(1.0);
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        let progress = (elapsed_ms / duration).min(1.0);
        let eased = self.easing.apply(progress);

        let current = (self.target.0 * eased, self.target.1 * eased);
        let raw_dy = current.0 - self.emitted.0 + self.remainder.0;
        let raw_dx = current.1 - self.emitted.1 + self.remainder.1;

        // Truncate to integer pixels, carry sub-pixel remainder
        let emit_y = raw_dy.trunc();
        let emit_x = raw_dx.trunc();
        self.remainder = (raw_dy - emit_y, raw_dx - emit_x);
        self.emitted = current;

        (emit_y, emit_x)
    }

    fn is_complete(&self) -> bool {
        let duration = self.duration_ms.max(1.0);
        self.start.elapsed().as_secs_f64() * 1000.0 >= duration
    }
}

// ─── Send-safe pointer wrapper ───────────────────────────────────────

/// Wrapper for CoreFoundation/CoreVideo raw pointers that need to move across threads.
/// Safety: Only used for pointers whose target APIs are documented as thread-safe
/// (CFRunLoopStop, CGEventTapEnable, CGEventPost). Only `Send` — not `Sync`,
/// because raw pointers should not be shared concurrently without external sync.
struct SendPtr(*const c_void);
unsafe impl Send for SendPtr {}

impl SendPtr {
    fn null() -> Self { Self(std::ptr::null()) }
    fn get(&self) -> *const c_void { self.0 }
    fn set(&mut self, p: *const c_void) { self.0 = p; }
    fn is_null(&self) -> bool { self.0.is_null() }
}

// ─── Context structs for C callbacks ─────────────────────────────────

/// Shared with the CGEventTap callback.
struct TapContext {
    settings: Arc<Mutex<ScrollSettings>>,
    animation: Arc<Mutex<Option<ScrollAnimation>>>,
    tap_port: Mutex<SendPtr>,
}

/// Shared with the CVDisplayLink callback.
struct DisplayLinkContext {
    animation: Arc<Mutex<Option<ScrollAnimation>>>,
    event_source: cg::EventSourceRef,
}

// ─── Scroll Engine ───────────────────────────────────────────────────

pub struct ScrollEngine {
    settings: Arc<Mutex<ScrollSettings>>,
    running: Arc<AtomicBool>,
    animation: Arc<Mutex<Option<ScrollAnimation>>>,
    run_loop: Arc<Mutex<Option<SendPtr>>>,
}

impl ScrollEngine {
    pub fn new(settings: Arc<Mutex<ScrollSettings>>) -> Self {
        Self {
            settings,
            running: Arc::new(AtomicBool::new(false)),
            animation: Arc::new(Mutex::new(None)),
            run_loop: Arc::new(Mutex::new(None)),
        }
    }

    pub fn start(&self) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        if !has_accessibility_permission() {
            return Err("Accessibility permission not granted".into());
        }

        self.running.store(true, Ordering::SeqCst);

        let settings = self.settings.clone();
        let running = self.running.clone();
        let animation = self.animation.clone();
        let run_loop = self.run_loop.clone();

        std::thread::Builder::new()
            .name("scroll-event-tap".into())
            .spawn(move || run_event_tap(settings, running, animation, run_loop))
            .map_err(|e| format!("Failed to spawn event tap thread: {e}"))?;

        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);

        if let Ok(lock) = self.run_loop.lock() {
            if let Some(ref rl) = *lock {
                if !rl.is_null() {
                    unsafe { cg::CFRunLoopStop(rl.get()); }
                }
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for ScrollEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

// ─── Event tap thread ────────────────────────────────────────────────

fn run_event_tap(
    settings: Arc<Mutex<ScrollSettings>>,
    running: Arc<AtomicBool>,
    animation: Arc<Mutex<Option<ScrollAnimation>>>,
    run_loop_holder: Arc<Mutex<Option<SendPtr>>>,
) {
    let ctx = Box::new(TapContext {
        settings,
        animation: animation.clone(),
        tap_port: Mutex::new(SendPtr::null()),
    });
    let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

    let tap = unsafe {
        cg::CGEventTapCreate(
            cg::HID_EVENT_TAP,
            cg::HEAD_INSERT,
            cg::TAP_OPTION_DEFAULT,
            1u64 << cg::SCROLL_WHEEL,
            on_scroll_event,
            ctx_ptr,
        )
    };

    if tap.is_null() {
        eprintln!("[SmoothScroll] CGEventTap creation failed — accessibility not granted");
        running.store(false, Ordering::SeqCst);
        unsafe { let _ = Box::from_raw(ctx_ptr as *mut TapContext); }
        return;
    }

    {
        let tap_ctx = unsafe { &*(ctx_ptr as *const TapContext) };
        tap_ctx.tap_port.lock()
            .expect("[SmoothScroll] tap_port mutex poisoned during setup")
            .set(tap);
    }

    let rl_source = unsafe { cg::CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0) };
    if rl_source.is_null() {
        eprintln!("[SmoothScroll] CFMachPortCreateRunLoopSource returned null");
        running.store(false, Ordering::SeqCst);
        unsafe {
            cg::CFRelease(tap);
            let _ = Box::from_raw(ctx_ptr as *mut TapContext);
        }
        return;
    }

    let rl = unsafe { cg::CFRunLoopGetCurrent() };

    if let Ok(mut holder) = run_loop_holder.lock() {
        *holder = Some(SendPtr(rl));
    }

    unsafe {
        cg::CFRunLoopAddSource(rl, rl_source, kCFRunLoopCommonModes as *const _ as *const c_void);
        cg::CGEventTapEnable(tap, true);
    }

    // Start CVDisplayLink for display-synced animation
    let display_link = start_display_link(animation, &running);

    // Block on the run loop until stopped
    while running.load(Ordering::SeqCst) {
        unsafe { cg::CFRunLoopRun(); }
    }

    // Shutdown
    stop_display_link(display_link);

    if let Ok(mut holder) = run_loop_holder.lock() {
        *holder = None;
    }

    unsafe {
        cg::CGEventTapEnable(tap, false);
        cg::CFRelease(rl_source);
        cg::CFRelease(tap);
        let _ = Box::from_raw(ctx_ptr as *mut TapContext);
    }
}

// ─── CVDisplayLink management ────────────────────────────────────────

/// Create a tagged CGEventSource. Returns null on failure.
fn create_tagged_event_source() -> cg::EventSourceRef {
    let source = unsafe { cg::CGEventSourceCreate(cg::SOURCE_STATE_PRIVATE) };
    if !source.is_null() {
        unsafe { cg::CGEventSourceSetUserData(source, cg::SYNTHETIC_MARKER); }
    }
    source
}

/// Start a CVDisplayLink that fires at the display's refresh rate.
fn start_display_link(
    animation: Arc<Mutex<Option<ScrollAnimation>>>,
    running: &Arc<AtomicBool>,
) -> Option<(cv::CVDisplayLinkRef, *mut DisplayLinkContext)> {
    let source = create_tagged_event_source();
    if source.is_null() {
        eprintln!("[SmoothScroll] Failed to create CGEventSource, falling back to sleep loop");
        start_fallback_animation_loop(animation, running.clone());
        return None;
    }

    // Clone animation Arc BEFORE boxing into raw pointer, so we have a safe copy
    // for the fallback path without risking use-after-free
    let animation_fallback = animation.clone();

    let dl_ctx = Box::new(DisplayLinkContext {
        animation,
        event_source: source,
    });
    let dl_ctx_ptr = Box::into_raw(dl_ctx);

    let mut display_link: cv::CVDisplayLinkRef = std::ptr::null_mut();
    let ret = unsafe { cv::CVDisplayLinkCreateWithActiveCGDisplays(&mut display_link) };

    if ret != cv::K_CV_RETURN_SUCCESS || display_link.is_null() {
        eprintln!("[SmoothScroll] CVDisplayLink creation failed ({}), falling back", ret);
        unsafe {
            let ctx = Box::from_raw(dl_ctx_ptr);
            cg::CFRelease(ctx.event_source);
        }
        start_fallback_animation_loop(animation_fallback, running.clone());
        return None;
    }

    let ret = unsafe {
        cv::CVDisplayLinkSetOutputCallback(
            display_link, on_display_link_frame, dl_ctx_ptr as *mut c_void,
        )
    };

    if ret != cv::K_CV_RETURN_SUCCESS {
        eprintln!("[SmoothScroll] CVDisplayLink callback setup failed");
        unsafe {
            cv::CVDisplayLinkRelease(display_link);
            let ctx = Box::from_raw(dl_ctx_ptr);
            cg::CFRelease(ctx.event_source);
        }
        return None;
    }

    let ret = unsafe { cv::CVDisplayLinkStart(display_link) };
    if ret != cv::K_CV_RETURN_SUCCESS {
        eprintln!("[SmoothScroll] CVDisplayLink start failed");
        unsafe {
            cv::CVDisplayLinkRelease(display_link);
            let ctx = Box::from_raw(dl_ctx_ptr);
            cg::CFRelease(ctx.event_source);
        }
        return None;
    }

    println!("[SmoothScroll] CVDisplayLink started — animation synced to display refresh rate");
    Some((display_link, dl_ctx_ptr))
}

fn stop_display_link(dl: Option<(cv::CVDisplayLinkRef, *mut DisplayLinkContext)>) {
    if let Some((display_link, ctx_ptr)) = dl {
        unsafe {
            cv::CVDisplayLinkStop(display_link);
            cv::CVDisplayLinkRelease(display_link);
            let ctx = Box::from_raw(ctx_ptr);
            if !ctx.event_source.is_null() {
                cg::CFRelease(ctx.event_source);
            }
        }
    }
}

/// Fallback if CVDisplayLink is unavailable.
fn start_fallback_animation_loop(
    animation: Arc<Mutex<Option<ScrollAnimation>>>,
    running: Arc<AtomicBool>,
) {
    let source = create_tagged_event_source();
    if source.is_null() {
        eprintln!("[SmoothScroll] Fallback: CGEventSource creation failed — scrolling will not work");
        return;
    }
    let source_ptr = SendPtr(source);

    std::thread::Builder::new()
        .name("scroll-animator-fallback".into())
        .spawn(move || {
            let src = source_ptr.get() as cg::EventSourceRef;
            const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_micros(8333);
            while running.load(Ordering::SeqCst) {
                process_animation_frame(&animation, src);
                std::thread::sleep(FRAME_INTERVAL);
            }
            unsafe { cg::CFRelease(src); }
        })
        .expect("[SmoothScroll] Failed to spawn fallback animation thread");
}

// ─── Shared animation frame logic ───────────────────────────────────

/// Process one animation frame — used by both CVDisplayLink and fallback loop.
fn process_animation_frame(
    animation: &Arc<Mutex<Option<ScrollAnimation>>>,
    source: cg::EventSourceRef,
) {
    let delta = {
        let mut lock = match animation.lock() {
            Ok(l) => l,
            Err(_) => return,
        };
        if let Some(ref mut anim) = *lock {
            let d = anim.tick();
            if anim.is_complete() {
                *lock = None;
            }
            Some(d)
        } else {
            None
        }
    };

    if let Some((dy, dx)) = delta {
        if dy.abs() > 0.5 || dx.abs() > 0.5 {
            post_scroll_event(source, dy as i32, dx as i32);
        }
    }
}

// ─── CVDisplayLink callback ──────────────────────────────────────────

unsafe extern "C" fn on_display_link_frame(
    _display_link: cv::CVDisplayLinkRef,
    _in_now: *const cv::CVTimeStamp,
    _in_output_time: *const cv::CVTimeStamp,
    _flags_in: cv::CVOptionFlags,
    _flags_out: *mut cv::CVOptionFlags,
    context: *mut c_void,
) -> cv::CVReturn {
    if context.is_null() {
        return cv::K_CV_RETURN_SUCCESS;
    }

    let ctx = &*(context as *const DisplayLinkContext);
    process_animation_frame(&ctx.animation, ctx.event_source);

    cv::K_CV_RETURN_SUCCESS
}

// ─── Emit synthetic scroll event ─────────────────────────────────────

fn post_scroll_event(source: cg::EventSourceRef, delta_y: i32, delta_x: i32) {
    unsafe {
        let event = cg::CGEventCreateScrollWheelEvent2(
            source, cg::PIXEL_UNITS, 2, delta_y, delta_x, 0,
        );
        if !event.is_null() {
            cg::CGEventPost(cg::SESSION_EVENT_TAP, event);
            cg::CFRelease(event);
        }
    }
}

// ─── CGEventTap callback ─────────────────────────────────────────────

unsafe extern "C" fn on_scroll_event(
    _proxy: cg::EventTapProxy,
    event_type: u32,
    event: cg::EventRef,
    user_info: *mut c_void,
) -> cg::EventRef {
    if user_info.is_null() || event.is_null() {
        return event;
    }

    if event_type == cg::TAP_DISABLED_BY_TIMEOUT {
        let ctx = &*(user_info as *const TapContext);
        if let Ok(port) = ctx.tap_port.lock() {
            if !port.is_null() {
                cg::CGEventTapEnable(port.get(), true);
            }
        }
        return event;
    }

    if event_type != cg::SCROLL_WHEEL {
        return event;
    }

    // Skip our own synthetic events
    if cg::CGEventGetIntegerValueField(event, cg::FIELD_USER_DATA) == cg::SYNTHETIC_MARKER {
        return event;
    }

    let ctx = &*(user_info as *const TapContext);

    // Lock ordering: settings first, then animation (see module doc)
    let settings = match ctx.settings.lock() {
        Ok(s) => s,
        Err(_) => return event,
    };

    if !settings.enabled {
        return event;
    }

    // Pass through continuous (trackpad) events
    if cg::CGEventGetIntegerValueField(event, cg::FIELD_IS_CONTINUOUS) != 0 {
        return event;
    }

    let raw_y = cg::CGEventGetIntegerValueField(event, cg::FIELD_DELTA_Y) as f64;
    let raw_x = cg::CGEventGetIntegerValueField(event, cg::FIELD_DELTA_X) as f64;

    if raw_y == 0.0 && raw_x == 0.0 {
        return event;
    }

    let magnitude = raw_y.abs().max(raw_x.abs());
    let accel_factor = 1.0 + settings.acceleration * (magnitude - 1.0).max(0.0);
    let target_y = raw_y * settings.scroll_speed * accel_factor;
    let target_x = raw_x * settings.scroll_speed * accel_factor;

    let duration = settings.animation_duration;
    let easing = settings.easing.clone();
    drop(settings); // release before locking animation

    if let Ok(mut anim) = ctx.animation.lock() {
        // Accumulate remaining scroll distance from in-flight animation
        let carry = if let Some(ref prev) = *anim {
            (prev.target.0 - prev.emitted.0, prev.target.1 - prev.emitted.1)
        } else {
            (0.0, 0.0)
        };

        *anim = Some(ScrollAnimation {
            target: (target_y + carry.0, target_x + carry.1),
            emitted: (0.0, 0.0),
            remainder: (0.0, 0.0),
            start: Instant::now(),
            duration_ms: duration,
            easing,
        });
    }

    std::ptr::null()
}

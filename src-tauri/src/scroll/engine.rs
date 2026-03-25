//! ScrollEngine — manages the event tap thread, CVDisplayLink, and lifecycle.

use super::ffi;
use super::physics::{
    on_display_link_frame, on_scroll_event, process_scroll_frame, DisplayLinkContext, TapContext,
};
use super::state::*;
use core_foundation::runloop::kCFRunLoopCommonModes;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct ScrollEngine {
    settings: Arc<Mutex<ScrollSettings>>,
    running: Arc<AtomicBool>,
    scroll_state: Arc<Mutex<ScrollState>>,
    run_loop: Arc<Mutex<Option<SendPtr>>>,
}

impl ScrollEngine {
    pub fn new(settings: Arc<Mutex<ScrollSettings>>) -> Self {
        Self {
            settings,
            running: Arc::new(AtomicBool::new(false)),
            scroll_state: Arc::new(Mutex::new(ScrollState::new())),
            run_loop: Arc::new(Mutex::new(None)),
        }
    }

    pub fn start(&self) -> Result<(), String> {
        dbg_log!("ScrollEngine::start() called");
        if self.running.load(Ordering::SeqCst) {
            dbg_log!("ScrollEngine::start() — already running");
            return Ok(());
        }
        if !super::has_accessibility_permission() {
            dbg_log!("ScrollEngine::start() — no accessibility permission");
            return Err("Accessibility permission not granted".into());
        }

        self.running.store(true, Ordering::SeqCst);
        dbg_log!("ScrollEngine: spawning event tap thread");

        let settings = self.settings.clone();
        let running = self.running.clone();
        let scroll_state = self.scroll_state.clone();
        let run_loop = self.run_loop.clone();

        std::thread::Builder::new()
            .name("scroll-event-tap".into())
            .spawn(move || run_event_tap(settings, running, scroll_state, run_loop))
            .map_err(|e| format!("Failed to spawn event tap thread: {e}"))?;

        dbg_log!("ScrollEngine: event tap thread spawned");
        Ok(())
    }

    pub fn stop(&self) {
        dbg_log!("ScrollEngine::stop() called");
        self.running.store(false, Ordering::SeqCst);

        if let Ok(lock) = self.run_loop.lock() {
            if let Some(ref rl) = *lock {
                if !rl.is_null() {
                    dbg_log!("ScrollEngine: stopping CFRunLoop");
                    unsafe { ffi::CFRunLoopStop(rl.get()); }
                }
            }
        }
        dbg_log!("ScrollEngine: stopped");
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
    scroll_state: Arc<Mutex<ScrollState>>,
    run_loop_holder: Arc<Mutex<Option<SendPtr>>>,
) {
    dbg_log!("run_event_tap: thread started");

    let ctx = Box::new(TapContext {
        settings: settings.clone(),
        scroll_state: scroll_state.clone(),
        tap_port: Mutex::new(SendPtr::null()),
    });
    let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

    dbg_log!("run_event_tap: creating CGEventTap (HID_EVENT_TAP, SCROLL_WHEEL)");
    let tap = unsafe {
        ffi::CGEventTapCreate(
            ffi::HID_EVENT_TAP,
            ffi::HEAD_INSERT,
            ffi::TAP_OPTION_DEFAULT,
            1u64 << ffi::SCROLL_WHEEL,
            on_scroll_event,
            ctx_ptr,
        )
    };

    if tap.is_null() {
        eprintln!("[SmoothScroll] CGEventTap creation failed — accessibility not granted");
        dbg_log!("run_event_tap: CGEventTapCreate returned NULL");
        running.store(false, Ordering::SeqCst);
        unsafe { let _ = Box::from_raw(ctx_ptr as *mut TapContext); }
        return;
    }
    dbg_log!("run_event_tap: CGEventTap created successfully");

    {
        let tap_ctx = unsafe { &*(ctx_ptr as *const TapContext) };
        tap_ctx
            .tap_port
            .lock()
            .expect("[SmoothScroll] tap_port mutex poisoned during setup")
            .set(tap);
    }

    let rl_source =
        unsafe { ffi::CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0) };
    if rl_source.is_null() {
        eprintln!("[SmoothScroll] CFMachPortCreateRunLoopSource returned null");
        running.store(false, Ordering::SeqCst);
        unsafe {
            ffi::CFRelease(tap);
            let _ = Box::from_raw(ctx_ptr as *mut TapContext);
        }
        return;
    }

    let rl = unsafe { ffi::CFRunLoopGetCurrent() };

    if let Ok(mut holder) = run_loop_holder.lock() {
        *holder = Some(SendPtr(rl));
    }

    unsafe {
        ffi::CFRunLoopAddSource(
            rl,
            rl_source,
            kCFRunLoopCommonModes as *const _ as *const c_void,
        );
        ffi::CGEventTapEnable(tap, true);
    }
    dbg_log!("run_event_tap: event tap enabled, run loop source added");

    // Start CVDisplayLink for display-synced scrolling
    dbg_log!("run_event_tap: starting CVDisplayLink");
    let display_link = start_display_link(scroll_state, settings, &running);
    dbg_log!("run_event_tap: CVDisplayLink started = {}", display_link.is_some());

    // Block on the run loop until stopped
    dbg_log!("run_event_tap: entering CFRunLoop (blocking)");
    while running.load(Ordering::SeqCst) {
        unsafe { ffi::CFRunLoopRun(); }
    }

    // Shutdown
    dbg_log!("run_event_tap: exited run loop — shutting down");
    stop_display_link(display_link);

    if let Ok(mut holder) = run_loop_holder.lock() {
        *holder = None;
    }

    unsafe {
        ffi::CGEventTapEnable(tap, false);
        ffi::CFRelease(rl_source);
        ffi::CFRelease(tap);
        let _ = Box::from_raw(ctx_ptr as *mut TapContext);
    }
}

// ─── CVDisplayLink management ────────────────────────────────────────

fn create_tagged_event_source() -> ffi::EventSourceRef {
    let source = unsafe { ffi::CGEventSourceCreate(ffi::SOURCE_STATE_PRIVATE) };
    if !source.is_null() {
        unsafe { ffi::CGEventSourceSetUserData(source, ffi::SYNTHETIC_MARKER); }
    }
    source
}

fn start_display_link(
    scroll_state: Arc<Mutex<ScrollState>>,
    settings: Arc<Mutex<ScrollSettings>>,
    running: &Arc<AtomicBool>,
) -> Option<(ffi::CVDisplayLinkRef, *mut DisplayLinkContext)> {
    let source = create_tagged_event_source();
    if source.is_null() {
        eprintln!("[SmoothScroll] Failed to create CGEventSource, falling back to sleep loop");
        start_fallback_loop(scroll_state, settings, running.clone());
        return None;
    }

    let state_fallback = scroll_state.clone();
    let settings_fallback = settings.clone();

    let dl_ctx = Box::new(DisplayLinkContext {
        scroll_state,
        settings,
        event_source: source,
    });
    let dl_ctx_ptr = Box::into_raw(dl_ctx);

    let mut display_link: ffi::CVDisplayLinkRef = std::ptr::null_mut();
    let ret =
        unsafe { ffi::CVDisplayLinkCreateWithActiveCGDisplays(&mut display_link) };

    if ret != ffi::K_CV_RETURN_SUCCESS || display_link.is_null() {
        eprintln!("[SmoothScroll] CVDisplayLink creation failed ({ret}), falling back");
        unsafe {
            let ctx = Box::from_raw(dl_ctx_ptr);
            ffi::CFRelease(ctx.event_source);
        }
        start_fallback_loop(state_fallback, settings_fallback, running.clone());
        return None;
    }

    let ret = unsafe {
        ffi::CVDisplayLinkSetOutputCallback(
            display_link,
            on_display_link_frame,
            dl_ctx_ptr as *mut c_void,
        )
    };

    if ret != ffi::K_CV_RETURN_SUCCESS {
        eprintln!("[SmoothScroll] CVDisplayLink callback setup failed");
        unsafe {
            ffi::CVDisplayLinkRelease(display_link);
            let ctx = Box::from_raw(dl_ctx_ptr);
            ffi::CFRelease(ctx.event_source);
        }
        return None;
    }

    let ret = unsafe { ffi::CVDisplayLinkStart(display_link) };
    if ret != ffi::K_CV_RETURN_SUCCESS {
        eprintln!("[SmoothScroll] CVDisplayLink start failed");
        unsafe {
            ffi::CVDisplayLinkRelease(display_link);
            let ctx = Box::from_raw(dl_ctx_ptr);
            ffi::CFRelease(ctx.event_source);
        }
        return None;
    }

    dbg_log!("CVDisplayLink started — velocity-based scrolling synced to display refresh");
    Some((display_link, dl_ctx_ptr))
}

fn stop_display_link(dl: Option<(ffi::CVDisplayLinkRef, *mut DisplayLinkContext)>) {
    if let Some((display_link, ctx_ptr)) = dl {
        unsafe {
            ffi::CVDisplayLinkStop(display_link);
            ffi::CVDisplayLinkRelease(display_link);
            let ctx = Box::from_raw(ctx_ptr);
            if !ctx.event_source.is_null() {
                ffi::CFRelease(ctx.event_source);
            }
        }
    }
}

fn start_fallback_loop(
    scroll_state: Arc<Mutex<ScrollState>>,
    settings: Arc<Mutex<ScrollSettings>>,
    running: Arc<AtomicBool>,
) {
    let source = create_tagged_event_source();
    if source.is_null() {
        eprintln!("[SmoothScroll] Fallback: CGEventSource creation failed");
        return;
    }
    let source_ptr = SendPtr(source);

    std::thread::Builder::new()
        .name("scroll-velocity-fallback".into())
        .spawn(move || {
            let src = source_ptr.get() as ffi::EventSourceRef;
            const FRAME_INTERVAL: std::time::Duration =
                std::time::Duration::from_micros(8333);
            while running.load(Ordering::SeqCst) {
                process_scroll_frame(&scroll_state, &settings, src);
                std::thread::sleep(FRAME_INTERVAL);
            }
            unsafe { ffi::CFRelease(src); }
        })
        .expect("[SmoothScroll] Failed to spawn fallback scroll thread");
}

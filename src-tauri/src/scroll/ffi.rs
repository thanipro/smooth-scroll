//! Raw FFI bindings for CoreGraphics, CoreVideo, and mach_absolute_time.

use std::ffi::c_void;

// ─── CoreGraphics ────────────────────────────────────────────────────

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

// CGEvent field IDs
pub const FIELD_IS_CONTINUOUS: u32 = 88;
pub const FIELD_DELTA_Y: u32 = 11;
pub const FIELD_DELTA_X: u32 = 12;
pub const FIELD_POINT_DELTA_Y: u32 = 96;
pub const FIELD_POINT_DELTA_X: u32 = 97;
pub const FIELD_SCROLL_PHASE: u32 = 99;
pub const FIELD_MOMENTUM_PHASE: u32 = 123;
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
    pub fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

// CoreFoundation helpers for building the accessibility options dict
extern "C" {
    pub static kAXTrustedCheckOptionPrompt: *const c_void;
    pub static kCFBooleanTrue: *const c_void;

    pub fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
}

// ─── CoreVideo (CVDisplayLink) ───────────────────────────────────────

#[repr(C)]
pub struct CVDisplayLink {
    _opaque: [u8; 0],
}
pub type CVDisplayLinkRef = *mut CVDisplayLink;
pub type CVReturn = i32;
pub type CVOptionFlags = u64;

#[repr(C)]
pub struct CVSMPTETime {
    pub subframes: i16,
    pub subframe_divisor: i16,
    pub counter: u32,
    pub time_type: u32,
    pub flags: u32,
    pub hours: i16,
    pub minutes: i16,
    pub seconds: i16,
    pub frames: i16,
}

#[repr(C)]
pub struct CVTimeStamp {
    pub version: u32,
    pub video_time_scale: i32,
    pub video_time: i64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub video_refresh_period: i64,
    pub smpte_time: CVSMPTETime,
    pub flags: u64,
    pub reserved: u64,
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

// ─── mach_absolute_time ──────────────────────────────────────────────

extern "C" {
    pub fn mach_absolute_time() -> u64;
}

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

static mut TIMEBASE: MachTimebaseInfo = MachTimebaseInfo { numer: 0, denom: 0 };
static TIMEBASE_INIT: std::sync::Once = std::sync::Once::new();

/// Convert mach_absolute_time ticks to nanoseconds.
pub fn ticks_to_ns(ticks: u64) -> u64 {
    unsafe {
        TIMEBASE_INIT.call_once(|| {
            mach_timebase_info(&raw mut TIMEBASE as *mut _);
        });
        ticks * TIMEBASE.numer as u64 / TIMEBASE.denom as u64
    }
}

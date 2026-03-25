//! Scroll settings, state, and physics constants.

use std::ffi::c_void;

// ─── Settings ────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScrollSettings {
    pub enabled: bool,
    /// Impulse multiplier: how many pixels of velocity each wheel tick adds.
    pub scroll_speed: f64,
    /// Acceleration factor: scales impulse with scroll magnitude.
    pub acceleration: f64,
    /// Per-frame velocity decay (0.0–1.0). Higher = more momentum.
    /// At 120Hz, 0.92 ≈ trackpad-like glide.
    pub inertia_decay: f64,
}

impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            scroll_speed: 0.5,
            acceleration: 0.5,
            inertia_decay: 0.92,
        }
    }
}

// ─── Physics constants ───────────────────────────────────────────────

/// Maximum velocity in pixels/frame to prevent runaway speed.
pub const VELOCITY_CAP: f64 = 80.0;
/// Velocity below which scrolling stops (pixels/frame).
pub const STOP_THRESHOLD: f64 = 0.3;
/// Reference frame interval for frame-rate-independent decay (120Hz).
pub const REFERENCE_DT_NS: f64 = 8_333_333.0;
/// Maximum scroll speed in pixels per second — prevents jarring jumps.
/// Time-based so it behaves identically on 60Hz, 120Hz, and variable refresh.
pub const MAX_PIXELS_PER_SECOND: f64 = 4800.0;
/// Number of frames over which to ramp an impulse (smooth start).
pub const IMPULSE_RAMP_FRAMES: f64 = 4.0;
/// Velocity threshold for two-phase decay: above this, decay is faster.
pub const HIGH_VELOCITY_THRESHOLD: f64 = 15.0;
/// Faster decay multiplier for high velocities (applied on top of base decay).
pub const HIGH_VELOCITY_DECAY_FACTOR: f64 = 0.85;

// ─── Scroll state (velocity-based) ──────────────────────────────────

pub struct ScrollState {
    pub velocity_y: f64,
    pub velocity_x: f64,
    pub remainder_y: f64,
    pub remainder_x: f64,
    /// Pending impulse that hasn't been applied yet (smooth ramp).
    pub pending_impulse_y: f64,
    pub pending_impulse_x: f64,
    pub last_frame_time: u64,
}

impl ScrollState {
    pub fn new() -> Self {
        Self {
            velocity_y: 0.0,
            velocity_x: 0.0,
            remainder_y: 0.0,
            remainder_x: 0.0,
            pending_impulse_y: 0.0,
            pending_impulse_x: 0.0,
            last_frame_time: 0,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.velocity_y.abs() < STOP_THRESHOLD
            && self.velocity_x.abs() < STOP_THRESHOLD
            && self.pending_impulse_y.abs() < STOP_THRESHOLD
            && self.pending_impulse_x.abs() < STOP_THRESHOLD
    }
}

// ─── Send-safe pointer wrapper ───────────────────────────────────────

/// Wrapper for CoreFoundation/CoreVideo raw pointers that need to move across threads.
/// Safety: Only used for pointers whose target APIs are documented as thread-safe
/// (CFRunLoopStop, CGEventTapEnable, CGEventPost). Only `Send` — not `Sync`.
pub struct SendPtr(pub *const c_void);
unsafe impl Send for SendPtr {}

impl SendPtr {
    pub fn null() -> Self { Self(std::ptr::null()) }
    pub fn get(&self) -> *const c_void { self.0 }
    pub fn set(&mut self, p: *const c_void) { self.0 = p; }
    pub fn is_null(&self) -> bool { self.0.is_null() }
}

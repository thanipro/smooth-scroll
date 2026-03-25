//! Scroll physics — frame processing, event interception, and synthetic event emission.

use super::ffi;
use super::state::*;
use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

#[cfg(debug_assertions)]
use std::sync::atomic::AtomicU64;

#[cfg(debug_assertions)]
pub(super) static SCROLL_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
#[cfg(debug_assertions)]
static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

// ─── Context structs for C callbacks ─────────────────────────────────

/// Shared with the CGEventTap callback.
pub(super) struct TapContext {
    pub settings: Arc<Mutex<ScrollSettings>>,
    pub scroll_state: Arc<Mutex<ScrollState>>,
    pub tap_port: Mutex<SendPtr>,
}

/// Shared with the CVDisplayLink callback.
pub(super) struct DisplayLinkContext {
    pub scroll_state: Arc<Mutex<ScrollState>>,
    pub settings: Arc<Mutex<ScrollSettings>>,
    pub event_source: ffi::EventSourceRef,
}

// ─── Frame processing ────────────────────────────────────────────────

/// Process one scroll frame — ramp pending impulse, emit velocity pixels, then decay.
pub fn process_scroll_frame(
    scroll_state: &Arc<Mutex<ScrollState>>,
    settings: &Arc<Mutex<ScrollSettings>>,
    source: ffi::EventSourceRef,
) {
    let base_decay = settings
        .try_lock()
        .map(|s| s.inertia_decay)
        .unwrap_or(0.92);

    let now = unsafe { ffi::mach_absolute_time() };

    let delta = {
        let mut state = match scroll_state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };

        if state.is_idle() {
            state.last_frame_time = now;
            return;
        }

        // 1. Smooth impulse ramp: feed pending impulse into velocity gradually
        if state.pending_impulse_y.abs() > 0.01 || state.pending_impulse_x.abs() > 0.01 {
            let ramp_y = state.pending_impulse_y / IMPULSE_RAMP_FRAMES;
            let ramp_x = state.pending_impulse_x / IMPULSE_RAMP_FRAMES;
            state.velocity_y += ramp_y;
            state.velocity_x += ramp_x;
            state.pending_impulse_y -= ramp_y;
            state.pending_impulse_x -= ramp_x;

            state.velocity_y = state.velocity_y.clamp(-VELOCITY_CAP, VELOCITY_CAP);
            state.velocity_x = state.velocity_x.clamp(-VELOCITY_CAP, VELOCITY_CAP);
        } else {
            state.pending_impulse_y = 0.0;
            state.pending_impulse_x = 0.0;
        }

        // Frame-rate-independent decay
        let dt_ns = if state.last_frame_time == 0 {
            REFERENCE_DT_NS
        } else {
            ffi::ticks_to_ns(now.saturating_sub(state.last_frame_time)) as f64
        };
        state.last_frame_time = now;

        let decay_power = dt_ns / REFERENCE_DT_NS;

        // 2. Two-phase decay: faster at high velocity, slower glide tail
        let speed = state.velocity_y.abs().max(state.velocity_x.abs());
        let effective_decay = if speed > HIGH_VELOCITY_THRESHOLD {
            let blend =
                ((speed - HIGH_VELOCITY_THRESHOLD) / HIGH_VELOCITY_THRESHOLD).min(1.0);
            let fast_decay = base_decay * HIGH_VELOCITY_DECAY_FACTOR;
            let blended = base_decay * (1.0 - blend) + fast_decay * blend;
            blended.powf(decay_power)
        } else {
            base_decay.powf(decay_power)
        };

        // 3. Time-based per-frame pixel cap
        let max_emit = (MAX_PIXELS_PER_SECOND * dt_ns / 1_000_000_000.0).max(1.0);

        let raw_dy = state.remainder_y + state.velocity_y;
        let raw_dx = state.remainder_x + state.velocity_x;
        let mut emit_y = raw_dy.trunc();
        let mut emit_x = raw_dx.trunc();

        emit_y = emit_y.clamp(-max_emit, max_emit);
        emit_x = emit_x.clamp(-max_emit, max_emit);

        // Only carry fractional remainder — capped excess is intentionally dropped
        let excess_y = raw_dy - emit_y;
        let excess_x = raw_dx - emit_x;
        state.remainder_y = excess_y - excess_y.trunc();
        state.remainder_x = excess_x - excess_x.trunc();

        // Apply decay
        state.velocity_y *= effective_decay;
        state.velocity_x *= effective_decay;

        // Stop if below threshold
        if state.velocity_y.abs() < STOP_THRESHOLD {
            state.velocity_y = 0.0;
            state.remainder_y = 0.0;
        }
        if state.velocity_x.abs() < STOP_THRESHOLD {
            state.velocity_x = 0.0;
            state.remainder_x = 0.0;
        }

        #[cfg(debug_assertions)]
        {
            let n = FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
            if n % 30 == 0 && (emit_y != 0.0 || emit_x != 0.0 || !state.is_idle()) {
                dbg_log!(
                    "scroll frame #{n}: emit=({emit_y:.0},{emit_x:.0}) vel=({:.1},{:.1}) pending=({:.1},{:.1}) decay={effective_decay:.4}",
                    state.velocity_y, state.velocity_x,
                    state.pending_impulse_y, state.pending_impulse_x
                );
            }
        }

        (emit_y as i32, emit_x as i32)
    };

    if delta.0 != 0 || delta.1 != 0 {
        post_scroll_event(source, delta.0, delta.1);
    }
}

// ─── Emit synthetic scroll event ─────────────────────────────────────

fn post_scroll_event(source: ffi::EventSourceRef, delta_y: i32, delta_x: i32) {
    unsafe {
        let event = ffi::CGEventCreateScrollWheelEvent2(
            source,
            ffi::PIXEL_UNITS,
            2,
            delta_y,
            delta_x,
            0,
        );
        if !event.is_null() {
            ffi::CGEventPost(ffi::SESSION_EVENT_TAP, event);
            ffi::CFRelease(event);
        }
    }
}

// ─── CVDisplayLink callback ──────────────────────────────────────────

pub(super) unsafe extern "C" fn on_display_link_frame(
    _display_link: ffi::CVDisplayLinkRef,
    _in_now: *const ffi::CVTimeStamp,
    _in_output_time: *const ffi::CVTimeStamp,
    _flags_in: ffi::CVOptionFlags,
    _flags_out: *mut ffi::CVOptionFlags,
    context: *mut c_void,
) -> ffi::CVReturn {
    if context.is_null() {
        return ffi::K_CV_RETURN_SUCCESS;
    }

    let ctx = &*(context as *const DisplayLinkContext);
    process_scroll_frame(&ctx.scroll_state, &ctx.settings, ctx.event_source);

    ffi::K_CV_RETURN_SUCCESS
}

// ─── CGEventTap callback ─────────────────────────────────────────────

pub(super) unsafe extern "C" fn on_scroll_event(
    _proxy: ffi::EventTapProxy,
    event_type: u32,
    event: ffi::EventRef,
    user_info: *mut c_void,
) -> ffi::EventRef {
    if user_info.is_null() || event.is_null() {
        return event;
    }

    if event_type == ffi::TAP_DISABLED_BY_TIMEOUT {
        dbg_log!("on_scroll_event: TAP_DISABLED_BY_TIMEOUT — re-enabling tap");
        let ctx = &*(user_info as *const TapContext);
        if let Ok(port) = ctx.tap_port.lock() {
            if !port.is_null() {
                ffi::CGEventTapEnable(port.get(), true);
            }
        }
        return event;
    }

    if event_type != ffi::SCROLL_WHEEL {
        return event;
    }

    // Skip our own synthetic events
    if ffi::CGEventGetIntegerValueField(event, ffi::FIELD_USER_DATA) == ffi::SYNTHETIC_MARKER {
        return event;
    }

    let ctx = &*(user_info as *const TapContext);

    // Lock ordering: settings first, then scroll_state
    let settings = match ctx.settings.lock() {
        Ok(s) => s,
        Err(_) => {
            dbg_log!("on_scroll_event: settings mutex poisoned!");
            return event;
        }
    };

    if !settings.enabled {
        return event;
    }

    let is_continuous =
        ffi::CGEventGetIntegerValueField(event, ffi::FIELD_IS_CONTINUOUS) != 0;
    let scroll_phase =
        ffi::CGEventGetIntegerValueField(event, ffi::FIELD_SCROLL_PHASE);
    let momentum_phase =
        ffi::CGEventGetIntegerValueField(event, ffi::FIELD_MOMENTUM_PHASE);

    // Real trackpad events have non-zero scroll/momentum phase — pass through.
    // Logi Options+ continuous events have both phases = 0, so they get processed.
    if is_continuous && (scroll_phase != 0 || momentum_phase != 0) {
        #[cfg(debug_assertions)]
        {
            let n = SCROLL_EVENT_COUNT.load(Ordering::Relaxed);
            if n % 100 == 0 {
                dbg_log!(
                    "on_scroll_event: passing through real trackpad event (phase={scroll_phase}, momentum={momentum_phase})"
                );
            }
        }
        return event;
    }

    // Continuous events (Logi Options+): read pixel deltas from PointDelta fields.
    // Discrete events (normal mouse): read line ticks from Delta fields.
    let (raw_y, raw_x) = if is_continuous {
        (
            ffi::CGEventGetIntegerValueField(event, ffi::FIELD_POINT_DELTA_Y) as f64,
            ffi::CGEventGetIntegerValueField(event, ffi::FIELD_POINT_DELTA_X) as f64,
        )
    } else {
        (
            ffi::CGEventGetIntegerValueField(event, ffi::FIELD_DELTA_Y) as f64,
            ffi::CGEventGetIntegerValueField(event, ffi::FIELD_DELTA_X) as f64,
        )
    };

    if raw_y == 0.0 && raw_x == 0.0 {
        return event;
    }

    // Compute impulse — scale differently for continuous vs discrete events.
    let (impulse_y, impulse_x, accel_factor);
    if is_continuous {
        let cont_scale = settings.scroll_speed / 3.0;
        accel_factor = 1.0;
        impulse_y = raw_y * cont_scale;
        impulse_x = raw_x * cont_scale;
    } else {
        let magnitude = raw_y.abs().max(raw_x.abs());
        accel_factor = 1.0 + settings.acceleration * (magnitude - 1.0).max(0.0);
        impulse_y = raw_y * settings.scroll_speed * accel_factor;
        impulse_x = raw_x * settings.scroll_speed * accel_factor;
    }

    drop(settings); // release before locking scroll_state

    if let Ok(mut state) = ctx.scroll_state.lock() {
        // Direction reversal: kill opposite momentum for responsive direction changes
        if impulse_y != 0.0
            && state.velocity_y.signum() != impulse_y.signum()
            && state.velocity_y != 0.0
        {
            state.velocity_y = 0.0;
            state.pending_impulse_y = 0.0;
            state.remainder_y = 0.0;
        }
        if impulse_x != 0.0
            && state.velocity_x.signum() != impulse_x.signum()
            && state.velocity_x != 0.0
        {
            state.velocity_x = 0.0;
            state.pending_impulse_x = 0.0;
            state.remainder_x = 0.0;
        }

        state.pending_impulse_y += impulse_y;
        state.pending_impulse_x += impulse_x;

        // Clamp pending to prevent extreme accumulation
        state.pending_impulse_y =
            state.pending_impulse_y.clamp(-VELOCITY_CAP * 2.0, VELOCITY_CAP * 2.0);
        state.pending_impulse_x =
            state.pending_impulse_x.clamp(-VELOCITY_CAP * 2.0, VELOCITY_CAP * 2.0);

        #[cfg(debug_assertions)]
        {
            let n = SCROLL_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
            if n % 10 == 0 {
                dbg_log!(
                    "on_scroll_event #{n}: raw=({raw_y},{raw_x}) impulse=({impulse_y:.1},{impulse_x:.1}) → pending=({:.1},{:.1}) vel=({:.1},{:.1}) accel={accel_factor:.2}",
                    state.pending_impulse_y, state.pending_impulse_x,
                    state.velocity_y, state.velocity_x
                );
            }
        }
    }

    std::ptr::null()
}

//! Scroll Engine — intercepts discrete mouse scroll events and replaces them
//! with smooth, velocity-based pixel scroll events using CoreGraphics.
//!
//! Architecture:
//!   Event Tap Thread    →  captures raw scroll events via CGEventTap, suppresses originals
//!   CVDisplayLink       →  fires at display refresh rate, emits velocity-based scroll events
//!   Shared state        →  `ScrollSettings` (user config) + `ScrollState` (velocity + remainder)
//!
//! Model:
//!   Each mouse wheel tick adds an impulse to the current velocity.
//!   Every display frame emits `velocity` pixels of scroll, then decays velocity.
//!   This produces smooth, consistent movement like a native macOS trackpad.
//!
//! Lock ordering (must always be acquired in this order to prevent deadlocks):
//!   1. settings
//!   2. scroll_state

mod engine;
pub mod ffi;
mod physics;
mod state;

pub use engine::ScrollEngine;
pub use state::ScrollSettings;

use std::ffi::c_void;

pub fn has_accessibility_permission() -> bool {
    let granted = unsafe { ffi::AXIsProcessTrusted() };
    dbg_log!("AXIsProcessTrusted() → {granted}");
    granted
}

/// Check accessibility and prompt macOS to show the permission dialog if not granted.
pub fn request_accessibility_permission() -> bool {
    unsafe {
        let keys = [ffi::kAXTrustedCheckOptionPrompt];
        let values = [ffi::kCFBooleanTrue];
        let options = ffi::CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &ffi::kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
            &ffi::kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
        );
        let granted = ffi::AXIsProcessTrustedWithOptions(options);
        if !options.is_null() {
            ffi::CFRelease(options);
        }
        dbg_log!("AXIsProcessTrustedWithOptions(prompt=true) → {granted}");
        granted
    }
}

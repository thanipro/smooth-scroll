//! Window snapping via the Accessibility API — move/resize the frontmost
//! app's focused window to halves, maximised, centred, or another display.

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_foundation::NSRect;
use std::ffi::c_void;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowOp {
    LeftHalf,
    RightHalf,
    Maximize,
    Center,
    NextDisplay,
}

impl WindowOp {
    pub fn label(self) -> &'static str {
        match self {
            WindowOp::LeftHalf => "Window: left half",
            WindowOp::RightHalf => "Window: right half",
            WindowOp::Maximize => "Window: maximize",
            WindowOp::Center => "Window: centre",
            WindowOp::NextDisplay => "Window: next display",
        }
    }
}

// ─── AX FFI ──────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGSize {
    width: f64,
    height: f64,
}

type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;
type AXError = i32;
const K_AX_VALUE_CGPOINT: u32 = 1;
const K_AX_VALUE_CGSIZE: u32 = 2;
const K_AX_ERROR_SUCCESS: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> AXValueRef;
    fn AXValueGetValue(value: AXValueRef, the_type: u32, value_ptr: *mut c_void) -> bool;
}

/// A rectangle in AX coordinates: origin top-left of the primary display, y down.
#[derive(Clone, Copy, Debug)]
struct AxRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

struct FocusedWindow {
    element: AXUIElementRef,
}

impl Drop for FocusedWindow {
    fn drop(&mut self) {
        unsafe { CFRelease(self.element) }
    }
}

impl FocusedWindow {
    fn of_frontmost_app() -> Option<Self> {
        let pid = NSWorkspace::sharedWorkspace()
            .frontmostApplication()?
            .processIdentifier();
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            return None;
        }
        let attr = CFString::new("AXFocusedWindow");
        let mut win: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut win) };
        unsafe { CFRelease(app) };
        if err != K_AX_ERROR_SUCCESS || win.is_null() {
            return None;
        }
        Some(Self { element: win })
    }

    fn frame(&self) -> Option<AxRect> {
        let mut pos = CGPoint { x: 0.0, y: 0.0 };
        let mut size = CGSize { width: 0.0, height: 0.0 };
        for (name, ty, out) in [
            ("AXPosition", K_AX_VALUE_CGPOINT, &mut pos as *mut CGPoint as *mut c_void),
            ("AXSize", K_AX_VALUE_CGSIZE, &mut size as *mut CGSize as *mut c_void),
        ] {
            let attr = CFString::new(name);
            let mut v: CFTypeRef = std::ptr::null();
            let err = unsafe { AXUIElementCopyAttributeValue(self.element, attr.as_concrete_TypeRef(), &mut v) };
            if err != K_AX_ERROR_SUCCESS || v.is_null() {
                return None;
            }
            let ok = unsafe { AXValueGetValue(v, ty, out) };
            unsafe { CFRelease(v) };
            if !ok {
                return None;
            }
        }
        Some(AxRect { x: pos.x, y: pos.y, w: size.width, h: size.height })
    }

    fn set_frame(&self, r: AxRect) {
        let pos = CGPoint { x: r.x, y: r.y };
        let size = CGSize { width: r.w, height: r.h };
        // Position → size → position: some apps clamp size to their minimum
        // and shift, and others refuse a position that is off-screen for the
        // *current* size. Doing position twice makes both kinds land right.
        self.set("AXPosition", K_AX_VALUE_CGPOINT, &pos as *const CGPoint as *const c_void);
        self.set("AXSize", K_AX_VALUE_CGSIZE, &size as *const CGSize as *const c_void);
        self.set("AXPosition", K_AX_VALUE_CGPOINT, &pos as *const CGPoint as *const c_void);
    }

    fn set(&self, name: &str, ty: u32, ptr: *const c_void) {
        let attr = CFString::new(name);
        let v = unsafe { AXValueCreate(ty, ptr) };
        if v.is_null() {
            return;
        }
        let err = unsafe { AXUIElementSetAttributeValue(self.element, attr.as_concrete_TypeRef(), v) };
        unsafe { CFRelease(v) };
        if err != K_AX_ERROR_SUCCESS {
            // -25200 (kAXErrorFailure) is what full-screen windows return:
            // macOS does not let the Accessibility API move or resize them.
            eprintln!(
                "[SmoothScroll] AX set {name} on '{}' failed: {err}{}",
                self.title(),
                if err == -25200 { " (full-screen window?)" } else { "" }
            );
        }
    }

    fn title(&self) -> String {
        let attr = CFString::new("AXTitle");
        let mut v: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(self.element, attr.as_concrete_TypeRef(), &mut v) };
        if err != K_AX_ERROR_SUCCESS || v.is_null() {
            return String::from("<no title>");
        }
        let s = unsafe { CFString::wrap_under_create_rule(v as CFStringRef) }.to_string();
        s
    }
}

// ─── Screens ─────────────────────────────────────────────────────────

/// Visible frames (menu bar and Dock excluded) of every screen, converted to
/// AX coordinates, in NSScreen order (index 0 = primary).
fn screen_frames_ax(mtm: MainThreadMarker) -> Vec<AxRect> {
    let screens = NSScreen::screens(mtm);
    let Some(primary) = screens.iter().next() else { return Vec::new() };
    let primary_h = primary.frame().size.height;
    screens
        .iter()
        .map(|s| ns_to_ax(s.visibleFrame(), primary_h))
        .collect()
}

/// NSScreen rects have a bottom-left origin on the primary display; AX rects
/// have a top-left origin. Flip the y axis around the primary display height.
fn ns_to_ax(r: NSRect, primary_h: f64) -> AxRect {
    AxRect {
        x: r.origin.x,
        y: primary_h - (r.origin.y + r.size.height),
        w: r.size.width,
        h: r.size.height,
    }
}

fn contains_center(screen: &AxRect, w: &AxRect) -> bool {
    let cx = w.x + w.w / 2.0;
    let cy = w.y + w.h / 2.0;
    cx >= screen.x && cx < screen.x + screen.w && cy >= screen.y && cy < screen.y + screen.h
}

// ─── Public entry ────────────────────────────────────────────────────

pub fn perform(op: WindowOp) {
    // NSScreen is main-thread only. Hotkey events are dispatched on the main
    // thread, so this is expected to succeed; bail loudly if it does not.
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("[SmoothScroll] window op called off the main thread");
        return;
    };
    let Some(win) = FocusedWindow::of_frontmost_app() else {
        eprintln!("[SmoothScroll] no focused window for frontmost app");
        return;
    };
    let Some(cur) = win.frame() else { return };
    let screens = screen_frames_ax(mtm);
    if screens.is_empty() {
        return;
    }
    let idx = screens.iter().position(|s| contains_center(s, &cur)).unwrap_or(0);
    let vf = screens[idx];

    let target = match op {
        WindowOp::LeftHalf => AxRect { x: vf.x, y: vf.y, w: (vf.w / 2.0).floor(), h: vf.h },
        WindowOp::RightHalf => {
            let half = (vf.w / 2.0).floor();
            AxRect { x: vf.x + vf.w - half, y: vf.y, w: half, h: vf.h }
        }
        WindowOp::Maximize => vf,
        WindowOp::Center => {
            let w = cur.w.min(vf.w);
            let h = cur.h.min(vf.h);
            AxRect {
                x: (vf.x + (vf.w - w) / 2.0).round(),
                y: (vf.y + (vf.h - h) / 2.0).round(),
                w,
                h,
            }
        }
        WindowOp::NextDisplay => {
            if screens.len() < 2 {
                return;
            }
            let nf = screens[(idx + 1) % screens.len()];
            // Keep the window's position and size proportional to the new display.
            let fx = (cur.x - vf.x) / vf.w;
            let fy = (cur.y - vf.y) / vf.h;
            let w = (cur.w / vf.w * nf.w).round().min(nf.w);
            let h = (cur.h / vf.h * nf.h).round().min(nf.h);
            AxRect {
                x: (nf.x + fx * nf.w).round().min(nf.x + nf.w - w),
                y: (nf.y + fy * nf.h).round().min(nf.y + nf.h - h),
                w,
                h,
            }
        }
    };
    #[allow(unused_variables)]
    let app_name = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|a| a.localizedName())
        .map(|n| n.to_string())
        .unwrap_or_default();
    dbg_log!(
        "window {op:?} on '{}' / '{}': screen#{idx} {vf:?}; {cur:?} → {target:?}",
        app_name,
        win.title()
    );
    win.set_frame(target);
    dbg_log!("  result {:?}", win.frame());
}

//! Global keyboard shortcuts — registration, persistence, and action execution.
//!
//! Shortcuts live in `<app config dir>/shortcuts.json`, separate from the
//! scroll settings so the two UIs can save independently.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut as HotKey, ShortcutState};
use crate::window::{self, WindowOp};

// ─── Model ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShortcutAction {
    /// Bring an application to the front. With `toggle`, pressing the key
    /// while that app is already frontmost hides it instead.
    App {
        name: String,
        #[serde(default)]
        toggle: bool,
    },
    /// Move/resize the frontmost app's focused window.
    Window { op: WindowOp },
}

impl ShortcutAction {
    pub fn label(&self) -> String {
        match self {
            ShortcutAction::App { name, .. } => name.clone(),
            ShortcutAction::Window { op } => op.label().to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShortcutBinding {
    pub id: String,
    /// Key combo in global-hotkey syntax, e.g. "Alt+S", "Control+Shift+T".
    pub keys: String,
    pub action: ShortcutAction,
}

impl ShortcutBinding {
    fn hotkey(&self) -> Option<HotKey> {
        self.keys.parse::<HotKey>().ok()
    }
}

/// Result of probing whether a key combo can be used.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShortcutAvailability {
    pub available: bool,
    /// "ok" | "used_here" | "taken" | "invalid"
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledApp {
    pub name: String,
    pub path: String,
}

pub struct ShortcutStore {
    pub bindings: Mutex<Vec<ShortcutBinding>>,
    pub path: Option<PathBuf>,
}

// ─── Persistence ─────────────────────────────────────────────────────

pub fn default_bindings() -> Vec<ShortcutBinding> {
    let win = |id: &str, keys: &str, op: WindowOp| ShortcutBinding {
        id: id.into(),
        keys: keys.into(),
        action: ShortcutAction::Window { op },
    };
    vec![
        ShortcutBinding {
            id: "default-slack".into(),
            keys: "Alt+S".into(),
            action: ShortcutAction::App { name: "Slack".into(), toggle: true },
        },
        ShortcutBinding {
            id: "default-terminal".into(),
            keys: "Alt+T".into(),
            action: ShortcutAction::App { name: "Ghostty".into(), toggle: true },
        },
        // Rectangle's conventions. ⌥⌘-arrows were deliberately avoided: they
        // are Ghostty's split navigation and browsers' tab switching.
        win("default-win-left", "Control+Alt+ArrowLeft", WindowOp::LeftHalf),
        win("default-win-right", "Control+Alt+ArrowRight", WindowOp::RightHalf),
        win("default-win-max", "Control+Alt+ArrowUp", WindowOp::Maximize),
        win("default-win-center", "Control+Alt+ArrowDown", WindowOp::Center),
        win("default-win-next", "Control+Alt+Command+ArrowRight", WindowOp::NextDisplay),
    ]
}

/// Marker file: which generation of defaults has been seeded into the user's
/// list. Lets new defaults appear once for existing users without re-adding
/// ones they deliberately removed.
const SEED_GENERATION: u32 = 2;

pub fn load_bindings(path: Option<&Path>) -> Vec<ShortcutBinding> {
    let Some(path) = path else { return default_bindings() };
    let mut bindings: Vec<ShortcutBinding> = match std::fs::read_to_string(path) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_else(|e| {
            eprintln!("[SmoothScroll] shortcuts.json unreadable ({e}); starting empty");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    let marker = path.with_extension("seed");
    let seeded: u32 = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if seeded < SEED_GENERATION {
        for d in default_bindings() {
            if !bindings.iter().any(|b| b.id == d.id) {
                bindings.push(d);
            }
        }
        let _ = std::fs::write(&marker, SEED_GENERATION.to_string());
        save_bindings(Some(path), &bindings);
    }
    bindings
}

pub fn save_bindings(path: Option<&Path>, bindings: &[ShortcutBinding]) {
    let Some(path) = path else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(bindings) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                eprintln!("[SmoothScroll] Failed to save shortcuts: {e}");
            }
        }
        Err(e) => eprintln!("[SmoothScroll] Failed to serialise shortcuts: {e}"),
    }
}

// ─── Registration ────────────────────────────────────────────────────

/// Register every stored binding. Bindings whose combo is held by another
/// app are left in the list (so the UI can show them) but logged.
pub fn register_all<R: Runtime>(app: &AppHandle<R>) {
    let store = app.state::<ShortcutStore>();
    let bindings = match store.bindings.lock() {
        Ok(b) => b.clone(),
        Err(_) => return,
    };
    for b in bindings {
        match b.hotkey() {
            Some(hk) => {
                if let Err(e) = app.global_shortcut().register(hk) {
                    eprintln!("[SmoothScroll] Could not register {}: {e}", b.keys);
                }
            }
            None => eprintln!("[SmoothScroll] Invalid shortcut syntax: {}", b.keys),
        }
    }
}

/// Probe a combo: valid syntax, not already used by us, and not held by
/// another process (macOS refuses to register a hotkey someone else owns).
pub fn check_availability<R: Runtime>(app: &AppHandle<R>, keys: &str) -> ShortcutAvailability {
    let Ok(hk) = keys.parse::<HotKey>() else {
        return ShortcutAvailability {
            available: false,
            status: "invalid".into(),
            detail: format!("'{keys}' is not a valid key combination"),
        };
    };
    if let Ok(bindings) = app.state::<ShortcutStore>().bindings.lock() {
        if let Some(b) = bindings.iter().find(|b| b.hotkey().as_ref() == Some(&hk)) {
            return ShortcutAvailability {
                available: false,
                status: "used_here".into(),
                detail: format!("Already assigned to {} in Smooth Scroll", b.action.label()),
            };
        }
    }
    // Trial registration — the only reliable way to learn whether another
    // app owns the combo. Unregister immediately on success.
    let gs = app.global_shortcut();
    match gs.register(hk) {
        Ok(()) => {
            let _ = gs.unregister(hk);
            ShortcutAvailability { available: true, status: "ok".into(), detail: "Available".into() }
        }
        Err(e) => ShortcutAvailability {
            available: false,
            status: "taken".into(),
            detail: format!("Held by another app or macOS ({e})"),
        },
    }
}

// ─── Event handling ──────────────────────────────────────────────────

pub fn handle_event<R: Runtime>(app: &AppHandle<R>, pressed: &HotKey, state: ShortcutState) {
    if state != ShortcutState::Pressed {
        return;
    }
    let binding = app
        .state::<ShortcutStore>()
        .bindings
        .lock()
        .ok()
        .and_then(|b| b.iter().find(|b| b.hotkey().as_ref() == Some(pressed)).cloned());
    let Some(binding) = binding else { return };
    dbg_log!("shortcut {} fired → {:?}", binding.keys, binding.action);
    match binding.action {
        ShortcutAction::App { name, toggle } => focus_or_toggle_app(&name, toggle),
        ShortcutAction::Window { op } => window::perform(op),
    }
}

/// Bring `name` to the front; if `toggle` and it is already frontmost, hide it.
fn focus_or_toggle_app(name: &str, toggle: bool) {
    if toggle && frontmost_app_is(name) {
        hide_frontmost_app();
        return;
    }
    // `open -a` launches if needed, otherwise activates and unhides.
    let _ = std::process::Command::new("open").arg("-a").arg(name).spawn();
}

#[cfg(target_os = "macos")]
fn frontmost_app_is(name: &str) -> bool {
    use objc2_app_kit::NSWorkspace;
    let ws = NSWorkspace::sharedWorkspace();
    let Some(front) = ws.frontmostApplication() else { return false };
    // Compare on the bundle's file name ("Slack.app") — stable across
    // localisation, unlike localizedName.
    let by_bundle = front
        .bundleURL()
        .and_then(|u| u.lastPathComponent())
        .map(|s| s.to_string())
        .map(|s| s.eq_ignore_ascii_case(&format!("{name}.app")))
        .unwrap_or(false);
    by_bundle
        || front
            .localizedName()
            .map(|s| s.to_string().eq_ignore_ascii_case(name))
            .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn hide_frontmost_app() {
    use objc2_app_kit::NSWorkspace;
    if let Some(front) = NSWorkspace::sharedWorkspace().frontmostApplication() {
        front.hide();
    }
}

// ─── Installed apps (for the picker) ─────────────────────────────────

pub fn installed_apps() -> Vec<InstalledApp> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(h) = home {
        dirs.push(h.join("Applications"));
    }
    let mut apps: Vec<InstalledApp> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("app") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    apps.push(InstalledApp { name: stem.to_string(), path: p.display().to_string() });
                }
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    apps
}

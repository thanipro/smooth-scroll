#[macro_use]
mod logging;
mod scroll;
mod shortcuts;
mod window;

use scroll::{has_accessibility_permission, request_accessibility_permission, ScrollEngine, ScrollSettings};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use shortcuts::{ShortcutAction, ShortcutAvailability, ShortcutBinding, ShortcutStore};

struct AppState {
    settings: Arc<Mutex<ScrollSettings>>,
    engine: Mutex<Option<ScrollEngine>>,
    /// Where settings are persisted (`<app config dir>/settings.json`).
    settings_path: Mutex<Option<std::path::PathBuf>>,
}

const SETTINGS_FILE: &str = "settings.json";
const SHORTCUTS_FILE: &str = "shortcuts.json";

/// `~/Library/Application Support/<bundle identifier>/settings.json` — the same
/// directory Tauri's `app_config_dir()` resolves to on macOS, computed here so
/// it is available before the app handle exists.
fn settings_file_path(identifier: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(identifier)
            .join(SETTINGS_FILE),
    )
}

/// Write the current settings to disk. Failures are logged, never fatal —
/// a missing config dir must not break scrolling.
fn persist_settings(state: &AppState) {
    let path = match state.settings_path.lock() {
        Ok(p) => p.clone(),
        Err(_) => None,
    };
    let Some(path) = path else { return };
    let snapshot = match state.settings.lock() {
        Ok(s) => s.clone(),
        Err(_) => return,
    };
    if let Err(e) = snapshot.save(&path) {
        eprintln!("[SmoothScroll] Failed to save settings to {}: {e}", path.display());
    } else {
        dbg_log!("Settings saved to {}", path.display());
    }
}

#[derive(serde::Serialize)]
struct EngineStatus {
    running: bool,
    accessibility_granted: bool,
    enabled: bool,
}

// ─── Tauri commands ──────────────────────────────────────────────────

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> Result<ScrollSettings, String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?.clone();
    dbg_log!("get_settings → {:?}", s);
    Ok(s)
}

#[tauri::command]
fn update_settings(state: tauri::State<AppState>, settings: ScrollSettings) -> Result<(), String> {
    dbg_log!("update_settings ← {:?}", settings);
    {
        let mut current = state.settings.lock().map_err(|e| e.to_string())?;
        *current = settings;
    }
    persist_settings(&state);
    Ok(())
}

#[tauri::command]
fn get_engine_status(state: tauri::State<AppState>) -> Result<EngineStatus, String> {
    let running = {
        let engine_lock = state.engine.lock().map_err(|e| e.to_string())?;
        engine_lock.as_ref().is_some_and(|e| e.is_running())
    };
    let enabled = state.settings.lock().map_err(|e| e.to_string())?.enabled;
    let accessibility = has_accessibility_permission();
    dbg_log!("get_engine_status → running={running}, accessibility={accessibility}, enabled={enabled}");
    Ok(EngineStatus { running, accessibility_granted: accessibility, enabled })
}

#[tauri::command]
fn check_accessibility() -> bool {
    let granted = has_accessibility_permission();
    dbg_log!("check_accessibility → {granted}");
    granted
}

#[tauri::command]
fn open_accessibility_settings() {
    dbg_log!("open_accessibility_settings invoked");
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
}

#[tauri::command]
fn start_scroll_engine(state: tauri::State<AppState>) -> Result<(), String> {
    dbg_log!("start_scroll_engine invoked");
    let mut engine_lock = state.engine.lock().map_err(|e| e.to_string())?;
    if engine_lock.as_ref().is_some_and(|e| e.is_running()) {
        dbg_log!("start_scroll_engine: already running, skipping");
        return Ok(());
    }
    engine_lock.take();

    let engine = ScrollEngine::new(state.settings.clone());
    match engine.start() {
        Ok(()) => {
            dbg_log!("start_scroll_engine: engine started successfully");
            *engine_lock = Some(engine);
            Ok(())
        }
        Err(e) => {
            dbg_log!("start_scroll_engine: FAILED — {e}");
            Err(e)
        }
    }
}

#[tauri::command]
fn stop_scroll_engine(state: tauri::State<AppState>) -> Result<(), String> {
    dbg_log!("stop_scroll_engine invoked");
    let mut engine_lock = state.engine.lock().map_err(|e| e.to_string())?;
    if let Some(engine) = engine_lock.take() {
        engine.stop();
        dbg_log!("stop_scroll_engine: engine stopped");
    } else {
        dbg_log!("stop_scroll_engine: no engine was running");
    }
    Ok(())
}

fn toggle_enabled(state: &tauri::State<AppState>) -> Result<bool, String> {
    let now_enabled = {
        let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
        settings.enabled = !settings.enabled;
        dbg_log!("toggle_enabled → now {}", settings.enabled);
        settings.enabled
    };
    persist_settings(state);
    Ok(now_enabled)
}

#[tauri::command]
fn toggle_scroll_engine(state: tauri::State<AppState>) -> Result<bool, String> {
    toggle_enabled(&state)
}

// ─── Shortcut commands ───────────────────────────────────────────────

#[tauri::command]
fn list_shortcuts(store: tauri::State<ShortcutStore>) -> Result<Vec<ShortcutBinding>, String> {
    Ok(store.bindings.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn check_shortcut(app: tauri::AppHandle, keys: String) -> ShortcutAvailability {
    shortcuts::check_availability(&app, &keys)
}

#[tauri::command]
fn add_shortcut(
    app: tauri::AppHandle,
    store: tauri::State<ShortcutStore>,
    keys: String,
    action: ShortcutAction,
) -> Result<ShortcutBinding, String> {
    let probe = shortcuts::check_availability(&app, &keys);
    if !probe.available {
        return Err(probe.detail);
    }
    let hk = keys
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .map_err(|e| e.to_string())?;
    app.global_shortcut().register(hk).map_err(|e| e.to_string())?;
    let binding = ShortcutBinding {
        id: format!("{:x}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)),
        keys,
        action,
    };
    {
        let mut b = store.bindings.lock().map_err(|e| e.to_string())?;
        b.push(binding.clone());
        shortcuts::save_bindings(store.path.as_deref(), &b);
    }
    Ok(binding)
}

#[tauri::command]
fn update_shortcut(
    store: tauri::State<ShortcutStore>,
    id: String,
    action: ShortcutAction,
) -> Result<(), String> {
    let mut b = store.bindings.lock().map_err(|e| e.to_string())?;
    let Some(entry) = b.iter_mut().find(|s| s.id == id) else {
        return Err("Shortcut not found".into());
    };
    entry.action = action;
    shortcuts::save_bindings(store.path.as_deref(), &b);
    Ok(())
}

#[tauri::command]
fn remove_shortcut(
    app: tauri::AppHandle,
    store: tauri::State<ShortcutStore>,
    id: String,
) -> Result<(), String> {
    let mut b = store.bindings.lock().map_err(|e| e.to_string())?;
    let Some(pos) = b.iter().position(|s| s.id == id) else {
        return Err("Shortcut not found".into());
    };
    let removed = b.remove(pos);
    if let Ok(hk) = removed.keys.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        let _ = app.global_shortcut().unregister(hk);
    }
    shortcuts::save_bindings(store.path.as_deref(), &b);
    Ok(())
}

#[tauri::command]
fn list_apps() -> Vec<shortcuts::InstalledApp> {
    shortcuts::installed_apps()
}

// ─── App entry point ─────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(debug_assertions)]
    logging::init();
    dbg_log!("=== Smooth Scroll starting ===");

    let ax_granted = request_accessibility_permission();
    dbg_log!("Accessibility permission (with prompt): {}", ax_granted);

    // Load persisted settings *before* the app (and its webview) is built, so
    // the engine and the settings window both start from the saved values.
    // Loading later in `setup` races the webview's initial `get_settings` call.
    let context = tauri::generate_context!();
    let settings_path = settings_file_path(&context.config().identifier);
    let loaded = match &settings_path {
        Some(p) => ScrollSettings::load(p),
        None => ScrollSettings::default(),
    };
    dbg_log!("Settings from {:?}: {:?}", settings_path, loaded);
    let loaded_enabled = loaded.enabled;
    let settings = Arc::new(Mutex::new(loaded));

    let shortcuts_path = settings_path
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.join(SHORTCUTS_FILE)));
    let bindings = shortcuts::load_bindings(shortcuts_path.as_deref());
    let shortcut_store = ShortcutStore { bindings: Mutex::new(bindings), path: shortcuts_path };

    let app_state = AppState {
        settings: settings.clone(),
        engine: Mutex::new(None),
        settings_path: Mutex::new(settings_path),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    shortcuts::handle_event(app, shortcut, event.state);
                })
                .build(),
        )
        .manage(app_state)
        .manage(shortcut_store)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                dbg_log!("Window close requested — hiding instead of quitting");
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            update_settings,
            get_engine_status,
            check_accessibility,
            open_accessibility_settings,
            start_scroll_engine,
            stop_scroll_engine,
            toggle_scroll_engine,
            list_shortcuts,
            check_shortcut,
            add_shortcut,
            update_shortcut,
            remove_shortcut,
            list_apps,
        ])
        .setup(move |app| {
            dbg_log!("Tauri setup starting — building tray menu");
            let toggle = CheckMenuItemBuilder::with_id("toggle", "Smooth Scroll")
                .checked(loaded_enabled)
                .build(app)?;
            let settings_item =
                MenuItemBuilder::with_id("settings", "Settings...").build(app)?;

            let autostart_mgr = app.autolaunch();
            let autostart_enabled = autostart_mgr.is_enabled().unwrap_or(false);
            dbg_log!("Launch at Login currently: {autostart_enabled}");

            let launch_at_login =
                CheckMenuItemBuilder::with_id("launch_at_login", "Launch at Login")
                    .checked(autostart_enabled)
                    .build(app)?;

            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

            let menu = MenuBuilder::new(app)
                .item(&toggle)
                .item(&settings_item)
                .item(&launch_at_login)
                .separator()
                .item(&quit)
                .build()?;

            let tray_icon =
                tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

            let _tray = TrayIconBuilder::new()
                .icon(tray_icon)
                .icon_as_template(true)
                .tooltip("Smooth Scroll")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "toggle" => {
                        dbg_log!("Tray menu: toggle clicked");
                        let state: tauri::State<AppState> = app.state();
                        if let Ok(now_enabled) = toggle_enabled(&state) {
                            dbg_log!("Tray menu: smooth scroll now {}", if now_enabled { "ON" } else { "OFF" });
                        }
                    }
                    "settings" => {
                        dbg_log!("Tray menu: settings clicked");
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "launch_at_login" => {
                        dbg_log!("Tray menu: launch_at_login clicked");
                        let autostart = app.autolaunch();
                        let currently_enabled = autostart.is_enabled().unwrap_or(false);
                        if currently_enabled {
                            let _ = autostart.disable();
                            dbg_log!("Autostart disabled");
                        } else {
                            let _ = autostart.enable();
                            dbg_log!("Autostart enabled");
                        }
                    }
                    "quit" => {
                        dbg_log!("Tray menu: quit clicked — shutting down");
                        let state: tauri::State<AppState> = app.state();
                        if let Ok(mut engine_lock) = state.engine.lock() {
                            if let Some(engine) = engine_lock.take() {
                                engine.stop();
                            }
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // Auto-start scroll engine
            dbg_log!("Auto-starting scroll engine...");
            let state: tauri::State<AppState> = app.state();
            let engine = ScrollEngine::new(state.settings.clone());
            match engine.start() {
                Ok(()) => {
                    dbg_log!("Auto-start: engine started OK");
                    if let Ok(mut lock) = state.engine.lock() {
                        *lock = Some(engine);
                    }
                }
                Err(e) => {
                    eprintln!("[SmoothScroll] Engine not started: {e}");
                    dbg_log!("Auto-start FAILED: {e}");
                }
            }

            // Global shortcuts (plugin is initialised by now)
            shortcuts::register_all(app.handle());

            dbg_log!("Tauri setup complete");
            Ok(())
        })
        .run(context)
        .expect("error while running tauri application");
}

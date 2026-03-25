#[macro_use]
mod logging;
mod scroll;

use scroll::{has_accessibility_permission, request_accessibility_permission, ScrollEngine, ScrollSettings};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

struct AppState {
    settings: Arc<Mutex<ScrollSettings>>,
    engine: Mutex<Option<ScrollEngine>>,
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
    let mut current = state.settings.lock().map_err(|e| e.to_string())?;
    *current = settings;
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
    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    settings.enabled = !settings.enabled;
    dbg_log!("toggle_enabled → now {}", settings.enabled);
    Ok(settings.enabled)
}

#[tauri::command]
fn toggle_scroll_engine(state: tauri::State<AppState>) -> Result<bool, String> {
    toggle_enabled(&state)
}

// ─── App entry point ─────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(debug_assertions)]
    logging::init();
    dbg_log!("=== Smooth Scroll starting ===");

    let ax_granted = request_accessibility_permission();
    dbg_log!("Accessibility permission (with prompt): {}", ax_granted);

    let settings = Arc::new(Mutex::new(ScrollSettings::default()));
    dbg_log!("Default settings: {:?}", *settings.lock().unwrap());

    let app_state = AppState {
        settings: settings.clone(),
        engine: Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .manage(app_state)
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
        ])
        .setup(|app| {
            dbg_log!("Tauri setup starting — building tray menu");
            let toggle = CheckMenuItemBuilder::with_id("toggle", "Smooth Scroll")
                .checked(true)
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

            dbg_log!("Tauri setup complete");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

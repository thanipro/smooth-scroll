mod scroll_engine;

use scroll_engine::{has_accessibility_permission, ScrollEngine, ScrollSettings};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};

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

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> Result<ScrollSettings, String> {
    Ok(state.settings.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn update_settings(state: tauri::State<AppState>, settings: ScrollSettings) -> Result<(), String> {
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
    Ok(EngineStatus {
        running,
        accessibility_granted: has_accessibility_permission(),
        enabled,
    })
}

#[tauri::command]
fn check_accessibility() -> bool {
    has_accessibility_permission()
}

#[tauri::command]
fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
}

#[tauri::command]
fn start_scroll_engine(state: tauri::State<AppState>) -> Result<(), String> {
    let mut engine_lock = state.engine.lock().map_err(|e| e.to_string())?;
    if engine_lock.as_ref().is_some_and(|e| e.is_running()) {
        return Ok(());
    }
    engine_lock.take();

    let engine = ScrollEngine::new(state.settings.clone());
    engine.start()?;
    *engine_lock = Some(engine);
    Ok(())
}

#[tauri::command]
fn stop_scroll_engine(state: tauri::State<AppState>) -> Result<(), String> {
    let mut engine_lock = state.engine.lock().map_err(|e| e.to_string())?;
    if let Some(engine) = engine_lock.take() {
        engine.stop();
    }
    Ok(())
}

/// Toggle enabled state — used by both the Tauri command and the tray menu.
fn toggle_enabled(state: &tauri::State<AppState>) -> Result<bool, String> {
    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    settings.enabled = !settings.enabled;
    Ok(settings.enabled)
}

#[tauri::command]
fn toggle_scroll_engine(state: tauri::State<AppState>) -> Result<bool, String> {
    toggle_enabled(&state)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let settings = Arc::new(Mutex::new(ScrollSettings::default()));

    let app_state = AppState {
        settings: settings.clone(),
        engine: Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(app_state)
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
            let toggle =
                MenuItemBuilder::with_id("toggle", "Toggle Smooth Scroll").build(app)?;
            let settings_item =
                MenuItemBuilder::with_id("settings", "Settings...").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

            let menu = MenuBuilder::new(app)
                .item(&toggle)
                .item(&settings_item)
                .separator()
                .item(&quit)
                .build()?;

            let tray_icon = tauri::image::Image::from_bytes(
                include_bytes!("../icons/tray-icon.png")
            )?;

            let _tray = TrayIconBuilder::new()
                .icon(tray_icon)
                .icon_as_template(true) // macOS: adapts to light/dark menu bar
                .tooltip("Smooth Scroll")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "toggle" => {
                        let state: tauri::State<AppState> = app.state();
                        let _ = toggle_enabled(&state);
                    }
                    "settings" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
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

            // Auto-start
            let state: tauri::State<AppState> = app.state();
            let engine = ScrollEngine::new(state.settings.clone());
            match engine.start() {
                Ok(()) => {
                    if let Ok(mut lock) = state.engine.lock() {
                        *lock = Some(engine);
                    }
                }
                Err(e) => {
                    eprintln!("[SmoothScroll] Engine not started: {e}");
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

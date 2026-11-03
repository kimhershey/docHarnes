//! DocuLens desktop shell: wires the Tauri runtime to `doculens-core`.

mod commands;
mod state;

use tauri::Manager;

use doculens_core::settings::Theme;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::bootstrap();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("doculens_core", log::LevelFilter::Debug)
                .level_for("doculens_lib", log::LevelFilter::Debug)
                .build(),
        )
        .manage(state)
        .setup(|app| {
            let state = app.state::<AppState>();
            // Apply the persisted theme to the native window (title bar) before the
            // first paint; the webview applies the same theme via `data-theme`.
            let theme = state.settings.lock().map(|s| s.ui.theme).unwrap_or_default();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_theme(native_theme(theme));
                let _ = window.set_title(doculens_core::version::APP_NAME);
            }
            // Open the workspace recorded in settings.json (decision 14).
            state.open_startup_workspace();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::settings_get,
            commands::settings_update,
            commands::settings_set_theme,
            commands::settings_set_language,
            commands::settings_set_ui_scale,
            commands::settings_set_active_model,
            commands::settings_set_tier_mode,
            commands::workspace_inspect,
            commands::workspace_open,
            commands::workspace_create,
            commands::workspace_close,
            commands::workspace_current,
            commands::workspace_forget,
            commands::workspace_set_retention,
            commands::workspace_set_preferred_model,
            commands::hw_probe,
            commands::hw_assess,
            commands::registry_list,
            commands::registry_required_files_text,
            commands::prerouter_route,
            commands::prerouter_resolve_date,
            commands::startup_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Native window theme for a persisted theme choice (`System` = follow OS).
pub(crate) fn native_theme(theme: Theme) -> Option<tauri::Theme> {
    match theme {
        Theme::Light | Theme::HighContrast => Some(tauri::Theme::Light),
        Theme::Dark => Some(tauri::Theme::Dark),
        Theme::System => None,
    }
}

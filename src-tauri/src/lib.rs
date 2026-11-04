//! DocuLens desktop shell: wires the Tauri runtime to `doculens-core`.

mod commands;
mod docs_bridge;
mod engine_bridge;
mod jobs;
mod knowledge_bridge;
mod pipeline_bridge;
mod printer;
mod protocol;
mod state;

use tauri::{Manager, RunEvent};

use doculens_core::settings::Theme;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::bootstrap();

    let app = tauri::Builder::default()
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
        .register_uri_scheme_protocol(protocol::SCHEME, protocol::handle)
        .setup(|app| {
            let handle = app.handle().clone();
            let state = app.state::<AppState>();
            let _ = state.app.set(handle.clone());
            state.jobs.attach(handle.clone());
            // Apply the persisted theme to the native window (title bar) before the
            // first paint; the webview applies the same theme via `data-theme`.
            let theme = state.settings.lock().map(|s| s.ui.theme).unwrap_or_default();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_theme(native_theme(theme));
                let _ = window.set_title(doculens_core::version::APP_NAME);
            }
            // Open the workspace recorded in settings.json (decision 14), then scan
            // the model folder, start the watcher and (when enabled) the engine.
            let _ = state.printer.set(std::sync::Arc::new(printer::WebviewPdfPrinter::new(handle.clone())));
            state.open_startup_workspace();
            engine_bridge::on_workspace_changed(&handle);
            docs_bridge::on_workspace_changed(&handle);
            knowledge_bridge::on_workspace_changed(&handle);
            pipeline_bridge::on_workspace_changed(&handle);
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
            commands::models_scan,
            commands::models_list,
            commands::models_import_plan,
            commands::models_import_run,
            commands::models_download,
            commands::models_hash,
            commands::models_delete,
            commands::engine_status,
            commands::engine_start,
            commands::engine_stop,
            commands::engine_restart,
            commands::engine_load_model,
            commands::engine_unload_model,
            commands::engine_refresh_models,
            commands::engine_chat_test,
            commands::engine_list_devices,
            commands::engine_binary_info,
            commands::engine_logs,
            commands::presets_preview,
            commands::jobs_list,
            commands::jobs_cancel,
            commands::jobs_clear_finished,
            commands::prerouter_route,
            commands::prerouter_resolve_date,
            commands::startup_status,
            commands::documents::documents_list,
            commands::documents::documents_stats,
            commands::documents::documents_get,
            commands::documents::documents_pages,
            commands::documents::documents_page_text,
            commands::documents::documents_import_plan,
            commands::documents::documents_import_run,
            commands::documents::documents_rescan,
            commands::documents::documents_inbox_scan,
            commands::documents::documents_rerender,
            commands::documents::documents_rotate_page,
            commands::documents::documents_delete,
            commands::documents::documents_set_tags,
            commands::documents::documents_set_doc_type,
            commands::documents::documents_set_title,
            commands::documents::documents_tags,
            commands::documents::documents_set_inbox_watch,
            commands::documents::documents_pdfium_info,
            commands::knowledge::knowledge_status,
            commands::knowledge::knowledge_collections,
            commands::knowledge::knowledge_collection_save,
            commands::knowledge::knowledge_collection_delete,
            commands::knowledge::knowledge_collection_docs,
            commands::knowledge::knowledge_collection_add,
            commands::knowledge::knowledge_collection_remove,
            commands::knowledge::knowledge_document_collections,
            commands::knowledge::knowledge_set_role,
            commands::knowledge::knowledge_index,
            commands::knowledge::knowledge_embed,
            commands::knowledge::knowledge_reset_embeddings,
            commands::knowledge::knowledge_search,
            commands::knowledge::knowledge_chunks,
            commands::knowledge::knowledge_answer,
            commands::knowledge::knowledge_answer_cancel,
            commands::knowledge::knowledge_answers,
            commands::knowledge::knowledge_answer_feedback,
            commands::knowledge::knowledge_answer_delete,
            commands::knowledge::knowledge_glossary,
            commands::knowledge::knowledge_glossary_save,
            commands::knowledge::knowledge_glossary_delete,
            commands::knowledge::knowledge_versions,
            commands::knowledge::knowledge_version_set,
            commands::knowledge::knowledge_version_clear,
            commands::pipeline::pipeline_status,
            commands::pipeline::pipeline_process,
            commands::pipeline::pipeline_doc_types,
            commands::pipeline::pipeline_schemas,
            commands::pipeline::pipeline_schema_get,
            commands::pipeline::pipeline_schema_versions,
            commands::pipeline::pipeline_schema_save,
            commands::pipeline::pipeline_schema_delete,
            commands::pipeline::pipeline_schema_set_default,
            commands::pipeline::pipeline_schema_set_enabled,
            commands::pipeline::pipeline_schema_lint,
            commands::pipeline::pipeline_schema_from_nl,
            commands::pipeline::pipeline_form_derive,
            commands::pipeline::pipeline_form_register,
            commands::pipeline::pipeline_document_schema,
            commands::pipeline::pipeline_runs,
            commands::pipeline::pipeline_fields,
            commands::pipeline::pipeline_field_history,
            commands::pipeline::pipeline_restore,
            commands::pipeline::pipeline_revalidate,
            commands::pipeline::pipeline_review_queue,
            commands::pipeline::pipeline_review_apply,
            commands::pipeline::pipeline_review_approve_document,
            commands::pipeline::pipeline_tables,
            commands::pipeline::pipeline_table_csv,
            commands::pipeline::pipeline_split,
            commands::pipeline::pipeline_merge,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Belt and braces: the job object / process group already ties the
            // sidecar to this process, but a clean stop lets the router shut its
            // model instances down gracefully.
            let state = app.state::<AppState>();
            let _ = state.engine.stop();
        }
    });
}

/// Native window theme for a persisted theme choice (`System` = follow OS).
pub(crate) fn native_theme(theme: Theme) -> Option<tauri::Theme> {
    match theme {
        Theme::Light => Some(tauri::Theme::Light),
        // The high-contrast palette is black-on-white-text (`color-scheme: dark`).
        Theme::Dark | Theme::HighContrast => Some(tauri::Theme::Dark),
        Theme::System => None,
    }
}

//! DocuLens desktop shell: wires the Tauri runtime to `doculens-core`.

mod commands;
mod docs_bridge;
mod engine_bridge;
mod generate_bridge;
mod jobs;
mod knowledge_bridge;
mod maintenance;
mod notify;
mod pipeline_bridge;
mod planner_bridge;
mod power;
mod printer;
mod projects_bridge;
mod protocol;
mod state;

use tauri::{Manager, RunEvent};

use doculens_core::settings::Theme;

use crate::state::AppState;

/// Size at which the app log is rotated, and how many rotated files are kept next to it.
const LOG_FILE_MAX_BYTES: u128 = 2 * 1024 * 1024;
const LOG_FILES_KEPT: usize = 20;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::bootstrap();
    let log_dir = state.app_paths.logs_dir();

    let app = tauri::Builder::default()
        // Registered first: a second launch hands over to the running instance and exits before
        // anything else is set up (two instances would overwrite each other's settings file and
        // could run two engines on one workspace).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| focus_main_window(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                // The app log lives with the rest of the app data — `<AppData>/DocuLens/logs`, the
                // folder the guide names and the daily purge cleans by the "앱 로그" retention —
                // not in the Tauri identifier folder (the plugin's default: one 40 KB file there
                // that neither of them knew about). Rotated files stay until the purge removes
                // them by age; `KeepSome` bounds the folder when the purge is off.
                .clear_targets()
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout))
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                    path: log_dir,
                    file_name: None,
                }))
                .max_file_size(LOG_FILE_MAX_BYTES)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(LOG_FILES_KEPT))
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
            if let Ok(old) = app.path().app_log_dir() {
                remove_legacy_logs(&old, &state.app_paths.logs_dir());
            }
            state.jobs.attach(handle.clone());
            planner_bridge::install_worker(&handle);
            power::install(&handle);
            maintenance::install_scheduler(&handle);
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
            commands::documents::documents_set_page_text,
            commands::documents::documents_page_layout,
            commands::documents::documents_edit_page_words,
            commands::documents::documents_add_page_words,
            commands::documents::documents_page_text_history,
            commands::documents::documents_undo_page_text,
            commands::documents::documents_redo_page_text,
            commands::documents::documents_revert_page_text,
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
            commands::knowledge::chat_sessions_list,
            commands::knowledge::chat_session_create,
            commands::knowledge::chat_session_rename,
            commands::knowledge::chat_session_delete,
            commands::knowledge::chat_messages_list,
            commands::knowledge::chat_message_put,
            commands::knowledge::chat_messages_truncate,
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
            commands::pipeline::pipeline_schema_discover,
            commands::pipeline::pipeline_schema_name_exists,
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
            commands::pipeline::pipeline_table_export,
            commands::pipeline::pipeline_split,
            commands::pipeline::pipeline_merge,
            commands::planner::planner_plan,
            commands::planner::planner_execute,
            commands::planner::organize_plan,
            commands::planner::organize_apply,
            commands::planner::journal_batches,
            commands::planner::journal_entries,
            commands::planner::journal_undo,
            commands::planner::journal_redo,
            commands::planner::aggregate_run,
            commands::planner::export_preview,
            commands::planner::export_run,
            commands::planner::jobs_get,
            commands::planner::jobs_pause,
            commands::planner::jobs_resume,
            commands::planner::jobs_retry,
            commands::planner::jobs_prioritize,
            commands::generate::generate_templates,
            commands::generate::generate_budget_info,
            commands::generate::generate_draft,
            commands::generate::generate_preview,
            commands::generate::generate_validate,
            commands::generate::generate_save,
            commands::generate::generate_drafts,
            commands::generate::generate_draft_get,
            commands::generate::generate_draft_put,
            commands::generate::generate_draft_delete,
            commands::projects::projects_list,
            commands::projects::project_create,
            commands::projects::project_update,
            commands::projects::project_delete,
            commands::projects::project_get,
            commands::projects::project_add_documents,
            commands::projects::project_reference_group,
            commands::projects::project_remove_document,
            commands::projects::project_sync_copies,
            commands::projects::project_templates,
            commands::projects::project_template_save,
            commands::projects::project_template_delete,
            commands::projects::project_define_rule,
            commands::projects::project_save_rule,
            commands::projects::project_delete_rule,
            commands::projects::project_check,
            commands::projects::project_runs,
            commands::projects::project_report,
            commands::projects::project_request_supplement,
            commands::projects::project_classify_preview,
            commands::projects::project_classify_apply,
            commands::projects::maintenance_status,
            commands::projects::maintenance_run_now,
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
            // The process exits without dropping the state: close the workspace by hand so its
            // database is checkpointed and its lock file removed.
            state.release_workspace_for_exit();
            // Sleep prevention (`power`) needs no release: its thread-bound state goes with the process.
        }
    });
}

/// Up to 0.7.x the log plugin wrote to its default folder, `<LocalAppData>/com.doculens.app/logs`.
/// The log files left there are removed once, so no old log outlives the "앱 로그" retention in a
/// folder the purge never looks at. Anything that is not a log file stays.
fn remove_legacy_logs(old: &std::path::Path, current: &std::path::Path) {
    if old == current {
        return;
    }
    let Ok(entries) = std::fs::read_dir(old) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "log" || e == "bak") {
            let _ = std::fs::remove_file(&path);
        }
    }
    let _ = std::fs::remove_dir(old); // succeeds only when nothing else is in it
}

/// A second launch of the app: bring the running window to the front instead.
fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::TempDir;

    /// PKG-11: the 0.7.x log folder is emptied of its log files (and removed when that leaves it
    /// empty); the current log folder is never touched.
    #[test]
    fn legacy_log_files_are_removed_once() {
        let tmp = TempDir::new();
        let old = tmp.path().join("com.doculens.app").join("logs");
        let current = tmp.path().join("DocuLens").join("logs");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        for name in ["DocuLens.log", "DocuLens_2026-11-01_09-00-00.log", "DocuLens_2026-11-01_09-00-00.log.bak"] {
            std::fs::write(old.join(name), b"old").unwrap();
        }
        std::fs::write(current.join("DocuLens.log"), b"new").unwrap();

        remove_legacy_logs(&current, &current);
        assert!(current.join("DocuLens.log").is_file(), "same folder: nothing to clean");
        remove_legacy_logs(&old, &current);
        assert!(!old.exists());
        assert!(current.join("DocuLens.log").is_file());

        // Something that is not a log keeps the folder and itself.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("notes.txt"), b"keep").unwrap();
        std::fs::write(old.join("DocuLens.log"), b"old").unwrap();
        remove_legacy_logs(&old, &current);
        assert!(old.join("notes.txt").is_file() && !old.join("DocuLens.log").exists());
        remove_legacy_logs(&tmp.path().join("missing"), &current);
    }
}

mod cleanup;
mod compare;
mod readiness;
mod record;
mod report_writer;
mod reports;

use serde::Serialize;
use tauri::window::{Effect, EffectsBuilder};
use tauri::{Manager, RunEvent};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    machine: String,
    /// True when the window got the Windows 11 Mica backdrop; the UI then drops its opaque
    /// background so the backdrop shows through.
    mica: bool,
}

/// Mica exists from Windows 11 (build 22000). Windows 10 and Server 2019 keep the solid
/// background, which matters because the window is created transparent.
fn supports_mica() -> bool {
    #[cfg(windows)]
    {
        windows_version::OsVersion::current().build >= 22000
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Closes the app through Tauri's exit API. Used by the idle timer, which only calls it when no job is running.
#[tauri::command]
fn app_exit(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn app_info(app: tauri::AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        machine: std::env::var("COMPUTERNAME").unwrap_or_default(),
        mica: supports_mica(),
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(cleanup::CleanupState::default())
        .manage(compare::CompareState::default())
        .manage(readiness::ReadinessState::default())
        .manage(record::RecordState::default())
        .setup(|app| {
            if supports_mica() {
                if let Some(window) = app.get_webview_window("main") {
                    window.set_effects(EffectsBuilder::new().effect(Effect::Mica).build())?;
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            cleanup::cleanup_drives,
            cleanup::cleanup_scan,
            cleanup::cleanup_cancel,
            cleanup::cleanup_children,
            cleanup::cleanup_path,
            cleanup::cleanup_reveal,
            cleanup::cleanup_delete,
            cleanup::cleanup_delete_preview,
            cleanup::cleanup_delete_many,
            cleanup::cleanup_last_run,
            cleanup::cleanup_restore_last,
            cleanup::cleanup_junk,
            cleanup::cleanup_dupes_find,
            cleanup::cleanup_dupes_cancel,
            cleanup::cleanup_dupes_groups,
            cleanup::cleanup_dupes_summary,
            cleanup::cleanup_dupes_all_but_oldest,
            cleanup::cleanup_history,
            cleanup::cleanup_growth,
            cleanup::history_roots,
            cleanup::history_delete,
            cleanup::cleanup_owners_read,
            cleanup::cleanup_owners_cancel,
            cleanup::cleanup_owners_table,
            cleanup::cleanup_owner_children,
            cleanup::cleanup_delete_duplicates,
            cleanup::cleanup_stale,
            cleanup::cleanup_stale_files,
            cleanup::app_elevated,
            cleanup::cleanup_quick_list,
            cleanup::cleanup_quick_clean,
            cleanup::app_restart_admin,
            app_exit,
            reports::reports_list,
            reports::runbook_create,
            reports::report_delete,
            reports::reports_prune,
            reports::report_export,
            reports::report_open,
            reports::branding_logo,
            reports::branding_set_logo,
            reports::branding_clear_logo,
            compare::compare_start,
            compare::compare_children,
            compare::compare_permission_detail,
            compare::compare_cancel,
            compare::compare_path,
            compare::compare_reveal,
            compare::compare_missing,
            readiness::readiness_start,
            readiness::readiness_cancel,
            readiness::readiness_children,
            readiness::readiness_root,
            readiness::readiness_permissions,
            readiness::readiness_reveal,
            record::record_start,
            record::record_retry,
            record::record_delta,
            record::record_copy_missing,
            record::record_finish,
            record::record_cancel,
            record::record_recover,
            record::record_load,
            record::record_list,
            record::record_reveal,
            record::share_connect,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Deep Server Analyzer");
    app.run(|handle, event| {
        // Removals wait a couple of seconds for their report write; nothing may be left behind.
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            handle.state::<cleanup::CleanupState>().flush_report();
        }
    });
}

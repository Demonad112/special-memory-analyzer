//! Reports page commands: list, delete, export (HTML or CSV) and open in the browser.

use report_core::{Entry, ReportKind};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Html,
    Csv,
}

fn roots() -> (PathBuf, PathBuf) {
    (report_core::reports_root(), transfer_core::records_root())
}

fn generated() -> String {
    transfer_core::format_local(transfer_core::now_ms())
}

#[tauri::command]
pub async fn reports_list() -> Result<Vec<Entry>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (reports, records) = roots();
        report_core::list(&reports, &records)
    })
    .await
    .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn report_delete(kind: ReportKind, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (reports, records) = roots();
        report_core::delete(&reports, &records, kind, &id)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Deletes saved Compare, Disk usage and Quick cleanup reports older than `days` days (0 keeps
/// everything). Record runs are never pruned. Returns how many were removed.
#[tauri::command]
pub async fn reports_prune(days: u32) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        report_core::prune(&report_core::reports_root(), days, transfer_core::now_ms())
    })
    .await
    .map_err(|err| err.to_string())
}

/// Writes the report to `path` (chosen in a save dialog), headed with the company name.
#[tauri::command]
pub async fn report_export(
    kind: ReportKind,
    id: String,
    format: Format,
    path: String,
    brand: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (reports, records) = roots();
        let document = report_core::document(&reports, &records, kind, &id)?;
        let text = match format {
            Format::Html => document.html(&brand, &generated()),
            Format::Csv => document.csv(),
        };
        fs::write(&path, text).map_err(|err| format!("Could not save {path}: {err}"))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Opens the report as a web page in the default browser (to read, print or save as PDF).
#[tauri::command]
pub async fn report_open(kind: ReportKind, id: String, brand: String) -> Result<(), String> {
    let page = tauri::async_runtime::spawn_blocking(move || {
        let (reports, records) = roots();
        let document = report_core::document(&reports, &records, kind, &id)?;
        let folder = std::env::temp_dir().join("Deep Server Analyzer reports");
        fs::create_dir_all(&folder).map_err(|err| err.to_string())?;
        let page = folder.join(format!("{id}.html"));
        fs::write(&page, document.html(&brand, &generated())).map_err(|err| err.to_string())?;
        Ok::<PathBuf, String>(page)
    })
    .await
    .map_err(|err| err.to_string())??;
    tauri_plugin_opener::open_path(&page, None::<&str>)
        .map_err(|err| format!("Could not open the report: {err}"))
}

//! Reports page commands: list, delete, export (HTML or CSV) and open in the browser.

use cleanup_core::Protected;
use report_core::{Entry, ReportKind};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Html,
    Csv,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Html => "html",
            Format::Csv => "csv",
        }
    }
}

/// Whether `path` is a drive-letter or UNC path. Spelled out so the answer is the same on every
/// OS the tests run on.
fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || path.starts_with("\\\\") || Path::new(path).is_absolute()
}

/// Checks the path the webview sent for `report_export`. It must be what a save dialog gives:
/// absolute, free of `..`, ending in the extension of the chosen format, outside the folders
/// Windows and installed programs need and the app's own data, and not an existing non-file.
fn check_export_path(path: &str, format: Format, protected: &Protected) -> Result<PathBuf, String> {
    let refused = |why: &str| Err(format!("Could not save {path}: {why}"));
    if !is_absolute(path) {
        return refused("choose a full path.");
    }
    let normal = path.replace('/', "\\");
    if normal.split('\\').any(|part| part == "..") {
        return refused("the path may not contain \"..\".");
    }
    let wanted = format.extension();
    let extension = Path::new(path).extension().and_then(|ext| ext.to_str());
    if !extension.is_some_and(|ext| ext.eq_ignore_ascii_case(wanted)) {
        return refused(&format!("the file must end in .{wanted}."));
    }
    // Everything on the drive is "inside the scan" for `check`, which refuses the protected
    // folders (Windows, Program Files, the app's data) and system names.
    let local = normal.strip_prefix("\\\\?\\").unwrap_or(&normal);
    let root = local
        .get(..3)
        .filter(|head| head.as_bytes()[0].is_ascii_alphabetic() && head.as_bytes()[1] == b':');
    let kept = match root {
        Some(root) => protected.check(Path::new(path), Path::new(root)),
        None => Ok(()),
    };
    if kept.is_err() || protected.is_protected_root(Path::new(path)) {
        return refused("that location is protected. Choose another folder.");
    }
    let target = PathBuf::from(path);
    if fs::symlink_metadata(&target).is_ok_and(|meta| !meta.is_file()) {
        return refused("that is not a regular file.");
    }
    Ok(target)
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

/// Writes the report to `path` (chosen in a save dialog and checked again here), headed with the
/// company name.
#[tauri::command]
pub async fn report_export(
    kind: ReportKind,
    id: String,
    format: Format,
    path: String,
    brand: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let protected = Protected::for_this_pc().with_extra_inside([
            transfer_core::records_root(),
            report_core::reports_root(),
            report_core::logs_root(),
        ]);
        let target = check_export_path(&path, format, &protected)?;
        let (reports, records) = roots();
        let document = report_core::document(&reports, &records, kind, &id)?;
        let text = match format {
            Format::Html => document.html(&brand, &generated()),
            Format::Csv => document.csv(),
        };
        fs::write(&target, text).map_err(|err| format!("Could not save {path}: {err}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn protected() -> Protected {
        Protected::new(
            &[r"C:\Windows", r"C:\Program Files"],
            &[r"C:\Users", r"C:\Users\tech"],
            &[r"C:\Users"],
        )
        .with_extra_inside([PathBuf::from(r"C:\Data\Deep Server Analyzer")])
    }

    fn check(path: &str, format: Format) -> Result<PathBuf, String> {
        check_export_path(path, format, &protected())
    }

    #[test]
    fn accepts_a_dialog_style_path_with_either_case_of_extension() {
        assert!(check(r"C:\Users\tech\Desktop\report.html", Format::Html).is_ok());
        assert!(check(r"D:\Reports\REPORT.CSV", Format::Csv).is_ok());
        assert!(check(r"\\server\share\report.html", Format::Html).is_ok());
    }

    #[test]
    fn refuses_relative_paths_and_parent_hops() {
        assert!(check("report.html", Format::Html).is_err());
        assert!(check(r"..\report.html", Format::Html).is_err());
        assert!(check(r"C:\Users\tech\..\..\Windows\x.html", Format::Html).is_err());
    }

    #[test]
    fn refuses_the_wrong_extension_for_the_format() {
        assert!(check(r"C:\Users\tech\x.csv", Format::Html).is_err());
        assert!(check(r"C:\Users\tech\x.html", Format::Csv).is_err());
        assert!(check(r"C:\Users\tech\x.dll", Format::Html).is_err());
        assert!(check(r"C:\Users\tech\x", Format::Csv).is_err());
    }

    #[test]
    fn refuses_system_program_and_app_data_folders() {
        assert!(check(r"C:\Windows\x.html", Format::Html).is_err());
        assert!(check(r"C:\windows\System32\x.csv", Format::Csv).is_err());
        assert!(check(r"C:\Program Files\App\x.html", Format::Html).is_err());
        assert!(check(r"C:\Data\Deep Server Analyzer\Reports\x.html", Format::Html).is_err());
        assert!(check(r"C:\$Recycle.Bin\x.html", Format::Html).is_err());
    }

    #[test]
    fn refuses_an_existing_folder_with_a_good_name() {
        let temp = std::env::temp_dir().join(format!("dsa-export-{}", std::process::id()));
        fs::create_dir_all(&temp).unwrap();
        let folder = temp.join("odd.html");
        fs::create_dir(&folder).unwrap();
        let path = folder.to_string_lossy().into_owned();
        let open = Protected::default();
        assert!(check_export_path(&path, Format::Html, &open).is_err());
        let file = temp.join("fine.html");
        fs::write(&file, "old").unwrap();
        let path = file.to_string_lossy().into_owned();
        // A temp path is only absolute in the Windows sense on Windows; skip the rest elsewhere.
        if is_absolute(&path) {
            assert!(check_export_path(&path, Format::Html, &open).is_ok());
        }
        let _ = fs::remove_dir_all(&temp);
    }
}

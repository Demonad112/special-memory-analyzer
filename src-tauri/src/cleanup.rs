//! Disk Cleanup commands: scan one folder or drive with streamed progress, keep the tree in
//! memory, hand the UI one folder level at a time, and recycle or delete items from it.

use cleanup_core::bulk::{self, ItemResult, Outcome, Preview, PreviewStatus, Target};
use cleanup_core::junk::{self, JunkGroup};
use cleanup_core::log::{self, LogEntry};
use cleanup_core::quick::{self, Places, QuickId, QuickWin};
use cleanup_core::restore::{self, LastRun, RestoreResult};
use cleanup_core::{Drive, Overview, Protected, Row};
use report_core::{
    Body, CleanReport, CleanedItem, FileItem, FolderItem, JobInfo, Removed, Saved, TypeItem,
    UsageReport,
};
use scan_core::{clean_path, scan, CancelToken, NodeId, ScanProgress, ScanState, Tree};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::State;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Returned when a scan is cancelled; the UI matches on it to stay quiet.
const CANCELLED: &str = "cancelled";
const NO_RESULT: &str = "That scan is no longer available. Scan again.";

/// The never-delete list for this PC, plus the app's own data folders wherever the environment
/// variables put them, so Disk Cleanup can't delete saved records, reports or the deletion log.
fn protected() -> Protected {
    protected_with([
        transfer_core::records_root(),
        report_core::reports_root(),
        report_core::logs_root(),
    ])
}

fn protected_with(data_roots: impl IntoIterator<Item = PathBuf>) -> Protected {
    Protected::for_this_pc().with_extra_inside(data_roots)
}

#[derive(Default)]
pub struct CleanupState {
    cancel: Mutex<Option<CancelToken>>,
    tree: Mutex<Option<Tree>>,
    /// The scan's saved report; deletes are added to it.
    report: Mutex<Option<Saved>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    path: String,
    overview: Overview,
    rows: Vec<Row>,
    elapsed_ms: u64,
    report_id: Option<String>,
}

#[tauri::command]
pub async fn cleanup_drives() -> Result<Vec<Drive>, String> {
    tauri::async_runtime::spawn_blocking(cleanup_core::drives)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn cleanup_scan(
    state: State<'_, CleanupState>,
    path: String,
    job: JobInfo,
    on_progress: Channel<ScanProgress>,
) -> Result<ScanResult, String> {
    let path = clean_path(&path);
    if path.as_os_str().is_empty() {
        return Err("Pick a drive or folder to scan.".into());
    }
    // One canonical form for the tree root, so every later never-delete check sees the real
    // folder whatever name the user typed (short name, admin share, subst drive).
    let path = scan_core::canonical_root(&path)?;
    let cancel = CancelToken::new();
    if let Some(previous) = lock(&state.cancel)?.replace(cancel.clone()) {
        previous.cancel();
    }
    // Free the previous tree before building the next one.
    lock(&state.tree)?.take();

    let started = Instant::now();
    let job_cancel = cancel.clone();
    let job_path = path.clone();
    let tree = tauri::async_runtime::spawn_blocking(move || {
        let scan_state = ScanState::new(job_cancel);
        thread::scope(|scope| {
            let job = scope.spawn(|| scan(&job_path, &scan_state));
            while !job.is_finished() {
                // A closed channel (page left) is not an error; the scan still finishes.
                let _ = on_progress.send(scan_state.progress());
                thread::sleep(PROGRESS_INTERVAL);
            }
            let _ = on_progress.send(scan_state.progress());
            job.join()
        })
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|_| "The scan stopped unexpectedly.".to_string())?;
    if cancel.is_cancelled() {
        return Err(CANCELLED.into());
    }
    let tree = tree.map_err(|err| err.to_string())?;

    let overview = cleanup_core::overview(&tree);
    let rows = cleanup_core::rows(&tree, 0);
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let report = usage_report(&path, &tree, &overview, &rows, elapsed_ms);
    let saved = report_core::save(&report_core::reports_root(), job, Body::DiskUsage(report)).ok();
    let result = ScanResult {
        path: path.display().to_string(),
        overview,
        rows,
        elapsed_ms,
        report_id: saved.as_ref().map(|saved| saved.id.clone()),
    };
    *lock(&state.tree)? = Some(tree);
    *lock(&state.report)? = saved;
    Ok(result)
}

/// Lines kept per list in a saved disk usage report.
const REPORT_ROWS: usize = 25;

fn usage_report(
    path: &std::path::Path,
    tree: &Tree,
    overview: &Overview,
    rows: &[Row],
    elapsed_ms: u64,
) -> UsageReport {
    let root = &overview.root;
    let drive = cleanup_core::drives().into_iter().find(|drive| {
        drive
            .path
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(path.display().to_string().trim_end_matches(['\\', '/']))
    });
    UsageReport {
        path: path.display().to_string(),
        size: root.size,
        files: root.files,
        dirs: root.dirs,
        unreadable: root.errors,
        cloud_files: root.cloud_files,
        cloud_bytes: root.cloud_bytes,
        drive_total: drive.as_ref().map(|drive| drive.total),
        drive_free: drive.as_ref().map(|drive| drive.free),
        elapsed_ms,
        top_folders: rows
            .iter()
            .take(REPORT_ROWS)
            .map(|row| FolderItem {
                name: tree.path(row.id).display().to_string(),
                size: row.size,
                files: row.files,
            })
            .collect(),
        largest_files: overview
            .largest_files
            .iter()
            .take(REPORT_ROWS)
            .map(|file| FileItem {
                path: tree.path(file.id).display().to_string(),
                size: file.size,
            })
            .collect(),
        types: overview
            .file_types
            .iter()
            .take(REPORT_ROWS)
            .map(|kind| TypeItem {
                extension: kind.extension.clone(),
                size: kind.size,
                files: kind.files,
            })
            .collect(),
        removed: Vec::new(),
        explorer: Some(report_core::explorer::build(tree)),
    }
}

#[tauri::command]
pub fn cleanup_cancel(state: State<'_, CleanupState>) -> Result<(), String> {
    if let Some(cancel) = lock(&state.cancel)?.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

/// What is directly inside one folder of the last scan.
#[tauri::command]
pub fn cleanup_children(state: State<'_, CleanupState>, id: NodeId) -> Result<Vec<Row>, String> {
    with_tree(&state, |tree| Ok(cleanup_core::rows(tree, id)))
}

#[tauri::command]
pub fn cleanup_path(state: State<'_, CleanupState>, id: NodeId) -> Result<String, String> {
    item_path(&state, id).map(|path| path.display().to_string())
}

/// Opens Explorer with the item selected.
#[tauri::command]
pub fn cleanup_reveal(state: State<'_, CleanupState>, id: NodeId) -> Result<(), String> {
    let path = item_path(&state, id)?;
    tauri_plugin_opener::reveal_item_in_dir(&path)
        .map_err(|err| format!("Could not open {} in Explorer: {err}", path.display()))
}

/// Recycles (or, when `permanent`, deletes) one item, then drops it from the scan. Returns the
/// updated totals and lists. This is a one-item bulk delete, so it refuses links, is logged to
/// `deletions.jsonl` and obeys the never-delete list like the bulk path does.
#[tauri::command]
pub async fn cleanup_delete(
    state: State<'_, CleanupState>,
    id: NodeId,
    permanent: bool,
) -> Result<Overview, String> {
    let elevated = cleanup_core::is_elevated();
    if permanent && !elevated {
        return Err(NEEDS_ADMIN.into());
    }
    let (target, root, token) = with_tree(&state, |tree| {
        let preview = bulk::preview(tree, &protected(), &[id]);
        let item = &preview.items[0];
        if item.status != PreviewStatus::Go {
            return Err(item
                .reason
                .clone()
                .unwrap_or_else(|| "That item cannot be deleted.".into()));
        }
        let target = Target {
            id,
            path: PathBuf::from(&item.path),
            size: item.size,
        };
        // The one-item confirmation already happened in the UI; a single huge folder must not
        // also need the bulk token.
        Ok((target, tree.path(0), preview.brake.map(|brake| brake.token)))
    })?;

    let job_target = target.clone();
    // The log line is written by run_logged; the one-item page has nowhere to show a log error.
    let (results, _) = tauri::async_runtime::spawn_blocking(move || {
        run_logged(&[job_target], permanent, elevated, &root, token.as_deref())
    })
    .await
    .map_err(|err| err.to_string())??;
    let result = results
        .first()
        .ok_or("The delete returned no result.".to_string())?;

    let gone = matches!(result.outcome, Outcome::Done | Outcome::NotFound);
    let mut guard = lock(&state.tree)?;
    let tree = guard.as_mut().ok_or(NO_RESULT)?;
    // A new scan may have replaced the tree while Windows was deleting.
    if gone && tree.is_live(id) && tree.path(id) == target.path {
        tree.remove(id);
        if let Some(saved) = lock(&state.report)?.as_mut() {
            if let Body::DiskUsage(report) = &mut saved.body {
                report.removed.push(Removed {
                    path: result.path.clone(),
                    size: target.size,
                    permanent,
                });
                // The delete happened; a report that can't be updated isn't worth an error.
                let _ = report_core::write(&report_core::reports_root(), saved);
            }
        }
    }
    if !gone {
        return Err(single_error(result));
    }
    Ok(cleanup_core::overview(tree))
}

/// What the page shows when a one-item delete did not take the item.
fn single_error(result: &ItemResult) -> String {
    let still_there = format!("{} is still there.", result.path);
    match result.message.as_deref() {
        None => format!("{still_there} Some files in it may be open in another program."),
        Some(message) if message == still_there => {
            format!("{still_there} Some files in it may be open in another program.")
        }
        Some(message) => message.to_string(),
    }
}

/// One `deletions.jsonl` line for an item the run tried.
fn log_entry(
    result: &ItemResult,
    permanent: bool,
    elevated: bool,
    run: &str,
    user: &str,
) -> LogEntry {
    let now = transfer_core::now_ms();
    LogEntry {
        time: transfer_core::format_utc(now),
        time_ms: now,
        user: user.to_string(),
        elevated,
        path: result.path.clone(),
        size: result.size,
        method: if permanent { "permanent" } else { "recycle" }.into(),
        result: result.outcome.code().into(),
        detail: result.message.clone(),
        run: Some(run.to_string()),
    }
}

/// Runs the deletes and appends each item's log line as soon as that item is done, so a crash
/// or kill mid-run still leaves a record of what went. The deletes happened whether or not the
/// log could be written, so a failed append is returned beside the results, not as an error.
fn run_logged(
    targets: &[Target],
    permanent: bool,
    elevated: bool,
    root: &Path,
    token: Option<&str>,
) -> Result<(Vec<ItemResult>, Option<String>), String> {
    let logs = report_core::logs_root();
    let run = format!("run-{}", transfer_core::now_ms());
    let user = transfer_core::user_name();
    let mut log_error = None;
    let results = bulk::run(
        targets,
        permanent,
        &protected(),
        root,
        token,
        &mut |result| {
            let entry = log_entry(result, permanent, elevated, &run, &user);
            if let Err(err) = log::append(&logs, std::slice::from_ref(&entry)) {
                log_error
                    .get_or_insert_with(|| format!("The deletion log could not be written: {err}"));
            }
        },
    )?;
    if let Some(message) = &log_error {
        eprintln!("{message}");
    }
    Ok((results, log_error))
}

const NEEDS_ADMIN: &str =
    "Deleting several items or deleting permanently needs administrator. Restart as administrator first.";

/// True when Deep Server Analyzer runs as administrator.
#[tauri::command]
pub fn app_elevated() -> bool {
    cleanup_core::is_elevated()
}

/// Junk the scan found, in groups. Nothing is selected for the user.
#[tauri::command]
pub async fn cleanup_junk(state: State<'_, CleanupState>) -> Result<Vec<JunkGroup>, String> {
    with_tree(&state, |tree| {
        Ok(junk::find(tree, &protected(), transfer_core::now_ms()))
    })
}

/// A dry run: what deleting `ids` would do to each item. Touches nothing.
#[tauri::command]
pub fn cleanup_delete_preview(
    state: State<'_, CleanupState>,
    ids: Vec<NodeId>,
) -> Result<Preview, String> {
    with_tree(&state, |tree| Ok(bulk::preview(tree, &protected(), &ids)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteManyResult {
    results: Vec<ItemResult>,
    overview: Overview,
    /// Set when the deletes happened but `deletions.jsonl` could not be written.
    log_error: Option<String>,
}

/// Recycles (or, when `permanent`, deletes) the items one by one and reports each result.
/// More than one item, or a permanent delete, needs administrator. A big run (see
/// `bulk::needs_brake`) also needs the `confirm_token` the dry run issued for it.
#[tauri::command]
pub async fn cleanup_delete_many(
    state: State<'_, CleanupState>,
    ids: Vec<NodeId>,
    permanent: bool,
    confirm_token: Option<String>,
) -> Result<DeleteManyResult, String> {
    let elevated = cleanup_core::is_elevated();
    if !elevated && (permanent || ids.len() > 1) {
        return Err(NEEDS_ADMIN.into());
    }
    let (targets, root) = with_tree(&state, |tree| {
        let protected = protected();
        let preview = bulk::preview(tree, &protected, &ids);
        let targets: Vec<Target> = preview
            .items
            .iter()
            .filter(|item| item.status == PreviewStatus::Go)
            .map(|item| Target {
                id: item.id,
                path: PathBuf::from(&item.path),
                size: item.size,
            })
            .collect();
        Ok((targets, tree.path(0)))
    })?;
    if targets.is_empty() {
        return Err("Nothing in the selection can be deleted.".into());
    }

    let job_targets = targets.clone();
    let (results, log_error) = tauri::async_runtime::spawn_blocking(move || {
        run_logged(
            &job_targets,
            permanent,
            elevated,
            &root,
            confirm_token.as_deref(),
        )
    })
    .await
    .map_err(|err| err.to_string())??;

    let mut guard = lock(&state.tree)?;
    let tree = guard.as_mut().ok_or(NO_RESULT)?;
    let mut removed = Vec::new();
    for result in results.iter().filter(|r| r.outcome == Outcome::Done) {
        // A new scan may have replaced the tree while Windows was deleting.
        if tree.is_live(result.id) && tree.path(result.id).display().to_string() == result.path {
            tree.remove(result.id);
        }
        removed.push(Removed {
            path: result.path.clone(),
            size: result.size,
            permanent,
        });
    }
    if let Some(saved) = lock(&state.report)?.as_mut() {
        if let Body::DiskUsage(report) = &mut saved.body {
            report.removed.extend(removed);
            let _ = report_core::write(&report_core::reports_root(), saved);
        }
    }
    Ok(DeleteManyResult {
        results,
        overview: cleanup_core::overview(tree),
        log_error,
    })
}

/// The last bulk delete in the deletion log, or nothing when there is none to restore.
#[tauri::command]
pub async fn cleanup_last_run() -> Result<Option<LastRun>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        restore::summary(&log::read_all(&report_core::logs_root()))
    })
    .await
    .map_err(|err| err.to_string())
}

/// Puts the last bulk delete back from the Recycle Bin, item by item, and logs what it did.
/// Items whose original path is taken again are left alone.
#[tauri::command]
pub async fn cleanup_restore_last() -> Result<Vec<RestoreResult>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let logs = report_core::logs_root();
        let entries = log::read_all(&logs);
        if restore::last_run(&entries).is_empty() {
            return Err("There is no cleanup to restore.".to_string());
        }
        let results = restore::restore_last(&entries, &cleanup_core::recycle_bin_items()?);

        let now = transfer_core::now_ms();
        let user = transfer_core::user_name();
        let elevated = cleanup_core::is_elevated();
        let logged: Vec<LogEntry> = results
            .iter()
            .map(|result| LogEntry {
                time: transfer_core::format_utc(now),
                time_ms: now,
                user: user.clone(),
                elevated,
                path: result.path.clone(),
                size: result.size,
                method: "restore".into(),
                result: result.outcome.code().into(),
                detail: result.message.clone(),
                run: None,
            })
            .collect();
        if let Err(err) = log::append(&logs, &logged) {
            eprintln!("The deletion log could not be written: {err}");
        }
        Ok(results)
    })
    .await
    .map_err(|err| err.to_string())?
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, String> {
    mutex.lock().map_err(|err| err.to_string())
}

fn with_tree<R>(
    state: &CleanupState,
    read: impl FnOnce(&Tree) -> Result<R, String>,
) -> Result<R, String> {
    let guard = lock(&state.tree)?;
    read(guard.as_ref().ok_or(NO_RESULT)?)
}

fn item_path(state: &CleanupState, id: NodeId) -> Result<PathBuf, String> {
    with_tree(state, |tree| {
        if tree.is_live(id) {
            Ok(tree.path(id))
        } else {
            Err("That item is no longer there.".into())
        }
    })
}

/// The quick cleanup cards, measured now.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickList {
    /// Running as administrator: the system cleanups can run.
    elevated: bool,
    items: Vec<QuickWin>,
}

#[tauri::command]
pub async fn cleanup_quick_list() -> Result<QuickList, String> {
    tauri::async_runtime::spawn_blocking(|| QuickList {
        elevated: cleanup_core::is_elevated(),
        items: quick::list(&Places::for_this_pc()),
    })
    .await
    .map_err(|err| err.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickResult {
    report: CleanReport,
    report_id: Option<String>,
}

/// Runs the chosen quick cleanups one after another and saves what they freed as a report.
#[tauri::command]
pub async fn cleanup_quick_clean(ids: Vec<QuickId>, job: JobInfo) -> Result<QuickResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let places = Places::for_this_pc();
        let elevated = cleanup_core::is_elevated();
        let items = ids
            .into_iter()
            .map(|id| {
                if id.needs_admin() && !elevated {
                    return CleanedItem {
                        title: id.title().into(),
                        error: Some("needs administrator".into()),
                        ..CleanedItem::default()
                    };
                }
                match quick::clean(&places, id) {
                    Ok(cleaned) => CleanedItem {
                        title: id.title().into(),
                        freed: cleaned.freed,
                        removed_files: cleaned.removed,
                        skipped_files: cleaned.skipped,
                        error: None,
                    },
                    Err(error) => CleanedItem {
                        title: id.title().into(),
                        error: Some(error),
                        ..CleanedItem::default()
                    },
                }
            })
            .collect();
        let report = CleanReport { items };
        let report_id = report_core::save(
            &report_core::reports_root(),
            job,
            Body::Cleanup(report.clone()),
        )
        .map(|saved| saved.id)
        .ok();
        QuickResult { report, report_id }
    })
    .await
    .map_err(|err| err.to_string())
}

/// Starts Deep Server Analyzer again as administrator (Windows asks first), then closes this copy.
#[tauri::command]
pub fn app_restart_admin(
    app: tauri::AppHandle,
    record: tauri::State<'_, crate::record::RecordState>,
) -> Result<(), String> {
    if record.is_running() {
        return Err("A record is running. Finish or cancel it before restarting.".into());
    }
    run_as_admin()?;
    app.exit(0);
    Ok(())
}

#[cfg(windows)]
fn run_as_admin() -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut std::ffi::c_void,
            verb: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }
    const SW_SHOWNORMAL: i32 = 1;
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let wide = |text: &std::ffi::OsStr| -> Vec<u16> { text.encode_wide().chain([0]).collect() };
    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    // SAFETY: null-terminated strings that outlive the call; null window, parameters and folder.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values up to 32 are errors, including "the user said no".
    if result <= 32 {
        return Err("Deep Server Analyzer was not restarted as administrator.".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_as_admin() -> Result<(), String> {
    Err("Only on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(outcome: Outcome, message: Option<&str>) -> ItemResult {
        ItemResult {
            id: 7,
            path: "C:\\scan\\old".into(),
            size: 42,
            outcome,
            message: message.map(Into::into),
        }
    }

    #[test]
    fn the_apps_data_folders_are_protected_wherever_they_are() {
        let root = Path::new(r"D:\Work");
        let logs = PathBuf::from(r"D:\Work\my-logs");
        let protected = protected_with([logs.clone(), PathBuf::from(r"D:\Work\records")]);

        assert!(protected
            .check(&logs.join("deletions.jsonl"), root)
            .is_err());
        assert!(protected.check(&logs, root).is_err());
        assert!(protected
            .check(Path::new(r"D:\Work\records\run-1"), root)
            .is_err());
        assert!(protected
            .check(Path::new(r"D:\Work\other\file.bin"), root)
            .is_ok());
    }

    #[test]
    fn a_result_becomes_a_log_line_with_the_run_and_method() {
        let skipped = result(Outcome::Skipped, Some("left alone"));
        let entry = log_entry(&skipped, false, true, "run-1", "tech");
        assert_eq!((entry.method.as_str(), entry.elevated), ("recycle", true));
        assert_eq!(entry.run.as_deref(), Some("run-1"));
        assert_eq!(entry.user, "tech");
        assert_eq!(entry.result, "skipped");
        assert_eq!(entry.detail.as_deref(), Some("left alone"));
        assert_eq!(entry.size, 42);
        let done = result(Outcome::Done, None);
        assert_eq!(
            log_entry(&done, true, false, "r", "tech").method,
            "permanent"
        );
    }

    #[test]
    fn a_single_delete_that_leaves_the_item_keeps_the_old_wording() {
        let still =
            "C:\\scan\\old is still there. Some files in it may be open in another program.";
        assert_eq!(single_error(&result(Outcome::Failed, None)), still);
        assert_eq!(
            single_error(&result(
                Outcome::Failed,
                Some("C:\\scan\\old is still there.")
            )),
            still
        );
        assert_eq!(
            single_error(&result(
                Outcome::InUse,
                Some("x is open in another program.")
            )),
            "x is open in another program."
        );
    }
}

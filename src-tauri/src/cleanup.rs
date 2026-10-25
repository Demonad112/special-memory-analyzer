//! Disk Cleanup commands: scan one folder or drive with streamed progress, keep the tree in
//! memory, hand the UI one folder level at a time, and recycle or delete items from it.

use crate::report_writer::ReportWriter;
use cleanup_core::bulk::{self, ItemResult, Outcome, Preview, PreviewStatus, Target};
use cleanup_core::dupes::{self, Found, Group, Skipped};
use cleanup_core::junk::{self, JunkGroup};
use cleanup_core::log::{self, LogEntry};
use cleanup_core::owners::{self, OwnerIndex, OwnerTable};
use cleanup_core::quick::{self, Places, QuickId, QuickWin};
use cleanup_core::restore::{self, LastRun, RestoreResult};
use cleanup_core::stale::{self, StaleFile, StaleReport};
use cleanup_core::{Drive, Overview, Protected, Row};
use report_core::snapshots::{self, Forecast, Growth, RootHistory, Snapshot};
use report_core::{
    Body, CleanReport, CleanedItem, DuplicateItem, DuplicateTotals, FileItem, FolderItem, JobInfo,
    OwnerItem, OwnerTotals, Removed, StaleFolder, StaleTotals, TypeItem, UsageReport,
};
use scan_core::{clean_path, scan, CancelToken, NodeId, ScanProgress, ScanState, Tree};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

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
    /// The scan's saved report; deletes are added to it, a few seconds later (see `ReportWriter`).
    report: ReportWriter,
    /// The last duplicate search on this scan's tree.
    dupes: Mutex<Option<Found>>,
    dupes_cancel: Mutex<Option<CancelToken>>,
    /// The owner of each file of this scan, once "By owner" has run (D-15). Dropped by a new scan.
    owners: Mutex<Option<OwnerIndex>>,
    owners_cancel: Mutex<Option<CancelToken>>,
    /// The root of the current scan, and when its snapshot was saved (`None` when history is off
    /// or the write failed). Replaced by every scan.
    history: Mutex<Option<ScanHistory>>,
    /// Changes with every scan (under the `dupes` lock), so a search that started on the previous
    /// tree keeps nothing.
    scan_seq: AtomicU64,
}

struct ScanHistory {
    root: String,
    saved_ms: Option<u64>,
}

impl CleanupState {
    /// Writes removals still waiting for the report. Called on exit.
    pub fn flush_report(&self) {
        self.report.flush();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    path: String,
    overview: Overview,
    rows: Vec<Row>,
    elapsed_ms: u64,
    report_id: Option<String>,
    /// A network share or mapped network drive: the page hides every delete action, and
    /// `bulk::run` refuses them anyway.
    network: bool,
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
    record: State<'_, crate::record::RecordState>,
    path: String,
    job: JobInfo,
    // Settings > Keep scan history; on when the webview does not say.
    history: Option<bool>,
    on_progress: Channel<ScanProgress>,
) -> Result<ScanResult, String> {
    let path = clean_path(&path);
    if path.as_os_str().is_empty() {
        return Err("Pick a drive or folder to scan.".into());
    }
    // One canonical form for the tree root, so every later never-delete check sees the real
    // folder whatever name the user typed (short name, admin share, subst drive).
    let path = scan_core::canonical_root(&path)?;
    // Held until the scan is done: an elevated record and a scan must not overlap.
    let _scan = record.scan_guard()?;
    let cancel = CancelToken::new();
    if let Some(previous) = lock(&state.cancel)?.replace(cancel.clone()) {
        previous.cancel();
    }
    // A duplicate search belongs to the previous tree.
    if let Some(search) = lock(&state.dupes_cancel)?.take() {
        search.cancel();
    }
    if let Some(pass) = lock(&state.owners_cancel)?.take() {
        pass.cancel();
    }
    {
        let mut dupes = lock(&state.dupes)?;
        let mut owners = lock(&state.owners)?;
        state.scan_seq.fetch_add(1, Ordering::SeqCst);
        dupes.take();
        owners.take();
    }
    // Free the previous tree before building the next one, and write the removals the previous
    // scan's report is still waiting for.
    lock(&state.tree)?.take();
    lock(&state.history)?.take();
    state.report.flush();

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
    let mut overview = overview;
    let saved = match report_core::save(&report_core::reports_root(), job, Body::DiskUsage(report))
    {
        Ok(saved) => Some(saved),
        Err(err) => {
            overview.warning = Some(report_warning(&err));
            None
        }
    };
    let result = ScanResult {
        path: path.display().to_string(),
        overview,
        rows,
        elapsed_ms,
        report_id: saved.as_ref().map(|saved| saved.id.clone()),
        network: cleanup_core::pathsafe::is_network_path(&path),
    };
    // A completed scan (not a cancelled one: that returned above) adds to the history.
    let saved_ms = if history.unwrap_or(true) {
        save_snapshot(&path, &tree)
    } else {
        None
    };
    *lock(&state.history)? = Some(ScanHistory {
        root: path.display().to_string(),
        saved_ms,
    });
    *lock(&state.tree)? = Some(tree);
    state.report.set(saved);
    Ok(result)
}

/// What the page shows when the scan's saved report could not be written or updated.
fn report_warning(err: &str) -> String {
    format!("The saved report is out of date: {err}")
}

/// The drive `path` is the root of, when it is one.
fn drive_of(path: &std::path::Path) -> Option<Drive> {
    cleanup_core::drives().into_iter().find(|drive| {
        drive
            .path
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(path.display().to_string().trim_end_matches(['\\', '/']))
    })
}

/// Writes the scan's history snapshot. A failure is logged, never shown: the scan itself worked.
fn save_snapshot(path: &std::path::Path, tree: &Tree) -> Option<u64> {
    let drive = drive_of(path);
    let taken = transfer_core::now_ms();
    let snapshot = Snapshot::from_tree(
        &path.display().to_string(),
        tree,
        (
            drive.as_ref().map(|d| d.total),
            drive.as_ref().map(|d| d.free),
        ),
        taken,
    );
    match snapshots::save(&report_core::reports_root(), &snapshot, taken) {
        Ok(_) => Some(taken),
        Err(err) => {
            report_core::applog::error(&format!("Could not save the scan history: {err}"));
            None
        }
    }
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
    let drive = drive_of(path);
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
        // Filled in when the user runs the panels (see `ReportWriter::amend`).
        duplicates: None,
        stale: None,
        owners: None,
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
    // Logged without a run id: "Restore last cleanup" covers bulk runs only.
    let (results, log_error) = tauri::async_runtime::spawn_blocking(move || {
        run_logged(
            &[job_target],
            false,
            permanent,
            elevated,
            &root,
            token.as_deref(),
        )
    })
    .await
    .map_err(|err| err.to_string())??;
    let result = results
        .first()
        .ok_or("The delete returned no result.".to_string())?;

    let gone = matches!(result.outcome, Outcome::Done | Outcome::NotFound);
    let (overview, removed) = {
        let mut guard = lock(&state.tree)?;
        let tree = guard.as_mut().ok_or(NO_RESULT)?;
        // A new scan may have replaced the tree while Windows was deleting.
        let removed = gone && tree.is_live(id) && tree.path(id) == target.path;
        if removed {
            tree.remove(id);
        }
        (cleanup_core::overview(tree), removed)
    };
    // The report is queued outside the tree lock: the write itself comes later.
    let warning = if removed {
        state.report.removed(vec![Removed {
            path: result.path.clone(),
            size: target.size,
            permanent,
        }])
    } else {
        None
    };
    if !gone {
        return Err(single_error(result));
    }
    let mut overview = overview;
    overview.warning = single_warning(log_error, warning);
    Ok(overview)
}

/// The deletion-log error and the saved-report error of a one-item delete, as the one warning
/// the page can show.
fn single_warning(log_error: Option<String>, report_error: Option<String>) -> Option<String> {
    let parts: Vec<String> = log_error
        .into_iter()
        .chain(report_error.map(|err| report_warning(&err)))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
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
    run: Option<&str>,
    user: &str,
) -> LogEntry {
    LogEntry {
        path: result.path.clone(),
        size: result.size,
        result: result.outcome.code().into(),
        detail: result.message.clone(),
        run: run.map(str::to_string),
        ..stamped(
            transfer_core::now_ms(),
            user,
            elevated,
            if permanent { "permanent" } else { "recycle" },
        )
    }
}

/// A log line with who, when and how filled in; the caller sets the rest.
fn stamped(now: u64, user: &str, elevated: bool, method: &str) -> LogEntry {
    LogEntry {
        time: transfer_core::format_utc(now),
        time_ms: now,
        user: user.to_string(),
        elevated,
        path: String::new(),
        size: 0,
        method: method.into(),
        result: String::new(),
        detail: None,
        run: None,
    }
}

/// The `deletions.jsonl` summary line for one quick-cleanup folder. It has no run id, so
/// "Restore last cleanup" never mistakes it for a bulk delete.
fn quick_log_entry(
    id: QuickId,
    target: &Path,
    done: &quick::Cleaned,
    elevated: bool,
    user: &str,
) -> LogEntry {
    LogEntry {
        path: target.to_string_lossy().into_owned(),
        size: done.freed,
        result: "done".into(),
        detail: Some(format!("{} files, quick cleanup {id:?}", done.removed)),
        ..stamped(transfer_core::now_ms(), user, elevated, id.log_method())
    }
}

/// Runs the deletes and appends each item's log line as soon as that item is done, so a crash
/// or kill mid-run still leaves a record of what went. A bulk run stamps its lines with a run id
/// (what "Restore last cleanup" groups by); a single delete passes `bulk = false` and is logged
/// without one. The deletes happened whether or not the
/// log could be written, so a failed append is returned beside the results, not as an error.
fn run_logged(
    targets: &[Target],
    bulk: bool,
    permanent: bool,
    elevated: bool,
    root: &Path,
    token: Option<&str>,
) -> Result<(Vec<ItemResult>, Option<String>), String> {
    run_logged_in(
        targets,
        Logged {
            bulk,
            permanent,
            elevated,
            protected: &protected(),
            logs: &report_core::logs_root(),
        },
        root,
        token,
    )
}

/// How a run is done and where it is logged.
struct Logged<'a> {
    bulk: bool,
    permanent: bool,
    elevated: bool,
    protected: &'a Protected,
    logs: &'a Path,
}

fn run_logged_in(
    targets: &[Target],
    how: Logged<'_>,
    root: &Path,
    token: Option<&str>,
) -> Result<(Vec<ItemResult>, Option<String>), String> {
    let Logged {
        bulk,
        permanent,
        elevated,
        protected,
        logs,
    } = how;
    let run = bulk.then(|| format!("run-{}", transfer_core::now_ms()));
    let user = transfer_core::user_name();
    let mut log_error = None;
    let results = bulk::run(targets, permanent, protected, root, token, &mut |result| {
        let entry = log_entry(result, permanent, elevated, run.as_deref(), &user);
        if let Err(err) = log::append(logs, std::slice::from_ref(&entry)) {
            log_error
                .get_or_insert_with(|| format!("The deletion log could not be written: {err}"));
        }
    })?;
    if let Some(message) = &log_error {
        report_core::applog::error(message);
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
pub async fn cleanup_junk(app: AppHandle) -> Result<Vec<JunkGroup>, String> {
    // The search walks the whole tree: keep it on the blocking pool.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        with_tree(&state, |tree| {
            Ok(junk::find(tree, &protected(), transfer_core::now_ms()))
        })
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Totals of a duplicate search; the groups themselves stay here and are paged.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupesSummary {
    min_size: u64,
    groups: u64,
    files: u64,
    reclaimable: u64,
    skipped: Skipped,
}

impl DupesSummary {
    fn of(found: &Found) -> Self {
        Self {
            min_size: found.min_size,
            groups: found.groups.len() as u64,
            files: found.files(),
            reclaimable: found.reclaimable(),
            skipped: found.skipped,
        }
    }
}

/// Finds files with identical content in the current scan, streaming progress. Only one search
/// runs at a time; a new one cancels the last. It reads files, so like a scan it holds the scan
/// guard and cannot overlap an elevated record.
#[tauri::command]
pub async fn cleanup_dupes_find(
    app: AppHandle,
    state: State<'_, CleanupState>,
    record: State<'_, crate::record::RecordState>,
    min_size: u64,
    on_progress: Channel<dupes::Progress>,
) -> Result<DupesSummary, String> {
    if !dupes::MIN_SIZE_CHOICES.contains(&min_size) {
        return Err("Pick one of the offered minimum sizes.".into());
    }
    let _scan = record.scan_guard()?;
    let seq = state.scan_seq.load(Ordering::SeqCst);
    let cancel = CancelToken::new();
    if let Some(previous) = lock(&state.dupes_cancel)?.replace(cancel.clone()) {
        previous.cancel();
    }
    lock(&state.dupes)?.take();

    let job_cancel = cancel.clone();
    let found = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        // Taken under the tree lock; the reads below hold no lock.
        let candidates = with_tree(&state, |tree| Ok(dupes::collect(tree, min_size)))?;
        let counters = dupes::Counters::default();
        thread::scope(|scope| {
            let job = scope.spawn(|| dupes::search(candidates, &job_cancel, &counters));
            while !job.is_finished() {
                let _ = on_progress.send(counters.progress());
                thread::sleep(PROGRESS_INTERVAL);
            }
            let _ = on_progress.send(counters.progress());
            job.join()
        })
        .map_err(|_| "The duplicate search stopped unexpectedly.".to_string())
    })
    .await
    .map_err(|err| err.to_string())??;
    let Some(found) = found.filter(|_| !cancel.is_cancelled()) else {
        return Err(CANCELLED.into());
    };
    let mut slot = lock(&state.dupes)?;
    // A scan that started meanwhile replaced the tree these ids point into.
    if state.scan_seq.load(Ordering::SeqCst) != seq {
        return Err(CANCELLED.into());
    }
    let summary = DupesSummary::of(&found);
    let totals = duplicate_totals(&found);
    *slot = Some(found);
    drop(slot);
    // The search itself succeeded; a report that could not be updated is logged, not shown.
    if let Some(err) = state
        .report
        .amend(move |report| report.duplicates = Some(Box::new(totals)))
    {
        report_core::applog::error(&report_warning(&err));
    }
    Ok(summary)
}

#[tauri::command]
pub fn cleanup_dupes_cancel(state: State<'_, CleanupState>) -> Result<(), String> {
    if let Some(cancel) = lock(&state.dupes_cancel)?.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

/// Reads the owner of every file in the current scan, streaming progress, and returns the owner
/// table. On demand only (D-15): one security read per file is too slow for every scan. Like a
/// scan it holds the scan guard, so it cannot overlap an elevated record. A new scan drops the
/// result.
#[tauri::command]
pub async fn cleanup_owners_read(
    app: AppHandle,
    state: State<'_, CleanupState>,
    record: State<'_, crate::record::RecordState>,
    on_progress: Channel<owners::Progress>,
) -> Result<OwnerTable, String> {
    let _scan = record.scan_guard()?;
    let seq = state.scan_seq.load(Ordering::SeqCst);
    let cancel = CancelToken::new();
    if let Some(previous) = lock(&state.owners_cancel)?.replace(cancel.clone()) {
        previous.cancel();
    }
    lock(&state.owners)?.take();

    let job_cancel = cancel.clone();
    let index = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        // Ids are taken under the tree lock; paths are fetched a batch at a time, so the tree
        // stays usable while the reads run.
        let targets = with_tree(&state, |tree| Ok(owners::collect(tree)))?;
        let paths = |ids: &[NodeId]| -> Vec<Option<PathBuf>> {
            let Ok(tree) = state.tree.lock() else {
                return vec![None; ids.len()];
            };
            let Some(tree) = tree
                .as_ref()
                .filter(|_| state.scan_seq.load(Ordering::SeqCst) == seq)
            else {
                return vec![None; ids.len()];
            };
            ids.iter()
                .map(|&id| (tree.is_live(id) && !tree.node(id).lossy_name).then(|| tree.path(id)))
                .collect()
        };
        let counters = owners::Counters::default();
        let names = scan_core::security::SidNames::default();
        thread::scope(|scope| {
            let job = scope.spawn(|| {
                owners::read_all(
                    &targets,
                    &paths,
                    &owners::read_system,
                    &names,
                    &job_cancel,
                    &counters,
                )
            });
            while !job.is_finished() {
                let _ = on_progress.send(counters.progress());
                thread::sleep(PROGRESS_INTERVAL);
            }
            let _ = on_progress.send(counters.progress());
            job.join()
        })
        .map_err(|_| "The owner pass stopped unexpectedly.".to_string())
    })
    .await
    .map_err(|err| err.to_string())??;
    let Some(index) = index.filter(|_| !cancel.is_cancelled()) else {
        return Err(CANCELLED.into());
    };
    let mut slot = lock(&state.owners)?;
    // A scan that started meanwhile replaced the tree these ids point into.
    if state.scan_seq.load(Ordering::SeqCst) != seq {
        return Err(CANCELLED.into());
    }
    let table = with_tree(&state, |tree| Ok(index.table(tree)))?;
    let totals = owner_totals(&table);
    *slot = Some(index);
    drop(slot);
    if let Some(err) = state
        .report
        .amend(move |report| report.owners = Some(Box::new(totals)))
    {
        report_core::applog::error(&report_warning(&err));
    }
    Ok(table)
}

#[tauri::command]
pub fn cleanup_owners_cancel(state: State<'_, CleanupState>) -> Result<(), String> {
    if let Some(cancel) = lock(&state.owners_cancel)?.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

const NO_OWNERS: &str = "The owner table is no longer available. Run By owner again.";

/// The owner table for the files still in the scan, or `None` when the pass has not run on this
/// scan. Deletes since the pass are reflected without reading anything again.
#[tauri::command]
pub fn cleanup_owners_table(state: State<'_, CleanupState>) -> Result<Option<OwnerTable>, String> {
    let owners = lock(&state.owners)?;
    let Some(index) = owners.as_ref() else {
        return Ok(None);
    };
    with_tree(&state, |tree| Ok(Some(index.table(tree))))
}

/// What is directly inside folder `id`, counting only the files `owner` (an index of the owner
/// table) owns.
#[tauri::command]
pub fn cleanup_owner_children(
    state: State<'_, CleanupState>,
    id: NodeId,
    owner: u32,
) -> Result<Vec<Row>, String> {
    let owners = lock(&state.owners)?;
    let index = owners.as_ref().ok_or(NO_OWNERS)?;
    with_tree(&state, |tree| Ok(index.rows_for(tree, id, owner)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInfo {
    taken_ms: u64,
    size: u64,
    drive_used: Option<u64>,
    drive_total: Option<u64>,
    /// Saved by the scan on screen.
    current: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    root: String,
    /// Newest first.
    snapshots: Vec<SnapshotInfo>,
    forecast: Forecast,
    now_ms: u64,
}

/// The saved snapshots of the scanned folder and the forecast from them.
#[tauri::command]
pub async fn cleanup_history(app: AppHandle) -> Result<History, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        let (root, current) = scan_history(&state)?;
        let saved = snapshots::list(&report_core::reports_root(), &root);
        let now_ms = transfer_core::now_ms();
        let forecast = snapshots::forecast(&saved, now_ms);
        Ok(History {
            root,
            snapshots: saved
                .iter()
                .rev()
                .map(|snapshot| SnapshotInfo {
                    taken_ms: snapshot.taken_ms,
                    size: snapshot.size,
                    drive_used: snapshot.drive_used(),
                    drive_total: snapshot.drive_total,
                    current: current == Some(snapshot.taken_ms),
                })
                .collect(),
            forecast,
            now_ms,
        })
    })
    .await
    .map_err(|err| err.to_string())?
}

fn scan_history(state: &CleanupState) -> Result<(String, Option<u64>), String> {
    lock(&state.history)?
        .as_ref()
        .map(|history| (history.root.clone(), history.saved_ms))
        .ok_or_else(|| NO_RESULT.to_string())
}

/// Rows of the growth list sent to the webview.
const GROWTH_ROWS: usize = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthView {
    #[serde(flatten)]
    growth: Growth,
    /// Changed folders beyond the rows sent.
    hidden: usize,
}

/// What grew since an earlier snapshot of this folder (`from_ms`; the newest earlier scan when
/// omitted), measured against the scan as it is now, deletes included.
#[tauri::command]
pub async fn cleanup_growth(app: AppHandle, from_ms: Option<u64>) -> Result<GrowthView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        let (root, current) = scan_history(&state)?;
        let saved = snapshots::list(&report_core::reports_root(), &root);
        let earlier = match from_ms {
            Some(at) => saved.iter().find(|snapshot| snapshot.taken_ms == at),
            None => saved
                .iter()
                .rev()
                .find(|snapshot| Some(snapshot.taken_ms) != current),
        }
        .ok_or("There is no earlier scan of this folder to compare with yet.")?;
        let now = with_tree(&state, |tree| {
            Ok(Snapshot::from_tree(
                &root,
                tree,
                (None, None),
                transfer_core::now_ms(),
            ))
        })?;
        let mut growth = snapshots::growth(earlier, &now);
        let hidden = growth.rows.len().saturating_sub(GROWTH_ROWS);
        growth.rows.truncate(GROWTH_ROWS);
        Ok(GrowthView { growth, hidden })
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Every folder that has scan history, for Settings.
#[tauri::command]
pub async fn history_roots() -> Result<Vec<RootHistory>, String> {
    tauri::async_runtime::spawn_blocking(|| snapshots::roots(&report_core::reports_root()))
        .await
        .map_err(|err| err.to_string())
}

/// Deletes the scan history of one folder (`key` from `history_roots`) or, without a key, of all.
#[tauri::command]
pub async fn history_delete(key: Option<String>) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        snapshots::delete(&report_core::reports_root(), key.as_deref())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Owners kept in a saved disk usage report.
const REPORT_OWNERS: usize = 25;

fn owner_totals(table: &OwnerTable) -> OwnerTotals {
    OwnerTotals {
        total_disk: table.total_disk,
        total_files: table.total_files,
        owners: table
            .rows
            .iter()
            .take(REPORT_OWNERS)
            .map(|row| OwnerItem {
                name: row.name.clone(),
                files: row.files,
                disk: row.disk,
                top_folders: row
                    .top_folders
                    .iter()
                    .map(|folder| folder.name.clone())
                    .collect(),
            })
            .collect(),
    }
}

/// Groups most worth cleaning first.
const DUPES_PAGE: usize = 200;

/// A page of the last duplicate search's groups.
#[tauri::command]
pub fn cleanup_dupes_groups(
    state: State<'_, CleanupState>,
    offset: usize,
    limit: usize,
) -> Result<Vec<Group>, String> {
    let dupes = lock(&state.dupes)?;
    let found = dupes.as_ref().ok_or(NO_DUPES)?;
    Ok(found
        .groups
        .iter()
        .skip(offset)
        .take(limit.min(DUPES_PAGE))
        .cloned()
        .collect())
}

/// The totals of the last duplicate search, as they are after deletes.
#[tauri::command]
pub fn cleanup_dupes_summary(
    state: State<'_, CleanupState>,
) -> Result<Option<DupesSummary>, String> {
    Ok(lock(&state.dupes)?.as_ref().map(DupesSummary::of))
}

/// What "Select all but the oldest in each group" ticks. Nothing is ever ticked unless the user
/// presses that button.
#[tauri::command]
pub fn cleanup_dupes_all_but_oldest(state: State<'_, CleanupState>) -> Result<Vec<NodeId>, String> {
    let dupes = lock(&state.dupes)?;
    Ok(dupes::all_but_oldest(dupes.as_ref().ok_or(NO_DUPES)?))
}

const NO_DUPES: &str = "That duplicate search is no longer available. Find duplicates again.";

/// Groups and folders kept in a saved disk usage report.
const REPORT_GROUPS: usize = 25;

fn duplicate_totals(found: &Found) -> DuplicateTotals {
    DuplicateTotals {
        min_size: found.min_size,
        groups: found.groups.len() as u64,
        files: found.files(),
        reclaimable: found.reclaimable(),
        top: found
            .groups
            .iter()
            .take(REPORT_GROUPS)
            .map(|group| DuplicateItem {
                name: group
                    .members
                    .first()
                    .map(|member| member.name.clone())
                    .unwrap_or_default(),
                size: group.size,
                copies: group.members.len() as u64,
                reclaimable: group.reclaimable,
                paths: group
                    .members
                    .iter()
                    .map(|member| member.path.display().to_string())
                    .collect(),
            })
            .collect(),
    }
}

fn stale_totals(report: &StaleReport) -> StaleTotals {
    StaleTotals {
        years: report.years,
        not_opened: report.not_opened,
        now_ms: report.now_ms,
        stale_files: report.stale_files,
        stale_bytes: report.stale_bytes,
        unknown_files: report.unknown_files,
        unknown_bytes: report.unknown_bytes,
        folders: report
            .groups
            .iter()
            .take(REPORT_GROUPS)
            .map(|group| StaleFolder {
                name: group.name.clone(),
                stale_files: group.stale_files,
                stale_bytes: group.stale_bytes,
                folder_bytes: group.folder_bytes,
                newest_ms: group.newest_ms,
            })
            .collect(),
    }
}

fn stale_options(years: u32, not_opened: bool) -> Result<stale::Options, String> {
    if !stale::YEAR_CHOICES.contains(&years) {
        return Err("Pick one of the offered ages.".into());
    }
    Ok(stale::Options { years, not_opened })
}

/// How much of the scan has not been modified in `years` years, per first-level folder. Reads
/// only the scan; no file is opened.
#[tauri::command]
pub async fn cleanup_stale(
    app: AppHandle,
    years: u32,
    not_opened: bool,
) -> Result<StaleReport, String> {
    let options = stale_options(years, not_opened)?;
    tauri::async_runtime::spawn_blocking(move || {
        let last_access = stale::last_access_updates();
        let state = app.state::<CleanupState>();
        let report = with_tree(&state, |tree| {
            Ok(stale::analyze(
                tree,
                transfer_core::now_ms(),
                options,
                last_access,
            ))
        })?;
        // The saved report keeps the last threshold the user looked at.
        let totals = stale_totals(&report);
        if let Some(err) = state
            .report
            .amend(move |saved| saved.stale = Some(Box::new(totals)))
        {
            report_core::applog::error(&report_warning(&err));
        }
        Ok(report)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// The largest stale files of one group of the stale report.
#[tauri::command]
pub async fn cleanup_stale_files(
    app: AppHandle,
    group: NodeId,
    years: u32,
    not_opened: bool,
) -> Result<Vec<StaleFile>, String> {
    let options = stale_options(years, not_opened)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        with_tree(&state, |tree| {
            Ok(stale::stale_files(
                tree,
                group,
                transfer_core::now_ms(),
                options,
                stale::FILES_LIMIT,
            ))
        })
    })
    .await
    .map_err(|err| err.to_string())?
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
    app: AppHandle,
    ids: Vec<NodeId>,
    permanent: bool,
    confirm_token: Option<String>,
) -> Result<DeleteManyResult, String> {
    let elevated = cleanup_core::is_elevated();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        delete_ids(
            &state,
            &ids,
            None,
            Logged {
                bulk: true,
                permanent,
                elevated,
                protected: &protected(),
                logs: &report_core::logs_root(),
            },
            confirm_token.as_deref(),
        )
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Deletes the duplicates `ids` from the last duplicate search, through the same pipeline as
/// any bulk delete. Before anything is touched, Rust checks that every group keeps a copy (even
/// when the webview sends every id), re-hashes the kept copy, and re-checks the size and time
/// of the others (`dupes::check_delete`).
#[tauri::command]
pub async fn cleanup_delete_duplicates(
    app: AppHandle,
    ids: Vec<NodeId>,
    permanent: bool,
    confirm_token: Option<String>,
) -> Result<DeleteManyResult, String> {
    let elevated = cleanup_core::is_elevated();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CleanupState>();
        delete_duplicates(
            &state,
            &ids,
            Logged {
                bulk: true,
                permanent,
                elevated,
                protected: &protected(),
                logs: &report_core::logs_root(),
            },
            confirm_token.as_deref(),
        )
    })
    .await
    .map_err(|err| err.to_string())?
}

fn delete_duplicates(
    state: &CleanupState,
    ids: &[NodeId],
    how: Logged<'_>,
    token: Option<&str>,
) -> Result<DeleteManyResult, String> {
    if !how.elevated && (how.permanent || ids.len() > 1) {
        return Err(NEEDS_ADMIN.into());
    }
    let seq = state.scan_seq.load(Ordering::SeqCst);
    // The search is copied out so the re-hash below holds no lock.
    let found = lock(&state.dupes)?.clone().ok_or(NO_DUPES)?;
    let picked = dupes::check_delete(&found, ids, &CancelToken::new())?;
    if state.scan_seq.load(Ordering::SeqCst) != seq {
        return Err(NO_DUPES.into());
    }
    let expected: std::collections::HashMap<NodeId, PathBuf> = found
        .groups
        .iter()
        .flat_map(|group| group.members.iter())
        .map(|member| (member.id, member.path.clone()))
        .collect();
    let done = delete_ids(state, &picked, Some(&expected), how, token)?;
    let gone: std::collections::HashSet<NodeId> = done
        .results
        .iter()
        .filter(|result| matches!(result.outcome, Outcome::Done | Outcome::NotFound))
        .map(|result| result.id)
        .collect();
    let mut slot = lock(&state.dupes)?;
    if state.scan_seq.load(Ordering::SeqCst) == seq {
        if let Some(found) = slot.as_mut() {
            found.forget(&gone);
            let totals = duplicate_totals(found);
            if let Some(err) = state
                .report
                .amend(move |report| report.duplicates = Some(Box::new(totals)))
            {
                report_core::applog::error(&report_warning(&err));
            }
        }
    }
    Ok(done)
}

/// The bulk delete behind both commands: dry run, then `bulk::run` with a log line per item,
/// then the tree and the saved report. With `expected`, every id must still name the path the
/// caller saw, or nothing is deleted.
fn delete_ids(
    state: &CleanupState,
    ids: &[NodeId],
    expected: Option<&std::collections::HashMap<NodeId, PathBuf>>,
    how: Logged<'_>,
    token: Option<&str>,
) -> Result<DeleteManyResult, String> {
    if !how.elevated && (how.permanent || ids.len() > 1) {
        return Err(NEEDS_ADMIN.into());
    }
    let (targets, root) = with_tree(state, |tree| {
        let preview = bulk::preview(tree, how.protected, ids);
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
        if let Some(expected) = expected {
            if targets
                .iter()
                .any(|target| expected.get(&target.id) != Some(&target.path))
            {
                return Err(NO_DUPES.into());
            }
        }
        Ok((targets, tree.path(0)))
    })?;
    if targets.is_empty() {
        return Err("Nothing in the selection can be deleted.".into());
    }
    let permanent = how.permanent;
    let (results, log_error) = run_logged_in(&targets, how, &root, token)?;

    let (overview, removed) = {
        let mut guard = lock(&state.tree)?;
        let tree = guard.as_mut().ok_or(NO_RESULT)?;
        let mut removed = Vec::new();
        for result in results.iter().filter(|r| r.outcome == Outcome::Done) {
            // A new scan may have replaced the tree while Windows was deleting.
            if tree.is_live(result.id) && tree.path(result.id).display().to_string() == result.path
            {
                tree.remove(result.id);
            }
            removed.push(Removed {
                path: result.path.clone(),
                size: result.size,
                permanent,
            });
        }
        (cleanup_core::overview(tree), removed)
    };
    // The report is queued outside the tree lock: the write itself comes later.
    let warning = state.report.removed(removed);
    let mut overview = overview;
    overview.warning = warning.map(|err| report_warning(&err));
    Ok(DeleteManyResult {
        results,
        overview,
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
        let results =
            restore::restore_last(&entries, &cleanup_core::recycle_bin_items()?, &protected());

        let now = transfer_core::now_ms();
        let user = transfer_core::user_name();
        let elevated = cleanup_core::is_elevated();
        let logged: Vec<LogEntry> = results
            .iter()
            .map(|result| LogEntry {
                path: result.path.clone(),
                size: result.size,
                result: result.outcome.code().into(),
                detail: result.message.clone(),
                ..stamped(now, &user, elevated, "restore")
            })
            .collect();
        if let Err(err) = log::append(&logs, &logged) {
            report_core::applog::error(&format!("The deletion log could not be written: {err}"));
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
pub async fn cleanup_quick_list(
    record: tauri::State<'_, crate::record::RecordState>,
) -> Result<QuickList, String> {
    let _scan = record.scan_guard()?;
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
pub async fn cleanup_quick_clean(
    record: tauri::State<'_, crate::record::RecordState>,
    ids: Vec<QuickId>,
    job: JobInfo,
) -> Result<QuickResult, String> {
    let _scan = record.scan_guard()?;
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
                let logs = report_core::logs_root();
                let user = transfer_core::user_name();
                // One summary line per folder, written as soon as that folder is done.
                let mut log_target = |target: &Path, done: &quick::Cleaned| {
                    let entry = quick_log_entry(id, target, done, elevated, &user);
                    if let Err(err) = log::append(&logs, std::slice::from_ref(&entry)) {
                        report_core::applog::error(&format!(
                            "The deletion log could not be written: {err}"
                        ));
                    }
                };
                match quick::clean_with(&places, id, &mut log_target) {
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
    fn a_single_delete_is_logged_but_never_becomes_the_last_run() {
        let bulk = |time_ms, path: &str| LogEntry {
            path: path.into(),
            time_ms,
            ..log_entry(
                &result(Outcome::Done, None),
                false,
                true,
                Some("run-1"),
                "t",
            )
        };
        let single = LogEntry {
            path: "C:\\scan\\single".into(),
            time_ms: 9_000,
            ..log_entry(&result(Outcome::Done, None), false, true, None, "t")
        };
        assert_eq!(single.run, None);
        assert_eq!(single.result, "done");

        let entries = [bulk(1_000, "C:\\a"), bulk(1_001, "C:\\b"), single];
        let last: Vec<&str> = restore::last_run(&entries)
            .into_iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(last, ["C:\\a", "C:\\b"]);
    }

    #[test]
    fn a_one_item_delete_shows_log_and_report_errors_in_one_warning() {
        assert_eq!(single_warning(None, None), None);
        let log_only = single_warning(
            Some("The deletion log could not be written: x".into()),
            None,
        );
        assert_eq!(
            log_only.as_deref(),
            Some("The deletion log could not be written: x")
        );
        let both = single_warning(Some("log".into()), Some("disk".into())).unwrap();
        assert!(both.starts_with("log "), "{both}");
        assert!(
            both.contains("The saved report is out of date: disk"),
            "{both}"
        );
    }

    #[test]
    fn a_result_becomes_a_log_line_with_the_run_and_method() {
        let skipped = result(Outcome::Skipped, Some("left alone"));
        let entry = log_entry(&skipped, false, true, Some("run-1"), "tech");
        assert_eq!((entry.method.as_str(), entry.elevated), ("recycle", true));
        assert_eq!(entry.run.as_deref(), Some("run-1"));
        assert_eq!(entry.user, "tech");
        assert_eq!(entry.result, "skipped");
        assert_eq!(entry.detail.as_deref(), Some("left alone"));
        assert_eq!(entry.size, 42);
        let done = result(Outcome::Done, None);
        assert_eq!(
            log_entry(&done, true, false, Some("r"), "tech").method,
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

#[cfg(test)]
mod quick_log_tests {
    use super::*;

    #[test]
    fn a_quick_cleanup_folder_becomes_one_summary_line_outside_any_run() {
        let done = quick::Cleaned {
            freed: 2048,
            removed: 7,
            skipped: 1,
        };
        let entry = quick_log_entry(QuickId::UserTemp, Path::new(r"C:\T"), &done, true, "tech");
        assert_eq!(entry.method, "permanent");
        assert_eq!((entry.size, entry.run.clone()), (2048, None));
        assert_eq!(
            entry.detail.as_deref(),
            Some("7 files, quick cleanup UserTemp")
        );
        let installers = quick_log_entry(QuickId::OldInstallers, Path::new("D"), &done, false, "t");
        assert_eq!(installers.method, "recycle");
    }
}

#[cfg(test)]
mod duplicate_delete_tests {
    use super::*;

    /// A folder under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::AtomicUsize;
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "dsa-dupes-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A scanned folder with one group of three equal files, searched as the panel would.
    fn searched(dir: &Path) -> CleanupState {
        let bytes: Vec<u8> = (0..200_000_u32).map(|at| (at % 251) as u8).collect();
        for name in ["a.iso", "b.iso", "c.iso"] {
            std::fs::write(dir.join(name), &bytes).unwrap();
        }
        let tree = scan(dir, &ScanState::default()).unwrap();
        let found = dupes::find(
            &tree,
            64 * 1024,
            &CancelToken::default(),
            &dupes::Counters::default(),
        )
        .unwrap();
        assert_eq!(found.groups.len(), 1);
        let state = CleanupState::default();
        *state.tree.lock().unwrap() = Some(tree);
        *state.dupes.lock().unwrap() = Some(found);
        state
    }

    fn group_ids(state: &CleanupState) -> Vec<NodeId> {
        state.dupes.lock().unwrap().as_ref().unwrap().groups[0]
            .members
            .iter()
            .map(|member| member.id)
            .collect()
    }

    fn how<'a>(protected: &'a Protected, logs: &'a Path) -> Logged<'a> {
        // Permanent: the Recycle Bin exists only on Windows.
        Logged {
            bulk: true,
            permanent: true,
            elevated: true,
            protected,
            logs,
        }
    }

    #[test]
    fn a_webview_call_with_every_copy_is_refused_and_touches_nothing() {
        let (dir, logs) = (TempDir::new(), TempDir::new());
        let state = searched(&dir.0);
        let protected = Protected::new(&[], &[], &[]);
        let every = group_ids(&state);

        let err = delete_duplicates(&state, &every, how(&protected, &logs.0), None)
            .err()
            .unwrap();
        assert!(err.contains("Every copy"), "{err}");
        for name in ["a.iso", "b.iso", "c.iso"] {
            assert!(dir.0.join(name).exists(), "{name} was deleted");
        }
        assert!(log::read_all(&logs.0).is_empty());
    }

    #[test]
    fn each_deleted_duplicate_gets_a_log_line_and_leaves_the_search() {
        let (dir, logs) = (TempDir::new(), TempDir::new());
        let state = searched(&dir.0);
        let protected = Protected::new(&[], &[], &[]);
        let ids = group_ids(&state);

        let done = delete_duplicates(&state, &ids[1..], how(&protected, &logs.0), None).unwrap();
        assert_eq!(done.results.len(), 2);
        assert!(done.results.iter().all(|r| r.outcome == Outcome::Done));

        let lines = log::read_all(&logs.0);
        let mut logged: Vec<&str> = lines.iter().map(|line| line.path.as_str()).collect();
        let mut deleted: Vec<&str> = done.results.iter().map(|r| r.path.as_str()).collect();
        logged.sort_unstable();
        deleted.sort_unstable();
        assert_eq!(logged, deleted);
        assert!(lines
            .iter()
            .all(|line| line.result == "done" && line.run.is_some()));

        let kept = state.tree.lock().unwrap().as_ref().unwrap().path(ids[0]);
        assert!(kept.exists());
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
        // The group is down to one file, so it is no longer a group.
        assert!(state
            .dupes
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .groups
            .is_empty());
    }

    #[test]
    fn a_new_scan_makes_old_ids_useless() {
        let (dir, logs) = (TempDir::new(), TempDir::new());
        let state = searched(&dir.0);
        let protected = Protected::new(&[], &[], &[]);
        let ids = group_ids(&state);
        // What `cleanup_scan` does to the search before it scans again.
        {
            let mut dupes = state.dupes.lock().unwrap();
            state.scan_seq.fetch_add(1, Ordering::SeqCst);
            dupes.take();
        }
        assert!(delete_duplicates(&state, &ids[1..], how(&protected, &logs.0), None).is_err());
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 3);
    }

    #[test]
    fn several_duplicates_need_administrator() {
        let (dir, logs) = (TempDir::new(), TempDir::new());
        let state = searched(&dir.0);
        let protected = Protected::new(&[], &[], &[]);
        let ids = group_ids(&state);
        let not_admin = Logged {
            elevated: false,
            permanent: false,
            ..how(&protected, &logs.0)
        };
        let err = delete_duplicates(&state, &ids[1..], not_admin, None)
            .err()
            .unwrap();
        assert_eq!(err, NEEDS_ADMIN);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 3);
    }
}

//! Compare commands: scan both folders in parallel, stream progress over a channel, keep the
//! result in memory and hand it to the UI one folder level at a time.

use report_core::{Body, CompareReport, JobInfo, MissingFile};
use scan_core::{
    check_pair, clean_path, compare, scan, CancelToken, DiffRow, DiffSummary, DiffTree,
    ScanProgress, ScanState, Side,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::State;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Returned when a compare is cancelled; the UI matches on it to stay quiet.
const CANCELLED: &str = "cancelled";

/// The running compare's cancel token and the last finished result.
#[derive(Default)]
pub struct CompareState {
    cancel: Mutex<Option<CancelToken>>,
    result: Mutex<Option<Compared>>,
}

struct Compared {
    left: PathBuf,
    right: PathBuf,
    diff: DiffTree,
    ignore_junk: bool,
}

/// What "Copy missing files" needs from the last compare.
pub struct MissingToCopy {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub paths: Vec<String>,
    pub ignore_junk: bool,
}

impl CompareState {
    /// The last compare's missing paths with its two folders, for a Record copy job.
    pub fn missing_to_copy(&self) -> Result<MissingToCopy, String> {
        let result = self.result.lock().map_err(|err| err.to_string())?;
        let compared = result.as_ref().ok_or(NO_RESULT)?;
        Ok(MissingToCopy {
            source: compared.left.clone(),
            destination: compared.right.clone(),
            paths: compared.diff.missing_paths(),
            ignore_junk: compared.ignore_junk,
        })
    }
}

const NO_RESULT: &str = "That compare result is no longer available. Compare again.";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CompareEvent {
    #[serde(rename_all = "camelCase")]
    Progress {
        left: ScanProgress,
        right: ScanProgress,
        left_done: bool,
        right_done: bool,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareResult {
    summary: DiffSummary,
    rows: Vec<DiffRow>,
    elapsed_ms: u64,
    /// The saved report on the Reports page; `None` when it could not be saved.
    report_id: Option<String>,
}

/// Most missing paths a saved compare report keeps.
const REPORT_MISSING_LIMIT: usize = 50_000;

/// Saves the compare on the Reports page. A failed save doesn't fail the compare.
fn save_report(
    left: &Path,
    right: &Path,
    diff: &DiffTree,
    elapsed_ms: u64,
    job: JobInfo,
) -> Option<String> {
    let summary = diff.summary();
    let missing_paths: Vec<MissingFile> = diff
        .missing_entries()
        .into_iter()
        .take(REPORT_MISSING_LIMIT)
        .map(|entry| MissingFile {
            path: entry.path,
            size: entry.size,
        })
        .collect();
    let report = CompareReport {
        source: left.display().to_string(),
        destination: right.display().to_string(),
        source_size: summary.left.size,
        source_files: summary.left.files,
        destination_size: summary.right.size,
        destination_files: summary.right.files,
        missing: summary.missing,
        missing_bytes: summary.missing_bytes,
        different: summary.different,
        extra: summary.extra,
        unreadable: summary.left_errors + summary.right_errors,
        elapsed_ms,
        missing_paths,
    };
    report_core::save(&report_core::reports_root(), job, Body::Compare(report))
        .map(|saved| saved.id)
        .ok()
}

#[tauri::command]
pub async fn compare_start(
    state: State<'_, CompareState>,
    left: String,
    right: String,
    ignore_junk: bool,
    job: JobInfo,
    on_event: Channel<CompareEvent>,
) -> Result<CompareResult, String> {
    let left = clean_path(&left);
    let right = clean_path(&right);
    check_pair(&left, &right)?;
    let cancel = CancelToken::new();
    if let Some(previous) = state
        .cancel
        .lock()
        .map_err(|err| err.to_string())?
        .replace(cancel.clone())
    {
        previous.cancel();
    }
    // Free the previous result before building the next one.
    state.result.lock().map_err(|err| err.to_string())?.take();

    let started = std::time::Instant::now();
    let job_cancel = cancel.clone();
    let (job_left, job_right) = (left.clone(), right.clone());
    let diff = tauri::async_runtime::spawn_blocking(move || {
        run(&job_left, &job_right, ignore_junk, &job_cancel, &on_event)
    })
    .await
    .map_err(|err| err.to_string())??;
    // A newer compare (or a cancel) arrived while this one was diffing.
    if cancel.is_cancelled() {
        return Err(CANCELLED.to_string());
    }

    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let result = CompareResult {
        summary: diff.summary().clone(),
        rows: diff.children(0).unwrap_or_default(),
        elapsed_ms,
        report_id: save_report(&left, &right, &diff, elapsed_ms, job),
    };
    *state.result.lock().map_err(|err| err.to_string())? = Some(Compared {
        left,
        right,
        diff,
        ignore_junk,
    });
    Ok(result)
}

fn run(
    left: &Path,
    right: &Path,
    ignore_junk: bool,
    cancel: &CancelToken,
    on_event: &Channel<CompareEvent>,
) -> Result<DiffTree, String> {
    let left_state = ScanState::new(cancel.clone()).ignoring_junk(ignore_junk);
    let right_state = ScanState::new(cancel.clone()).ignoring_junk(ignore_junk);
    let send_progress = |left_done: bool, right_done: bool| {
        // A closed channel (page left) is not an error; the scan still finishes.
        let _ = on_event.send(CompareEvent::Progress {
            left: left_state.progress(),
            right: right_state.progress(),
            left_done,
            right_done,
        });
    };

    let (left_tree, right_tree) = thread::scope(|scope| {
        let left_job = scope.spawn(|| scan(left, &left_state));
        let right_job = scope.spawn(|| scan(right, &right_state));
        while !(left_job.is_finished() && right_job.is_finished()) {
            send_progress(left_job.is_finished(), right_job.is_finished());
            thread::sleep(PROGRESS_INTERVAL);
        }
        (left_job.join(), right_job.join())
    });
    send_progress(true, true);
    if cancel.is_cancelled() {
        return Err(CANCELLED.to_string());
    }

    let left_tree = left_tree
        .map_err(|_| "The source scan stopped unexpectedly.".to_string())?
        .map_err(|err| format!("Source: {err}"))?;
    let right_tree = right_tree
        .map_err(|_| "The destination scan stopped unexpectedly.".to_string())?
        .map_err(|err| format!("Destination: {err}"))?;
    Ok(compare(&left_tree, &right_tree))
}

/// Rows below one folder of the last compare.
#[tauri::command]
pub fn compare_children(state: State<'_, CompareState>, id: u32) -> Result<Vec<DiffRow>, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    result
        .as_ref()
        .and_then(|compared| compared.diff.children(id))
        .ok_or_else(|| NO_RESULT.to_string())
}

/// Full path of a row on one side of the last compare.
fn full_path(state: &CompareState, id: u32, side: Side) -> Result<PathBuf, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    let compared = result.as_ref().ok_or(NO_RESULT)?;
    let relative = compared
        .diff
        .relative_path(id, side)
        .ok_or("That item is not on this side.")?;
    let root = match side {
        Side::Left => &compared.left,
        Side::Right => &compared.right,
    };
    Ok(root.join(relative))
}

#[tauri::command]
pub fn compare_path(state: State<'_, CompareState>, id: u32, side: Side) -> Result<String, String> {
    full_path(&state, id, side).map(|path| path.display().to_string())
}

/// Opens Explorer with the item selected.
#[tauri::command]
pub fn compare_reveal(state: State<'_, CompareState>, id: u32, side: Side) -> Result<(), String> {
    let path = full_path(&state, id, side)?;
    tauri_plugin_opener::reveal_item_in_dir(&path)
        .map_err(|err| format!("Could not open {} in Explorer: {err}", path.display()))
}

/// Source files missing at the destination, relative to the source, plus the source folder.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingList {
    source: String,
    destination: String,
    paths: Vec<String>,
}

#[tauri::command]
pub fn compare_missing(state: State<'_, CompareState>) -> Result<MissingList, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    let compared = result.as_ref().ok_or(NO_RESULT)?;
    Ok(MissingList {
        source: compared.left.display().to_string(),
        destination: compared.right.display().to_string(),
        paths: compared.diff.missing_paths(),
    })
}

#[tauri::command]
pub fn compare_cancel(state: State<'_, CompareState>) -> Result<(), String> {
    if let Some(cancel) = state.cancel.lock().map_err(|err| err.to_string())?.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

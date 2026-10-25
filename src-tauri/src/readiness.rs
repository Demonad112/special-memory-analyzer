//! Migration readiness commands: scan a source folder, check every name and path for the target,
//! stream progress over a channel, keep the result in memory and hand it to the UI one folder
//! level at a time.

use migrate_core::{
    Finding, FindingKind, Options, ProblemKind, Readiness, ReadinessProgress, ReadinessReport,
    ReadinessState as CheckState, Row,
};
use report_core::{Body, JobInfo};
use scan_core::{canonical_root, clean_path, CancelToken, TargetProfile};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::State;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Returned when a check is cancelled; the UI matches on it to stay quiet.
const CANCELLED: &str = "cancelled";
const NO_RESULT: &str = "That readiness result is no longer available. Check again.";

/// The running check's cancel token and the last finished result.
#[derive(Default)]
pub struct ReadinessState {
    cancel: Mutex<Option<CancelToken>>,
    result: Mutex<Option<Readiness>>,
}

impl ReadinessState {
    /// Keeps `result` unless `cancel` fired. The check runs under the result lock: a newer check
    /// cancels this one first and then clears the result, so a stale result cannot land after.
    fn store(&self, cancel: &CancelToken, result: Readiness) -> Result<(), String> {
        let mut slot = self.result.lock().map_err(|err| err.to_string())?;
        if cancel.is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        *slot = Some(result);
        Ok(())
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ReadinessEvent {
    Progress { progress: ReadinessProgress },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessResult {
    /// The totals and counts; the problem list stays in Rust (see `readiness_children`).
    report: ReadinessReport,
    /// The source folder itself, with its own problems and the rows below it.
    root: Row,
    rows: Vec<Row>,
    /// The saved report on the Reports page; `None` when it could not be saved.
    report_id: Option<String>,
}

/// The planned destination root, which is only ever used as text to measure paths: empty, or an
/// absolute Windows path (`D:\...` or `\\server\share\...`).
fn destination_root(input: &str) -> Result<String, String> {
    let text = clean_path(input).to_string_lossy().into_owned();
    if text.is_empty() {
        return Ok(text);
    }
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    if drive || text.starts_with(r"\\") {
        Ok(text)
    } else {
        Err(
            "The destination root must be a full path, such as D:\\Shares\\Finance or \\\\server\\share."
                .into(),
        )
    }
}

/// Saves the report on the Reports page. A failed save doesn't fail the check.
fn save_report(report: &ReadinessReport, job: JobInfo) -> Option<String> {
    match report_core::save(
        &report_core::reports_root(),
        job,
        Body::Readiness(report.clone()),
    ) {
        Ok(saved) => Some(saved.id),
        Err(err) => {
            report_core::applog::error(&format!("The readiness report was not saved: {err}"));
            None
        }
    }
}

/// The source folder as a row: its own problems (an unreadable source shows here) and the count.
fn root_row(result: &Readiness, kind: Option<ProblemKind>) -> Row {
    let rows = result.children(0, kind).unwrap_or_default();
    let below: u64 = rows.iter().map(|row| row.problems).sum();
    let root = result.tree().root();
    let own = result
        .own(0)
        .iter()
        .filter(|problem| kind.is_none_or(|kind| problem.kind == kind))
        .cloned()
        .collect::<Vec<_>>();
    Row {
        id: 0,
        name: root.name.clone(),
        kind: root.kind,
        problems: below + own.len() as u64,
        has_children: below > 0,
        own,
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn readiness_start(
    state: State<'_, ReadinessState>,
    record: State<'_, crate::record::RecordState>,
    path: String,
    destination: String,
    target: TargetProfile,
    check_in_use: bool,
    check_permissions: bool,
    every_file: bool,
    job: JobInfo,
    on_event: Channel<ReadinessEvent>,
) -> Result<ReadinessResult, String> {
    let root = canonical_root(&clean_path(&path))?;
    let options = Options {
        target,
        destination_root: destination_root(&destination)?,
        check_in_use,
        check_permissions,
        every_file,
        // Only recorded in the report: never taken from the webview.
        elevated: cleanup_core::is_elevated(),
    };
    // Held until the check is done: an elevated record and a scan must not overlap.
    let _scan = record.scan_guard()?;
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

    let job_cancel = cancel.clone();
    let (result, report_id) = tauri::async_runtime::spawn_blocking(move || {
        let result = run(&root, &options, &job_cancel, &on_event)?;
        // A newer check (or a cancel) arrived while this one was finishing.
        if job_cancel.is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        let report_id = save_report(result.report(), job);
        Ok::<_, String>((result, report_id))
    })
    .await
    .map_err(|err| err.to_string())??;
    let mut report = result.report().clone();
    report.problems = Vec::new();
    // The findings stay in Rust too (see `readiness_permissions`); the totals go up front.
    report.permissions = report.permissions.summary();
    let reply = ReadinessResult {
        report,
        root: root_row(&result, None),
        rows: result.children(0, None).unwrap_or_default(),
        report_id,
    };
    state.store(&cancel, result)?;
    Ok(reply)
}

fn run(
    root: &Path,
    options: &Options,
    cancel: &CancelToken,
    on_event: &Channel<ReadinessEvent>,
) -> Result<Readiness, String> {
    let check = CheckState::new(cancel.clone());
    let send_progress = || {
        // A closed channel (page left) is not an error; the check still finishes.
        let _ = on_event.send(ReadinessEvent::Progress {
            progress: check.progress(),
        });
    };
    let outcome = thread::scope(|scope| {
        let job = scope.spawn(|| migrate_core::run(root, options, &check));
        while !job.is_finished() {
            send_progress();
            thread::sleep(PROGRESS_INTERVAL);
        }
        job.join()
    });
    send_progress();
    if cancel.is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    outcome
        .map_err(|_| "The readiness check stopped unexpectedly.".to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn readiness_cancel(state: State<'_, ReadinessState>) -> Result<(), String> {
    if let Some(cancel) = state.cancel.lock().map_err(|err| err.to_string())?.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

/// Rows below one folder of the last check, with problems of `kind` (or any).
#[tauri::command]
pub fn readiness_children(
    state: State<'_, ReadinessState>,
    id: u32,
    kind: Option<ProblemKind>,
) -> Result<Vec<Row>, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    result
        .as_ref()
        .and_then(|result| result.children(id, kind))
        .ok_or_else(|| NO_RESULT.to_string())
}

/// The source folder's row for `kind` (or any): the counts change with the filter.
#[tauri::command]
pub fn readiness_root(
    state: State<'_, ReadinessState>,
    kind: Option<ProblemKind>,
) -> Result<Row, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    result
        .as_ref()
        .map(|result| root_row(result, kind))
        .ok_or_else(|| NO_RESULT.to_string())
}

/// One page of the findings of one type.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingsPage {
    total: u64,
    findings: Vec<Finding>,
}

/// Largest page `readiness_permissions` returns.
const MAX_PAGE: usize = 500;

/// A page of the permission findings of `kind` from the last check.
#[tauri::command]
pub fn readiness_permissions(
    state: State<'_, ReadinessState>,
    kind: FindingKind,
    offset: usize,
    limit: usize,
) -> Result<FindingsPage, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    let permissions = &result.as_ref().ok_or(NO_RESULT)?.report().permissions;
    Ok(FindingsPage {
        total: permissions.count_of(kind),
        findings: permissions
            .listed(kind)
            .skip(offset)
            .take(limit.min(MAX_PAGE))
            .cloned()
            .collect(),
    })
}

fn full_path(state: &ReadinessState, id: u32) -> Result<PathBuf, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    let result = result.as_ref().ok_or(NO_RESULT)?;
    result.full_path(id).ok_or_else(|| {
        "That item's name is not valid Unicode, so it cannot be opened by name.".to_string()
    })
}

/// Opens Explorer with the item selected.
#[tauri::command]
pub fn readiness_reveal(state: State<'_, ReadinessState>, id: u32) -> Result<(), String> {
    let path = full_path(&state, id)?;
    tauri_plugin_opener::reveal_item_in_dir(&path)
        .map_err(|err| format!("Could not open {} in Explorer: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_destination_root_is_empty_or_a_full_windows_path() {
        assert_eq!(destination_root("  ").unwrap(), "");
        assert_eq!(
            destination_root(r#""D:\Shares\Finance""#).unwrap(),
            r"D:\Shares\Finance"
        );
        assert_eq!(destination_root("e:/data").unwrap(), "e:/data");
        assert_eq!(
            destination_root(r"\\nas\finance").unwrap(),
            r"\\nas\finance"
        );
        for refused in ["Finance", r"D:Finance", r"..\x", "/mnt/x"] {
            assert!(destination_root(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_cancelled_result_is_not_stored() {
        let dir = std::env::temp_dir().join(format!("dsa-readiness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let check = CheckState::new(CancelToken::new());
        let result = migrate_core::run(&dir, &Options::default(), &check).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let state = ReadinessState::default();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(state.store(&cancel, result).unwrap_err(), CANCELLED);
        assert!(state.result.lock().unwrap().is_none());
    }
}

//! Compare commands: scan both folders in parallel, stream progress over a channel, keep the
//! result in memory and hand it to the UI one folder level at a time.

use migrate_core::acl_audit::{server_of, Counters};
use migrate_core::acl_diff::system_read;
use migrate_core::{
    compare_permissions, compare_rows as rows, CompareOptions, CompareRow, PermDetail,
    PermissionsCompare, PermissionsResult,
};
use report_core::{Body, CompareReport, JobInfo, MissingFile};
use scan_core::security::{Descriptor, SidNames, What};
use scan_core::{
    check_pair, clean_path, compare, scan, CancelToken, DiffSummary, DiffTree, ScanError,
    ScanProgress, ScanState, Side,
};
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

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
    /// Set when the compare was asked to compare permissions.
    permissions: Option<PermissionsResult>,
}

/// What "Copy missing files" needs from the last compare.
pub struct MissingToCopy {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub paths: Vec<String>,
    pub ignore_junk: bool,
}

impl CompareState {
    /// Keeps `compared` as the last result unless `cancel` fired. The check runs under the
    /// result lock: a newer compare cancels this one first and then clears the result, so a
    /// stale result cannot land after that.
    fn store(&self, cancel: &CancelToken, compared: Compared) -> Result<(), String> {
        let mut result = self.result.lock().map_err(|err| err.to_string())?;
        if cancel.is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        *result = Some(compared);
        Ok(())
    }

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
    /// The permissions pass after the size compare: pairs read so far of the total.
    Permissions { done: u64, total: u64 },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareResult {
    summary: DiffSummary,
    rows: Vec<CompareRow>,
    elapsed_ms: u64,
    /// The saved report on the Reports page; `None` when it could not be saved.
    report_id: Option<String>,
    /// What the permissions pass did, without the item details (see `compare_permission_detail`).
    /// Absent when the compare did not compare permissions.
    #[serde(skip_serializing_if = "Option::is_none")]
    permissions: Option<PermissionsCompare>,
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
    permissions: Option<&PermissionsResult>,
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
        permission_mismatches: permissions.map_or(0, |result| result.report().mismatches()),
        missing_paths,
        permissions: permissions
            .map(|result| result.report().clone())
            .unwrap_or_default(),
    };
    report_core::save(&report_core::reports_root(), job, Body::Compare(report))
        .map(|saved| saved.id)
        .ok()
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn compare_start(
    state: State<'_, CompareState>,
    record: State<'_, crate::record::RecordState>,
    left: String,
    right: String,
    ignore_junk: bool,
    compare_permissions: bool,
    job: JobInfo,
    on_event: Channel<CompareEvent>,
) -> Result<CompareResult, String> {
    let left = clean_path(&left);
    let right = clean_path(&right);
    check_pair(&left, &right)?;
    // Held until the scan is done: an elevated record and a scan must not overlap.
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

    let started = std::time::Instant::now();
    let job_cancel = cancel.clone();
    let (job_left, job_right) = (left.clone(), right.clone());
    // Diffing, reading permissions and saving the report (which walks every missing entry) all
    // stay off the async runtime.
    let (diff, permissions, elapsed_ms, report_id) =
        tauri::async_runtime::spawn_blocking(move || {
            let diff = run(&job_left, &job_right, ignore_junk, &job_cancel, &on_event)?;
            // A newer compare (or a cancel) arrived while this one was diffing.
            if job_cancel.is_cancelled() {
                return Err(CANCELLED.to_string());
            }
            let permissions = if compare_permissions {
                Some(read_permissions(
                    &diff,
                    &job_left,
                    &job_right,
                    &job_cancel,
                    &on_event,
                )?)
            } else {
                None
            };
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let report_id = save_report(
                &job_left,
                &job_right,
                &diff,
                elapsed_ms,
                job,
                permissions.as_ref(),
            );
            Ok::<_, String>((diff, permissions, elapsed_ms, report_id))
        })
        .await
        .map_err(|err| err.to_string())??;
    let result = CompareResult {
        summary: diff.summary().clone(),
        rows: rows(&diff, 0, permissions.as_ref()).unwrap_or_default(),
        elapsed_ms,
        report_id,
        permissions: permissions.as_ref().map(|result| result.report().summary()),
    };
    state.store(
        &cancel,
        Compared {
            left,
            right,
            diff,
            ignore_junk,
            permissions,
        },
    )?;
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

/// Reads and compares the permissions of every matched pair, streaming progress about every
/// 100 ms. Off Windows nothing is read and the result says so.
fn read_permissions(
    diff: &DiffTree,
    left: &Path,
    right: &Path,
    cancel: &CancelToken,
    on_event: &Channel<CompareEvent>,
) -> Result<PermissionsResult, String> {
    if !cfg!(windows) {
        return Ok(PermissionsResult::not_supported());
    }
    pass(
        diff,
        left,
        right,
        cancel,
        cleanup_core::is_elevated(),
        &system_read,
        &|done, total| {
            // A closed channel (page left) is not an error; the pass still finishes.
            let _ = on_event.send(CompareEvent::Permissions { done, total });
        },
    )
}

type Reader<'a> = dyn Fn(&Path, What) -> io::Result<Descriptor> + Sync + 'a;

/// [`read_permissions`] with the system read and the progress sink handed in, so it is tested
/// everywhere.
fn pass(
    diff: &DiffTree,
    left: &Path,
    right: &Path,
    cancel: &CancelToken,
    elevated: bool,
    read: &Reader,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<PermissionsResult, String> {
    // Each side's names are asked of its own server (D-07).
    let source_server = server_of(&left.to_string_lossy());
    let destination_server = server_of(&right.to_string_lossy());
    let options = CompareOptions {
        source_server: source_server.clone(),
        destination_server: destination_server.clone(),
    };
    let names = SidNames::default();
    let counters = Counters::default();
    let send = || {
        progress(
            counters.done.load(Ordering::Relaxed),
            counters.total.load(Ordering::Relaxed),
        );
    };
    let outcome = thread::scope(|scope| {
        let job = scope.spawn(|| {
            compare_permissions(diff, left, right, &options, read, &names, cancel, &counters)
        });
        while !job.is_finished() {
            send();
            thread::sleep(PROGRESS_INTERVAL);
        }
        job.join()
    });
    send();
    match outcome {
        Err(_) => Err("The permissions check stopped unexpectedly.".to_string()),
        Ok(Err(ScanError::Cancelled)) => Err(CANCELLED.to_string()),
        Ok(Err(err)) => Err(err.to_string()),
        Ok(Ok(result)) => Ok(result.with_context(
            elevated,
            source_server.as_deref(),
            destination_server.as_deref(),
        )),
    }
}

/// Rows below one folder of the last compare.
#[tauri::command]
pub fn compare_children(
    state: State<'_, CompareState>,
    id: u32,
) -> Result<Vec<CompareRow>, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    result
        .as_ref()
        .and_then(|compared| rows(&compared.diff, id, compared.permissions.as_ref()))
        .ok_or_else(|| NO_RESULT.to_string())
}

/// What differs on one row's permissions. `None` when the row was not compared or the report
/// limit left its details out (the row's status is still known).
#[tauri::command]
pub fn compare_permission_detail(
    state: State<'_, CompareState>,
    id: u32,
) -> Result<Option<PermDetail>, String> {
    let result = state.result.lock().map_err(|err| err.to_string())?;
    let compared = result.as_ref().ok_or(NO_RESULT)?;
    Ok(compared
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.detail(id))
        .cloned())
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

/// Building every missing path can take a while on a big tree, so it runs on the blocking
/// pool; the state is reached through the handle because the closure must be `'static`.
#[tauri::command]
pub async fn compare_missing(app: AppHandle) -> Result<MissingList, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CompareState>();
        let result = state.result.lock().map_err(|err| err.to_string())?;
        let compared = result.as_ref().ok_or(NO_RESULT)?;
        Ok(MissingList {
            source: compared.left.display().to_string(),
            destination: compared.right.display().to_string(),
            paths: compared.diff.missing_paths(),
        })
    })
    .await
    .map_err(|err| err.to_string())?
}

#[tauri::command]
pub fn compare_cancel(state: State<'_, CompareState>) -> Result<(), String> {
    if let Some(cancel) = state.cancel.lock().map_err(|err| err.to_string())?.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use migrate_core::PermStatus;

    /// A compare of two empty folders, which is all `store` needs to hold.
    fn empty_compare(tag: &str) -> Compared {
        let root = std::env::temp_dir().join(format!("dsa-compare-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let state = ScanState::new(CancelToken::new());
        let tree = scan(&root, &state).unwrap();
        let diff = compare(&tree, &tree);
        let _ = std::fs::remove_dir_all(&root);
        Compared {
            left: root.clone(),
            right: root,
            diff,
            ignore_junk: false,
            permissions: None,
        }
    }

    #[test]
    fn a_cancelled_compare_never_stores_its_result() {
        let state = CompareState::default();
        let cancel = CancelToken::new();
        // A newer compare cancelled this one after its own check but before the store.
        cancel.cancel();
        let err = state
            .store(&cancel, empty_compare("cancelled"))
            .unwrap_err();
        assert_eq!(err, CANCELLED);
        assert!(state.missing_to_copy().is_err(), "nothing may be stored");
    }

    #[test]
    fn a_finished_compare_stores_its_result() {
        let state = CompareState::default();
        state
            .store(&CancelToken::new(), empty_compare("finished"))
            .unwrap();
        assert!(state.missing_to_copy().unwrap().paths.is_empty());
    }

    /// Two folders with the same layout: `a/f.txt`, `b/` and `top.txt`.
    fn two_trees(tag: &str) -> (PathBuf, PathBuf, DiffTree) {
        let base = std::env::temp_dir().join(format!("dsa-compare-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (left, right) = (base.join("left"), base.join("right"));
        for root in [&left, &right] {
            std::fs::create_dir_all(root.join("a")).unwrap();
            std::fs::create_dir_all(root.join("b")).unwrap();
            std::fs::write(root.join("a").join("f.txt"), b"xx").unwrap();
            std::fs::write(root.join("top.txt"), b"x").unwrap();
        }
        let state = ScanState::new(CancelToken::new());
        let diff = compare(
            &scan(&left, &state).unwrap(),
            &scan(&right, &state).unwrap(),
        );
        (left, right, diff)
    }

    fn descriptor(owner: &str) -> Descriptor {
        Descriptor {
            owner: Some(owner.to_string()),
            ..Descriptor::default()
        }
    }

    #[test]
    fn rows_carry_the_permissions_cell_when_the_compare_looked() {
        let (left, right, diff) = two_trees("cells");
        let read = |path: &Path, _: What| -> io::Result<Descriptor> {
            // The destination's folder `a` has another owner.
            // On Windows the pass opens the exact `\\?\` form of a path.
            let plain = PathBuf::from(scan_core::strip_verbatim(&path.to_string_lossy()));
            let other = plain.starts_with(&right) && plain.ends_with("a");
            Ok(descriptor(if other { "S-1-5-18" } else { "S-1-5-32-544" }))
        };
        let result = pass(
            &diff,
            &left,
            &right,
            &CancelToken::new(),
            false,
            &read,
            &|_, _| {},
        )
        .unwrap();
        let rows = rows(&diff, 0, Some(&result)).unwrap();
        let cell = |name: &str| {
            rows.iter()
                .find(|row| row.row.name == name)
                .unwrap()
                .permissions
                .clone()
        };
        let a = cell("a").unwrap();
        assert_eq!(a.status, Some(PermStatus::OwnerDiffers));
        assert_eq!(a.summary, "Owner: S-1-5-32-544 → S-1-5-18");
        assert_eq!(cell("b").unwrap().status, Some(PermStatus::Same));
        assert_eq!(
            cell("top.txt"),
            None,
            "a file with no entries of its own is not reported"
        );
        // The root was compared too, and one folder below it differs.
        let root = result.cell(0).unwrap();
        assert_eq!((root.status, root.below), (Some(PermStatus::Same), 1));
        let report = result.report().summary();
        assert!(report.items.is_empty());
        assert_eq!(report.count_of(PermStatus::OwnerDiffers), 1);
        let a_id = rows.iter().find(|row| row.row.name == "a").unwrap().row.id;
        assert_eq!(result.detail(a_id).unwrap().path, "a");
        // A compare that did not look has no cells at all.
        assert!(super::rows(&diff, 0, None)
            .unwrap()
            .iter()
            .all(|row| row.permissions.is_none()));
    }

    #[test]
    fn the_pass_reports_progress_and_ends_with_everything_done() {
        let (left, right, diff) = two_trees("progress");
        let read = |_: &Path, _: What| -> io::Result<Descriptor> { Ok(descriptor("S-1-5-18")) };
        let last = Mutex::new((0, 0));
        let result = pass(
            &diff,
            &left,
            &right,
            &CancelToken::new(),
            true,
            &read,
            &|done, total| *last.lock().unwrap() = (done, total),
        )
        .unwrap();
        // The root, a and b are the folder pairs; top.txt and a/f.txt are file pairs.
        assert_eq!(*last.lock().unwrap(), (5, 5));
        assert!(result.report().elevated);
        assert_eq!(result.report().differences(), 0);
    }

    #[test]
    fn a_cancelled_pass_returns_the_cancelled_marker() {
        let (left, right, diff) = two_trees("cancelled-pass");
        let read = |_: &Path, _: What| -> io::Result<Descriptor> { Ok(descriptor("S-1-5-18")) };
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = pass(&diff, &left, &right, &cancel, false, &read, &|_, _| {}).unwrap_err();
        assert_eq!(err, CANCELLED);
    }
}

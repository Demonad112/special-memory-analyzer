//! Record commands: list the source, then copy it (Copy mode) or watch another tool copy it
//! (Watch mode), streaming progress over a channel. Runs are stored by `transfer-core`, so a
//! finished record can be reopened as a report.

use crate::compare::CompareState;
use scan_core::{check_pair, clean_path};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::State;
use transfer_core::prepare::{OnlyPaths, PreflightReport};
use transfer_core::recovery::RecoveryResult;
use transfer_core::run::Selection;
use transfer_core::store::{self, RunStore};
use transfer_core::watch::{WatchControl, WatchOptions};
use transfer_core::PreserveOptions;
use transfer_core::{
    ConflictPolicy, FailureReason, ItemResult, ItemStatus, Phase, RunSummary, TransferMode,
    TransferProgress, TransferSettings, TransferSink, VerifyLevel,
};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Not-copied rows sent to the report page; the counts always cover all of them.
const MAX_NOT_COPIED_ROWS: usize = 10_000;

/// The running record's cancel and finish switches. One record at a time.
// ponytail: one job slot; a job map if two records ever need to run at once.
#[derive(Default)]
pub struct RecordState(
    Mutex<Option<WatchControl>>,
    /// The recovery folder last used for each run, so the report can reveal it by run id.
    Mutex<HashMap<String, PathBuf>>,
);

impl RecordState {
    fn begin(&self) -> Result<WatchControl, String> {
        let mut job = self.0.lock().map_err(|err| err.to_string())?;
        if job.is_some() {
            return Err("A record is already running. Finish or cancel it first.".into());
        }
        // Copying may use backup rights to keep permissions and owner; scans never do.
        transfer_core::secure::enable_privileges();
        let control = WatchControl::default();
        *job = Some(control.clone());
        Ok(control)
    }

    fn end(&self) {
        if let Ok(mut job) = self.0.lock() {
            job.take();
        }
    }

    fn remember_recovery(&self, run_id: &str, folder: &str) {
        if let Ok(mut folders) = self.1.lock() {
            folders.insert(run_id.to_string(), PathBuf::from(folder));
        }
    }

    fn recovery_folder(&self, run_id: &str) -> Option<PathBuf> {
        self.1.lock().ok()?.get(run_id).cloned()
    }

    fn control(&self) -> Option<WatchControl> {
        self.0.lock().ok().and_then(|job| job.clone())
    }

    /// True while a record is running (the app must not exit under it).
    pub fn is_running(&self) -> bool {
        self.control().is_some()
    }
}

/// Sends progress at most every 100 ms, and always the last one.
struct ChannelSink<'a> {
    channel: &'a Channel<TransferProgress>,
    sent: Option<Instant>,
    pending: Option<TransferProgress>,
}

impl<'a> ChannelSink<'a> {
    fn new(channel: &'a Channel<TransferProgress>) -> Self {
        Self {
            channel,
            sent: None,
            pending: None,
        }
    }

    fn flush(&mut self) {
        if let Some(progress) = self.pending.take() {
            // A closed channel (page left) is not an error; the job carries on.
            let _ = self.channel.send(progress);
            self.sent = Some(Instant::now());
        }
    }
}

impl TransferSink for ChannelSink<'_> {
    fn progress(&mut self, progress: &TransferProgress) {
        self.pending = Some(progress.clone());
        let due = self
            .sent
            .is_none_or(|sent| sent.elapsed() >= PROGRESS_INTERVAL);
        if due || progress.phase == Phase::Done {
            self.flush();
        }
    }

    fn item(&mut self, _item: &ItemResult) {}
}

/// A stored run, as the report page shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetails {
    summary: RunSummary,
    preflight: Option<PreflightReport>,
    /// The first [`MAX_NOT_COPIED_ROWS`] not-copied rows, folders and files.
    not_copied: Vec<ItemResult>,
    /// Client, ticket and technician typed when it started.
    job: report_core::JobInfo,
    /// Where the run's files are kept.
    folder: String,
}

fn load(run_id: &str) -> Result<RunDetails, String> {
    let store = RunStore::open(&transfer_core::records_root(), run_id).map_err(to_text)?;
    let not_copied = store
        .latest_results()
        .map_err(to_text)?
        .into_values()
        .filter(|item| item.status == ItemStatus::NotCopied)
        .take(MAX_NOT_COPIED_ROWS)
        .collect();
    Ok(RunDetails {
        summary: store.summary().map_err(to_text)?,
        preflight: store.preflight().ok(),
        not_copied,
        job: report_core::record_job(&store),
        folder: store.dir().display().to_string(),
    })
}

fn to_text(err: impl ToString) -> String {
    err.to_string()
}

/// Keeps the PC from going to sleep while it lives, so a long copy or watch isn't cut off. The
/// screen may still turn off. Windows ties this to the thread, so create it on the job's thread.
struct KeepAwake;

impl KeepAwake {
    fn new() -> Self {
        const ES_SYSTEM_REQUIRED: u32 = 0x1;
        set_execution_state(ES_SYSTEM_REQUIRED);
        Self
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        set_execution_state(0);
    }
}

#[cfg(windows)]
fn set_execution_state(flags: u32) {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    // SAFETY: takes a flags value only; no pointers or handles.
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | flags);
    }
}

#[cfg(not(windows))]
fn set_execution_state(_flags: u32) {}

/// Runs `work` on a blocking thread as the one record job.
async fn job<F>(state: &RecordState, work: F) -> Result<RunDetails, String>
where
    F: FnOnce(&WatchControl) -> Result<String, String> + Send + 'static,
{
    let control = state.begin()?;
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let _awake = KeepAwake::new();
        work(&control)
    })
    .await;
    state.end();
    let run_id = outcome.map_err(to_text)??;
    tauri::async_runtime::spawn_blocking(move || load(&run_id))
        .await
        .map_err(to_text)?
}

/// The Record page's check boxes, see [`TransferSettings`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordOptions {
    ignore_junk: bool,
    download_cloud: bool,
    /// What the copy keeps; both on when missing.
    #[serde(default)]
    preserve: PreserveOptions,
    /// Client, ticket and technician for the report.
    #[serde(default)]
    job: report_core::JobInfo,
    /// What to do with a different file already at the destination; Skip when missing.
    #[serde(default)]
    conflict: Option<ConflictPolicy>,
}

/// Lists the source, then copies it or watches the destination. Returns when the record ends:
/// copy finished, watch finished (by [`record_finish`] or once everything arrived), or cancel.
#[tauri::command]
pub async fn record_start(
    state: State<'_, RecordState>,
    source: String,
    destination: String,
    mode: TransferMode,
    verify: VerifyLevel,
    options: RecordOptions,
    on_event: Channel<TransferProgress>,
) -> Result<RunDetails, String> {
    let source = clean_path(&source);
    let destination = clean_path(&destination);
    check_pair(&source, &destination)?;
    let settings = TransferSettings {
        source: source.display().to_string(),
        destination: destination.display().to_string(),
        mode,
        verify,
        conflict: match mode {
            TransferMode::Copy => options.conflict.unwrap_or(ConflictPolicy::Skip),
            _ => ConflictPolicy::Skip,
        },
        ignore_junk: options.ignore_junk,
        download_cloud: options.download_cloud,
        preserve: options.preserve,
    };
    let job_info = options.job;
    job(&state, move |control| {
        let root = transfer_core::records_root();
        let mut sink = ChannelSink::new(&on_event);
        let (summary, preflight) =
            transfer_core::prepare::prepare(&root, settings, &control.cancel, &mut sink)
                .map_err(to_text)?;
        if !job_info.is_empty() {
            // The record still runs if this can't be saved; the report then has no job details.
            let _ = report_core::save_record_job(&root, &summary.id, &job_info);
        }
        if mode == TransferMode::Copy {
            check_copy_preflight(&preflight)?;
        }
        let finished = match mode {
            TransferMode::Copy => {
                transfer_core::run::run_copy(&root, &summary.id, None, &control.cancel, &mut sink)
            }
            TransferMode::Watch => transfer_core::watch::watch(
                &root,
                &summary.id,
                &WatchOptions::default(),
                control,
                &mut sink,
            ),
        };
        sink.flush();
        finished.map(|summary| summary.id).map_err(to_text)
    })
    .await
}

/// Refuses a copy that can't write to the destination or won't fit.
fn check_copy_preflight(preflight: &PreflightReport) -> Result<(), String> {
    if let Some(error) = &preflight.write_error {
        return Err(format!(
            "Deep Server Analyzer can't write to the destination: {error}"
        ));
    }
    if !preflight.enough_space {
        return Err(format!(
            "Not enough space at the destination: the copy needs {} bytes and {} are free.",
            preflight.bytes_needed,
            preflight.free_bytes.unwrap_or_default()
        ));
    }
    Ok(())
}

/// Compare's "Copy missing files": a Copy record limited to what the last compare found missing
/// at the destination. Same progress, verify, report and retry as any record.
#[tauri::command]
pub async fn record_copy_missing(
    state: State<'_, RecordState>,
    compare: State<'_, CompareState>,
    verify: VerifyLevel,
    job: report_core::JobInfo,
    preserve: Option<PreserveOptions>,
    on_event: Channel<TransferProgress>,
) -> Result<RunDetails, String> {
    let missing = compare.missing_to_copy()?;
    if missing.paths.is_empty() {
        return Err("Nothing is missing at the destination.".into());
    }
    check_pair(&missing.source, &missing.destination)?;
    let settings = TransferSettings {
        source: missing.source.display().to_string(),
        destination: missing.destination.display().to_string(),
        mode: TransferMode::Copy,
        verify,
        conflict: ConflictPolicy::Skip,
        ignore_junk: missing.ignore_junk,
        download_cloud: false,
        preserve: preserve.unwrap_or_default(),
    };
    self::job(&state, move |control| {
        let root = transfer_core::records_root();
        let mut sink = ChannelSink::new(&on_event);
        let only = OnlyPaths::new(missing.paths.iter().map(String::as_str));
        let (summary, preflight) = transfer_core::prepare::prepare_only(
            &root,
            settings,
            Some(&only),
            &control.cancel,
            &mut sink,
        )
        .map_err(to_text)?;
        if !job.is_empty() {
            let _ = report_core::save_record_job(&root, &summary.id, &job);
        }
        check_copy_preflight(&preflight)?;
        let finished =
            transfer_core::run::run_copy(&root, &summary.id, None, &control.cancel, &mut sink);
        sink.flush();
        finished.map(|summary| summary.id).map_err(to_text)
    })
    .await
}

/// Copies the not-copied files again: the given relative paths, or all of them.
#[tauri::command]
pub async fn record_retry(
    state: State<'_, RecordState>,
    run_id: String,
    paths: Vec<String>,
    reason: Option<FailureReason>,
    on_event: Channel<TransferProgress>,
) -> Result<RunDetails, String> {
    job(&state, move |control| {
        let mut sink = ChannelSink::new(&on_event);
        let selection = Selection { paths, reason };
        let finished = transfer_core::run::retry(
            &transfer_core::records_root(),
            &run_id,
            &selection,
            &control.cancel,
            &mut sink,
        );
        sink.flush();
        finished.map(|summary| summary.id).map_err(to_text)
    })
    .await
}

/// Watch mode: stop watching and give every file that has not arrived a verdict.
#[tauri::command]
pub fn record_finish(state: State<'_, RecordState>) -> bool {
    state
        .control()
        .map(|control| control.finish.cancel())
        .is_some()
}

/// Stops the running record. Copy mode lists the rest as Cancelled, so Retry continues it.
#[tauri::command]
pub fn record_cancel(state: State<'_, RecordState>) -> bool {
    state
        .control()
        .map(|control| control.cancel.cancel())
        .is_some()
}

/// Copies every not-copied file to `folder` (default: `<destination>_NotCopied`) with a list.
#[tauri::command]
pub async fn record_recover(
    state: State<'_, RecordState>,
    run_id: String,
    folder: Option<String>,
) -> Result<RecoveryResult, String> {
    let control = state.begin()?;
    let remembered_run = run_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _awake = KeepAwake::new();
        let chosen = folder
            .filter(|folder| !folder.trim().is_empty())
            .map(|folder| clean_path(&folder));
        transfer_core::recovery::copy_to_recovery(
            &transfer_core::records_root(),
            &run_id,
            &Selection::default(),
            chosen.as_deref(),
            &control.cancel,
            &mut |_, _| {},
        )
        .map_err(to_text)
    })
    .await;
    state.end();
    let result = result.map_err(to_text)??;
    state.remember_recovery(&remembered_run, &result.folder);
    Ok(result)
}

#[tauri::command]
pub async fn record_load(run_id: String) -> Result<RunDetails, String> {
    tauri::async_runtime::spawn_blocking(move || load(&run_id))
        .await
        .map_err(to_text)?
}

/// Stored records, newest first.
#[tauri::command]
pub async fn record_list() -> Result<Vec<RunSummary>, String> {
    tauri::async_runtime::spawn_blocking(|| store::list_runs(&transfer_core::records_root()))
        .await
        .map_err(to_text)
}

/// Signs in to the network share a UNC path is on, with an account the user types. Returns the
/// share (`\\server\share`). The password is passed to Windows for this call and not stored.
#[tauri::command]
pub async fn share_connect(
    path: String,
    username: String,
    password: String,
) -> Result<String, String> {
    let share = transfer_core::share::share_root(&path)
        .ok_or_else(|| "That is not a network path (\\\\server\\share).".to_string())?;
    let target = share.clone();
    tauri::async_runtime::spawn_blocking(move || {
        transfer_core::share::connect(&target, &username, &password)
    })
    .await
    .map_err(to_text)??;
    Ok(share)
}

/// What `record_reveal` opens.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RevealSide {
    Source,
    Destination,
    /// The folder `record_recover` last copied the missed files to.
    Recovery,
}

/// The path to show in Explorer, built from the run's own settings rather than from the page.
fn reveal_target(
    source: &str,
    destination: &str,
    relative_path: &str,
    side: RevealSide,
    recovery: Option<PathBuf>,
) -> Result<PathBuf, String> {
    if !transfer_core::prepare::safe_relative(relative_path) {
        return Err("That path is not allowed.".into());
    }
    let base = match side {
        RevealSide::Source => PathBuf::from(source),
        RevealSide::Destination => PathBuf::from(destination),
        RevealSide::Recovery => {
            return recovery.ok_or_else(|| "No recovery folder was made for this run.".to_string())
        }
    };
    Ok(transfer_core::prepare::destination_path(
        &base,
        relative_path,
    ))
}

/// Opens Explorer at a file or folder of a run: a source or destination file by its relative
/// path (empty for the root folder), or the run's recovery folder.
#[tauri::command]
pub fn record_reveal(
    state: State<'_, RecordState>,
    run_id: String,
    relative_path: String,
    side: RevealSide,
) -> Result<(), String> {
    let store = RunStore::open(&transfer_core::records_root(), &run_id).map_err(to_text)?;
    let settings = store.summary().map_err(to_text)?.settings;
    let path = reveal_target(
        &settings.source,
        &settings.destination,
        &relative_path,
        side,
        state.recovery_folder(&run_id),
    )?;
    tauri_plugin_opener::reveal_item_in_dir(&path)
        .map_err(|err| format!("Could not open {} in Explorer: {err}", path.display()))
}

#[cfg(test)]
mod reveal_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn builds_the_path_from_the_run_not_from_the_page() {
        let target = reveal_target(r"C:\Src", r"D:\Dst", "a/b.txt", RevealSide::Source, None);
        assert_eq!(
            target.unwrap(),
            Path::new(r"C:\Src").join("a").join("b.txt")
        );
        let target = reveal_target(r"C:\Src", r"D:\Dst", "", RevealSide::Destination, None);
        assert_eq!(target.unwrap(), PathBuf::from(r"D:\Dst"));
    }

    #[test]
    fn refuses_a_relative_path_that_leaves_the_folder() {
        let target = reveal_target("s", "d", "../x", RevealSide::Source, None);
        assert!(target.is_err());
    }

    #[test]
    fn the_recovery_folder_is_the_remembered_one_or_an_error() {
        let folder = PathBuf::from(r"E:\Rescue\r1");
        let target = reveal_target("s", "d", "", RevealSide::Recovery, Some(folder.clone()));
        assert_eq!(target.unwrap(), folder);
        assert!(reveal_target("s", "d", "", RevealSide::Recovery, None).is_err());
    }
}

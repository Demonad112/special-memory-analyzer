//! Step 1 of a run: list the source into `manifest.jsonl` and check, before anything is copied,
//! what would go wrong (space, names, long paths, write access), so it can be fixed first.

use crate::copy::{times_match, PART_SUFFIX};
use crate::delta::{self, Class, Dest, LeftBehind, Source};
use crate::reason::{classify, FailureReason, Side};
use crate::store::{LogStatus, PathKey, RunStore};
use crate::volume::volume_info;
use crate::walk::{self, Entry};
use crate::{
    machine_name, now_ms, user_name, CancelToken, ConflictPolicy, ItemKind, ItemResult, ItemStatus,
    Phase, Result, RunState, RunSummary, RunTotals, TransferError, TransferMode, TransferProgress,
    TransferSettings, TransferSink,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub use scan_core::names::{is_valid_windows_name, LONG_PATH_LIMIT};
/// The pre-flight file lists at most this many issues; the count covers all of them.
pub const MAX_LISTED_ISSUES: usize = 1000;

/// One row of `manifest.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub relative_path: String,
    pub kind: ItemKind,
    #[serde(default)]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<u64>,
    /// Set when pre-flight found a problem that stops this file from being copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<FailureReason>,
    /// An online-only (OneDrive) file. Reading it would download it, so Watch never hashes or
    /// opens such a source.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cloud: bool,
    /// Delta runs: a different file is at the destination and this one replaces it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// The file will not be copied.
    Blocker,
    /// The file will be copied but may cause trouble later.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightIssue {
    pub relative_path: String,
    pub reason: FailureReason,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub run_id: String,
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub excluded: u64,
    /// Files already at the destination with the same size and date.
    pub already_there: u64,
    /// Space the copy needs at the destination.
    pub bytes_needed: u64,
    pub free_bytes: Option<u64>,
    /// `C:\` or `\\server\share` of the destination.
    pub volume: Option<String>,
    pub enough_space: bool,
    pub writable: bool,
    pub write_error: Option<String>,
    /// Folders or files that could not be read while listing the source.
    pub scan_errors: u64,
    pub blocked: u64,
    pub issue_count: u64,
    pub issues: Vec<PreflightIssue>,
}

/// A subset of the source to copy, from a compare's missing list: the listed files, empty
/// folders, and the folders on the way to them. Everything else is left out of the run.
#[derive(Debug, Default)]
pub struct OnlyPaths {
    files: HashSet<String>,
    dirs: HashSet<String>,
}

impl OnlyPaths {
    /// `paths` are relative to the source with `\` or `/`; a trailing separator marks an empty
    /// folder. Names compare case-insensitively, like Windows.
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        let mut only = Self::default();
        for path in paths {
            let is_dir = path.ends_with(['\\', '/']);
            let key = path.replace('\\', "/").trim_matches('/').to_lowercase();
            if key.is_empty() {
                continue;
            }
            let mut end = 0;
            while let Some(slash) = key[end..].find('/') {
                end += slash;
                only.dirs.insert(key[..end].to_owned());
                end += 1;
            }
            if is_dir {
                only.dirs.insert(key);
            } else {
                only.files.insert(key);
            }
        }
        only
    }

    fn has_file(&self, relative_path: &str) -> bool {
        self.files.contains(&relative_path.to_lowercase())
    }

    fn has_dir(&self, relative_path: &str) -> bool {
        self.dirs.contains(&relative_path.to_lowercase())
    }
}

/// Lists the source and runs the pre-flight check into a new run folder under `root`.
/// Blocked files and unreadable folders are written to the results straight away, so they
/// show in the "Not copied" list before the copy starts.
pub fn prepare(
    root: &Path,
    settings: TransferSettings,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
) -> Result<(RunSummary, PreflightReport)> {
    prepare_only(root, settings, None, cancel, sink)
}

/// [`prepare`], limited to `only` when given.
pub fn prepare_only(
    root: &Path,
    settings: TransferSettings,
    only: Option<&OnlyPaths>,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
) -> Result<(RunSummary, PreflightReport)> {
    prepare_run(root, settings, only, None, cancel, sink)
}

/// The pre-flight of a delta run: "Copy changes since this run" (decision D-10).
///
/// Starts a new run with the same source, destination and settings as `previous_run_id`, whose
/// manifest holds only the files a second pass has to copy:
/// - files missing at the destination (new), with a different size or date (changed), or that
///   the previous run listed as not copied (retried);
/// - not the files that are unchanged, and not the files edited at the destination after the
///   previous run finished: those are counted and listed in `delta-left.jsonl`.
///
/// Files only at the destination are counted and left. Nothing there is ever removed. The
/// previous run's folder is only read.
pub fn prepare_delta(
    root: &Path,
    previous_run_id: &str,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
) -> Result<(RunSummary, PreflightReport)> {
    let previous = RunStore::open(root, previous_run_id)?;
    let before = previous.summary()?;
    if before.settings.mode != TransferMode::Copy {
        return Err(TransferError::BadRun(
            "A watched copy can't be followed up: another tool did that copy.".to_owned(),
        ));
    }
    if !matches!(before.state, RunState::Completed | RunState::Cancelled) {
        return Err(TransferError::BadRun(
            "Only a finished run (completed or cancelled) can be followed by a delta run."
                .to_owned(),
        ));
    }
    let finished_at_ms = before.finished_at_ms.ok_or_else(|| {
        TransferError::BadRun("The previous run has no finish time to compare against.".to_owned())
    })?;
    let retry = previous
        .latest_results_where(|item| {
            item.status == ItemStatus::NotCopied && item.kind == ItemKind::File
        })?
        .keys()
        .map(|path| lower_key(path))
        .collect();
    let mut left_before = HashMap::new();
    if let Some(rows) = previous.delta_left::<LeftBehind>()? {
        for row in rows {
            let row = row?;
            left_before.insert(lower_key(&row.relative_path), row);
        }
    }
    let plan = DeltaPlan {
        previous_run_id: previous_run_id.to_owned(),
        previous_finished_ms: finished_at_ms,
        retry,
        left_before,
        counts: DeltaCounts::default(),
        left: Vec::new(),
    };
    prepare_run(root, before.settings, None, Some(plan), cancel, sink)
}

/// What a delta run found while planning, and the files it left alone.
struct DeltaPlan {
    previous_run_id: String,
    previous_finished_ms: u64,
    /// Lowercased-path fingerprints of the files the previous run did not copy.
    retry: HashSet<PathKey>,
    /// What the previous run itself left alone, so a file stays left until it changes again.
    left_before: HashMap<PathKey, LeftBehind>,
    counts: DeltaCounts,
    left: Vec<LeftBehind>,
}

#[derive(Default)]
struct DeltaCounts {
    changed: u64,
    new: u64,
    retried: u64,
    unchanged: u64,
    changed_at_destination: u64,
}

fn prepare_run(
    root: &Path,
    settings: TransferSettings,
    only: Option<&OnlyPaths>,
    delta: Option<DeltaPlan>,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
) -> Result<(RunSummary, PreflightReport)> {
    let source = PathBuf::from(&settings.source);
    let destination = PathBuf::from(&settings.destination);
    check_roots(&source, &destination)?;

    let store = RunStore::create(root)?;
    let mut summary = RunSummary {
        id: store.id(),
        settings: settings.clone(),
        state: RunState::Prepared,
        created_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        machine: machine_name(),
        user: user_name(),
        totals: RunTotals::default(),
        error: None,
        previous_run_id: delta.as_ref().map(|plan| plan.previous_run_id.clone()),
    };
    store.save_summary(&summary)?;

    let mut report = PreflightReport {
        run_id: store.id(),
        files: 0,
        folders: 0,
        bytes: 0,
        excluded: 0,
        already_there: 0,
        bytes_needed: 0,
        free_bytes: None,
        volume: None,
        enough_space: true,
        writable: true,
        write_error: None,
        scan_errors: 0,
        blocked: 0,
        issue_count: 0,
        issues: Vec::new(),
    };
    let scan = scan_source(
        &store,
        &settings,
        &source,
        &destination,
        (only, delta),
        cancel,
        sink,
        &mut report,
    )
    .and_then(|scanned| match scanned.delta {
        Some(plan) => finish_delta(
            &store,
            &settings,
            &destination,
            plan,
            &scanned.source_keys,
            cancel,
            sink,
            &mut summary.totals,
        ),
        None => Ok(()),
    });
    if let Err(error) = scan {
        summary.state = if matches!(error, TransferError::Cancelled) {
            RunState::Cancelled
        } else {
            RunState::Failed
        };
        summary.error = Some(error.to_string());
        store.save_summary(&summary)?;
        return Err(error);
    }

    if let Some(volume) = volume_info(&destination) {
        report.free_bytes = Some(volume.free_bytes);
        report.volume = Some(volume.root);
        report.enough_space = volume.free_bytes >= report.bytes_needed;
    }
    if settings.mode == TransferMode::Copy {
        if let Err(error) = probe_write(&destination) {
            report.writable = false;
            report.write_error = Some(error.to_string());
        }
    }

    summary.totals.planned_files = report.files;
    summary.totals.planned_bytes = report.bytes;
    summary.totals.folders = report.folders;
    summary.totals.excluded = report.excluded;
    summary.totals = store.latest_totals(&summary.totals)?;
    store.save_summary(&summary)?;
    store.save_preflight(&report)?;
    store.audit(
        "prepare",
        LogStatus::Succeeded,
        "Source listed and pre-flight checked",
        &[
            ("source", settings.source.clone().into()),
            ("destination", settings.destination.clone().into()),
            ("files", report.files.into()),
            ("bytes", report.bytes.into()),
            ("blocked", report.blocked.into()),
            ("scanErrors", report.scan_errors.into()),
        ],
    );
    sink.progress(&TransferProgress {
        run_id: store.id(),
        phase: Phase::Done,
        files_done: report.files,
        files_total: report.files,
        bytes_done: 0,
        bytes_total: report.bytes,
        copied: 0,
        skipped: 0,
        not_copied: summary.totals.not_copied,
        current: None,
    });
    Ok((summary, report))
}

fn check_roots(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(TransferError::InvalidPath(format!(
            "The source is not a folder: {}",
            source.display()
        )));
    }
    if destination.as_os_str().is_empty() {
        return Err(TransferError::InvalidPath(
            "Choose a destination folder".to_owned(),
        ));
    }
    let key = |path: &Path| {
        std::path::absolute(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let (source_key, destination_key) = (key(source), key(destination));
    if destination_key == source_key || destination_key.starts_with(&format!("{source_key}\\")) {
        return Err(TransferError::InvalidPath(
            "The destination is inside the source. Choose a folder outside it.".to_owned(),
        ));
    }
    Ok(())
}

struct ScanState<'a> {
    settings: &'a TransferSettings,
    destination: &'a Path,
    only: Option<&'a OnlyPaths>,
    delta: Option<DeltaPlan>,
    /// Fingerprints of the lowercased paths, to spot names that differ only by case.
    seen: HashSet<PathKey>,
}

/// What listing the source leaves behind for a delta run to finish with.
struct Scanned {
    delta: Option<DeltaPlan>,
    source_keys: HashSet<PathKey>,
}

#[allow(clippy::too_many_arguments)]
fn scan_source(
    store: &RunStore,
    settings: &TransferSettings,
    source: &Path,
    destination: &Path,
    (only, delta): (Option<&OnlyPaths>, Option<DeltaPlan>),
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
    report: &mut PreflightReport,
) -> Result<Scanned> {
    let tree = walk::scan_tree(source, settings.ignore_junk, cancel, &mut |scanned| {
        sink.progress(&TransferProgress {
            run_id: store.id(),
            phase: Phase::Scanning,
            files_done: scanned.files,
            files_total: 0,
            bytes_done: 0,
            bytes_total: scanned.bytes,
            copied: 0,
            skipped: 0,
            not_copied: 0,
            current: Some(scanned.current.clone()).filter(|current| !current.is_empty()),
        });
    })?;

    let mut state = ScanState {
        settings,
        destination,
        only,
        delta,
        seen: HashSet::new(),
    };
    let mut manifest = store.manifest_writer()?;
    let mut results = store.results_writer()?;
    walk::visit(&tree, &mut |entry| {
        let (row, result, enter) = state.visit(entry, report);
        if let Some(row) = row {
            manifest.write(&row)?;
        }
        if let Some(result) = result {
            sink.item(&result);
            results.write(&result)?;
        }
        Ok(enter)
    })?;
    manifest.finish()?;
    results.finish()?;
    Ok(Scanned {
        delta: state.delta,
        source_keys: state.seen,
    })
}

/// The last part of planning a delta run: saves the files left alone, counts the files that are
/// only at the destination, and puts the counts on the run.
#[allow(clippy::too_many_arguments)]
fn finish_delta(
    store: &RunStore,
    settings: &TransferSettings,
    destination: &Path,
    plan: DeltaPlan,
    source_keys: &HashSet<PathKey>,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
    totals: &mut RunTotals,
) -> Result<()> {
    let mut left = store.delta_left_writer()?;
    for row in &plan.left {
        left.write(row)?;
    }
    left.finish()?;
    let destination_only = count_destination_only(
        destination,
        settings.ignore_junk,
        source_keys,
        cancel,
        &mut |scanned| {
            sink.progress(&TransferProgress {
                run_id: store.id(),
                phase: Phase::Scanning,
                files_done: scanned.files,
                files_total: 0,
                bytes_done: 0,
                bytes_total: scanned.bytes,
                copied: 0,
                skipped: 0,
                not_copied: 0,
                current: Some(scanned.current.clone()).filter(|current| !current.is_empty()),
            });
        },
    )?;
    totals.changed = plan.counts.changed;
    totals.new = plan.counts.new;
    totals.retried = plan.counts.retried;
    totals.unchanged = plan.counts.unchanged;
    totals.changed_at_destination = plan.counts.changed_at_destination;
    totals.destination_only = destination_only;
    Ok(())
}

/// Files at the destination that the source does not have. Only counted: nothing is touched.
fn count_destination_only(
    destination: &Path,
    ignore_junk: bool,
    source_keys: &HashSet<PathKey>,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(&scan_core::ScanProgress),
) -> Result<u64> {
    if !destination.is_dir() {
        return Ok(0);
    }
    let tree = walk::scan_tree(destination, ignore_junk, cancel, on_progress)?;
    let mut only_here = 0;
    walk::visit(&tree, &mut |entry| {
        if let Entry::File { relative_path, .. } = entry {
            // A half-written copy from an interrupted run is not a file of the destination.
            let partial = relative_path.ends_with(PART_SUFFIX);
            if !partial && !source_keys.contains(&PathKey::of(&relative_path.to_lowercase())) {
                only_here += 1;
            }
        }
        Ok(true)
    })?;
    Ok(only_here)
}

impl ScanState<'_> {
    /// The manifest row, an early result row, and whether to enter a folder.
    fn visit(
        &mut self,
        entry: Entry,
        report: &mut PreflightReport,
    ) -> (Option<ManifestEntry>, Option<ItemResult>, bool) {
        match entry {
            Entry::Folder { relative_path } => {
                if self.only.is_some_and(|only| !only.has_dir(&relative_path)) {
                    return (None, None, false);
                }
                report.folders += 1;
                let blocked = self.check_name(&relative_path, report);
                let result = blocked.map(|reason| {
                    report.blocked += 1;
                    blocked_result(&relative_path, ItemKind::Folder, 0, reason)
                });
                let row = ManifestEntry {
                    relative_path,
                    kind: ItemKind::Folder,
                    size: 0,
                    modified_at_ms: None,
                    blocked,
                    cloud: false,
                    replace: false,
                };
                // A blocked folder cannot be created, so its contents cannot be copied either.
                (Some(row), result, blocked.is_none())
            }
            Entry::File {
                relative_path,
                size,
                modified_ms: modified_at_ms,
                cloud,
            } => {
                if self.only.is_some_and(|only| !only.has_file(&relative_path)) {
                    return (None, None, false);
                }
                let mut delta_class = None;
                let mut replace = false;
                if self.delta.is_some() {
                    let source = Source {
                        size,
                        modified_ms: modified_at_ms,
                    };
                    let (class, dest) = self.delta_class(&relative_path, source);
                    match class {
                        Class::Unchanged => {
                            self.seen.insert(lower_key(&relative_path));
                            report.already_there += 1;
                            if let Some(plan) = &mut self.delta {
                                plan.counts.unchanged += 1;
                            }
                            return (None, None, false);
                        }
                        Class::ChangedAtDestination => {
                            self.seen.insert(lower_key(&relative_path));
                            let (dest_size, dest_modified_ms) = match dest {
                                Dest::File { size, modified_ms } => (size, modified_ms),
                                _ => (0, None),
                            };
                            if let Some(plan) = &mut self.delta {
                                plan.counts.changed_at_destination += 1;
                                plan.left.push(LeftBehind {
                                    relative_path,
                                    size,
                                    source_modified_ms: modified_at_ms,
                                    dest_size,
                                    dest_modified_ms,
                                });
                            }
                            return (None, None, false);
                        }
                        Class::New | Class::Changed | Class::Retried => {
                            replace = matches!(dest, Dest::File { .. });
                            delta_class = Some(class);
                        }
                    }
                }
                report.files += 1;
                report.bytes += size;
                let blocked = self.check_name(&relative_path, report).or_else(|| {
                    // Watch mode leaves it to the copying program; only Copy would download it.
                    let skip = cloud
                        && self.settings.mode == TransferMode::Copy
                        && !self.settings.download_cloud;
                    skip.then_some(FailureReason::CloudOnly)
                });
                let target = destination_path(self.destination, &relative_path);
                if blocked.is_none() {
                    match (delta_class, &mut self.delta) {
                        (Some(class), Some(plan)) => {
                            report.bytes_needed += size;
                            match class {
                                Class::New => plan.counts.new += 1,
                                Class::Changed => plan.counts.changed += 1,
                                _ => plan.counts.retried += 1,
                            }
                        }
                        _ => self.count_space(&target, size, modified_at_ms, report),
                    }
                }
                let result = blocked.map(|reason| {
                    report.blocked += 1;
                    blocked_result(&relative_path, ItemKind::File, size, reason)
                });
                let row = ManifestEntry {
                    relative_path,
                    kind: ItemKind::File,
                    size,
                    modified_at_ms,
                    blocked,
                    cloud,
                    replace,
                };
                (Some(row), result, false)
            }
            Entry::InvalidName {
                relative_path,
                kind,
                size,
            } => {
                match kind {
                    ItemKind::File => {
                        report.files += 1;
                        report.bytes += size;
                    }
                    ItemKind::Folder => report.folders += 1,
                }
                report.blocked += 1;
                report.issue_count += 1;
                if report.issues.len() < MAX_LISTED_ISSUES {
                    report.issues.push(PreflightIssue {
                        relative_path: relative_path.clone(),
                        reason: FailureReason::InvalidName,
                        severity: Severity::Blocker,
                        detail: Some(
                            "The name is not valid Unicode, so it cannot be opened by name"
                                .to_owned(),
                        ),
                    });
                }
                let reason = FailureReason::InvalidName;
                let result = blocked_result(&relative_path, kind, size, reason);
                let row = ManifestEntry {
                    relative_path,
                    kind,
                    size,
                    modified_at_ms: None,
                    blocked: Some(reason),
                    cloud: false,
                    replace: false,
                };
                (Some(row), Some(result), false)
            }
            Entry::Link => {
                report.excluded += 1;
                (None, None, false)
            }
            Entry::Unreadable {
                relative_path,
                error,
            } => {
                report.scan_errors += 1;
                let mut result = ItemResult::not_copied(
                    relative_path,
                    ItemKind::Folder,
                    0,
                    classify(&error, Side::Source),
                    Some(Side::Source),
                    error.to_string(),
                );
                result.os_code = error.raw_os_error();
                (None, Some(result), false)
            }
        }
    }

    /// How a delta run treats a source file, and what the destination held when it looked.
    fn delta_class(&self, relative_path: &str, source: Source) -> (Class, Dest) {
        let Some(plan) = &self.delta else {
            return (Class::New, Dest::Missing);
        };
        let key = lower_key(relative_path);
        let dest = Dest::read(&destination_path(self.destination, relative_path));
        let class = delta::classify(
            source,
            dest,
            plan.retry.contains(&key),
            plan.previous_finished_ms,
            plan.left_before.get(&key),
        );
        (class, dest)
    }

    /// A blocking name problem, recording issues (a long path is only a warning).
    fn check_name(
        &mut self,
        relative_path: &str,
        report: &mut PreflightReport,
    ) -> Option<FailureReason> {
        let mut add = |reason, severity, detail: Option<String>| {
            report.issue_count += 1;
            if report.issues.len() < MAX_LISTED_ISSUES {
                report.issues.push(PreflightIssue {
                    relative_path: relative_path.to_owned(),
                    reason,
                    severity,
                    detail,
                });
            }
        };
        if let Some(name) = relative_path
            .split('/')
            .find(|name| !is_valid_windows_name(name))
        {
            add(
                FailureReason::InvalidName,
                Severity::Blocker,
                Some(name.to_owned()),
            );
            return Some(FailureReason::InvalidName);
        }
        if !self.seen.insert(lower_key(relative_path)) {
            add(FailureReason::NameCollision, Severity::Blocker, None);
            return Some(FailureReason::NameCollision);
        }
        let length = destination_path(self.destination, relative_path)
            .to_string_lossy()
            .encode_utf16()
            .count();
        if length > LONG_PATH_LIMIT {
            add(
                FailureReason::PathTooLong,
                Severity::Warning,
                Some(format!("{length} characters")),
            );
        }
        None
    }

    fn count_space(
        &self,
        target: &Path,
        size: u64,
        modified_at_ms: Option<u64>,
        report: &mut PreflightReport,
    ) {
        let Ok(existing) = fs::metadata(target) else {
            report.bytes_needed += size;
            return;
        };
        let source_time = modified_at_ms.map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
        if existing.is_file()
            && existing.len() == size
            && times_match(source_time, existing.modified().ok())
        {
            report.already_there += 1;
        } else if self.settings.conflict != ConflictPolicy::Skip {
            // The new copy is written beside the old file before it replaces it.
            report.bytes_needed += size;
        }
    }
}

/// The fingerprint of a relative path as `seen` and the delta sets keep it: case-insensitive.
fn lower_key(relative_path: &str) -> PathKey {
    PathKey::of(&relative_path.to_lowercase())
}

fn blocked_result(
    relative_path: &str,
    kind: ItemKind,
    size: u64,
    reason: FailureReason,
) -> ItemResult {
    let side = if reason == FailureReason::CloudOnly {
        Side::Source
    } else {
        Side::Destination
    };
    ItemResult::not_copied(
        relative_path,
        kind,
        size,
        reason,
        Some(side),
        reason.explanation(),
    )
}

/// Whether a `/`-separated relative path stays inside the folder it is joined to: no `.` or `..`
/// part, and no part that names a drive or starts a root (`:` or `\\`). Paths in the run files
/// come from preflight and are always safe; this guards against a run file edited by hand.
pub fn safe_relative(relative_path: &str) -> bool {
    relative_path
        .split('/')
        .filter(|part| !part.is_empty())
        .all(|part| part != "." && part != ".." && !part.contains([':', '\\']))
}

/// `dest` joined with a `/`-separated relative path.
pub fn destination_path(destination: &Path, relative_path: &str) -> PathBuf {
    relative_path
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(destination.to_path_buf(), |path, part| path.join(part))
}

/// Creates the destination if needed and writes and deletes a small file in it.
fn probe_write(destination: &Path) -> std::io::Result<()> {
    let created = !destination.exists();
    fs::create_dir_all(destination)?;
    let probe = destination.join(format!(".analyzer-write-test-{}.tmp", std::process::id()));
    let written = fs::write(&probe, b"Deep Server Analyzer write test");
    let _ = fs::remove_file(&probe);
    if created {
        // Only removes it if still empty; the copy creates it again.
        let _ = fs::remove_dir(destination);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use crate::{NullSink, VerifyLevel};

    pub(crate) fn settings(source: &Path, destination: &Path) -> TransferSettings {
        TransferSettings {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            mode: TransferMode::Copy,
            verify: VerifyLevel::SizeAndTime,
            conflict: ConflictPolicy::Skip,
            ignore_junk: false,
            download_cloud: false,
            preserve: Default::default(),
        }
    }

    #[test]
    fn windows_name_rules() {
        for good in [
            "a.txt",
            "CONFIG.SYS",
            "con-notes.txt",
            ".gitignore",
            "COM10",
        ] {
            assert!(is_valid_windows_name(good), "{good}");
        }
        for bad in [
            "CON",
            "con.txt",
            "Nul",
            "LPT1.log",
            "trailing.",
            "trailing ",
            "a:b",
            "why?",
            "",
        ] {
            assert!(!is_valid_windows_name(bad), "{bad}");
        }
    }

    #[test]
    fn prepare_lists_the_source_and_counts_space() {
        let dir = TempDir::new("prepare-basic");
        dir.write("src/a.txt", b"aaaa");
        dir.write("src/sub/b.txt", b"bb");
        dir.write("src/empty/c.txt", b"");
        let source = dir.path("src");
        let destination = dir.path("dst");

        let (summary, report) = prepare(
            &dir.path("runs"),
            settings(&source, &destination),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!(summary.state, RunState::Prepared);
        assert_eq!((report.files, report.bytes, report.bytes_needed), (3, 6, 6));
        assert_eq!(report.folders, 2); // sub, empty
        assert!(report.writable && report.enough_space);
        assert!(
            !destination.exists(),
            "the write probe cleans up after itself"
        );
        let store = RunStore::open(&dir.path("runs"), &summary.id).unwrap();
        let rows: Vec<ManifestEntry> = store.manifest().unwrap().map(|row| row.unwrap()).collect();
        let paths: Vec<&str> = rows.iter().map(|row| row.relative_path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["a.txt", "empty", "empty/c.txt", "sub", "sub/b.txt"]
        );
    }

    #[test]
    fn prepare_counts_files_already_at_the_destination() {
        let dir = TempDir::new("prepare-there");
        dir.write("src/a.txt", b"same");
        dir.write("src/b.txt", b"new");
        let source = dir.path("src");
        let destination = dir.path("dst");
        crate::copy::copy_file(
            &source.join("a.txt"),
            &destination.join("a.txt"),
            &crate::CopyOptions::default(),
            &CancelToken::default(),
            &mut |_| {},
        )
        .unwrap();

        let (_, report) = prepare(
            &dir.path("runs"),
            settings(&source, &destination),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!((report.already_there, report.bytes_needed), (1, 3));
    }

    #[test]
    fn junk_is_left_out_of_the_listing_when_asked() {
        let dir = TempDir::new("prepare-junk");
        dir.write("src/a.txt", b"a");
        dir.write("src/Thumbs.db", b"junk");
        let settings = TransferSettings {
            ignore_junk: true,
            ..settings(&dir.path("src"), &dir.path("dst"))
        };
        let (summary, report) = prepare(
            &dir.path("runs"),
            settings,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();
        assert_eq!((report.files, report.bytes), (1, 1));
        assert!(summary.settings.ignore_junk, "kept for the report");
    }

    #[test]
    fn only_paths_match_any_separator_and_case_and_include_ancestors() {
        let only = OnlyPaths::new(["Sub\\Deep\\B.txt", "top.txt", "Empty\\Folder\\"]);
        assert!(only.has_file("sub/deep/b.txt"));
        assert!(only.has_file("TOP.txt"));
        assert!(!only.has_file("sub/other.txt"));
        assert!(only.has_dir("Sub"));
        assert!(only.has_dir("sub/deep"));
        assert!(only.has_dir("empty/folder"));
        assert!(only.has_dir("empty"));
        assert!(!only.has_dir("elsewhere"));
    }

    #[test]
    fn prepare_only_lists_just_the_chosen_files() {
        let dir = TempDir::new("prepare-only");
        dir.write("src/a.txt", b"aaaa");
        dir.write("src/sub/b.txt", b"bb");
        dir.write("src/sub/c.txt", b"c");
        dir.write("src/other/d.txt", b"dddd");
        let source = dir.path("src");
        let destination = dir.path("dst");
        let only = OnlyPaths::new(["sub\\b.txt", "a.txt"]);

        let (summary, report) = prepare_only(
            &dir.path("runs"),
            settings(&source, &destination),
            Some(&only),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!((report.files, report.bytes), (2, 6));
        assert_eq!(report.folders, 1);
        let store = RunStore::open(&dir.path("runs"), &summary.id).unwrap();
        let rows: Vec<ManifestEntry> = store.manifest().unwrap().map(|row| row.unwrap()).collect();
        let paths: Vec<&str> = rows.iter().map(|row| row.relative_path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "sub", "sub/b.txt"]);
    }

    /// A name that is not valid Unicode: an unpaired surrogate on Windows (NTFS allows it), an
    /// invalid UTF-8 byte on Unix.
    fn lossy_name(stem: &str) -> std::ffi::OsString {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let mut units = vec![0xD800_u16];
            units.extend(stem.encode_utf16());
            std::ffi::OsString::from_wide(&units)
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut bytes = vec![0xFF_u8];
            bytes.extend(stem.bytes());
            std::ffi::OsString::from_vec(bytes)
        }
    }

    #[test]
    fn names_that_are_not_valid_unicode_are_listed_as_invalid_and_never_copied() {
        let dir = TempDir::new("prepare-lossy");
        dir.write("src/ok.txt", b"fine");
        let file = dir.path("src").join(lossy_name("file.txt"));
        let folder = dir.path("src").join(lossy_name("folder"));
        if fs::write(&file, b"odd").is_err() || fs::create_dir_all(&folder).is_err() {
            return; // this file system cannot hold such names
        }
        fs::write(folder.join("inside.txt"), b"hidden").unwrap();
        let (source, destination) = (dir.path("src"), dir.path("dst"));

        let (summary, report) = prepare(
            &dir.path("runs"),
            settings(&source, &destination),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!(report.blocked, 2);
        assert!(report
            .issues
            .iter()
            .all(|issue| issue.reason == FailureReason::InvalidName));
        let store = RunStore::open(&dir.path("runs"), &summary.id).unwrap();
        let rows: Vec<ManifestEntry> = store.manifest().unwrap().map(|row| row.unwrap()).collect();
        let blocked: Vec<_> = rows.iter().filter(|row| row.blocked.is_some()).collect();
        assert_eq!(blocked.len(), 2);
        assert!(blocked
            .iter()
            .all(|row| row.blocked == Some(FailureReason::InvalidName)));
        assert!(
            !rows
                .iter()
                .any(|row| row.relative_path.ends_with("inside.txt")),
            "the folder with the odd name is not entered"
        );

        let summary = crate::run::run_copy(
            &dir.path("runs"),
            &summary.id,
            None,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();
        assert_eq!((summary.totals.copied, summary.totals.not_copied), (1, 2));
        assert!(destination.join("ok.txt").is_file());
        let copied: Vec<_> = fs::read_dir(&destination).unwrap().collect();
        assert_eq!(copied.len(), 1, "nothing was made under a rewritten name");
    }

    #[test]
    fn relative_paths_that_could_leave_the_folder_are_not_safe() {
        assert!(safe_relative(""));
        assert!(safe_relative("docs/report, final.txt"));
        assert!(safe_relative("/leading/slash.txt"));
        assert!(!safe_relative("../outside.txt"));
        assert!(!safe_relative("docs/../../outside.txt"));
        assert!(!safe_relative("./here.txt"));
        assert!(!safe_relative("C:/Windows/x.dll"));
        assert!(!safe_relative(r"docs\..\x.txt"));
        assert!(!safe_relative(r"\\server\share\x.txt"));
        assert!(!safe_relative("file.txt:stream"));
    }

    #[test]
    fn long_destination_paths_are_warnings() {
        let dir = TempDir::new("prepare-long");
        let deep = "d".repeat(120);
        dir.write(&format!("src/{deep}/{deep}/f.txt"), b"f");
        let (summary, report) = prepare(
            &dir.path("runs"),
            settings(&dir.path("src"), &dir.path("dst")),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        // The long folder and the file in it.
        assert_eq!(report.issues.len(), 2);
        assert!(report
            .issues
            .iter()
            .all(|issue| issue.reason == FailureReason::PathTooLong
                && issue.severity == Severity::Warning));
        assert_eq!(summary.totals.not_copied, 0);
    }

    #[test]
    fn destination_inside_the_source_is_refused() {
        let dir = TempDir::new("prepare-inside");
        dir.write("src/a.txt", b"a");
        let error = prepare(
            &dir.path("runs"),
            settings(&dir.path("src"), &dir.path("src/backup")),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap_err();
        assert!(matches!(error, TransferError::InvalidPath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_folders_are_listed_as_not_copied_before_the_copy() {
        let dir = TempDir::new("prepare-denied");
        dir.write("src/ok.txt", b"ok");
        dir.write("src/private/secret.txt", b"s");
        let _guard = crate::test_support::DenyGuard::new(&dir.path("src/private"), "(RD)");

        let (summary, report) = prepare(
            &dir.path("runs"),
            settings(&dir.path("src"), &dir.path("dst")),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!(report.scan_errors, 1);
        assert_eq!(summary.totals.not_copied, 1);
        let store = RunStore::open(&dir.path("runs"), &summary.id).unwrap();
        let latest = store.latest_results().unwrap();
        let row = &latest["private"];
        assert_eq!(row.status, ItemStatus::NotCopied);
        assert_eq!(row.kind, ItemKind::Folder);
        assert_eq!(row.reason, Some(FailureReason::AccessDenied));
    }

    mod delta_runs {
        use super::*;
        use crate::delta::LeftBehind;
        use crate::run::run_copy;
        use crate::{ItemStatus, TransferError};
        use std::time::{Duration, SystemTime};

        const DAY: Duration = Duration::from_secs(86_400);

        fn set_time(path: &Path, time: SystemTime) {
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(time)
                .unwrap();
        }

        /// A finished first run over `src` into `dst`. Returns the run id.
        fn first_run(dir: &TempDir) -> String {
            let (summary, _) = prepare(
                &dir.path("runs"),
                settings(&dir.path("src"), &dir.path("dst")),
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap();
            let summary = run_copy(
                &dir.path("runs"),
                &summary.id,
                None,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap();
            assert_eq!(summary.state, RunState::Completed);
            summary.id
        }

        fn delta(dir: &TempDir, previous: &str) -> (RunSummary, PreflightReport) {
            prepare_delta(
                &dir.path("runs"),
                previous,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap()
        }

        fn manifest_paths(dir: &TempDir, run_id: &str) -> Vec<String> {
            let store = RunStore::open(&dir.path("runs"), run_id).unwrap();
            store
                .manifest::<ManifestEntry>()
                .unwrap()
                .map(|row| row.unwrap())
                .filter(|row| row.kind == ItemKind::File)
                .map(|row| row.relative_path)
                .collect()
        }

        fn left_alone(dir: &TempDir, run_id: &str) -> Vec<LeftBehind> {
            let store = RunStore::open(&dir.path("runs"), run_id).unwrap();
            store
                .delta_left::<LeftBehind>()
                .unwrap()
                .map(|rows| rows.map(|row| row.unwrap()).collect())
                .unwrap_or_default()
        }

        #[test]
        fn copies_only_what_changed_and_leaves_the_rest() {
            let dir = TempDir::new("delta-basic");
            dir.write("src/same.txt", b"same");
            dir.write("src/size.txt", b"short");
            dir.write("src/newer.txt", b"newer");
            dir.write("src/older.txt", b"older");
            dir.write("src/sub/keep.txt", b"keep");
            let first = first_run(&dir);

            // After the first run: the source changes.
            dir.write("src/size.txt", b"much longer now");
            let touched = SystemTime::now() + DAY;
            set_time(&dir.path("src/newer.txt"), touched);
            set_time(&dir.path("src/older.txt"), SystemTime::now() - 30 * DAY);
            dir.write("src/new.txt", b"brand new");
            dir.write("src/sub/deeper/new2.txt", b"new in a new folder");
            // A file the first run could not copy: listed as not copied, and absent.
            dir.write("src/failed.txt", b"failed before");
            let store = RunStore::open(&dir.path("runs"), &first).unwrap();
            let mut results = store.results_writer().unwrap();
            results
                .write(&ItemResult::not_copied(
                    "failed.txt",
                    ItemKind::File,
                    13,
                    FailureReason::FileLocked,
                    Some(Side::Source),
                    "in use",
                ))
                .unwrap();
            results.finish().unwrap();

            let (summary, report) = delta(&dir, &first);

            assert_eq!(summary.previous_run_id.as_deref(), Some(first.as_str()));
            let totals = &summary.totals;
            assert_eq!(
                (
                    totals.changed,
                    totals.new,
                    totals.retried,
                    totals.unchanged,
                    totals.changed_at_destination,
                    totals.destination_only
                ),
                (3, 2, 1, 2, 0, 0)
            );
            assert_eq!(report.already_there, 2);
            assert_eq!(
                manifest_paths(&dir, &summary.id),
                [
                    "failed.txt",
                    "new.txt",
                    "newer.txt",
                    "older.txt",
                    "size.txt",
                    "sub/deeper/new2.txt"
                ]
            );
            assert_eq!(totals.planned_files, 6);

            let done = run_copy(
                &dir.path("runs"),
                &summary.id,
                None,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap();
            assert_eq!(
                (done.state, done.totals.copied, done.totals.not_copied),
                (RunState::Completed, 6, 0)
            );
            assert_eq!(done.totals.new, 2, "the plan survives the recount");
            let read = |name: &str| fs::read(dir.path(&format!("dst/{name}"))).unwrap();
            assert_eq!(read("size.txt"), b"much longer now");
            assert_eq!(read("older.txt"), b"older");
            assert_eq!(read("failed.txt"), b"failed before");
            assert_eq!(read("sub/deeper/new2.txt"), b"new in a new folder");
            let modified = |name: &str| {
                fs::metadata(dir.path(&format!("dst/{name}")))
                    .unwrap()
                    .modified()
                    .unwrap()
            };
            let source_modified = |name: &str| {
                fs::metadata(dir.path(&format!("src/{name}")))
                    .unwrap()
                    .modified()
                    .unwrap()
            };
            assert!(
                crate::copy::times_match(
                    Some(modified("older.txt")),
                    Some(source_modified("older.txt"))
                ),
                "an older source date is carried over, so the next delta run sees it as unchanged"
            );

            // A third pass over the same trees has nothing left to do.
            let (again, _) = delta(&dir, &done.id);
            assert_eq!(again.previous_run_id.as_deref(), Some(done.id.as_str()));
            assert_eq!(
                (
                    again.totals.changed,
                    again.totals.new,
                    again.totals.retried,
                    again.totals.unchanged
                ),
                (0, 0, 0, 8)
            );
            assert!(manifest_paths(&dir, &again.id).is_empty());
        }

        #[test]
        fn a_file_edited_at_the_destination_after_the_run_is_left_and_listed() {
            let dir = TempDir::new("delta-edited");
            dir.write("src/doc.txt", b"original");
            dir.write("src/other.txt", b"other");
            let first = first_run(&dir);

            fs::write(dir.path("dst/doc.txt"), b"edited by the owner").unwrap();
            set_time(
                &dir.path("dst/doc.txt"),
                SystemTime::now() + 3600 * Duration::from_secs(1),
            );
            dir.write("src/other.txt", b"other, changed in the source");

            let (summary, _) = delta(&dir, &first);
            assert_eq!(summary.totals.changed_at_destination, 1);
            assert_eq!(summary.totals.changed, 1);
            assert_eq!(manifest_paths(&dir, &summary.id), ["other.txt"]);
            let left = left_alone(&dir, &summary.id);
            assert_eq!(left.len(), 1);
            assert_eq!(left[0].relative_path, "doc.txt");
            assert_eq!(left[0].dest_size, 19);

            let done = run_copy(
                &dir.path("runs"),
                &summary.id,
                None,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap();
            assert_eq!(done.totals.not_copied, 0);
            assert_eq!(
                fs::read(dir.path("dst/doc.txt")).unwrap(),
                b"edited by the owner"
            );
            assert_eq!(
                fs::read(dir.path("dst/other.txt")).unwrap(),
                b"other, changed in the source"
            );

            // The next delta run still leaves it, although the edit is now older than the
            // previous run's end.
            dir.write("src/doc.txt", b"original, then changed in the source too");
            let (next, _) = delta(&dir, &done.id);
            assert_eq!(next.totals.changed_at_destination, 1);
            assert!(manifest_paths(&dir, &next.id).is_empty());
            assert_eq!(left_alone(&dir, &next.id).len(), 1);
        }

        #[test]
        fn a_destination_edit_made_after_a_crash_is_left_not_overwritten() {
            // DSA-089: the recovered run finished when its results were last written, not when
            // the app was started again.
            let dir = TempDir::new("delta-recovered");
            dir.write("src/a.txt", b"original");
            let first = first_run(&dir);
            let store = RunStore::open(&dir.path("runs"), &first).unwrap();
            let crashed = SystemTime::now() - 3 * DAY;
            let results = store.dir().join("results.jsonl");
            set_time(&results, crashed);
            let mut summary = store.summary().unwrap();
            summary.state = RunState::Running;
            summary.finished_at_ms = None;
            store.save_summary(&summary).unwrap();

            // After the crash, before the app was opened again: the destination file is edited.
            dir.write("dst/a.txt", b"edited at the destination");
            set_time(&dir.path("dst/a.txt"), crashed + DAY);
            // The source also differs, so a plain delta run would copy it.
            dir.write("src/a.txt", b"original, changed again at the source");
            set_time(&dir.path("src/a.txt"), crashed - DAY);

            assert!(crate::run::recover_interrupted(&store).unwrap());
            let finished = store.summary().unwrap().finished_at_ms.unwrap();
            let crashed_ms = crashed
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            assert!(
                finished.abs_diff(crashed_ms) < 2_000,
                "{finished} {crashed_ms}"
            );

            let (next, _) = delta(&dir, &first);
            assert_eq!(next.totals.changed_at_destination, 1);
            assert!(manifest_paths(&dir, &next.id).is_empty());
            assert_eq!(left_alone(&dir, &next.id).len(), 1);
            assert_eq!(
                fs::read(dir.path("dst/a.txt")).unwrap(),
                b"edited at the destination"
            );
        }

        #[test]
        fn files_only_at_the_destination_are_counted_and_never_removed() {
            let dir = TempDir::new("delta-dest-only");
            dir.write("src/a.txt", b"a");
            let first = first_run(&dir);
            dir.write("dst/extra.txt", b"only here");
            dir.write("dst/folder/deeper/more.txt", b"also only here");
            dir.write("dst/folder/other.txt", b"and here");
            dir.write("dst/a.txt.dspart", b"half a copy");

            let (summary, _) = delta(&dir, &first);
            assert_eq!(summary.totals.destination_only, 3);
            let done = run_copy(
                &dir.path("runs"),
                &summary.id,
                None,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap();
            assert_eq!(done.totals.destination_only, 3);
            for kept in [
                "extra.txt",
                "folder/deeper/more.txt",
                "folder/other.txt",
                "a.txt",
            ] {
                assert!(dir.path(&format!("dst/{kept}")).is_file(), "{kept}");
            }
        }

        #[test]
        fn a_replacement_waits_when_the_file_is_edited_between_planning_and_copying() {
            let dir = TempDir::new("delta-race");
            dir.write("src/doc.txt", b"original");
            let first = first_run(&dir);
            dir.write("src/doc.txt", b"changed in the source");
            let (summary, _) = delta(&dir, &first);
            assert_eq!(summary.totals.changed, 1);

            // Someone saves a different version at the destination before the copy gets there.
            fs::write(dir.path("dst/doc.txt"), b"saved by a person meanwhile").unwrap();
            set_time(&dir.path("dst/doc.txt"), SystemTime::now() + DAY);

            let done = run_copy(
                &dir.path("runs"),
                &summary.id,
                None,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap();
            assert_eq!((done.totals.copied, done.totals.not_copied), (0, 1));
            assert_eq!(
                fs::read(dir.path("dst/doc.txt")).unwrap(),
                b"saved by a person meanwhile"
            );
            let store = RunStore::open(&dir.path("runs"), &summary.id).unwrap();
            let row = &store.latest_results().unwrap()["doc.txt"];
            assert_eq!(row.status, ItemStatus::NotCopied);
            assert_eq!(row.reason, Some(FailureReason::TargetExistsDifferent));
        }

        #[test]
        fn cancelling_a_delta_run_leaves_the_previous_run_untouched() {
            let dir = TempDir::new("delta-cancel");
            dir.write("src/a.txt", b"a");
            dir.write("src/b.txt", b"b");
            let first = first_run(&dir);
            dir.write("src/a.txt", b"a changed");

            let snapshot = |id: &str| {
                let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(dir.path("runs").join(id))
                    .unwrap()
                    .map(|entry| entry.unwrap())
                    .map(|entry| {
                        (
                            entry.file_name().to_string_lossy().into_owned(),
                            fs::read(entry.path()).unwrap(),
                        )
                    })
                    .collect();
                files.sort();
                files
            };
            let before = snapshot(&first);
            let dest_before = fs::read(dir.path("dst/a.txt")).unwrap();

            let (summary, _) = delta(&dir, &first);
            let cancel = CancelToken::default();
            cancel.cancel();
            let done =
                run_copy(&dir.path("runs"), &summary.id, None, &cancel, &mut NullSink).unwrap();

            assert_eq!(done.state, RunState::Cancelled);
            assert_eq!(done.totals.copied, 0);
            assert_eq!(
                snapshot(&first),
                before,
                "the previous run's files are byte for byte the same"
            );
            assert_eq!(fs::read(dir.path("dst/a.txt")).unwrap(), dest_before);

            // A cancelled delta run is a finished run too: the next delta continues from it.
            let (next, _) = delta(&dir, &done.id);
            assert_eq!(
                next.totals.retried, 1,
                "the file cancelled before its copy is tried again"
            );
            assert_eq!(manifest_paths(&dir, &next.id), ["a.txt"]);
        }

        #[test]
        fn a_cancel_while_planning_leaves_the_previous_run_untouched() {
            let dir = TempDir::new("delta-cancel-plan");
            dir.write("src/a.txt", b"a");
            let first = first_run(&dir);
            let before = fs::read(dir.path("runs").join(&first).join("run.json")).unwrap();
            let cancel = CancelToken::default();
            cancel.cancel();
            let error =
                prepare_delta(&dir.path("runs"), &first, &cancel, &mut NullSink).unwrap_err();
            assert!(matches!(error, TransferError::Cancelled));
            assert_eq!(
                fs::read(dir.path("runs").join(&first).join("run.json")).unwrap(),
                before
            );
        }

        #[test]
        fn only_a_finished_copy_run_can_be_followed() {
            let dir = TempDir::new("delta-refuse");
            dir.write("src/a.txt", b"a");
            let (prepared, _) = prepare(
                &dir.path("runs"),
                settings(&dir.path("src"), &dir.path("dst")),
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap();
            let error = prepare_delta(
                &dir.path("runs"),
                &prepared.id,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap_err();
            assert!(matches!(error, TransferError::BadRun(_)), "{error}");

            let store = RunStore::open(&dir.path("runs"), &prepared.id).unwrap();
            let mut watched = store.summary().unwrap();
            watched.settings.mode = TransferMode::Watch;
            watched.state = RunState::Completed;
            watched.finished_at_ms = Some(1);
            store.save_summary(&watched).unwrap();
            let error = prepare_delta(
                &dir.path("runs"),
                &prepared.id,
                &CancelToken::default(),
                &mut NullSink,
            )
            .unwrap_err();
            assert!(error.to_string().contains("watched"), "{error}");
            assert!(prepare_delta(
                &dir.path("runs"),
                "nope",
                &CancelToken::default(),
                &mut NullSink
            )
            .is_err());
        }

        #[test]
        fn a_run_saved_before_delta_sync_can_be_followed() {
            let dir = TempDir::new("delta-old");
            dir.write("src/a.txt", b"a");
            let first = first_run(&dir);
            let path = dir.path("runs").join(&first).join("run.json");
            let mut json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            json.as_object_mut().unwrap().remove("previousRunId");
            fs::write(&path, json.to_string()).unwrap();

            let (summary, _) = delta(&dir, &first);
            assert_eq!(summary.totals.unchanged, 1);
            assert_eq!(summary.previous_run_id.as_deref(), Some(first.as_str()));
        }
    }
}

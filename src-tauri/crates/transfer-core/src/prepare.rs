//! Step 1 of a run: list the source into `manifest.jsonl` and check, before anything is copied,
//! what would go wrong (space, names, long paths, write access), so it can be fixed first.

use crate::copy::times_match;
use crate::reason::{classify, FailureReason, Side};
use crate::store::{LogStatus, PathKey, RunStore};
use crate::volume::volume_info;
use crate::walk::{self, Entry};
use crate::{
    machine_name, now_ms, user_name, CancelToken, ConflictPolicy, ItemKind, ItemResult, Phase,
    Result, RunState, RunSummary, RunTotals, TransferError, TransferMode, TransferProgress,
    TransferSettings, TransferSink,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
        only,
        cancel,
        sink,
        &mut report,
    );
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
    /// Fingerprints of the lowercased paths, to spot names that differ only by case.
    seen: HashSet<PathKey>,
}

#[allow(clippy::too_many_arguments)]
fn scan_source(
    store: &RunStore,
    settings: &TransferSettings,
    source: &Path,
    destination: &Path,
    only: Option<&OnlyPaths>,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
    report: &mut PreflightReport,
) -> Result<()> {
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
    results.finish()
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
                    self.count_space(&target, size, modified_at_ms, report);
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
        if !self.seen.insert(PathKey::of(&relative_path.to_lowercase())) {
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
    #[cfg(windows)]
    use crate::ItemStatus;
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
}

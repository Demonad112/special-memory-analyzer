//! Step 2: copy the files of a prepared run, and later retry what was not copied.

use crate::copy::{copy_file_with_retry, ConflictPolicy, CopyOptions, CopyOutcome};
use crate::prepare::{destination_path, ManifestEntry};
use crate::reason::{classify, FailureReason, Side};
use crate::store::{JsonlWriter, LogStatus, RunStore};
use crate::walk::{self, Entry};
use crate::{
    now_ms, CancelToken, ItemKind, ItemResult, ItemStatus, Phase, Result, RunState, RunSummary,
    TransferError, TransferProgress, TransferSettings, TransferSink,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Which not-copied rows an action applies to. No paths and no reason means all of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub reason: Option<FailureReason>,
}

impl Selection {
    pub fn matches(&self, item: &ItemResult) -> bool {
        item.status == ItemStatus::NotCopied
            && (self.paths.is_empty() || self.paths.contains(&item.relative_path))
            && self.reason.is_none_or(|reason| item.reason == Some(reason))
    }
}

/// Copies every file of a prepared run. Cancelling marks the files not yet copied as
/// Cancelled, so "Retry" on that group continues the run.
pub fn run_copy(
    root: &Path,
    run_id: &str,
    retry_delays: Option<Vec<std::time::Duration>>,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
) -> Result<RunSummary> {
    let store = RunStore::open(root, run_id)?;
    let mut summary = store.summary()?;
    if summary.state != RunState::Prepared {
        return Err(TransferError::BadRun(
            "This run has already been started. Retry its files, or prepare a new run.".to_owned(),
        ));
    }
    summary.state = RunState::Running;
    summary.started_at_ms = Some(now_ms());
    store.save_summary(&summary)?;
    store.audit("start", LogStatus::Started, "Copy started", &[]);

    let settings = summary.settings.clone();
    let mut options = copy_options(&settings);
    if let Some(delays) = retry_delays {
        options.retry_delays = delays;
    }
    let mut copier = Copier::new(&store, &settings, options, cancel, sink)?;
    copier.progress.files_total = summary.totals.planned_files;
    copier.progress.bytes_total = summary.totals.planned_bytes;
    copier.progress.not_copied = summary.totals.not_copied;

    let outcome = (|| -> Result<()> {
        for row in store.manifest::<ManifestEntry>()? {
            let row = row?;
            if row.blocked.is_some() {
                // Already listed as not copied by pre-flight.
                if row.kind == ItemKind::File {
                    copier.progress.files_done += 1;
                    copier.progress.bytes_done += row.size;
                }
                continue;
            }
            match row.kind {
                ItemKind::Folder => copier.make_folder(&row.relative_path)?,
                ItemKind::File => copier.copy(&row.relative_path, row.size, None)?,
            }
        }
        Ok(())
    })();
    copier.finish()?;
    finish(&store, &mut summary, cancel, outcome, "Copy")
}

/// Copies the selected not-copied files again. A folder that could not be read is listed again
/// and its contents copied. Conflicts, mismatches and incomplete files are overwritten, since
/// the user asked for them explicitly.
pub fn retry(
    root: &Path,
    run_id: &str,
    selection: &Selection,
    cancel: &CancelToken,
    sink: &mut dyn TransferSink,
) -> Result<RunSummary> {
    let store = RunStore::open(root, run_id)?;
    let mut summary = store.summary()?;
    let selected: Vec<ItemResult> = store
        .latest_results()?
        .into_values()
        .filter(|item| selection.matches(item))
        .filter(|item| {
            // Online-only files stay put: a new record with "Download" on copies them.
            !matches!(
                item.reason,
                Some(
                    FailureReason::InvalidName
                        | FailureReason::NameCollision
                        | FailureReason::CloudOnly
                )
            )
        })
        .collect();
    let previous = summary.state;
    summary.state = RunState::Running;
    store.save_summary(&summary)?;
    store.audit(
        "retry",
        LogStatus::Started,
        "Retry started",
        &[("items", (selected.len() as u64).into())],
    );

    let settings = summary.settings.clone();
    let mut copier = Copier::new(&store, &settings, copy_options(&settings), cancel, sink)?;
    copier.progress.files_total = selected.len() as u64;
    copier.progress.bytes_total = selected.iter().map(|item| item.size).sum();

    let outcome = (|| -> Result<()> {
        for item in &selected {
            match item.kind {
                ItemKind::Folder => copier.copy_folder_tree(&item.relative_path)?,
                ItemKind::File => {
                    let overwrite = matches!(
                        item.reason,
                        Some(
                            FailureReason::TargetExistsDifferent
                                | FailureReason::VerifyMismatch
                                | FailureReason::Incomplete
                        )
                    );
                    copier.copy(
                        &item.relative_path,
                        item.size,
                        overwrite.then_some(ConflictPolicy::Overwrite),
                    )?;
                }
            }
        }
        Ok(())
    })();
    copier.finish()?;
    if previous == RunState::Prepared && outcome.is_ok() {
        // A retry before the copy only covers the pre-flight rows; the run still needs a start.
        summary.state = RunState::Prepared;
        store.refresh_totals(&mut summary)?;
        store.save_summary(&summary)?;
        return Ok(summary);
    }
    finish(&store, &mut summary, cancel, outcome, "Retry")
}

pub(crate) fn copy_options(settings: &TransferSettings) -> CopyOptions {
    CopyOptions {
        verify: settings.verify,
        conflict: settings.conflict,
        preserve: settings.preserve,
        ..CopyOptions::default()
    }
}

fn finish(
    store: &RunStore,
    summary: &mut RunSummary,
    cancel: &CancelToken,
    outcome: Result<()>,
    what: &str,
) -> Result<RunSummary> {
    summary.finished_at_ms = Some(now_ms());
    let (state, status, message) = match &outcome {
        Ok(()) if cancel.is_cancelled() => (
            RunState::Cancelled,
            LogStatus::Cancelled,
            format!("{what} cancelled"),
        ),
        Ok(()) => (
            RunState::Completed,
            LogStatus::Succeeded,
            format!("{what} finished"),
        ),
        Err(error) => (
            RunState::Failed,
            LogStatus::Failed,
            format!("{what} stopped: {error}"),
        ),
    };
    summary.state = state;
    if let Err(error) = &outcome {
        summary.error = Some(error.to_string());
    }
    store.refresh_totals(summary)?;
    store.save_summary(summary)?;
    store.audit(
        "finish",
        status,
        message,
        &[
            ("copied", summary.totals.copied.into()),
            ("notCopied", summary.totals.not_copied.into()),
        ],
    );
    outcome.map(|()| summary.clone())
}

/// Copies files one at a time, writing a result row for each and reporting progress.
pub(crate) struct Copier<'a> {
    source: PathBuf,
    destination: PathBuf,
    ignore_junk: bool,
    download_cloud: bool,
    options: CopyOptions,
    cancel: &'a CancelToken,
    sink: &'a mut dyn TransferSink,
    results: JsonlWriter,
    /// Folders this run created. Their dates and attributes are set last, deepest first, because
    /// writing files into a folder changes its modified time.
    made_folders: Vec<String>,
    pub(crate) progress: TransferProgress,
}

impl<'a> Copier<'a> {
    pub(crate) fn new(
        store: &RunStore,
        settings: &TransferSettings,
        options: CopyOptions,
        cancel: &'a CancelToken,
        sink: &'a mut dyn TransferSink,
    ) -> Result<Self> {
        Ok(Self {
            source: PathBuf::from(&settings.source),
            destination: PathBuf::from(&settings.destination),
            ignore_junk: settings.ignore_junk,
            download_cloud: settings.download_cloud,
            options,
            cancel,
            sink,
            results: store.results_writer()?,
            made_folders: Vec::new(),
            progress: TransferProgress {
                run_id: store.id(),
                phase: Phase::Copying,
                files_done: 0,
                files_total: 0,
                bytes_done: 0,
                bytes_total: 0,
                copied: 0,
                skipped: 0,
                not_copied: 0,
                current: None,
            },
        })
    }

    fn record(&mut self, item: ItemResult) -> Result<()> {
        match item.status {
            ItemStatus::Copied | ItemStatus::Arrived => self.progress.copied += 1,
            ItemStatus::SkippedIdentical => self.progress.skipped += 1,
            ItemStatus::NotCopied => self.progress.not_copied += 1,
        }
        self.sink.item(&item);
        self.results.write(&item)
    }

    fn make_folder(&mut self, relative_path: &str) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Ok(());
        }
        let target = destination_path(&self.destination, relative_path);
        let existed = target.is_dir();
        let created = fs::create_dir_all(&target);
        if created.is_ok() && !existed {
            self.made_folders.push(relative_path.to_owned());
        }
        if let Err(error) = created {
            let mut item = ItemResult::not_copied(
                relative_path,
                ItemKind::Folder,
                0,
                classify(&error, Side::Destination),
                Some(Side::Destination),
                error.to_string(),
            );
            item.os_code = error.raw_os_error();
            self.record(item)?;
        }
        Ok(())
    }

    pub(crate) fn copy(
        &mut self,
        relative_path: &str,
        size: u64,
        conflict: Option<ConflictPolicy>,
    ) -> Result<()> {
        if self.cancel.is_cancelled() {
            let item = ItemResult::not_copied(
                relative_path,
                ItemKind::File,
                size,
                FailureReason::Cancelled,
                None,
                FailureReason::Cancelled.explanation(),
            );
            self.progress.files_done += 1;
            return self.record(item);
        }
        self.progress.current = Some(relative_path.to_owned());
        self.sink.progress(&self.progress);

        let mut options = self.options.clone();
        if let Some(conflict) = conflict {
            options.conflict = conflict;
        }
        let source = destination_path(&self.source, relative_path);
        let target = destination_path(&self.destination, relative_path);
        let start_bytes = self.progress.bytes_done;
        let attempt = {
            let progress = &mut self.progress;
            let sink = &mut *self.sink;
            let mut last_emit = std::time::Instant::now();
            copy_file_with_retry(&source, &target, &options, self.cancel, &mut |bytes| {
                progress.bytes_done += bytes;
                if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
                    last_emit = std::time::Instant::now();
                    sink.progress(progress);
                }
            })
        };
        // A retried copy re-counts its bytes; keep the total at one pass per file.
        self.progress.bytes_done = start_bytes + size;
        self.progress.files_done += 1;

        let item = match attempt.result {
            Ok(CopyOutcome::Copied {
                bytes,
                hash,
                warnings,
            }) => ItemResult {
                source_hash: hash.clone(),
                dest_hash: hash,
                attempts: attempt.attempts,
                message: (!warnings.is_empty()).then(|| warnings.join(" ")),
                ..ItemResult::done(relative_path, bytes, ItemStatus::Copied)
            },
            Ok(CopyOutcome::SkippedIdentical) => {
                ItemResult::done(relative_path, size, ItemStatus::SkippedIdentical)
            }
            Err(failure) => {
                let mut item = ItemResult::not_copied(
                    relative_path,
                    ItemKind::File,
                    size,
                    failure.reason,
                    failure.side,
                    failure.message,
                );
                item.os_code = failure.os_code;
                item.dest_hash = failure.dest_hash;
                item.attempts = attempt.attempts;
                item
            }
        };
        self.record(item)
    }

    /// Lists a folder that could not be read before, and copies what is in it now.
    fn copy_folder_tree(&mut self, relative_folder: &str) -> Result<()> {
        let root = destination_path(&self.source, relative_folder);
        let prefix = |relative: &str| format!("{relative_folder}/{relative}");
        let mut entries = Vec::new();
        let skip_cloud = !self.download_cloud;
        let walked =
            walk::scan_tree(&root, self.ignore_junk, self.cancel, &mut |_| {}).and_then(|tree| {
                walk::visit(&tree, &mut |entry| {
                    match entry {
                        Entry::Folder { relative_path } => {
                            entries.push(Ok((prefix(&relative_path), ItemKind::Folder, 0, false)));
                        }
                        Entry::File {
                            relative_path,
                            size,
                            cloud,
                            ..
                        } => entries.push(Ok((
                            prefix(&relative_path),
                            ItemKind::File,
                            size,
                            cloud && skip_cloud,
                        ))),
                        Entry::Link => {}
                        Entry::Unreadable {
                            relative_path,
                            error,
                        } => entries.push(Err((prefix(&relative_path), error))),
                    }
                    Ok(true)
                })
            });
        if let Err(error) = walked {
            let message = match error {
                TransferError::Cancelled => return Ok(()),
                other => other.to_string(),
            };
            let reason = if root.exists() {
                FailureReason::AccessDenied
            } else {
                FailureReason::SourceVanished
            };
            let item = ItemResult::not_copied(
                relative_folder,
                ItemKind::Folder,
                0,
                reason,
                Some(Side::Source),
                message,
            );
            return self.record(item);
        }

        self.make_folder(relative_folder)?;
        self.progress.files_total += entries.len() as u64;
        for entry in entries {
            match entry {
                Ok((path, ItemKind::Folder, _, _)) => self.make_folder(&path)?,
                Ok((path, ItemKind::File, size, true)) => {
                    let reason = FailureReason::CloudOnly;
                    self.progress.files_done += 1;
                    self.record(ItemResult::not_copied(
                        path,
                        ItemKind::File,
                        size,
                        reason,
                        Some(Side::Source),
                        reason.explanation(),
                    ))?;
                }
                Ok((path, ItemKind::File, size, false)) => {
                    self.progress.bytes_total += size;
                    self.copy(&path, size, None)?;
                }
                Err((path, error)) => {
                    let mut item = ItemResult::not_copied(
                        path,
                        ItemKind::Folder,
                        0,
                        classify(&error, Side::Source),
                        Some(Side::Source),
                        error.to_string(),
                    );
                    item.os_code = error.raw_os_error();
                    self.record(item)?;
                }
            }
        }
        // The folder itself is readable now: replace its not-copied row.
        let mut done = ItemResult::done(relative_folder, 0, ItemStatus::Copied);
        done.kind = ItemKind::Folder;
        self.record(done)
    }

    /// Best effort: a folder whose dates can't be set is still a copied folder.
    fn keep_folder_metadata(&mut self) {
        let mut folders = std::mem::take(&mut self.made_folders);
        folders.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
        for relative in folders {
            let source = destination_path(&self.source, &relative);
            if let Ok(wanted) = crate::meta::read(&source) {
                let target = destination_path(&self.destination, &relative);
                let _ = crate::meta::apply(&source, &target, &wanted, &self.options.preserve);
            }
        }
    }

    pub(crate) fn finish(mut self) -> Result<()> {
        self.keep_folder_metadata();
        self.progress.phase = Phase::Done;
        self.progress.current = None;
        self.sink.progress(&self.progress);
        self.results.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::prepare;
    use crate::test_support::TempDir;
    use crate::{ConflictPolicy, NullSink, TransferMode, VerifyLevel};

    fn settings(dir: &TempDir) -> TransferSettings {
        TransferSettings {
            source: dir.path("src").display().to_string(),
            destination: dir.path("dst").display().to_string(),
            mode: TransferMode::Copy,
            verify: VerifyLevel::Hash,
            conflict: ConflictPolicy::Skip,
            ignore_junk: false,
            download_cloud: false,
            preserve: Default::default(),
        }
    }

    fn prepared(dir: &TempDir) -> String {
        prepare(
            &dir.path("runs"),
            settings(dir),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap()
        .0
        .id
    }

    struct CancelAfter {
        items: usize,
        token: CancelToken,
    }

    impl TransferSink for CancelAfter {
        fn progress(&mut self, _progress: &TransferProgress) {}
        fn item(&mut self, _item: &ItemResult) {
            if self.items > 0 {
                self.items -= 1;
                if self.items == 0 {
                    self.token.cancel();
                }
            }
        }
    }

    #[test]
    fn copies_a_tree_with_empty_folders_and_hashes() {
        let dir = TempDir::new("run-copy");
        dir.write("src/a.txt", b"alpha");
        dir.write("src/sub/deeper/b.txt", b"beta");
        fs::create_dir_all(dir.path("src/empty")).unwrap();
        let id = prepared(&dir);

        let summary = run_copy(
            &dir.path("runs"),
            &id,
            None,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!(summary.state, RunState::Completed);
        assert_eq!(
            (
                summary.totals.copied,
                summary.totals.copied_bytes,
                summary.totals.not_copied
            ),
            (2, 9, 0)
        );
        assert_eq!(fs::read(dir.path("dst/sub/deeper/b.txt")).unwrap(), b"beta");
        assert!(dir.path("dst/empty").is_dir());
        let latest = RunStore::open(&dir.path("runs"), &id)
            .unwrap()
            .latest_results()
            .unwrap();
        assert_eq!(
            latest["a.txt"].source_hash.as_deref(),
            Some(blake3::hash(b"alpha").to_hex().as_str())
        );
        assert!(run_copy(
            &dir.path("runs"),
            &id,
            None,
            &CancelToken::default(),
            &mut NullSink
        )
        .is_err());
    }

    #[test]
    fn cancelling_marks_the_rest_cancelled_and_retry_finishes_the_job() {
        let dir = TempDir::new("run-cancel");
        for name in ["a", "b", "c", "d"] {
            dir.write(&format!("src/{name}.txt"), name.as_bytes());
        }
        let id = prepared(&dir);
        let token = CancelToken::default();
        let mut sink = CancelAfter {
            items: 1,
            token: token.clone(),
        };

        let summary = run_copy(&dir.path("runs"), &id, None, &token, &mut sink).unwrap();

        assert_eq!(summary.state, RunState::Cancelled);
        assert_eq!((summary.totals.copied, summary.totals.not_copied), (1, 3));
        let leftovers: Vec<_> = fs::read_dir(dir.path("dst"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(crate::copy::PART_SUFFIX)
            })
            .collect();
        assert!(leftovers.is_empty());

        let selection = Selection {
            paths: Vec::new(),
            reason: Some(FailureReason::Cancelled),
        };
        let summary = retry(
            &dir.path("runs"),
            &id,
            &selection,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();
        assert_eq!(summary.state, RunState::Completed);
        assert_eq!((summary.totals.copied, summary.totals.not_copied), (4, 0));
    }

    #[test]
    fn retrying_a_conflict_overwrites_the_destination() {
        let dir = TempDir::new("run-conflict");
        dir.write("src/a.txt", b"from source");
        dir.write("dst/a.txt", b"old");
        let id = prepared(&dir);
        let summary = run_copy(
            &dir.path("runs"),
            &id,
            None,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();
        assert_eq!(summary.totals.not_copied, 1);

        let selection = Selection {
            paths: vec!["a.txt".to_owned()],
            reason: None,
        };
        let summary = retry(
            &dir.path("runs"),
            &id,
            &selection,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!(summary.totals.not_copied, 0);
        assert_eq!(fs::read(dir.path("dst/a.txt")).unwrap(), b"from source");
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_file_is_listed_and_retry_copies_it_once_released() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = TempDir::new("run-locked");
        dir.write("src/free.txt", b"free");
        let locked = dir.write("src/locked.txt", b"locked");
        let id = prepared(&dir);
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();

        let summary = run_copy(
            &dir.path("runs"),
            &id,
            Some(Vec::new()),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();
        let latest = RunStore::open(&dir.path("runs"), &id)
            .unwrap()
            .latest_results()
            .unwrap();
        assert_eq!(summary.totals.not_copied, 1);
        assert_eq!(latest["locked.txt"].reason, Some(FailureReason::FileLocked));
        // The recovery copy would have to read the same locked source.
        assert!(!latest["locked.txt"].recoverable);

        drop(lock);
        let selection = Selection {
            paths: Vec::new(),
            reason: Some(FailureReason::FileLocked),
        };
        let summary = retry(
            &dir.path("runs"),
            &id,
            &selection,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();
        assert_eq!(summary.totals.not_copied, 0);
        assert_eq!(fs::read(dir.path("dst/locked.txt")).unwrap(), b"locked");
    }

    #[cfg(windows)]
    #[test]
    fn an_unreadable_folder_is_copied_by_retry_once_readable() {
        let dir = TempDir::new("run-folder-retry");
        dir.write("src/private/inner/secret.txt", b"secret");
        let guard = crate::test_support::DenyGuard::new(&dir.path("src/private"), "(RD)");
        let id = prepared(&dir);
        let summary = run_copy(
            &dir.path("runs"),
            &id,
            None,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();
        assert_eq!(summary.totals.not_copied, 1);

        drop(guard);
        let summary = retry(
            &dir.path("runs"),
            &id,
            &Selection::default(),
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap();

        assert_eq!((summary.totals.copied, summary.totals.not_copied), (1, 0));
        assert_eq!(
            fs::read(dir.path("dst/private/inner/secret.txt")).unwrap(),
            b"secret"
        );
    }
}

//! "Copy to recovery folder": puts not-copied files somewhere else (by default beside the
//! destination) with their folder structure, plus a `NOT-COPIED.csv` listing them.

use crate::copy::{copy_file, ConflictPolicy, CopyOptions, CopyOutcome};
use crate::prepare::{destination_path, safe_relative};
use crate::reason::{FailureReason, Side};
use crate::run::Selection;
use crate::store::{LogStatus, RunStore};
use crate::volume::volume_info;
use crate::{CancelToken, ItemKind, ItemResult, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const LIST_FILE_NAME: &str = "NOT-COPIED.csv";

/// Whether a not-copied row can be copied to a recovery folder: only files, and only when
/// the source itself was fine (the problem was at the destination, or transient).
pub fn recoverable(kind: ItemKind, reason: FailureReason, side: Option<Side>) -> bool {
    kind == ItemKind::File
        && side != Some(Side::Source)
        && !matches!(
            reason,
            FailureReason::SourceVanished
                | FailureReason::InvalidName
                | FailureReason::NameCollision
        )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryResult {
    pub folder: String,
    pub list_file: String,
    pub copied: u64,
    /// Selected rows that could not be put in the recovery folder, with why.
    pub skipped: Vec<RecoverySkip>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySkip {
    pub relative_path: String,
    pub message: String,
}

/// `<destination>_NotCopied\<run id>`, or `<chosen>\<run id>`.
pub fn recovery_folder(destination: &Path, chosen: Option<&Path>, run_id: &str) -> PathBuf {
    let base = match chosen {
        Some(chosen) => chosen.to_path_buf(),
        None => match destination.file_name() {
            Some(name) => {
                let mut name = name.to_os_string();
                name.push("_NotCopied");
                destination.with_file_name(name)
            }
            // A drive root such as E:\ has no name.
            None => destination.join("Analyzer_NotCopied"),
        },
    };
    base.join(run_id)
}

pub fn copy_to_recovery(
    root: &Path,
    run_id: &str,
    selection: &Selection,
    chosen: Option<&Path>,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(usize, usize),
) -> Result<RecoveryResult> {
    let store = RunStore::open(root, run_id)?;
    let summary = store.summary()?;
    let source = PathBuf::from(&summary.settings.source);
    let destination = PathBuf::from(&summary.settings.destination);
    let folder = recovery_folder(&destination, chosen, run_id);
    fs::create_dir_all(&folder)?;
    let same_volume = volume(&folder) == volume(&destination);

    let selected: Vec<ItemResult> = store
        .latest_results()?
        .into_values()
        .filter(|item| selection.matches(item))
        .collect();
    let options = CopyOptions {
        conflict: ConflictPolicy::Overwrite,
        retry_delays: Vec::new(),
        ..CopyOptions::default()
    };
    let mut result = RecoveryResult {
        folder: folder.display().to_string(),
        list_file: folder.join(LIST_FILE_NAME).display().to_string(),
        copied: 0,
        skipped: Vec::new(),
    };
    let mut recovered = Vec::with_capacity(selected.len());
    on_progress(0, selected.len());
    for (done, item) in selected.iter().enumerate() {
        let skip = if !item.recoverable {
            Some("The source could not be read, so there is nothing to copy.".to_owned())
        } else if item.reason == Some(FailureReason::DiskFull) && same_volume {
            Some("The recovery folder is on the same full disk as the destination.".to_owned())
        } else if !safe_relative(&item.relative_path) {
            Some(FailureReason::InvalidName.explanation().to_owned())
        } else if cancel.is_cancelled() {
            Some("Cancelled".to_owned())
        } else {
            let copied = copy_file(
                &destination_path(&source, &item.relative_path),
                &destination_path(&folder, &item.relative_path),
                &options,
                cancel,
                &mut |_| {},
            );
            match copied {
                Ok(CopyOutcome::Copied { .. } | CopyOutcome::SkippedIdentical) => None,
                Err(failure) => Some(failure.message),
            }
        };
        match skip {
            None => {
                result.copied += 1;
                recovered.push(true);
            }
            Some(message) => {
                result.skipped.push(RecoverySkip {
                    relative_path: item.relative_path.clone(),
                    message,
                });
                recovered.push(false);
            }
        }
        on_progress(done + 1, selected.len());
    }

    fs::write(
        folder.join(LIST_FILE_NAME),
        list_csv(&source, &selected, &recovered),
    )?;
    store.audit(
        "recovery",
        LogStatus::Succeeded,
        "Not-copied files copied to a recovery folder",
        &[
            ("folder", result.folder.clone().into()),
            ("copied", result.copied.into()),
            ("skipped", (result.skipped.len() as u64).into()),
        ],
    );
    Ok(result)
}

fn volume(path: &Path) -> Option<String> {
    volume_info(path).map(|info| info.root.to_lowercase())
}

fn list_csv(source: &Path, items: &[ItemResult], recovered: &[bool]) -> String {
    let quote = |value: &str| {
        if value.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_owned()
        }
    };
    // BOM so Excel reads non-ASCII names correctly.
    let mut csv = String::from("\u{feff}path,reason,os_message,size,source,in_recovery_folder\r\n");
    for (item, recovered) in items.iter().zip(recovered) {
        let reason = item.reason.map(FailureReason::title).unwrap_or_default();
        csv.push_str(&format!(
            "{},{},{},{},{},{}\r\n",
            quote(&item.relative_path),
            quote(reason),
            quote(item.message.as_deref().unwrap_or("")),
            item.size,
            quote(
                &destination_path(source, &item.relative_path)
                    .display()
                    .to_string()
            ),
            if *recovered { "yes" } else { "no" }
        ));
    }
    csv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::prepare;
    use crate::run::run_copy;
    use crate::test_support::TempDir;
    use crate::{ConflictPolicy, NullSink, TransferMode, TransferSettings, VerifyLevel};

    #[test]
    fn recovery_folder_sits_beside_the_destination() {
        let client = Path::new("Client");
        assert_eq!(
            recovery_folder(&client.join("Data"), None, "r1"),
            client.join("Data_NotCopied").join("r1")
        );
        assert_eq!(
            recovery_folder(&client.join("Data"), Some(Path::new("Rescue")), "r1"),
            Path::new("Rescue").join("r1")
        );
    }

    #[cfg(windows)]
    #[test]
    fn recovery_folder_on_a_drive_root_gets_a_named_folder() {
        assert_eq!(
            recovery_folder(Path::new(r"E:\Client\Data"), None, "r1"),
            PathBuf::from(r"E:\Client\Data_NotCopied\r1")
        );
        assert_eq!(
            recovery_folder(Path::new(r"E:\"), None, "r1"),
            PathBuf::from(r"E:\Analyzer_NotCopied\r1")
        );
    }

    #[test]
    fn only_files_with_a_readable_source_are_recoverable() {
        assert!(recoverable(
            ItemKind::File,
            FailureReason::DiskFull,
            Some(Side::Destination)
        ));
        assert!(recoverable(ItemKind::File, FailureReason::Cancelled, None));
        assert!(!recoverable(
            ItemKind::File,
            FailureReason::FileLocked,
            Some(Side::Source)
        ));
        assert!(!recoverable(
            ItemKind::File,
            FailureReason::SourceVanished,
            Some(Side::Source)
        ));
        assert!(!recoverable(
            ItemKind::Folder,
            FailureReason::AccessDenied,
            Some(Side::Destination)
        ));
    }

    #[test]
    fn copies_conflicting_files_to_the_recovery_folder_with_a_list() {
        let dir = TempDir::new("recovery");
        dir.write("src/docs/report, final.txt", b"new version");
        dir.write("dst/docs/report, final.txt", b"someone else's");
        let settings = TransferSettings {
            source: dir.path("src").display().to_string(),
            destination: dir.path("dst").display().to_string(),
            mode: TransferMode::Copy,
            verify: VerifyLevel::SizeAndTime,
            conflict: ConflictPolicy::Skip,
            ignore_junk: false,
            download_cloud: false,
            preserve: Default::default(),
        };
        let token = CancelToken::default();
        let (summary, _) = prepare(&dir.path("runs"), settings, &token, &mut NullSink).unwrap();
        run_copy(&dir.path("runs"), &summary.id, None, &token, &mut NullSink).unwrap();

        let mut progress = Vec::new();
        let result = copy_to_recovery(
            &dir.path("runs"),
            &summary.id,
            &Selection::default(),
            None,
            &token,
            &mut |done, total| progress.push((done, total)),
        )
        .unwrap();

        assert_eq!(result.copied, 1);
        assert_eq!(progress, vec![(0, 1), (1, 1)]);
        let folder = dir.path("dst_NotCopied").join(&summary.id);
        assert_eq!(
            fs::read(folder.join("docs/report, final.txt")).unwrap(),
            b"new version"
        );
        let list = fs::read_to_string(folder.join(LIST_FILE_NAME)).unwrap();
        assert!(list.contains(
            "\"docs/report, final.txt\",A different file is already at the destination,"
        ));
        assert!(list.trim_end().ends_with(",yes"));
        // The destination is left alone.
        assert_eq!(
            fs::read(dir.path("dst/docs/report, final.txt")).unwrap(),
            b"someone else's"
        );
    }
}

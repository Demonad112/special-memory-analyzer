//! Delta sync: which files a second pass over the same source and destination copies.
//!
//! The decision uses metadata only (size and modified time), never file content, so it costs
//! what a normal pre-flight costs and never opens a OneDrive online-only file. Nothing here (or
//! anywhere in a delta run) removes anything at the destination.

use crate::copy::TIME_TOLERANCE;
use crate::store::RunStore;
use crate::RunSummary;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// What the destination holds at a source file's path, read without following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Missing,
    File {
        size: u64,
        modified_ms: Option<u64>,
    },
    /// A folder, symlink or junction where the source has a file. Never replaced.
    Other,
}

impl Dest {
    /// Reads the destination path's own metadata. A link is never followed, and the file is not
    /// opened, so an online-only placeholder or a deduplicated file is judged by its listed size
    /// and time like any other.
    pub fn read(path: &Path) -> Self {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return Dest::Missing;
        };
        if !meta.is_file() {
            return Dest::Other;
        }
        Dest::File {
            size: meta.len(),
            modified_ms: meta.modified().ok().and_then(to_ms),
        }
    }
}

fn to_ms(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_millis() as u64)
}

/// A source file as the manifest lists it.
#[derive(Debug, Clone, Copy)]
pub struct Source {
    pub size: u64,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Same size and date at the destination: left alone.
    Unchanged,
    /// Not at the destination and not tried before.
    New,
    /// At the destination with a different size or date: replaced.
    Changed,
    /// The previous run listed it as not copied: tried again.
    Retried,
    /// Edited at the destination after the previous run: left alone and listed.
    ChangedAtDestination,
}

/// A file a delta run left alone because it was edited at the destination. The next delta run
/// reads these back, so a file left once stays left until someone changes it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftBehind {
    pub relative_path: String,
    /// The source file at the time.
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub source_modified_ms: Option<u64>,
    /// The destination file that was left: its size and modified time identify it.
    #[serde(default)]
    pub dest_size: u64,
    #[serde(default)]
    pub dest_modified_ms: Option<u64>,
}

/// The run a delta run follows, as the report shows it ("Changes since run of 21 Oct, 18:02").
/// The times are missing when that run's folder has been removed since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousRun {
    pub id: String,
    pub created_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
}

/// The run `summary` follows, or `None` for an ordinary run (and for every run saved before
/// delta sync). `store` is any run folder under the same records root.
pub fn previous_run(store: &RunStore, summary: &RunSummary) -> Option<PreviousRun> {
    let id = summary.previous_run_id.clone()?;
    let before = store
        .dir()
        .parent()
        .and_then(|root| RunStore::open(root, &id).ok())
        .and_then(|previous| previous.summary().ok());
    Some(PreviousRun {
        created_at_ms: before.as_ref().map(|run| run.created_at_ms),
        finished_at_ms: before.and_then(|run| run.finished_at_ms),
        id,
    })
}

/// The files a delta run left alone at the destination, at most `cap` of them; the run's
/// `changed_at_destination` total counts all of them.
pub fn left_behind(store: &RunStore, cap: usize) -> Vec<LeftBehind> {
    store
        .delta_left::<LeftBehind>()
        .ok()
        .flatten()
        .map(|rows| rows.map_while(|row| row.ok()).take(cap).collect())
        .unwrap_or_default()
}

fn tolerance_ms() -> u64 {
    TIME_TOLERANCE.as_millis() as u64
}

/// Whether two times differ by more than the tolerance; an unknown time never counts.
pub fn times_differ(left: Option<u64>, right: Option<u64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.abs_diff(right) > tolerance_ms(),
        _ => false,
    }
}

/// Decides what a delta run does with one source file (decision D-10).
///
/// - `retry`: the previous run listed this path as not copied.
/// - `previous_finished_ms`: when the previous run finished.
/// - `left_before`: what the previous run recorded as left alone at this path, if anything.
pub fn classify(
    source: Source,
    dest: Dest,
    retry: bool,
    previous_finished_ms: u64,
    left_before: Option<&LeftBehind>,
) -> Class {
    let (dest_size, dest_ms) = match dest {
        Dest::Missing => return if retry { Class::Retried } else { Class::New },
        Dest::Other => return Class::ChangedAtDestination,
        Dest::File { size, modified_ms } => (size, modified_ms),
    };
    // Left alone last time and untouched since: still not ours to overwrite.
    if left_before.is_some_and(|left| {
        left.dest_size == dest_size && !times_differ(left.dest_modified_ms, dest_ms)
    }) {
        return Class::ChangedAtDestination;
    }
    // Newer than both the source and the end of the previous run: someone worked on it there.
    let after_previous_run =
        dest_ms.is_some_and(|dest| dest > previous_finished_ms + tolerance_ms());
    let newer_than_source = match (dest_ms, source.modified_ms) {
        (Some(dest), Some(source)) => dest > source + tolerance_ms(),
        (Some(_), None) => true,
        _ => false,
    };
    if after_previous_run && newer_than_source {
        return Class::ChangedAtDestination;
    }
    if retry {
        return Class::Retried;
    }
    if dest_size != source.size || times_differ(source.modified_ms, dest_ms) {
        Class::Changed
    } else {
        Class::Unchanged
    }
}

/// Whether the destination file was modified after `since_ms` (a run's start) and is not the
/// source's own copy. Checked again just before a replacement, since planning can be hours old.
pub fn edited_since(dest: Dest, source: Source, since_ms: u64) -> bool {
    match dest {
        Dest::Missing => false,
        Dest::Other => true,
        Dest::File { size, modified_ms } => {
            let same_as_source =
                size == source.size && !times_differ(source.modified_ms, modified_ms);
            !same_as_source && modified_ms.is_some_and(|dest| dest > since_ms + tolerance_ms())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINISHED: u64 = 1_000_000_000;
    const SOURCE: Source = Source {
        size: 10,
        modified_ms: Some(FINISHED - 50_000),
    };

    fn file(size: u64, modified_ms: u64) -> Dest {
        Dest::File {
            size,
            modified_ms: Some(modified_ms),
        }
    }

    fn pick(dest: Dest, retry: bool) -> Class {
        classify(SOURCE, dest, retry, FINISHED, None)
    }

    #[test]
    fn a_file_with_the_same_size_and_date_is_unchanged() {
        let same = SOURCE.modified_ms.unwrap();
        assert_eq!(pick(file(10, same), false), Class::Unchanged);
        // Inside the 2 s tolerance, either way (FAT stores 2 s steps).
        assert_eq!(pick(file(10, same + 2_000), false), Class::Unchanged);
        assert_eq!(pick(file(10, same - 2_000), false), Class::Unchanged);
    }

    #[test]
    fn a_missing_file_is_new_or_retried() {
        assert_eq!(pick(Dest::Missing, false), Class::New);
        assert_eq!(pick(Dest::Missing, true), Class::Retried);
    }

    #[test]
    fn a_different_size_is_changed() {
        let same = SOURCE.modified_ms.unwrap();
        assert_eq!(pick(file(11, same), false), Class::Changed);
        assert_eq!(pick(file(0, same), false), Class::Changed);
    }

    #[test]
    fn a_different_date_is_changed_whether_the_destination_is_older_or_newer() {
        let same = SOURCE.modified_ms.unwrap();
        assert_eq!(pick(file(10, same - 2_001), false), Class::Changed);
        // A source restored with an older date: the destination is newer than the source but was
        // written before the previous run ended, so it is the old copy.
        assert_eq!(pick(file(10, same + 2_001), false), Class::Changed);
    }

    #[test]
    fn a_file_the_previous_run_did_not_copy_is_retried_even_when_it_looks_identical() {
        let same = SOURCE.modified_ms.unwrap();
        // A bad copy keeps the source's size and date, so it would look identical.
        assert_eq!(pick(file(10, same), true), Class::Retried);
        assert_eq!(pick(file(3, same - 9_000), true), Class::Retried);
    }

    #[test]
    fn a_destination_file_edited_after_the_previous_run_is_left_alone() {
        let edited = FINISHED + 60_000;
        assert_eq!(pick(file(25, edited), false), Class::ChangedAtDestination);
        // Also when the previous run had listed it as not copied: not ours to overwrite.
        assert_eq!(pick(file(25, edited), true), Class::ChangedAtDestination);
    }

    #[test]
    fn a_destination_newer_than_the_source_but_older_than_the_previous_run_is_replaced() {
        let before_finish = FINISHED - 10_000;
        assert_eq!(pick(file(25, before_finish), false), Class::Changed);
    }

    #[test]
    fn a_destination_newer_than_the_previous_run_but_not_than_the_source_is_replaced() {
        // The source was touched later still, so the source wins; the destination is not "newer
        // than both".
        let source = Source {
            size: 10,
            modified_ms: Some(FINISHED + 90_000),
        };
        let class = classify(source, file(25, FINISHED + 60_000), false, FINISHED, None);
        assert_eq!(class, Class::Changed);
    }

    #[test]
    fn a_source_with_no_date_is_judged_by_the_end_of_the_previous_run_alone() {
        let source = Source {
            size: 10,
            modified_ms: None,
        };
        let edited = classify(source, file(10, FINISHED + 60_000), false, FINISHED, None);
        assert_eq!(edited, Class::ChangedAtDestination);
        let old = classify(source, file(10, FINISHED - 60_000), false, FINISHED, None);
        assert_eq!(old, Class::Unchanged);
    }

    #[test]
    fn a_folder_or_link_where_the_source_has_a_file_is_left_alone() {
        assert_eq!(pick(Dest::Other, false), Class::ChangedAtDestination);
        assert_eq!(pick(Dest::Other, true), Class::ChangedAtDestination);
    }

    #[test]
    fn a_file_left_alone_last_time_stays_left_until_it_changes_again() {
        let left = LeftBehind {
            relative_path: "a.txt".into(),
            size: 10,
            source_modified_ms: SOURCE.modified_ms,
            dest_size: 25,
            dest_modified_ms: Some(FINISHED - 5_000),
        };
        // The edit is older than the previous run's end (that run found it first), so only the
        // record keeps it safe.
        let at = |size: u64, ms: u64| {
            classify(
                SOURCE,
                file(size, ms),
                false,
                FINISHED + 600_000,
                Some(&left),
            )
        };
        assert_eq!(at(25, FINISHED - 5_000), Class::ChangedAtDestination);
        assert_eq!(
            at(25, FINISHED - 4_000),
            Class::ChangedAtDestination,
            "inside 2 s"
        );
        // Edited again: a different size or date no longer matches the record.
        assert_eq!(at(26, FINISHED - 5_000), Class::Changed);
        assert_eq!(at(25, FINISHED - 1_000), Class::Changed);
        assert_eq!(at(25, FINISHED + 5_000), Class::Changed);
    }

    #[test]
    fn times_are_compared_at_the_precision_the_run_saves() {
        // The run saves milliseconds; NTFS holds 100 ns ticks. A time read back from the saved
        // row is the same instant at the tolerance used for every comparison.
        let ticks = std::time::Duration::from_nanos(1_790_000_000_123_456_700);
        let time = UNIX_EPOCH + ticks;
        let saved = to_ms(time).unwrap();
        assert_eq!(saved, 1_790_000_000_123);
        let row = LeftBehind {
            relative_path: "a.txt".into(),
            size: 1,
            source_modified_ms: Some(saved),
            dest_size: 1,
            dest_modified_ms: Some(saved),
        };
        let json = serde_json::to_string(&row).unwrap();
        let back: LeftBehind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, row);
        assert!(!times_differ(back.dest_modified_ms, to_ms(time)));
    }

    #[test]
    fn a_replacement_waits_when_the_destination_was_edited_after_planning() {
        let since = FINISHED;
        let planned = file(30, since - 5_000);
        assert!(!edited_since(planned, SOURCE, since));
        assert!(edited_since(file(31, since + 60_000), SOURCE, since));
        assert!(edited_since(Dest::Other, SOURCE, since));
        assert!(!edited_since(Dest::Missing, SOURCE, since));
        // Our own copy (same size and date as the source) is never "edited", even if it landed
        // after the start.
        let ours = file(10, SOURCE.modified_ms.unwrap());
        assert!(!edited_since(ours, SOURCE, since));
    }

    #[test]
    fn reading_the_destination_reports_missing_files_and_folders() {
        let dir = crate::test_support::TempDir::new("delta-dest");
        let file_path = dir.write("f.txt", b"hello");
        std::fs::create_dir_all(dir.path("folder")).unwrap();
        assert_eq!(Dest::read(&dir.path("nope.txt")), Dest::Missing);
        assert_eq!(Dest::read(&dir.path("folder")), Dest::Other);
        match Dest::read(&file_path) {
            Dest::File { size, modified_ms } => {
                assert_eq!(size, 5);
                assert!(modified_ms.is_some());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_previous_run_is_found_by_id_and_a_missing_one_keeps_only_its_id() {
        use crate::{
            ConflictPolicy, RunState, RunTotals, TransferMode, TransferSettings, VerifyLevel,
        };
        let dir = crate::test_support::TempDir::new("delta-previous");
        let root = dir.path("runs");
        let summary_for = |store: &RunStore, previous: Option<&str>| RunSummary {
            id: store.id(),
            settings: TransferSettings {
                source: "s".into(),
                destination: "d".into(),
                mode: TransferMode::Copy,
                verify: VerifyLevel::SizeAndTime,
                conflict: ConflictPolicy::Skip,
                ignore_junk: false,
                download_cloud: false,
                preserve: Default::default(),
            },
            state: RunState::Completed,
            created_at_ms: 1_000,
            started_at_ms: Some(1_100),
            finished_at_ms: Some(2_000),
            machine: String::new(),
            user: String::new(),
            totals: RunTotals::default(),
            error: None,
            previous_run_id: previous.map(str::to_owned),
        };
        let first = RunStore::create(&root).unwrap();
        let first_summary = summary_for(&first, None);
        first.save_summary(&first_summary).unwrap();
        let second = RunStore::create(&root).unwrap();
        let second_summary = summary_for(&second, Some(&first.id()));
        second.save_summary(&second_summary).unwrap();

        assert_eq!(previous_run(&first, &first_summary), None);
        assert_eq!(
            previous_run(&second, &second_summary),
            Some(PreviousRun {
                id: first.id(),
                created_at_ms: Some(1_000),
                finished_at_ms: Some(2_000),
            })
        );
        std::fs::remove_dir_all(first.dir()).unwrap();
        let gone = previous_run(&second, &second_summary).unwrap();
        assert_eq!((gone.id, gone.created_at_ms), (first.id(), None));
    }

    #[test]
    fn the_left_alone_list_is_read_back_and_capped() {
        let dir = crate::test_support::TempDir::new("delta-left");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        assert!(left_behind(&store, 10).is_empty(), "not a delta run");
        let mut writer = store.delta_left_writer().unwrap();
        for number in 0..5 {
            writer
                .write(&LeftBehind {
                    relative_path: format!("f{number}.txt"),
                    size: 1,
                    source_modified_ms: None,
                    dest_size: 2,
                    dest_modified_ms: Some(3),
                })
                .unwrap();
        }
        writer.finish().unwrap();
        assert_eq!(left_behind(&store, 10).len(), 5);
        let capped = left_behind(&store, 2);
        assert_eq!(capped[1].relative_path, "f1.txt");
        assert_eq!(capped.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_destination_is_not_followed() {
        let dir = crate::test_support::TempDir::new("delta-link");
        let target = dir.write("target.txt", b"hello");
        let link = dir.path("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(Dest::read(&link), Dest::Other);
    }

    /// A junction needs no administrator rights, so this runs on the Windows CI job.
    #[cfg(windows)]
    #[test]
    fn a_junction_at_the_destination_is_not_followed() {
        let dir = crate::test_support::TempDir::new("delta-junction");
        let target = dir.path("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("inside.txt"), b"hello").unwrap();
        let junction = dir.path("junction");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .expect("mklink should run")
            .status;
        assert!(status.success(), "mklink /J failed");
        assert_eq!(Dest::read(&junction), Dest::Other);
        // Nothing was read through it.
        assert!(target.join("inside.txt").is_file());
        let _ = std::fs::remove_dir(&junction);
    }

    /// NTFS keeps 100 ns ticks; a time set on a file reads back within the tolerance after the
    /// round trip through the milliseconds a run saves.
    #[cfg(windows)]
    #[test]
    fn an_ntfs_time_survives_the_saved_millisecond_format() {
        let dir = crate::test_support::TempDir::new("delta-ticks");
        let path = dir.write("f.txt", b"x");
        let time = UNIX_EPOCH + std::time::Duration::from_nanos(1_790_000_000_123_456_700);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(time)
            .unwrap();
        let read = match Dest::read(&path) {
            Dest::File { modified_ms, .. } => modified_ms,
            other => panic!("{other:?}"),
        };
        let saved = serde_json::to_string(&read).unwrap();
        let back: Option<u64> = serde_json::from_str(&saved).unwrap();
        assert_eq!(back, to_ms(time));
        assert!(!times_differ(back, read));
    }
}

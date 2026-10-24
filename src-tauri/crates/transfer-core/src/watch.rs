//! Watch mode: another program copies (Explorer, robocopy…) and Deep Server Analyzer checks what arrives
//! at the destination against the source list made by [`crate::prepare::prepare`].
//!
//! A file counts as arrived once its size has stayed the same for a while, it can be opened
//! without anyone else holding it, and its size and date match the source. File events can be
//! lost (buffer overflow, some network shares), so every pending file is also checked on a
//! timer, and the watch falls back to polling when the watcher reports an error. Finishing
//! checks every file that has not arrived; the reason given for those is inferred.

use crate::copy::{hash_file, times_match};
use crate::prepare::{destination_path, safe_relative, ManifestEntry, LONG_PATH_LIMIT};
use crate::reason::{classify, FailureReason, Side};
use crate::store::{LogStatus, RunStore};
use crate::{
    now_ms, CancelToken, ItemKind, ItemResult, ItemStatus, Phase, Result, RunState, RunSummary,
    TransferError, TransferMode, TransferProgress, TransferSink, VerifyLevel,
};
use notify::{RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// How long a file's size must stay the same before it can count as arrived.
    pub stable_for: Duration,
    /// Check every pending file this often even when file events arrive.
    pub reconcile_every: Duration,
    /// Check interval once the watcher has failed or overflowed.
    pub poll_every: Duration,
    /// Finish by itself after this long without changes, once something arrived.
    pub quiet_period: Option<Duration>,
    /// Poll from the start instead of using file events.
    pub force_polling: bool,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            stable_for: Duration::from_secs(3),
            reconcile_every: Duration::from_secs(30),
            poll_every: Duration::from_secs(5),
            quiet_period: None,
            force_polling: false,
        }
    }
}

/// `cancel` stops without a verdict; `finish` runs the final check.
#[derive(Debug, Clone, Default)]
pub struct WatchControl {
    pub cancel: CancelToken,
    pub finish: CancelToken,
}

const TICK: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Track {
    Waiting,
    Seen { size: u64, since: Instant },
    Arrived,
}

struct Pending {
    entry: ManifestEntry,
    track: Track,
}

pub fn watch(
    root: &Path,
    run_id: &str,
    options: &WatchOptions,
    control: &WatchControl,
    sink: &mut dyn TransferSink,
) -> Result<RunSummary> {
    let store = RunStore::open(root, run_id)?;
    let mut summary = store.summary()?;
    if summary.settings.mode != TransferMode::Watch || summary.state != RunState::Prepared {
        return Err(TransferError::BadRun(
            "This run is not a prepared watch run.".to_owned(),
        ));
    }
    let source = PathBuf::from(&summary.settings.source);
    let destination = PathBuf::from(&summary.settings.destination);
    fs::create_dir_all(&destination)?;
    let hashed = summary.settings.verify == VerifyLevel::Hash;

    let mut files = Vec::new();
    for row in store.manifest::<ManifestEntry>()? {
        let row = row?;
        if row.kind == ItemKind::File && row.blocked.is_none() {
            files.push(Pending {
                entry: row,
                track: Track::Waiting,
            });
        }
    }
    let index: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(position, file)| (file.entry.relative_path.to_lowercase(), position))
        .collect();

    summary.state = RunState::Watching;
    summary.started_at_ms = Some(now_ms());
    store.save_summary(&summary)?;
    store.audit("watch", LogStatus::Started, "Watching the destination", &[]);

    let (sender, events) = mpsc::channel();
    let mut polling = options.force_polling;
    let mut watcher = None;
    if !polling {
        match notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        }) {
            Ok(mut created) => match created.watch(&destination, RecursiveMode::Recursive) {
                Ok(()) => watcher = Some(created),
                Err(_) => polling = true,
            },
            Err(_) => polling = true,
        }
    }

    let mut results = store.results_writer()?;
    let mut progress = TransferProgress {
        run_id: store.id(),
        phase: Phase::Watching,
        files_done: 0,
        files_total: files.len() as u64,
        bytes_done: 0,
        bytes_total: files.iter().map(|file| file.entry.size).sum(),
        copied: 0,
        skipped: 0,
        not_copied: summary.totals.not_copied,
        current: None,
    };
    sink.progress(&progress);

    let mut last_full_check = Instant::now();
    let mut last_change = Instant::now();
    let mut arriving: HashSet<usize> = HashSet::new();
    let check_context = CheckContext {
        destination: &destination,
        stable_for: options.stable_for,
        hashed,
        source: &source,
    };
    loop {
        if control.cancel.is_cancelled() || control.finish.is_cancelled() {
            break;
        }
        std::thread::sleep(TICK);

        let mut dirty: HashSet<usize> = std::mem::take(&mut arriving);
        let mut check_all = false;
        while let Ok(event) = events.try_recv() {
            match event {
                Ok(event) => {
                    if event.need_rescan() {
                        check_all = true;
                    }
                    for path in &event.paths {
                        if let Some(position) =
                            relative_key(&destination, path).and_then(|key| index.get(&key))
                        {
                            dirty.insert(*position);
                        }
                    }
                }
                // Overflow or a dropped share: fall back to checking on a timer.
                Err(_) => polling = true,
            }
        }
        let interval = scaled_interval(
            if polling {
                options.poll_every
            } else {
                options.reconcile_every
            },
            files.len() - progress.files_done as usize,
        );
        if check_all || last_full_check.elapsed() >= interval {
            last_full_check = Instant::now();
            dirty.extend(0..files.len());
        }
        // Arrived is final: a later event on the same file (antivirus, a touched timestamp, the
        // copier rewriting it) must not count it twice.
        dirty.retain(|&position| files[position].track != Track::Arrived);

        for position in dirty {
            let before = files[position].track;
            let after = check_context.check(&files[position].entry, before);
            if after != before {
                last_change = Instant::now();
            }
            files[position].track = after;
            match after {
                Track::Arrived => {
                    let entry = &files[position].entry;
                    let item = ItemResult::done(
                        entry.relative_path.clone(),
                        entry.size,
                        ItemStatus::Arrived,
                    );
                    progress.files_done += 1;
                    progress.bytes_done += entry.size;
                    progress.copied += 1;
                    progress.current = Some(entry.relative_path.clone());
                    sink.item(&item);
                    results.write(&item)?;
                }
                Track::Seen { .. } => {
                    arriving.insert(position);
                }
                Track::Waiting => {}
            }
        }
        sink.progress(&progress);

        let all_arrived = progress.files_done == files.len() as u64;
        let quiet = options
            .quiet_period
            .is_some_and(|quiet| progress.copied > 0 && last_change.elapsed() >= quiet);
        if all_arrived || quiet {
            break;
        }
    }
    drop(watcher);

    if control.cancel.is_cancelled() {
        summary.state = RunState::Cancelled;
        summary.finished_at_ms = Some(now_ms());
        results.finish()?;
        store.refresh_totals(&mut summary)?;
        store.save_summary(&summary)?;
        store.audit(
            "watch",
            LogStatus::Cancelled,
            "Watch stopped without a final check",
            &[],
        );
        return Ok(summary);
    }

    // Final check: every file that has not arrived gets a verdict.
    progress.phase = Phase::Finishing;
    sink.progress(&progress);
    for file in files.iter().filter(|file| file.track != Track::Arrived) {
        let item = check_context.verdict(&file.entry);
        match item.status {
            ItemStatus::NotCopied => progress.not_copied += 1,
            _ => progress.copied += 1,
        }
        progress.files_done += 1;
        sink.item(&item);
        results.write(&item)?;
    }
    results.finish()?;
    progress.phase = Phase::Done;
    progress.current = None;
    sink.progress(&progress);

    summary.state = RunState::Completed;
    summary.finished_at_ms = Some(now_ms());
    store.refresh_totals(&mut summary)?;
    store.save_summary(&summary)?;
    store.audit(
        "watch",
        LogStatus::Succeeded,
        "Watch finished",
        &[
            ("arrived", summary.totals.copied.into()),
            ("notCopied", summary.totals.not_copied.into()),
            ("polling", polling.into()),
        ],
    );
    Ok(summary)
}

fn relative_key(destination: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(destination).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/").to_lowercase())
}

struct CheckContext<'a> {
    destination: &'a Path,
    source: &'a Path,
    stable_for: Duration,
    hashed: bool,
}

impl CheckContext<'_> {
    /// Moves a file towards Arrived: first seen, then unchanged for `stable_for`, free and matching.
    fn check(&self, entry: &ManifestEntry, track: Track) -> Track {
        if !safe_relative(&entry.relative_path) {
            return Track::Waiting;
        }
        let target = destination_path(self.destination, &entry.relative_path);
        let Ok(meta) = fs::metadata(&target) else {
            return Track::Waiting;
        };
        let size = meta.len();
        match track {
            Track::Seen { size: seen, since } if seen == size => {
                if since.elapsed() >= self.stable_for && self.matches(entry, &target) {
                    Track::Arrived
                } else {
                    track
                }
            }
            _ => Track::Seen {
                size,
                since: Instant::now(),
            },
        }
    }

    fn matches(&self, entry: &ManifestEntry, target: &Path) -> bool {
        let Ok(meta) = fs::metadata(target) else {
            return false;
        };
        let source_time = entry
            .modified_at_ms
            .map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
        meta.len() == entry.size
            && times_match(source_time, meta.modified().ok())
            && open_exclusively(target)
            // An online-only source is not read, so it has no hash to compare; size and time stand.
            && (!self.hashed || entry.cloud || self.hashes_match(entry, target))
    }

    fn hashes_match(&self, entry: &ManifestEntry, target: &Path) -> bool {
        let source = destination_path(self.source, &entry.relative_path);
        match (hash_file(&source), hash_file(target)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }

    /// The final word on a file that has not arrived yet.
    fn verdict(&self, entry: &ManifestEntry) -> ItemResult {
        let target = destination_path(self.destination, &entry.relative_path);
        let inferred = |reason: FailureReason, side: Option<Side>, message: String| {
            let mut item = ItemResult::not_copied(
                entry.relative_path.clone(),
                ItemKind::File,
                entry.size,
                reason,
                side,
                message,
            );
            item.inferred = true;
            item
        };
        let Ok(meta) = fs::metadata(&target) else {
            if entry.cloud {
                // Opening the source would download it, so its state is not checked.
                return inferred(
                    FailureReason::CloudOnly,
                    Some(Side::Source),
                    "Online-only file; it was not opened, so nothing says why it did not arrive"
                        .to_owned(),
                );
            }
            // Why might the other program have skipped it? Check the source for a likely cause.
            let source = destination_path(self.source, &entry.relative_path);
            if let Err(error) = fs::File::open(&source) {
                let mut item = inferred(
                    classify(&error, Side::Source),
                    Some(Side::Source),
                    error.to_string(),
                );
                item.os_code = error.raw_os_error();
                return item;
            }
            let length = target.to_string_lossy().encode_utf16().count();
            if length > LONG_PATH_LIMIT {
                return inferred(
                    FailureReason::PathTooLong,
                    Some(Side::Destination),
                    format!("The destination path is {length} characters long"),
                );
            }
            return inferred(
                FailureReason::Missing,
                Some(Side::Destination),
                "Not found at the destination".to_owned(),
            );
        };
        if !open_exclusively(&target) || meta.len() < entry.size {
            return inferred(
                FailureReason::Incomplete,
                Some(Side::Destination),
                format!("{} of {} bytes at the destination", meta.len(), entry.size),
            );
        }
        if self.matches(entry, &target) {
            let mut item =
                ItemResult::done(entry.relative_path.clone(), entry.size, ItemStatus::Arrived);
            item.inferred = true;
            return item;
        }
        inferred(
            FailureReason::VerifyMismatch,
            Some(Side::Destination),
            "The destination file's size, date or content differs from the source".to_owned(),
        )
    }
}

/// Pending files above which full re-checks slow down: stat-ing 100k files every few seconds
/// on a network share is heavier than the copy being watched.
const LARGE_WATCH: usize = 20_000;
const LARGE_WATCH_INTERVAL: Duration = Duration::from_secs(120);

fn scaled_interval(interval: Duration, pending: usize) -> Duration {
    if pending > LARGE_WATCH {
        interval.max(LARGE_WATCH_INTERVAL)
    } else {
        interval
    }
}

/// True when nobody has the file open for writing (the copying program is done with it).
///
/// Sharing read and delete, but not write, makes the open fail while a writer holds the file,
/// without blocking other readers or a rename/delete by the copying program during the probe.
#[cfg(windows)]
fn open_exclusively(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_DELETE: u32 = 0x4;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(path)
        .is_ok()
}

#[cfg(not(windows))]
fn open_exclusively(path: &Path) -> bool {
    fs::File::open(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::prepare;
    use crate::test_support::TempDir;
    use crate::{ConflictPolicy, NullSink, TransferSettings};

    #[test]
    fn large_watches_recheck_less_often() {
        let five = Duration::from_secs(5);
        assert_eq!(scaled_interval(five, 10), five);
        assert_eq!(scaled_interval(five, LARGE_WATCH + 1), LARGE_WATCH_INTERVAL);
        assert_eq!(
            scaled_interval(Duration::from_secs(600), LARGE_WATCH + 1),
            Duration::from_secs(600)
        );
    }

    fn watch_run(dir: &TempDir, verify: VerifyLevel) -> String {
        let settings = TransferSettings {
            source: dir.path("src").display().to_string(),
            destination: dir.path("dst").display().to_string(),
            mode: TransferMode::Watch,
            verify,
            conflict: ConflictPolicy::Skip,
            ignore_junk: false,
            download_cloud: false,
            preserve: Default::default(),
        };
        prepare(
            &dir.path("runs"),
            settings,
            &CancelToken::default(),
            &mut NullSink,
        )
        .unwrap()
        .0
        .id
    }

    fn fast(force_polling: bool) -> WatchOptions {
        WatchOptions {
            stable_for: Duration::from_millis(100),
            reconcile_every: Duration::from_millis(300),
            poll_every: Duration::from_millis(100),
            quiet_period: None,
            force_polling,
        }
    }

    /// Cancels after `seconds` so a broken test fails instead of hanging.
    fn watchdog(control: &WatchControl, seconds: u64) {
        let cancel = control.cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            cancel.cancel();
        });
    }

    #[test]
    fn finishing_lists_the_file_another_program_skipped() {
        let dir = TempDir::new("watch-missing");
        for name in ["a", "b", "skipped"] {
            dir.write(&format!("src/{name}.txt"), name.as_bytes());
        }
        let id = watch_run(&dir, VerifyLevel::SizeAndTime);
        let control = WatchControl::default();
        watchdog(&control, 20);
        let (src, dst, finish) = (dir.path("src"), dir.path("dst"), control.finish.clone());
        let copier = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            for name in ["a.txt", "b.txt"] {
                fs::copy(src.join(name), dst.join(name)).unwrap();
            }
            std::thread::sleep(Duration::from_millis(500));
            finish.cancel();
        });

        let summary = watch(
            &dir.path("runs"),
            &id,
            &fast(false),
            &control,
            &mut NullSink,
        )
        .unwrap();
        copier.join().unwrap();

        assert_eq!(summary.state, RunState::Completed);
        assert_eq!((summary.totals.copied, summary.totals.not_copied), (2, 1));
        let latest = RunStore::open(&dir.path("runs"), &id)
            .unwrap()
            .latest_results()
            .unwrap();
        let skipped = &latest["skipped.txt"];
        assert_eq!(skipped.reason, Some(FailureReason::Missing));
        assert!(skipped.inferred);
        assert!(skipped.recoverable);
    }

    #[test]
    fn polling_finishes_by_itself_when_everything_has_arrived() {
        let dir = TempDir::new("watch-poll");
        dir.write("src/one.txt", b"1");
        dir.write("src/sub/two.txt", b"22");
        let id = watch_run(&dir, VerifyLevel::Hash);
        let control = WatchControl::default();
        watchdog(&control, 20);
        let (src, dst) = (dir.path("src"), dir.path("dst"));
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            fs::create_dir_all(dst.join("sub")).unwrap();
            fs::copy(src.join("one.txt"), dst.join("one.txt")).unwrap();
            fs::copy(src.join("sub/two.txt"), dst.join("sub/two.txt")).unwrap();
        });

        let summary = watch(&dir.path("runs"), &id, &fast(true), &control, &mut NullSink).unwrap();

        assert_eq!(
            summary.state,
            RunState::Completed,
            "finished before the watchdog"
        );
        assert_eq!((summary.totals.copied, summary.totals.not_copied), (2, 0));
    }

    #[test]
    fn a_file_touched_again_after_arriving_is_counted_once() {
        let dir = TempDir::new("watch-touched");
        dir.write("src/a.txt", b"first");
        dir.write("src/b.txt", b"second");
        let id = watch_run(&dir, VerifyLevel::Hash);
        let control = WatchControl::default();
        watchdog(&control, 20);
        let (src, dst) = (dir.path("src"), dir.path("dst"));
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            fs::copy(src.join("a.txt"), dst.join("a.txt")).unwrap();
            // Rewritten with the same content after it has arrived.
            std::thread::sleep(Duration::from_millis(800));
            fs::copy(src.join("a.txt"), dst.join("a.txt")).unwrap();
            std::thread::sleep(Duration::from_millis(800));
            fs::copy(src.join("b.txt"), dst.join("b.txt")).unwrap();
        });

        let summary = watch(
            &dir.path("runs"),
            &id,
            &fast(false),
            &control,
            &mut NullSink,
        )
        .unwrap();

        assert_eq!(
            summary.state,
            RunState::Completed,
            "finished before the watchdog"
        );
        assert_eq!((summary.totals.copied, summary.totals.not_copied), (2, 0));
    }

    #[test]
    fn a_different_file_at_the_destination_is_a_mismatch() {
        let dir = TempDir::new("watch-mismatch");
        dir.write("src/a.txt", b"the real content");
        let id = watch_run(&dir, VerifyLevel::SizeAndTime);
        dir.write("dst/a.txt", b"something else, but longer");
        let control = WatchControl::default();
        control.finish.cancel();

        let summary = watch(&dir.path("runs"), &id, &fast(true), &control, &mut NullSink).unwrap();

        assert_eq!(summary.totals.not_copied, 1);
        let latest = RunStore::open(&dir.path("runs"), &id)
            .unwrap()
            .latest_results()
            .unwrap();
        assert_eq!(latest["a.txt"].reason, Some(FailureReason::VerifyMismatch));
    }

    fn cloud_entry(size: u64) -> ManifestEntry {
        ManifestEntry {
            relative_path: "online.docx".to_owned(),
            kind: ItemKind::File,
            size,
            modified_at_ms: None,
            blocked: None,
            cloud: true,
        }
    }

    #[test]
    fn an_online_only_source_is_never_hashed_or_opened() {
        // The source is not there at all: any attempt to read it would fail the check.
        let dir = TempDir::new("watch-cloud");
        dir.write("dst/online.docx", b"12345");
        let context = CheckContext {
            destination: &dir.path("dst"),
            source: &dir.path("src"),
            stable_for: Duration::ZERO,
            hashed: true,
        };
        let entry = cloud_entry(5);

        assert!(context.matches(&entry, &dir.path("dst/online.docx")));
        let item = context.verdict(&entry);
        assert_eq!(item.status, ItemStatus::Arrived);

        fs::remove_file(dir.path("dst/online.docx")).unwrap();
        let item = context.verdict(&entry);
        assert_eq!(item.reason, Some(FailureReason::CloudOnly));
        assert_eq!(item.side, Some(Side::Source));
    }
}

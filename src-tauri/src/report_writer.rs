//! Keeps the Disk Cleanup scan's saved report up to date as items are deleted, without
//! rewriting a multi-MB file on every delete.
//!
//! Removals are queued, and one write follows [`FLUSH_DELAY`] after the first of a burst. The
//! write happens outside every lock the delete path holds. A new scan and app exit flush first,
//! so a quit loses nothing; only a crash inside the delay can.

use report_core::{Body, Removed, Saved};
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Condvar;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

/// How long removals wait before the report is rewritten.
pub const FLUSH_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct ReportWriter {
    inner: Arc<Inner>,
}

struct Inner {
    /// `None` means the app's reports folder, looked up at write time.
    root: Option<PathBuf>,
    delay: Duration,
    slot: Mutex<Slot>,
    /// Serialises writes, so a flush on exit waits for one already running.
    writing: Mutex<()>,
    /// Signalled after every flush that reaches the end, so tests wait instead of polling the disk.
    #[cfg(test)]
    flushed: Condvar,
}

#[derive(Default)]
struct Slot {
    /// Changes with every new scan, so a write that started for the old report puts nothing back.
    epoch: u64,
    saved: Option<Saved>,
    /// The tracked report is a disk usage report. Unlike `saved`, this stays true while a write
    /// has the report out of the slot, so removals arriving mid-write are queued, not dropped.
    tracking: bool,
    pending: Vec<Removed>,
    /// A write failed; the report on disk is behind `saved`.
    stale: bool,
    timer_running: bool,
    /// The last write's error, handed to the next delete to show.
    error: Option<String>,
    #[cfg(test)]
    writes: usize,
    /// Removals that reached the disk.
    #[cfg(test)]
    written: usize,
}

impl Default for ReportWriter {
    fn default() -> Self {
        Self::new(None, FLUSH_DELAY)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not stop removals from reaching the report.
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

impl ReportWriter {
    fn new(root: Option<PathBuf>, delay: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                root,
                delay,
                slot: Mutex::new(Slot::default()),
                writing: Mutex::new(()),
                #[cfg(test)]
                flushed: Condvar::new(),
            }),
        }
    }

    /// Starts tracking a freshly written report. The previous one is flushed first.
    pub fn set(&self, saved: Option<Saved>) {
        self.flush();
        let mut slot = lock(&self.inner.slot);
        slot.epoch += 1;
        slot.tracking = matches!(
            saved.as_ref().map(|saved| &saved.body),
            Some(Body::DiskUsage(_))
        );
        slot.saved = saved;
        slot.pending.clear();
        slot.stale = false;
        slot.error = None;
    }

    /// Queues removals for the report. Returns the error of an earlier write that failed, once,
    /// for the page to show as a warning.
    pub fn removed(&self, items: Vec<Removed>) -> Option<String> {
        let mut slot = lock(&self.inner.slot);
        if !slot.tracking {
            return None;
        }
        slot.pending.extend(items);
        if !slot.timer_running {
            slot.timer_running = true;
            let writer = self.clone();
            thread::spawn(move || {
                thread::sleep(writer.inner.delay);
                lock(&writer.inner.slot).timer_running = false;
                writer.flush();
            });
        }
        slot.error.take()
    }

    /// Writes queued removals now. A failure is kept for the next [`removed`](Self::removed) and
    /// retried by the next flush.
    pub fn flush(&self) {
        let _writing = lock(&self.inner.writing);
        #[cfg(test)]
        let queued;
        let (epoch, saved) = {
            let mut slot = lock(&self.inner.slot);
            if slot.pending.is_empty() && !slot.stale {
                return;
            }
            let Some(mut saved) = slot.saved.take() else {
                return;
            };
            let pending = std::mem::take(&mut slot.pending);
            #[cfg(test)]
            {
                queued = pending.len();
            }
            if let Body::DiskUsage(report) = &mut saved.body {
                report.removed.extend(pending);
            }
            (slot.epoch, saved)
        };

        let root = self
            .inner
            .root
            .clone()
            .unwrap_or_else(report_core::reports_root);
        let outcome = report_core::write(&root, &saved);

        let mut slot = lock(&self.inner.slot);
        // A new scan that took over while this wrote keeps its own report.
        if slot.epoch == epoch {
            // Removals that arrived during the write are still queued; the report goes back as is.
            slot.saved = Some(saved);
            match outcome {
                Ok(()) => {
                    slot.stale = false;
                    #[cfg(test)]
                    {
                        slot.written += queued;
                    }
                }
                Err(err) => {
                    slot.stale = true;
                    slot.error = Some(err);
                }
            }
        }
        #[cfg(test)]
        {
            slot.writes += 1;
            self.inner.flushed.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use report_core::{JobInfo, UsageReport};

    /// A folder under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "dsa-report-writer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const LONG: Duration = Duration::from_secs(3600);

    fn removal(n: u64) -> Removed {
        Removed {
            path: format!(r"C:\scan\item-{n}"),
            size: n,
            permanent: false,
        }
    }

    fn scan_report(root: &std::path::Path) -> Saved {
        let body = Body::DiskUsage(UsageReport {
            path: r"C:\scan".into(),
            ..UsageReport::default()
        });
        report_core::save(root, JobInfo::default(), body).unwrap()
    }

    fn removed_on_disk(root: &std::path::Path, id: &str) -> usize {
        match report_core::load(root, id).unwrap().body {
            Body::DiskUsage(report) => report.removed.len(),
            _ => panic!("not a disk usage report"),
        }
    }

    fn writes(writer: &ReportWriter) -> usize {
        lock(&writer.inner.slot).writes
    }

    /// Blocks until `count` removals have been written, woken by each flush. The long limit only
    /// keeps a hang from stalling the run; a flush never depends on it.
    fn wait_for_written(writer: &ReportWriter, count: usize) {
        let slot = lock(&writer.inner.slot);
        let (slot, timeout) = writer
            .inner
            .flushed
            .wait_timeout_while(slot, Duration::from_secs(60), |slot| slot.written < count)
            .unwrap_or_else(|err| err.into_inner());
        assert!(
            !timeout.timed_out(),
            "never flushed: {} of {count} written",
            slot.written
        );
    }

    #[test]
    fn a_burst_of_deletes_is_one_write() {
        let dir = TempDir::new();
        let saved = scan_report(dir.0.as_path());
        let id = saved.id.clone();
        let writer = ReportWriter::new(Some(dir.0.as_path().to_path_buf()), LONG);
        writer.set(Some(saved));

        for n in 0..10 {
            assert_eq!(writer.removed(vec![removal(n)]), None);
        }
        assert_eq!(
            removed_on_disk(dir.0.as_path(), &id),
            0,
            "nothing written yet"
        );
        assert_eq!(writes(&writer), 0);

        writer.flush();
        assert_eq!(removed_on_disk(dir.0.as_path(), &id), 10);
        assert_eq!(writes(&writer), 1);
        writer.flush();
        assert_eq!(writes(&writer), 1, "nothing queued, nothing written");
    }

    #[test]
    fn removals_reach_the_disk_after_the_delay() {
        let dir = TempDir::new();
        let saved = scan_report(dir.0.as_path());
        let id = saved.id.clone();
        let writer = ReportWriter::new(
            Some(dir.0.as_path().to_path_buf()),
            Duration::from_millis(50),
        );
        writer.set(Some(saved));
        writer.removed(vec![removal(1)]);
        writer.removed(vec![removal(2)]);

        wait_for_written(&writer, 2);
        assert_eq!(removed_on_disk(dir.0.as_path(), &id), 2);
        // Later removals start a new timer.
        writer.removed(vec![removal(3)]);
        wait_for_written(&writer, 3);
        assert_eq!(removed_on_disk(dir.0.as_path(), &id), 3);
    }

    #[test]
    fn removals_that_arrive_during_a_write_are_kept() {
        let dir = TempDir::new();
        let saved = scan_report(dir.0.as_path());
        let id = saved.id.clone();
        let writer = ReportWriter::new(Some(dir.0.as_path().to_path_buf()), LONG);
        writer.set(Some(saved));
        // A write has the report out of the slot while it runs.
        let in_flight = lock(&writer.inner.slot).saved.take();

        assert_eq!(writer.removed(vec![removal(1)]), None);
        assert_eq!(
            lock(&writer.inner.slot).pending.len(),
            1,
            "queued, not dropped"
        );

        lock(&writer.inner.slot).saved = in_flight;
        writer.flush();
        assert_eq!(removed_on_disk(dir.0.as_path(), &id), 1);
    }

    #[test]
    fn a_new_scan_flushes_the_old_report_and_keeps_the_new_one_clean() {
        let dir = TempDir::new();
        let first = scan_report(dir.0.as_path());
        let second = scan_report(dir.0.as_path());
        let (first_id, second_id) = (first.id.clone(), second.id.clone());
        assert_ne!(first_id, second_id);
        let writer = ReportWriter::new(Some(dir.0.as_path().to_path_buf()), LONG);
        writer.set(Some(first));
        writer.removed(vec![removal(1)]);

        writer.set(Some(second));
        assert_eq!(removed_on_disk(dir.0.as_path(), &first_id), 1);
        writer.flush();
        assert_eq!(removed_on_disk(dir.0.as_path(), &second_id), 0);
    }

    #[test]
    fn a_failed_write_is_reported_once_and_retried() {
        let dir = TempDir::new();
        let saved = scan_report(dir.0.as_path());
        let id = saved.id.clone();
        // A file where the reports folder should be makes every write fail.
        let blocked = dir.0.as_path().join("blocked");
        fs_write(&blocked);
        let writer = ReportWriter::new(Some(blocked.join("Reports")), LONG);
        writer.set(Some(saved));
        writer.removed(vec![removal(1)]);
        writer.flush();

        let warning = writer.removed(vec![removal(2)]);
        assert!(warning.is_some(), "the failed write is surfaced");
        assert_eq!(writer.removed(vec![removal(3)]), None, "and only once");

        // The folder becomes writable: the retry writes everything queued so far.
        std::fs::remove_file(&blocked).unwrap();
        std::fs::create_dir_all(blocked.join("Reports")).unwrap();
        writer.flush();
        let saved = report_core::load(&blocked.join("Reports"), &id).unwrap();
        match saved.body {
            Body::DiskUsage(report) => assert_eq!(report.removed.len(), 3),
            _ => panic!("not a disk usage report"),
        }
    }

    fn fs_write(path: &std::path::Path) {
        std::fs::write(path, b"not a folder").unwrap();
    }

    #[test]
    fn other_report_kinds_and_no_report_are_left_alone() {
        let writer = ReportWriter::new(None, LONG);
        assert_eq!(writer.removed(vec![removal(1)]), None);
        writer.flush();
        assert_eq!(writes(&writer), 0);
    }
}

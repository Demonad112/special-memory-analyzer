//! `Logs\app.log`: what the app itself reports when something goes wrong, so a field failure can
//! be diagnosed from a file because the GUI build discards stderr. One line per message: time,
//! level, text. It holds no file contents, only messages and paths the user chose. Capped at
//! 1 MB with 3 rotated copies (`app.1.log` newest), the same scheme as the deletion log.
//!
//! Logging never fails a command: a write that cannot happen drops the line.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

pub const LOG_FILE: &str = "app.log";
const ROTATIONS: u32 = 3;
/// The log is moved aside once it reaches this size.
pub const MAX_BYTES: u64 = 1024 * 1024;

/// Keeps two threads from rotating or interleaving at the same time.
static WRITE: Mutex<()> = Mutex::new(());

fn rotated_name(n: u32) -> String {
    format!("app.{n}.log")
}

/// Writes an error line to stderr (so `tauri dev` still shows it) and to `app.log`.
pub fn error(message: &str) {
    log("ERROR", message);
}

/// Writes a warning line to stderr and to `app.log`.
pub fn warn(message: &str) {
    log("WARN", message);
}

fn log(level: &str, message: &str) {
    eprintln!("{message}");
    let _guard = WRITE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = append_limited(&crate::logs_root(), level, message, MAX_BYTES);
}

/// One line, so a name with a line break in it cannot pass for a second log line.
fn one_line(message: &str) -> String {
    message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn rotate(dir: &Path, file: &Path) -> io::Result<()> {
    let _ = fs::remove_file(dir.join(rotated_name(ROTATIONS)));
    for n in (1..ROTATIONS).rev() {
        let from = dir.join(rotated_name(n));
        if from.exists() {
            fs::rename(&from, dir.join(rotated_name(n + 1)))?;
        }
    }
    fs::rename(file, dir.join(rotated_name(1)))
}

fn append_limited(dir: &Path, level: &str, message: &str, max_bytes: u64) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let file = dir.join(LOG_FILE);
    if fs::metadata(&file).is_ok_and(|meta| meta.len() >= max_bytes) {
        // A failed rotation must not lose the new line: keep appending to the big file.
        let _ = rotate(dir, &file);
    }
    let line = format!(
        "{} {level} {}\n",
        transfer_core::format_utc(transfer_core::now_ms()),
        one_line(message)
    );
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_becomes_one_timestamped_line_even_with_a_line_break_in_it() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Logs");
        append_limited(
            &dir,
            "ERROR",
            "could not read C:\\a\r\nERROR fake",
            MAX_BYTES,
        )
        .unwrap();
        append_limited(&dir, "WARN", "second", MAX_BYTES).unwrap();
        let text = fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" ERROR could not read C:\\a  ERROR fake"));
        assert!(lines[0].contains(" UTC "));
        assert!(lines[1].ends_with(" WARN second"));
    }

    #[test]
    fn a_full_log_rotates_and_only_three_copies_are_kept() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        for n in 0..6 {
            append_limited(&dir, "ERROR", &format!("line {n}"), 1).unwrap();
        }
        let read = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        assert!(read(LOG_FILE).contains("line 5"));
        assert!(read("app.1.log").contains("line 4"));
        assert!(read("app.2.log").contains("line 3"));
        assert!(read("app.3.log").contains("line 2"));
        assert!(!dir.join("app.4.log").exists());
    }

    #[test]
    fn a_log_folder_that_cannot_be_created_drops_the_line_without_panicking() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("file");
        fs::write(&blocker, b"x").unwrap();
        assert!(append_limited(&blocker.join("Logs"), "ERROR", "x", MAX_BYTES).is_err());
    }
}

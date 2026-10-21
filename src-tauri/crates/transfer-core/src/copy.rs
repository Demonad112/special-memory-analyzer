//! Copies one file: streamed through a buffer into `<name>.dspart`, renamed when complete,
//! modified time kept, then checked against the source.
//!
//! Long paths: Rust's Windows file APIs add the `\\?\` prefix themselves once a path is too long
//! for the classic API; direct Win32 calls in [`crate::meta`] add it with `extended_path`. A name
//! so long that `.dspart` would push it over the 255-character limit gets a short temp name.
//!
//! After the rename and the check, [`crate::meta::apply`] puts the source's created, accessed and
//! modified times and its attributes on the copy.

use crate::meta::{self, PreserveOptions};
use crate::reason::{classify, FailureReason, Side};
use crate::secure;
use crate::CancelToken;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const BUFFER_SIZE: usize = 1024 * 1024;
/// Suffix of a file that is still being written. It is never left behind on success, failure or cancel.
pub const PART_SUFFIX: &str = ".dspart";
/// FAT/exFAT store times in 2-second steps, so a copy there never matches to the millisecond.
pub const TIME_TOLERANCE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyLevel {
    /// Size and modified time after the copy.
    SizeAndTime,
    /// Also a BLAKE3 hash of the source while copying, compared with a re-read of the destination.
    Hash,
}

/// What to do when a different file already has the destination name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Leave it and list the file as "A different file is already at the destination".
    Skip,
    /// Replace it only when the source is newer.
    OverwriteIfNewer,
    /// Replace it (used when the user retries a conflict).
    Overwrite,
}

#[derive(Debug, Clone)]
pub struct CopyOptions {
    pub verify: VerifyLevel,
    pub conflict: ConflictPolicy,
    /// Report a destination with the same size and time as [`CopyOutcome::SkippedIdentical`]
    /// instead of copying. Off when the caller already knows the content differs.
    pub skip_identical: bool,
    /// Waits before each automatic retry of a transient error; the length is the retry count.
    pub retry_delays: Vec<Duration>,
    /// Dates and attributes carried over once the file is in place.
    pub preserve: PreserveOptions,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            verify: VerifyLevel::SizeAndTime,
            conflict: ConflictPolicy::Skip,
            skip_identical: true,
            preserve: PreserveOptions::default(),
            retry_delays: vec![
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    Copied {
        bytes: u64,
        /// Hex BLAKE3 of the source (and, verified, of the destination) when hashing is on.
        hash: Option<String>,
        /// Parts of the metadata that could not be kept (the copy itself is fine).
        warnings: Vec<String>,
    },
    /// The destination already has this file with the same size and time.
    SkippedIdentical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFailure {
    pub reason: FailureReason,
    pub side: Option<Side>,
    pub os_code: Option<i32>,
    pub message: String,
    /// Destination hash when a hash check failed.
    pub dest_hash: Option<String>,
}

impl CopyFailure {
    fn io(error: &io::Error, side: Side) -> Self {
        Self {
            reason: classify(error, side),
            side: Some(side),
            os_code: error.raw_os_error(),
            message: error.to_string(),
            dest_hash: None,
        }
    }

    fn new(reason: FailureReason, side: Option<Side>, message: impl Into<String>) -> Self {
        Self {
            reason,
            side,
            os_code: None,
            message: message.into(),
            dest_hash: None,
        }
    }

    fn cancelled() -> Self {
        Self::new(FailureReason::Cancelled, None, "Cancelled")
    }
}

/// Result of [`copy_file_with_retry`]: the outcome plus how many attempts it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyAttempt {
    pub result: Result<CopyOutcome, CopyFailure>,
    pub attempts: u32,
}

/// [`copy_file`], retried after the configured delays while the failure is transient
/// (file in use, network drop).
pub fn copy_file_with_retry(
    source: &Path,
    destination: &Path,
    options: &CopyOptions,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> CopyAttempt {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let result = copy_file(source, destination, options, cancel, on_bytes);
        let retry_delay = match &result {
            Err(failure) if failure.reason.is_transient() => {
                options.retry_delays.get(attempts as usize - 1).copied()
            }
            _ => None,
        };
        let Some(delay) = retry_delay else {
            return CopyAttempt { result, attempts };
        };
        if !sleep_unless_cancelled(delay, cancel) {
            return CopyAttempt {
                result: Err(CopyFailure::cancelled()),
                attempts,
            };
        }
    }
}

fn sleep_unless_cancelled(delay: Duration, cancel: &CancelToken) -> bool {
    let step = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < delay {
        if cancel.is_cancelled() {
            return false;
        }
        let next = step.min(delay - waited);
        std::thread::sleep(next);
        waited += next;
    }
    !cancel.is_cancelled()
}

/// Copies one file. `on_bytes` gets the number of bytes written after each buffer.
pub fn copy_file(
    source: &Path,
    destination: &Path,
    options: &CopyOptions,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<CopyOutcome, CopyFailure> {
    if cancel.is_cancelled() {
        return Err(CopyFailure::cancelled());
    }
    let mut input =
        secure::open_source(source).map_err(|error| CopyFailure::io(&error, Side::Source))?;
    let source_meta = input
        .metadata()
        .map_err(|error| CopyFailure::io(&error, Side::Source))?;
    let source_size = source_meta.len();
    let source_time = source_meta.modified().ok();
    let wanted_meta = meta::read(source).ok();

    match fs::metadata(destination) {
        Ok(existing) if existing.is_dir() => {
            return Err(CopyFailure::new(
                FailureReason::TargetExistsDifferent,
                Some(Side::Destination),
                "A folder with this name is at the destination",
            ));
        }
        Ok(existing) => {
            if options.skip_identical
                && existing.len() == source_size
                && times_match(source_time, existing.modified().ok())
            {
                return Ok(CopyOutcome::SkippedIdentical);
            }
            let source_newer = match (source_time, existing.modified().ok()) {
                (Some(source), Some(dest)) => source > dest + TIME_TOLERANCE,
                _ => false,
            };
            match options.conflict {
                ConflictPolicy::Overwrite => {}
                ConflictPolicy::OverwriteIfNewer if source_newer => {}
                _ => {
                    return Err(CopyFailure::new(
                        FailureReason::TargetExistsDifferent,
                        Some(Side::Destination),
                        "A file with a different size or date is at the destination",
                    ));
                }
            }
        }
        Err(_) => {}
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| CopyFailure::io(&error, Side::Destination))?;
    }
    let part = part_path(destination);
    let result = write_part(
        &mut input,
        &part,
        source_time,
        options.verify == VerifyLevel::Hash,
        cancel,
        on_bytes,
    );
    drop(input);
    let (bytes, hash) = match result {
        Ok(done) => done,
        Err(failure) => {
            let _ = fs::remove_file(&part);
            return Err(failure);
        }
    };
    if bytes != source_size {
        let _ = fs::remove_file(&part);
        return Err(CopyFailure::new(
            FailureReason::VerifyMismatch,
            Some(Side::Source),
            format!(
                "The source changed while it was copied ({source_size} bytes before, {bytes} read)"
            ),
        ));
    }
    // A read-only file that is being replaced would make the rename fail.
    meta::clear_read_only(destination);
    if let Err(error) = fs::rename(&part, destination) {
        let _ = fs::remove_file(&part);
        return Err(CopyFailure::io(&error, Side::Destination));
    }

    verify(destination, bytes, source_time, hash.as_deref())?;
    let warnings = wanted_meta
        .map(|wanted| meta::apply(source, destination, &wanted, &options.preserve))
        .unwrap_or_default();
    Ok(CopyOutcome::Copied {
        bytes,
        hash,
        warnings,
    })
}

/// Longest file name for which `name.dspart` still fits the 255-character limit of one path part.
const MAX_PART_BASE: usize = 255 - PART_SUFFIX.len();

/// `name.ext` → `name.ext.dspart` in the same folder. A name too long for that becomes
/// `~<hash of the name>.dspart`.
pub fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    if name.encode_wide_len() > MAX_PART_BASE {
        let digest = blake3::hash(name.to_string_lossy().as_bytes()).to_hex();
        name = format!("~{}", &digest[..16]).into();
    }
    name.push(PART_SUFFIX);
    destination.with_file_name(name)
}

/// UTF-16 length on Windows (the unit of the 255 limit), bytes elsewhere.
trait NameLen {
    fn encode_wide_len(&self) -> usize;
}

impl NameLen for std::ffi::OsString {
    #[cfg(windows)]
    fn encode_wide_len(&self) -> usize {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().count()
    }

    #[cfg(not(windows))]
    fn encode_wide_len(&self) -> usize {
        self.len()
    }
}

fn write_part(
    input: &mut File,
    part: &Path,
    source_time: Option<SystemTime>,
    hash: bool,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<(u64, Option<String>), CopyFailure> {
    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part)
        .map_err(|error| CopyFailure::io(&error, Side::Destination))?;
    let mut hasher = hash.then(blake3::Hasher::new);
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut total = 0_u64;
    loop {
        if cancel.is_cancelled() {
            return Err(CopyFailure::cancelled());
        }
        let read = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(CopyFailure::io(&error, Side::Source)),
        };
        output
            .write_all(&buffer[..read])
            .map_err(|error| CopyFailure::io(&error, Side::Destination))?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&buffer[..read]);
        }
        total += read as u64;
        on_bytes(read as u64);
    }
    if let Some(time) = source_time {
        output
            .set_modified(time)
            .map_err(|error| CopyFailure::io(&error, Side::Destination))?;
    }
    // Flush to the device before the rename: a USB stick pulled after "Copied" keeps the file.
    output
        .sync_all()
        .map_err(|error| CopyFailure::io(&error, Side::Destination))?;
    Ok((
        total,
        hasher.map(|hasher| hasher.finalize().to_hex().to_string()),
    ))
}

fn verify(
    destination: &Path,
    bytes: u64,
    source_time: Option<SystemTime>,
    source_hash: Option<&str>,
) -> Result<(), CopyFailure> {
    let meta =
        fs::metadata(destination).map_err(|error| CopyFailure::io(&error, Side::Destination))?;
    if meta.len() != bytes {
        return Err(CopyFailure::new(
            FailureReason::VerifyMismatch,
            Some(Side::Destination),
            format!("Destination has {} bytes, expected {bytes}", meta.len()),
        ));
    }
    if !times_match(source_time, meta.modified().ok()) {
        // Some network servers set the time again when the file is closed; set it once more.
        let fixed = source_time.is_some_and(|time| {
            OpenOptions::new()
                .write(true)
                .open(destination)
                .and_then(|file| file.set_modified(time))
                .is_ok()
        });
        let now = fs::metadata(destination)
            .ok()
            .and_then(|m| m.modified().ok());
        if !fixed || !times_match(source_time, now) {
            return Err(CopyFailure::new(
                FailureReason::VerifyMismatch,
                Some(Side::Destination),
                "The destination's modified time does not match the source",
            ));
        }
    }
    if let Some(expected) = source_hash {
        let actual =
            hash_file(destination).map_err(|error| CopyFailure::io(&error, Side::Destination))?;
        if actual != expected {
            let mut failure = CopyFailure::new(
                FailureReason::VerifyMismatch,
                Some(Side::Destination),
                "The destination's content hash does not match the source",
            );
            failure.dest_hash = Some(actual);
            return Err(failure);
        }
    }
    Ok(())
}

/// Hex BLAKE3 of a whole file, read in chunks.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Equal within [`TIME_TOLERANCE`]; a missing time on either side counts as equal.
pub fn times_match(left: Option<SystemTime>, right: Option<SystemTime>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            let difference = left
                .duration_since(right)
                .or_else(|_| right.duration_since(left))
                .unwrap_or_default();
            difference <= TIME_TOLERANCE
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn no_retry() -> CopyOptions {
        CopyOptions {
            retry_delays: Vec::new(),
            ..CopyOptions::default()
        }
    }

    fn copy(
        source: &Path,
        destination: &Path,
        options: &CopyOptions,
    ) -> Result<CopyOutcome, CopyFailure> {
        copy_file(
            source,
            destination,
            options,
            &CancelToken::default(),
            &mut |_| {},
        )
    }

    #[test]
    fn copies_content_and_modified_time_and_leaves_no_part_file() {
        let dir = TempDir::new("copy-basic");
        let source = dir.write("src/a.bin", &vec![7_u8; 3 * BUFFER_SIZE + 5]);
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let destination = dir.path("dst/nested/a.bin");
        let mut progress = 0;

        let outcome = copy_file(
            &source,
            &destination,
            &no_retry(),
            &CancelToken::default(),
            &mut |bytes| progress += bytes,
        )
        .expect("copy should succeed");

        assert_eq!(
            outcome,
            CopyOutcome::Copied {
                bytes: 3 * BUFFER_SIZE as u64 + 5,
                hash: None,
                warnings: Vec::new()
            }
        );
        assert_eq!(progress, 3 * BUFFER_SIZE as u64 + 5);
        assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
        assert_eq!(fs::metadata(&destination).unwrap().modified().unwrap(), old);
        assert!(!part_path(&destination).exists());
    }

    #[test]
    fn hash_mode_returns_the_blake3_of_the_content() {
        let dir = TempDir::new("copy-hash");
        let source = dir.write("a.txt", b"hello");
        let options = CopyOptions {
            verify: VerifyLevel::Hash,
            ..no_retry()
        };

        let outcome = copy(&source, &dir.path("out/a.txt"), &options).unwrap();

        assert_eq!(
            outcome,
            CopyOutcome::Copied {
                bytes: 5,
                hash: Some(blake3::hash(b"hello").to_hex().to_string()),
                warnings: Vec::new()
            }
        );
    }

    #[test]
    fn a_name_too_long_for_the_part_suffix_gets_a_short_temp_name() {
        let long = "a".repeat(250);
        let part = part_path(Path::new("out").join(&long).as_path());
        let name = part.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(PART_SUFFIX) && name.len() <= 255, "{name}");
        let normal = part_path(Path::new("out/a.txt"));
        assert_eq!(normal, Path::new("out/a.txt.dspart"));
    }

    #[test]
    fn a_255_character_name_copies() {
        let dir = TempDir::new("copy-longname");
        let name = format!("{}.txt", "n".repeat(251));
        let source = dir.write(&name, b"long");
        let destination = dir.path("out").join(&name);

        let outcome = copy(&source, &destination, &no_retry()).unwrap();

        assert!(matches!(outcome, CopyOutcome::Copied { bytes: 4, .. }));
        assert_eq!(fs::read(&destination).unwrap(), b"long");
    }

    #[test]
    fn identical_destination_is_skipped_and_a_different_one_is_flagged() {
        let dir = TempDir::new("copy-conflict");
        let source = dir.write("a.txt", b"source");
        let destination = dir.path("out/a.txt");
        copy(&source, &destination, &no_retry()).unwrap();

        assert_eq!(
            copy(&source, &destination, &no_retry()).unwrap(),
            CopyOutcome::SkippedIdentical
        );

        fs::write(&destination, b"changed at the destination").unwrap();
        let failure = copy(&source, &destination, &no_retry()).unwrap_err();
        assert_eq!(failure.reason, FailureReason::TargetExistsDifferent);
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"changed at the destination"
        );

        let overwrite = CopyOptions {
            conflict: ConflictPolicy::Overwrite,
            ..no_retry()
        };
        assert!(matches!(
            copy(&source, &destination, &overwrite),
            Ok(CopyOutcome::Copied { .. })
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"source");
    }

    #[test]
    fn overwrite_if_newer_replaces_only_older_destinations() {
        let dir = TempDir::new("copy-newer");
        let source = dir.write("a.txt", b"new content");
        let destination = dir.write("out/a.txt", b"old");
        let set_time = |path: &Path, secs: u64| {
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        let options = CopyOptions {
            conflict: ConflictPolicy::OverwriteIfNewer,
            ..no_retry()
        };

        set_time(&source, 1_600_000_000);
        set_time(&destination, 1_700_000_000);
        assert_eq!(
            copy(&source, &destination, &options).unwrap_err().reason,
            FailureReason::TargetExistsDifferent
        );

        set_time(&destination, 1_500_000_000);
        assert!(matches!(
            copy(&source, &destination, &options),
            Ok(CopyOutcome::Copied { .. })
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"new content");
    }

    #[test]
    fn missing_source_is_a_vanished_source() {
        let dir = TempDir::new("copy-missing");
        let failure = copy(
            &dir.path("nope.txt"),
            &dir.path("out/nope.txt"),
            &no_retry(),
        )
        .unwrap_err();
        assert_eq!(failure.reason, FailureReason::SourceVanished);
        assert_eq!(failure.side, Some(Side::Source));
    }

    #[test]
    fn cancelling_mid_file_removes_the_part_file() {
        let dir = TempDir::new("copy-cancel");
        let source = dir.write("big.bin", &vec![1_u8; 4 * BUFFER_SIZE]);
        let destination = dir.path("out/big.bin");
        let token = CancelToken::default();

        let failure = copy_file(&source, &destination, &no_retry(), &token, &mut |_| {
            token.cancel()
        })
        .unwrap_err();

        assert_eq!(failure.reason, FailureReason::Cancelled);
        assert!(!destination.exists());
        assert!(!part_path(&destination).exists());
    }

    #[test]
    fn a_path_longer_than_260_characters_copies() {
        let dir = TempDir::new("copy-long");
        let segment = "a-fairly-long-folder-name-for-testing-long-paths";
        let relative: PathBuf = std::iter::repeat_n(segment, 6)
            .collect::<PathBuf>()
            .join("file.txt");
        let source = dir.write("src/x.txt", b"long");
        let destination = dir.path("dst").join(&relative);
        assert!(destination.as_os_str().len() > 260);

        copy(&source, &destination, &no_retry()).expect("long path should copy");

        assert_eq!(fs::read(&destination).unwrap(), b"long");
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_source_is_retried_and_reported_as_in_use() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = TempDir::new("copy-locked");
        let source = dir.write("locked.txt", b"data");
        let destination = dir.path("out/locked.txt");
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&source)
            .unwrap();
        let options = CopyOptions {
            retry_delays: vec![Duration::ZERO, Duration::ZERO],
            ..CopyOptions::default()
        };

        let attempt = copy_file_with_retry(
            &source,
            &destination,
            &options,
            &CancelToken::default(),
            &mut |_| {},
        );
        assert_eq!(attempt.attempts, 3);
        assert_eq!(
            attempt.result.unwrap_err().reason,
            FailureReason::FileLocked
        );

        drop(lock);
        let attempt = copy_file_with_retry(
            &source,
            &destination,
            &options,
            &CancelToken::default(),
            &mut |_| {},
        );
        assert_eq!(attempt.attempts, 1);
        assert!(attempt.result.is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn a_read_only_destination_folder_is_access_denied() {
        let dir = TempDir::new("copy-denied");
        let source = dir.write("a.txt", b"data");
        let target = dir.path("readonly");
        fs::create_dir_all(&target).unwrap();
        let _guard = crate::test_support::DenyGuard::new(&target, "(W)");

        let failure = copy(&source, &target.join("a.txt"), &no_retry()).unwrap_err();

        assert_eq!(failure.reason, FailureReason::AccessDenied);
        assert_eq!(failure.side, Some(Side::Destination));
    }

    #[test]
    fn times_within_two_seconds_match() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(times_match(
            Some(base),
            Some(base + Duration::from_millis(1999))
        ));
        assert!(!times_match(
            Some(base),
            Some(base + Duration::from_millis(2001))
        ));
        assert!(times_match(None, Some(base)));
    }
}

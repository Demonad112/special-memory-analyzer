//! BLAKE3 content hashes: the whole file (Record's verify, the duplicate finder's last step) and
//! a cheap head-and-tail hash that rules out most same-size files before a full read.
//!
//! Callers never pass an online-only (OneDrive) file or a link: reading the first would download
//! it. The scan tree marks both ([`crate::Node::cloud_files`], [`crate::Kind::Link`]).

use crate::CancelToken;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const BUFFER_SIZE: usize = 1024 * 1024;
/// How much of each end [`hash_head_tail`] reads for the duplicate finder.
pub const HEAD_TAIL_BYTES: u64 = 64 * 1024;

/// Hex BLAKE3 of a whole file, read in chunks. Cancelling stops it between chunks with an
/// `Interrupted` error.
pub fn hash_file(path: &Path, cancel: &CancelToken) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        if cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
        }
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

/// Hex BLAKE3 of the file's length, its first `chunk` bytes and its last `chunk` bytes. A file
/// of at most `2 * chunk` bytes is hashed whole after the length. Equal results mean "maybe the
/// same content"; only [`hash_file`] decides.
pub fn hash_head_tail(path: &Path, chunk: u64) -> io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&length.to_le_bytes());
    if length <= chunk.saturating_mul(2) {
        read_into(&mut file, length, &mut hasher)?;
    } else {
        read_into(&mut file, chunk, &mut hasher)?;
        file.seek(SeekFrom::Start(length - chunk))?;
        read_into(&mut file, chunk, &mut hasher)?;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Feeds exactly `count` bytes from the current position into `hasher`. A file that got shorter
/// since its length was read is an `UnexpectedEof` error.
fn read_into(file: &mut File, count: u64, hasher: &mut blake3::Hasher) -> io::Result<()> {
    let mut taken = file.take(count);
    let copied = io::copy(&mut taken, hasher)?;
    if copied != count {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "The file changed while it was read",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn file(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn whole_file_hash_matches_blake3_and_stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = vec![7_u8; 3 * BUFFER_SIZE];
        let path = file(&dir, "big.bin", &bytes);

        let cancel = CancelToken::default();
        let whole = hash_file(&path, &cancel).unwrap();
        assert_eq!(whole, blake3::hash(&bytes).to_hex().to_string());

        cancel.cancel();
        let stopped = hash_file(&path, &cancel).unwrap_err();
        assert_eq!(stopped.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn head_and_tail_see_both_ends_but_not_the_middle() {
        let dir = tempfile::tempdir().unwrap();
        let chunk = 16;
        let base: Vec<u8> = (0..100).collect();
        let mut middle = base.clone();
        middle[50] ^= 0xFF;
        let mut head = base.clone();
        head[0] ^= 0xFF;
        let mut tail = base.clone();
        tail[99] ^= 0xFF;

        let of =
            |name: &str, bytes: &[u8]| hash_head_tail(&file(&dir, name, bytes), chunk).unwrap();
        let original = of("base", &base);
        assert_eq!(original, of("copy", &base));
        assert_eq!(original, of("middle", &middle), "the middle is not read");
        assert_ne!(original, of("head", &head));
        assert_ne!(original, of("tail", &tail));
    }

    #[test]
    fn head_and_tail_include_the_length() {
        let dir = tempfile::tempdir().unwrap();
        // Same first and last 4 bytes, different lengths.
        let short = hash_head_tail(&file(&dir, "short", b"abcdXXwxyz"), 4).unwrap();
        let long = hash_head_tail(&file(&dir, "long", b"abcdXXXXwxyz"), 4).unwrap();
        assert_ne!(short, long);
    }

    #[test]
    fn small_files_are_hashed_whole() {
        let dir = tempfile::tempdir().unwrap();
        let a = hash_head_tail(&file(&dir, "a", b"0123456789"), 5).unwrap();
        let b = hash_head_tail(&file(&dir, "b", b"0123496789"), 5).unwrap();
        assert_ne!(a, b, "a 10-byte file with 5-byte ends is read in full");
        let empty = hash_head_tail(&file(&dir, "empty", b""), HEAD_TAIL_BYTES).unwrap();
        assert_eq!(empty.len(), 64);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(hash_head_tail(&missing, HEAD_TAIL_BYTES).is_err());
        assert!(hash_file(&missing, &CancelToken::default()).is_err());
    }
}

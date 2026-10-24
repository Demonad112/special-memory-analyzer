//! The company logo on exported reports.
//!
//! The user picks a PNG or JPEG in Settings; Rust checks it and keeps its own copy at
//! `Reports\branding\logo.png` (or `.jpg`). Exports read that copy, so the webview never hands a
//! path to an export. SVG is refused: it can carry script. The size cap keeps every exported
//! report small.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Largest logo accepted: 256 KB.
pub const MAX_LOGO_BYTES: usize = 256 * 1024;

const FOLDER: &str = "branding";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Start of image, then the first marker's lead byte.
const JPEG_SIGNATURE: &[u8] = b"\xff\xd8\xff";

/// `FILE_ATTRIBUTE_DIRECTORY`.
const ATTR_DIRECTORY: u32 = 0x10;
/// Offline, recall-on-open and recall-on-data-access: the data is in the cloud. The same bits
/// the scan uses (`scan_core::list`), which keeps its helper private.
const ATTR_CLOUD: u32 = 0x1000 | 0x4_0000 | 0x40_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogoFormat {
    Png,
    Jpeg,
}

impl LogoFormat {
    pub fn mime(self) -> &'static str {
        match self {
            LogoFormat::Png => "image/png",
            LogoFormat::Jpeg => "image/jpeg",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            LogoFormat::Png => "png",
            LogoFormat::Jpeg => "jpg",
        }
    }
}

/// A checked logo, ready to embed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logo {
    pub format: LogoFormat,
    pub bytes: Vec<u8>,
}

impl Logo {
    /// `data:image/png;base64,...`: the report CSP allows `img-src data:` and nothing else.
    pub fn data_uri(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.format.mime(),
            STANDARD.encode(&self.bytes)
        )
    }
}

/// The format the bytes start with, by signature only (the file name does not count).
pub fn sniff(bytes: &[u8]) -> Option<LogoFormat> {
    if bytes.starts_with(PNG_SIGNATURE) {
        Some(LogoFormat::Png)
    } else if bytes.starts_with(JPEG_SIGNATURE) {
        Some(LogoFormat::Jpeg)
    } else {
        None
    }
}

/// Checks the size and signature of a logo.
pub fn check(bytes: &[u8]) -> Result<LogoFormat, String> {
    if bytes.len() > MAX_LOGO_BYTES {
        return Err(format!(
            "The logo is {}. Pick a PNG or JPEG of at most 256 KB.",
            crate::format_bytes(bytes.len() as u64)
        ));
    }
    sniff(bytes).ok_or_else(|| "That file is not a PNG or JPEG image. Pick a .png or .jpg.".into())
}

/// Refuses a source that is a folder, a link or an online-only cloud file (never opened, so
/// OneDrive does not download it). `attributes` are the Windows file attributes; `is_link` is a
/// name-surrogate reparse point (symlink, junction). A OneDrive file kept on this PC is also a
/// reparse point, but not a link, and is fine.
pub fn check_source(attributes: u32, is_link: bool) -> Result<(), String> {
    if attributes & ATTR_DIRECTORY != 0 {
        return Err("That is a folder. Pick a PNG or JPEG file.".into());
    }
    if attributes & ATTR_CLOUD != 0 {
        return Err(
            "That file is online-only in OneDrive or another cloud folder. Make it available on this PC first, or pick another copy."
                .into(),
        );
    }
    if is_link {
        return Err("That is a link, not a file. Pick the image itself.".into());
    }
    Ok(())
}

#[cfg(windows)]
fn attributes(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn attributes(meta: &fs::Metadata) -> u32 {
    if meta.is_dir() {
        ATTR_DIRECTORY
    } else {
        0
    }
}

/// Reads the picked file, never past the cap, after the attribute checks. The path itself is
/// not followed if it is a link.
pub fn read_source(path: &Path) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|err| format!("Can't read that file: {err}"))?;
    // On Windows `is_symlink` is true for name-surrogate reparse points only.
    check_source(attributes(&meta), meta.file_type().is_symlink())?;
    if !meta.is_file() {
        return Err("Pick a PNG or JPEG file.".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_LOGO_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|err| format!("Can't read that file: {err}"))?;
    Ok(bytes)
}

fn folder(reports_root: &Path) -> PathBuf {
    reports_root.join(FOLDER)
}

fn stored_path(reports_root: &Path, format: LogoFormat) -> PathBuf {
    folder(reports_root).join(format!("logo.{}", format.extension()))
}

/// Checks `bytes` and saves them as the logo, replacing one of the other format.
pub fn store(reports_root: &Path, bytes: Vec<u8>) -> Result<Logo, String> {
    let format = check(&bytes)?;
    fs::create_dir_all(folder(reports_root))
        .map_err(|err| format!("Could not save the logo: {err}"))?;
    transfer_core::store::write_atomic(&stored_path(reports_root, format), &bytes)
        .map_err(|err| format!("Could not save the logo: {err}"))?;
    for other in [LogoFormat::Png, LogoFormat::Jpeg] {
        if other != format {
            remove(&stored_path(reports_root, other))?;
        }
    }
    Ok(Logo { format, bytes })
}

/// Reads, checks and saves the logo the user picked.
pub fn set_from(reports_root: &Path, source: &Path) -> Result<Logo, String> {
    store(reports_root, read_source(source)?)
}

/// Removes the logo; reports go back to the name alone.
pub fn clear(reports_root: &Path) -> Result<(), String> {
    for format in [LogoFormat::Png, LogoFormat::Jpeg] {
        remove(&stored_path(reports_root, format))?;
    }
    Ok(())
}

fn remove(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("Could not remove the old logo: {err}"))
        }
        _ => Ok(()),
    }
}

/// The saved logo, checked again (a file swapped in by hand is ignored), or `None`.
pub fn load(reports_root: &Path) -> Option<Logo> {
    [LogoFormat::Png, LogoFormat::Jpeg]
        .into_iter()
        .find_map(|format| {
            let bytes = read_source(&stored_path(reports_root, format)).ok()?;
            (check(&bytes).ok()? == format).then_some(Logo { format, bytes })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(b"\0\0\0\rIHDR rest of a tiny image");
        bytes
    }

    fn jpeg() -> Vec<u8> {
        let mut bytes = b"\xff\xd8\xff\xe0\0\x10JFIF\0".to_vec();
        bytes.extend_from_slice(&[0; 32]);
        bytes
    }

    #[test]
    fn a_png_and_a_jpeg_are_accepted_by_signature() {
        assert_eq!(check(&png()), Ok(LogoFormat::Png));
        assert_eq!(check(&jpeg()), Ok(LogoFormat::Jpeg));
        let exact = [png(), vec![0; MAX_LOGO_BYTES - png().len()]].concat();
        assert_eq!(check(&exact), Ok(LogoFormat::Png), "256 KB exactly is fine");
    }

    #[test]
    fn svg_text_and_oversized_files_are_refused() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
        assert!(check(svg).is_err());
        assert!(check(b"<?xml version=\"1.0\"?><svg/>").is_err());
        // A text file renamed logo.png: the name does not count.
        assert!(check(b"just some text, renamed to logo.png").is_err());
        assert!(check(b"").is_err());
        assert!(check(&PNG_SIGNATURE[..7]).is_err(), "a cut signature");
        let big = [png(), vec![0; 300 * 1024]].concat();
        let err = check(&big).unwrap_err();
        assert!(err.contains("256 KB"), "{err}");
    }

    #[test]
    fn folders_links_and_online_only_files_are_refused_before_reading() {
        const REPARSE: u32 = 0x400;
        assert!(check_source(0x20, false).is_ok(), "archive bit only");
        // A OneDrive file kept on this PC: a reparse point, not a link, not online-only.
        assert!(check_source(0x20 | REPARSE, false).is_ok());
        assert!(check_source(ATTR_DIRECTORY, false).is_err());
        let err = check_source(REPARSE, true).unwrap_err();
        assert!(err.contains("link"), "{err}");
        for bit in [0x1000, 0x4_0000, 0x40_0000] {
            let err = check_source(bit | REPARSE, false).unwrap_err();
            assert!(err.contains("online-only"), "{bit:#x}: {err}");
        }
    }

    #[test]
    fn a_picked_file_is_copied_and_the_other_format_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path().join("Reports");
        let picked = temp.path().join("acme.png");
        fs::write(&picked, png()).unwrap();
        let logo = set_from(&reports, &picked).unwrap();
        assert_eq!(logo.format, LogoFormat::Png);
        assert_eq!(fs::read(reports.join("branding/logo.png")).unwrap(), png());
        // The export reads Rust's copy: the picked file can go.
        fs::remove_file(&picked).unwrap();
        assert_eq!(load(&reports), Some(logo));

        let picked = temp.path().join("acme.jpeg");
        fs::write(&picked, jpeg()).unwrap();
        set_from(&reports, &picked).unwrap();
        assert!(!reports.join("branding/logo.png").exists());
        assert_eq!(load(&reports).unwrap().format, LogoFormat::Jpeg);

        clear(&reports).unwrap();
        assert_eq!(load(&reports), None);
        clear(&reports).unwrap();
    }

    #[test]
    fn a_refused_file_leaves_the_saved_logo_alone() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path().join("Reports");
        store(&reports, png()).unwrap();
        let svg = temp.path().join("logo.svg");
        fs::write(&svg, "<svg/>").unwrap();
        assert!(set_from(&reports, &svg).is_err());
        let big = temp.path().join("big.png");
        fs::write(&big, [png(), vec![0; 300 * 1024]].concat()).unwrap();
        assert!(set_from(&reports, &big).is_err());
        assert!(set_from(&reports, temp.path()).is_err(), "a folder");
        assert!(set_from(&reports, &temp.path().join("gone.png")).is_err());
        assert_eq!(load(&reports).unwrap().bytes, png());
    }

    #[test]
    fn a_saved_file_swapped_by_hand_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path().join("Reports");
        fs::create_dir_all(reports.join(FOLDER)).unwrap();
        fs::write(reports.join("branding/logo.png"), "<svg/>").unwrap();
        assert_eq!(load(&reports), None);
        // A JPEG under the PNG name is not the PNG logo either.
        fs::write(reports.join("branding/logo.png"), jpeg()).unwrap();
        assert_eq!(load(&reports), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_an_image_is_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real.png");
        fs::write(&real, png()).unwrap();
        let link = temp.path().join("link.png");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = read_source(&link).unwrap_err();
        assert!(err.contains("link"), "{err}");
    }

    #[test]
    fn the_data_uri_is_base64_with_the_right_type() {
        let logo = Logo {
            format: LogoFormat::Jpeg,
            bytes: vec![0xff, 0xd8, 0xff],
        };
        assert_eq!(logo.data_uri(), "data:image/jpeg;base64,/9j/");
    }
}

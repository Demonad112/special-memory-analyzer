//! Path checks every delete and restore shares: how paths are compared and what counts as a link.

use std::fs;
use std::path::Path;

/// Lowercase, backslashes, no trailing backslash, no `\\?\` prefix.
pub fn normalize(path: &str) -> String {
    let path = path.trim().replace('/', "\\").to_lowercase();
    if let Some(rest) = path.strip_prefix("\\\\?\\unc\\") {
        return format!("\\\\{}", rest.trim_end_matches('\\'));
    }
    let path = path.strip_prefix("\\\\?\\").unwrap_or(&path);
    path.trim_end_matches('\\').to_string()
}

/// True when a folder between `scan_root` (exclusive) and `path` (exclusive) is a link now. The
/// scan saw plain folders; a swapped parent would send the delete through to another place.
pub fn has_reparse_ancestor(path: &Path, scan_root: &Path) -> bool {
    // Compare depths, not prefixes, so a differently cased root still stops the walk.
    let root_depth = scan_root.components().count();
    path.ancestors()
        .skip(1)
        .take_while(|ancestor| ancestor.components().count() > root_depth)
        .any(is_reparse_point)
}

/// True for `\\server\share\...` and `\\?\UNC\server\share\...`. Device paths (`\\?\C:\`,
/// `\\.\`) are not network paths.
pub fn is_unc(path: &str) -> bool {
    let path = path.trim().replace('/', "\\");
    const VERBATIM_UNC: &str = "\\\\?\\UNC\\";
    if path
        .get(..VERBATIM_UNC.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(VERBATIM_UNC))
    {
        return path.len() > VERBATIM_UNC.len();
    }
    path.starts_with("\\\\") && !path.starts_with("\\\\?\\") && !path.starts_with("\\\\.\\")
}

/// The drive letter of `C:\x` or `\\?\C:\x`, upper-cased.
pub fn drive_letter(path: &str) -> Option<char> {
    let path = path.trim();
    let path = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    (chars.next() == Some(':')).then(|| letter.to_ascii_uppercase())
}

/// `GetDriveTypeW` answer for a network drive (`DRIVE_REMOTE`). A `subst` or local drive is not.
pub fn is_remote_drive_type(drive_type: u32) -> bool {
    drive_type == 4
}

/// True when `path` is on the network: a UNC path, or a drive letter that `drive_type` reports
/// as remote. `drive_type` is `GetDriveTypeW` on Windows; tests pass a fake.
pub fn is_network_path_with(path: &str, drive_type: &dyn Fn(char) -> Option<u32>) -> bool {
    if is_unc(path) {
        return true;
    }
    drive_letter(path)
        .and_then(drive_type)
        .is_some_and(is_remote_drive_type)
}

/// The real [`is_network_path_with`]: asks Windows what kind of drive the letter is.
pub fn is_network_path(path: &Path) -> bool {
    is_network_path_with(&path.to_string_lossy(), &|letter| drive_type_of(letter))
}

#[cfg(windows)]
fn drive_type_of(letter: char) -> Option<u32> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
    // SAFETY: `root` is a null-terminated drive root such as "Z:\".
    Some(unsafe { GetDriveTypeW(root.as_ptr()) })
}

#[cfg(not(windows))]
fn drive_type_of(_letter: char) -> Option<u32> {
    None
}

/// Why a delete on the network is refused.
pub const NETWORK_REFUSAL: &str = "Deleting on a network share or mapped network drive is not available: the Recycle Bin does not cover network paths, so every delete would be permanent. Scan and reports still work.";

/// True for a symlink or junction (a Windows reparse point).
pub fn is_reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        fs::symlink_metadata(path)
            .is_ok_and(|meta| meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unc_paths_in_every_spelling_are_network_paths() {
        for path in [
            r"\\server\share\x",
            r"\\SERVER\C$\Users",
            r"\\?\UNC\server\share\x",
            r"\\?\unc\server\share",
            "//server/share/x",
            r"  \\server\share  ",
        ] {
            assert!(is_unc(path), "{path}");
        }
        for path in [
            r"C:\x",
            r"\\?\C:\x",
            r"\\.\PhysicalDrive0",
            r"\\?\UNC\",
            r"\\?\Volume{1}\x",
            "relative\\x",
            "",
        ] {
            assert!(!is_unc(path), "{path}");
        }
    }

    #[test]
    fn a_drive_letter_is_read_from_plain_and_verbatim_paths() {
        assert_eq!(drive_letter(r"z:\data"), Some('Z'));
        assert_eq!(drive_letter(r"\\?\Z:\data"), Some('Z'));
        assert_eq!(drive_letter("Z:"), Some('Z'));
        assert_eq!(drive_letter(r"\\server\share"), None);
        assert_eq!(drive_letter("1:\\x"), None);
        assert_eq!(drive_letter(""), None);
    }

    #[test]
    fn only_a_remote_drive_type_makes_a_drive_letter_a_network_path() {
        // DRIVE_FIXED is 3 (a local disk, or a `subst` of a local folder), DRIVE_REMOTE is 4.
        let types = |letter: char| match letter {
            'C' | 'S' => Some(3),
            'Z' => Some(4),
            'E' => Some(5),
            _ => None,
        };
        assert!(is_network_path_with(r"Z:\Data", &types));
        assert!(is_network_path_with(r"\\?\Z:\Data", &types));
        assert!(!is_network_path_with(r"C:\Data", &types));
        assert!(!is_network_path_with(r"S:\subst", &types));
        assert!(!is_network_path_with(r"E:\disc", &types));
        assert!(!is_network_path_with(r"Q:\unknown", &types));
        assert!(is_network_path_with(r"\\server\share\x", &types));
        assert!(!is_network_path_with("relative", &types));
    }

    #[test]
    fn a_plain_parent_chain_is_not_a_reparse_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/file.bin"), b"x").unwrap();
        assert!(!has_reparse_ancestor(&root.join("a/b/file.bin"), &root));
        assert!(!has_reparse_ancestor(&root.join("a"), &root));
    }

    /// A directory link: a junction on Windows (`mklink /J`, no privilege needed), a symlink elsewhere.
    fn dir_link(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(made.status.success(), "mklink /J failed");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn normalize_folds_case_slashes_prefixes_and_trailing_separators() {
        assert_eq!(normalize(r"C:\Users\Sam\"), r"c:\users\sam");
        assert_eq!(normalize("C:/Users/Sam"), r"c:\users\sam");
        assert_eq!(normalize(r"\\?\C:\Users"), r"c:\users");
        assert_eq!(normalize(r"\\?\UNC\Server\Share\"), r"\\server\share");
        assert_eq!(normalize("  C:\\Temp  "), r"c:\temp");
    }

    #[test]
    fn a_link_is_a_reparse_point_and_a_plain_folder_or_missing_path_is_not() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let link = temp.path().join("link");
        dir_link(&link, &real);
        assert!(is_reparse_point(&link));
        assert!(!is_reparse_point(&real));
        assert!(!is_reparse_point(&temp.path().join("missing")));
    }

    #[test]
    fn a_link_above_the_path_is_a_reparse_ancestor_but_the_root_itself_is_not_checked() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let other = temp.path().join("other");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(other.join("deep")).unwrap();
        fs::write(other.join("deep/file.bin"), b"x").unwrap();
        dir_link(&root.join("swapped"), &other);
        assert!(has_reparse_ancestor(
            &root.join("swapped/deep/file.bin"),
            &root
        ));
        // The link itself is not its own ancestor: callers test it with `is_reparse_point`.
        assert!(!has_reparse_ancestor(&root.join("swapped"), &root));
        // A link at or above the scan root is outside the walk.
        assert!(!has_reparse_ancestor(
            &root.join("swapped/deep/file.bin"),
            &root.join("swapped")
        ));
    }
}

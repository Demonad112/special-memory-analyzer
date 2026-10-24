//! Path checks every delete and restore shares: how paths are compared and what counts as a link.

use std::fs;
use std::path::Path;

/// Lowercase, backslashes, no trailing backslash, no `\\?\` prefix.
pub(crate) fn normalize(path: &str) -> String {
    let path = path.trim().replace('/', "\\").to_lowercase();
    if let Some(rest) = path.strip_prefix("\\\\?\\unc\\") {
        return format!("\\\\{}", rest.trim_end_matches('\\'));
    }
    let path = path.strip_prefix("\\\\?\\").unwrap_or(&path);
    path.trim_end_matches('\\').to_string()
}

/// True when a folder between `scan_root` (exclusive) and `path` (exclusive) is a link now. The
/// scan saw plain folders; a swapped parent would send the delete through to another place.
pub(crate) fn has_reparse_ancestor(path: &Path, scan_root: &Path) -> bool {
    // Compare depths, not prefixes, so a differently cased root still stops the walk.
    let root_depth = scan_root.components().count();
    path.ancestors()
        .skip(1)
        .take_while(|ancestor| ancestor.components().count() > root_depth)
        .any(is_reparse_point)
}

/// True for a symlink or junction (a Windows reparse point).
pub(crate) fn is_reparse_point(path: &Path) -> bool {
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

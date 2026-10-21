use std::path::{Path, PathBuf};

/// A file with this name next to the exe turns on portable mode.
pub const PORTABLE_MARKER: &str = "portable.txt";

/// `<folder>\Data` when `folder` holds the portable marker.
pub fn data_dir_in(folder: &Path) -> Option<PathBuf> {
    folder
        .join(PORTABLE_MARKER)
        .is_file()
        .then(|| folder.join("Data"))
}

/// The portable data folder next to the running exe, if portable mode is on.
///
/// Records and reports go there instead of `%LOCALAPPDATA%\Deep Server Analyzer`, so the folder can
/// travel on a USB stick.
pub fn portable_data_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    data_dir_in(exe.parent()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_the_marker() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(data_dir_in(folder.path()), None);
        std::fs::write(folder.path().join(PORTABLE_MARKER), "").unwrap();
        assert_eq!(data_dir_in(folder.path()), Some(folder.path().join("Data")));
    }

    #[test]
    fn a_marker_folder_does_not_count() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join(PORTABLE_MARKER)).unwrap();
        assert_eq!(data_dir_in(folder.path()), None);
    }
}

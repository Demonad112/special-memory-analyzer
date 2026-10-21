use std::fs;
use std::path::{Path, PathBuf};

/// A path as typed or pasted: trims spaces and the quotes Explorer's "Copy as path" adds.
pub fn clean_path(input: &str) -> PathBuf {
    let trimmed = input.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(trimmed);
    PathBuf::from(unquoted.trim())
}

/// Rejects a source and destination that are the same folder or contain each other: the outer
/// scan would count the inner folder too and the result would mean nothing. Paths that don't
/// resolve are let through so the scan reports them properly.
pub fn check_pair(left: &Path, right: &Path) -> Result<(), String> {
    let (Ok(left), Ok(right)) = (fs::canonicalize(left), fs::canonicalize(right)) else {
        return Ok(());
    };
    if left == right {
        return Err(
            "Source and destination are the same folder. Pick two different folders.".into(),
        );
    }
    if right.starts_with(&left) {
        return Err(
            "The destination is inside the source, so the source would count it too. Pick folders that don't contain each other."
                .into(),
        );
    }
    if left.starts_with(&right) {
        return Err(
            "The source is inside the destination, so the destination would count it too. Pick folders that don't contain each other."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_spaces_and_copy_as_path_quotes() {
        assert_eq!(clean_path("  C:\\Data  "), PathBuf::from("C:\\Data"));
        assert_eq!(clean_path("\"C:\\My Data\""), PathBuf::from("C:\\My Data"));
        assert_eq!(clean_path("\"unbalanced"), PathBuf::from("\"unbalanced"));
    }

    #[test]
    fn rejects_same_and_nested_folders() {
        let temp = tempfile::tempdir().unwrap();
        let inner = temp.path().join("inner");
        let other = temp.path().join("other");
        fs::create_dir_all(&inner).unwrap();
        fs::create_dir_all(&other).unwrap();

        assert!(check_pair(temp.path(), temp.path())
            .unwrap_err()
            .contains("same folder"));
        assert!(check_pair(temp.path(), &inner)
            .unwrap_err()
            .contains("destination is inside"));
        assert!(check_pair(&inner, temp.path())
            .unwrap_err()
            .contains("source is inside"));
        assert!(check_pair(&inner, &other).is_ok());
        assert!(check_pair(&inner, &temp.path().join("missing")).is_ok());
    }
}

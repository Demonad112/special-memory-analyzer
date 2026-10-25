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

/// The folder to delete in (or to check against the never-delete list), in one canonical form: symlinks, 8.3 short names and `subst`
/// drives resolved, the `\\?\` prefix gone. Refuses roots that reach the local machine by another
/// name (`\\localhost\c$`, `\\?\GLOBALROOT`, `\\?\Volume{...}`), which a path-prefix check
/// against `C:\Windows` would never match. A path that does not resolve is an error, so callers
/// refuse to delete rather than guess.
pub fn canonical_root(path: &Path) -> Result<PathBuf, String> {
    canonical(path, Aliases::Refuse)
}

/// [`canonical_root`] for a folder that is only read (a scan, a readiness check). It still
/// refuses device paths, but accepts this PC's own admin shares (`\\localhost\C$\Users`): they
/// are network paths, so Disk Cleanup can never delete through them (`bulk::run` refuses), and
/// the never-delete list is not what protects them.
pub fn canonical_scan_root(path: &Path) -> Result<PathBuf, String> {
    canonical(path, Aliases::AllowLocalShares)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Aliases {
    Refuse,
    AllowLocalShares,
}

fn canonical(path: &Path, aliases: Aliases) -> Result<PathBuf, String> {
    let computer = std::env::var("COMPUTERNAME").ok();
    refuse_alias(&path.to_string_lossy(), computer.as_deref(), aliases)?;
    let resolved = fs::canonicalize(path)
        .map_err(|err| format!("Can't resolve {}: {err}", path.to_string_lossy()))?;
    let text = resolved.to_string_lossy();
    refuse_alias(&text, computer.as_deref(), aliases)?;
    Ok(PathBuf::from(strip_verbatim(&text)))
}

/// `\\?\C:\x` becomes `C:\x`; `\\?\UNC\host\share\x` becomes `\\host\share\x`.
pub fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(r"UNC\") {
            return format!(r"\\{}", &rest[4..]);
        }
        return rest.to_string();
    }
    path.to_string()
}

const REFUSED_ROOT: &str = "This folder is reached through a name that hides where it really is (a local network share, a volume GUID or a device path). Pick it by its drive letter, for example C:\\Users.";

/// Rejects spellings of the local disk that do not match a protected folder by name.
fn refuse_alias(path: &str, computer: Option<&str>, aliases: Aliases) -> Result<(), String> {
    let lower = path.trim().replace('/', "\\").to_lowercase();
    if lower.starts_with(r"\\?\globalroot") || lower.starts_with(r"\\?\volume{") {
        return Err(REFUSED_ROOT.into());
    }
    let unc = lower.strip_prefix(r"\\?\unc\").or_else(|| {
        lower
            .strip_prefix(r"\\")
            .filter(|rest| !rest.starts_with(['?']))
    });
    let Some(unc) = unc else {
        return Ok(());
    };
    if aliases == Aliases::AllowLocalShares {
        return Ok(());
    }
    let host = unc.split('\\').next().unwrap_or_default();
    let own = computer.map(str::to_lowercase);
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]" | ".")
        || host.ends_with(".ipv6-literal.net") && host.starts_with("0-0-0-0-0-0-0-1")
        || own.as_deref() == Some(host);
    if local {
        return Err(REFUSED_ROOT.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_admin_share_is_refused_as_a_root_but_not_as_a_scan_root() {
        for path in [
            r"\\localhost\c$\Windows",
            r"\\LOCALHOST\C$\Users\Public",
            r"\\127.0.0.1\c$",
            r"\\?\UNC\localhost\c$\x",
            r"\\MYPC\c$\x",
        ] {
            let err = refuse_alias(path, Some("MYPC"), Aliases::Refuse).unwrap_err();
            assert!(err.contains("hides where it really is"), "{path}");
            assert!(
                refuse_alias(path, Some("MYPC"), Aliases::AllowLocalShares).is_ok(),
                "{path}"
            );
        }
        // Another server is fine either way.
        assert!(refuse_alias(r"\\fs01\C$\x", Some("MYPC"), Aliases::Refuse).is_ok());
    }

    #[test]
    fn device_paths_are_refused_even_for_a_scan_root() {
        for path in [r"\\?\GLOBALROOT\Device\x", r"\\?\Volume{1234}\x"] {
            assert!(refuse_alias(path, None, Aliases::AllowLocalShares).is_err());
            assert!(refuse_alias(path, None, Aliases::Refuse).is_err());
        }
    }

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

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(strip_verbatim(r"\\?\C:\Windows"), r"C:\Windows");
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share\x"), r"\\srv\share\x");
        assert_eq!(strip_verbatim(r"\\?\unc\srv\share"), r"\\srv\share");
        assert_eq!(strip_verbatim(r"D:\Data"), r"D:\Data");
    }

    #[test]
    fn aliases_of_the_local_disk_are_refused() {
        for path in [
            r"\\localhost\c$\Windows",
            r"\\LOCALHOST\C$",
            r"\\127.0.0.1\c$\Windows",
            r"\\::1\c$",
            r"\\.\C:\Windows",
            r"\\?\UNC\localhost\c$\Windows",
            r"\\?\GLOBALROOT\Device\HarddiskVolume3\Windows",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\Windows",
            r"//localhost/c$/Windows",
            r"\\SERVER01\c$\Windows",
        ] {
            let computer = Some("server01");
            assert!(
                refuse_alias(path, computer, Aliases::Refuse).is_err(),
                "{path}"
            );
        }
        for path in [
            r"C:\Windows",
            r"\\?\C:\Windows",
            r"\\fileserver\share\x",
            r"\\?\UNC\fileserver\share",
        ] {
            assert!(
                refuse_alias(path, Some("server01"), Aliases::Refuse).is_ok(),
                "{path}"
            );
        }
    }

    #[test]
    fn a_missing_root_is_an_error_and_a_real_one_resolves() {
        let temp = tempfile::tempdir().unwrap();
        let resolved = canonical_root(temp.path()).unwrap();
        assert!(resolved.is_dir());
        assert!(!resolved.to_string_lossy().starts_with(r"\\?\"));
        assert!(canonical_root(&temp.path().join("missing")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_short_name_resolves_to_the_long_one() {
        assert_eq!(
            canonical_root(Path::new(r"C:\PROGRA~1")).unwrap(),
            PathBuf::from(r"C:\Program Files")
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_admin_share_of_this_pc_is_refused() {
        let err = canonical_root(Path::new(r"\\localhost\c$\Windows")).unwrap_err();
        assert!(err.contains("hides where it really is"), "{err}");
    }
}

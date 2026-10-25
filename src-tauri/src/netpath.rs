//! Which server to ask for account names (owners and access lists) for a folder. A
//! `\\server\share` path names it; a mapped network drive (`Z:\`) has to be resolved to the
//! `\\server\share` it points at, or every name would be asked of this PC and server-local
//! groups would look orphaned (D-07).

use cleanup_core::pathsafe::{drive_letter, drive_type_of, is_remote_drive_type};
use migrate_core::acl_audit::server_of;
use std::path::Path;

/// Where account names for one folder are asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamesFrom {
    /// The server, or `None` for this PC.
    pub server: Option<String>,
    /// Said in the result when a network drive could not be resolved and this PC was used.
    pub note: String,
}

/// The one rule, with the two Windows questions handed in so it is tested everywhere:
/// `drive_type` is `GetDriveTypeW` for a drive letter, `resolve` gives the `\\server\share` a
/// mapped letter points at. [`server_of`] is the only parser of a UNC path.
pub fn names_from_with(
    path: &str,
    drive_type: &dyn Fn(char) -> Option<u32>,
    resolve: &dyn Fn(char) -> Option<String>,
) -> NamesFrom {
    if let Some(server) = server_of(path) {
        return NamesFrom {
            server: Some(server),
            note: String::new(),
        };
    }
    let Some(letter) = drive_letter(path) else {
        return NamesFrom::default();
    };
    if !drive_type(letter).is_some_and(is_remote_drive_type) {
        // A local, subst or removable drive: this PC holds the accounts.
        return NamesFrom::default();
    }
    match resolve(letter).as_deref().and_then(server_of) {
        Some(server) => NamesFrom {
            server: Some(server),
            note: String::new(),
        },
        None => NamesFrom {
            server: None,
            note: format!(
                "Drive {letter}: is a network drive, but Windows did not say which server it is on, so account names were looked up on this PC. Names of groups that live on the server may show as not found."
            ),
        },
    }
}

/// [`names_from_with`] on this PC.
pub fn names_from(path: &Path) -> NamesFrom {
    names_from_with(
        &path.to_string_lossy(),
        &drive_type_of,
        &transfer_core::share::mapped_drive_root,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // GetDriveTypeW: 3 = fixed, 4 = remote.
    fn types(letter: char) -> Option<u32> {
        match letter {
            'C' | 'S' => Some(3),
            'Y' | 'Z' => Some(4),
            _ => None,
        }
    }

    fn resolver(letter: char) -> Option<String> {
        match letter {
            'Z' => Some(r"\\fs01\Finance".to_string()),
            'Y' => Some("garbage".to_string()),
            _ => None,
        }
    }

    fn from(path: &str) -> NamesFrom {
        names_from_with(path, &types, &resolver)
    }

    #[test]
    fn a_unc_path_names_its_server_without_asking_windows() {
        let boom = |_: char| -> Option<u32> { panic!("asked") };
        for path in [
            r"\\fs01\C$\Users\Public",
            r"\\?\UNC\fs01\Finance\x",
            r"\\localhost\C$\Users\Public",
        ] {
            let found = names_from_with(path, &boom, &boom_s);
            assert!(found.server.is_some(), "{path}");
            assert!(found.note.is_empty());
        }
        assert_eq!(from(r"\\fs01\C$\x").server.as_deref(), Some("fs01"));
        assert_eq!(
            from(r"\\localhost\C$\Users\Public").server.as_deref(),
            Some("localhost")
        );
    }

    fn boom_s(_: char) -> Option<String> {
        panic!("asked")
    }

    #[test]
    fn a_mapped_drive_gives_the_server_it_points_at() {
        let found = from(r"Z:\Data\2024");
        assert_eq!(found.server.as_deref(), Some("fs01"));
        assert!(found.note.is_empty());
        assert_eq!(from(r"\\?\Z:\Data").server.as_deref(), Some("fs01"));
        assert_eq!(from("z:").server.as_deref(), Some("fs01"));
    }

    #[test]
    fn a_local_or_subst_drive_is_this_pc_with_no_note() {
        for path in [r"C:\Users", r"S:\subst", r"Q:\unknown", "relative"] {
            assert_eq!(from(path), NamesFrom::default(), "{path}");
        }
    }

    #[test]
    fn a_drive_that_does_not_resolve_falls_back_to_this_pc_with_a_note() {
        // Windows says it is a network drive but gives nothing, or something that is not UNC.
        let found = from(r"Y:\Data");
        assert_eq!(found.server, None);
        assert!(found.note.contains("Drive Y:"), "{}", found.note);
        assert!(found.note.contains("this PC"));
        let unresolved = names_from_with(r"Z:\x", &types, &|_| None);
        assert_eq!(unresolved.server, None);
        assert!(unresolved.note.contains("Drive Z:"));
    }
}

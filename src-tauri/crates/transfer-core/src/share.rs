//! Sign-in to a network share (`\\server\share`) with a user name and password, for shares the
//! signed-in Windows account cannot open. The connection lasts until sign-out and is never saved
//! to the Windows profile; the password is used for this one call and not kept.
//!
//! Also lists the disk shares of a server (`NetShareEnum`), for the Servers section in Settings.

use serde::Serialize;

/// `\\server\share` for a UNC path (also the `\\?\UNC\server\share` form), or `None` for a path
/// on a local drive.
pub fn share_root(path: &str) -> Option<String> {
    let path = path.trim().replace('/', r"\");
    let rest = path
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| path.strip_prefix(r"\\"))?;
    let mut parts = rest.split('\\').filter(|part| !part.is_empty());
    let server = parts.next()?;
    let share = parts.next()?;
    if server == "?" || server == "." {
        return None;
    }
    Some(format!(r"\\{server}\{share}"))
}

/// Windows' words for the errors a sign-in is likely to hit.
pub fn connect_error_text(code: u32) -> String {
    match code {
        5 => "Access denied for that account.".into(),
        53 | 67 => "The share could not be found. Check the server and share name.".into(),
        86 | 1326 => "The user name or password is wrong.".into(),
        1219 => "Windows already has a connection to this server under another account. \
                 Sign out of it first (net use \\\\server /delete) and try again."
            .into(),
        1222 => "The network is not reachable.".into(),
        1244 => "The server needs a different kind of sign-in.".into(),
        other => format!("Windows could not connect (error {other})."),
    }
}

/// The server name from what the user typed: `fs01`, `\\\\fs01`, `\\\\fs01\\Finance\\x` or
/// `fs01.corp.local`. `None` for an empty name, a drive path or a name with characters no
/// server name has.
pub fn server_name(input: &str) -> Option<String> {
    let input = input.trim().replace('/', r"\");
    let name = input.trim_start_matches('\\').split('\\').next()?;
    let valid = !name.is_empty()
        && name != "?"
        && name != "."
        && !name.ends_with(':')
        && !name.contains(|c: char| c.is_whitespace() || r#":*?"<>|"#.contains(c));
    valid.then(|| name.to_string())
}

/// One share as `NetShareEnum` (level 1) reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawShare {
    pub name: String,
    /// `SHARE_INFO_1.shi1_type`.
    pub kind: u32,
    pub remark: String,
}

/// A disk share of a server, as the Servers section lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareInfo {
    pub name: String,
    pub comment: String,
    /// An administrative or special share (`C$`, `ADMIN$`): marked, and listed after the rest.
    pub hidden: bool,
}

const STYPE_MASK: u32 = 0xFF;
const STYPE_DISKTREE: u32 = 0;
const STYPE_SPECIAL: u32 = 0x8000_0000;

/// Keeps the disk shares (no printers, devices or IPC$), marks the special ones and those whose
/// name ends in `$` as hidden, and sorts the normal shares before the hidden ones, each group by
/// name without regard to case.
pub fn disk_shares(raw: Vec<RawShare>) -> Vec<ShareInfo> {
    let mut shares: Vec<ShareInfo> = raw
        .into_iter()
        .filter(|share| share.kind & STYPE_MASK == STYPE_DISKTREE && !share.name.is_empty())
        .map(|share| ShareInfo {
            hidden: share.kind & STYPE_SPECIAL != 0 || share.name.ends_with('$'),
            comment: share.remark,
            name: share.name,
        })
        .collect();
    shares.sort_by_cached_key(|share| (share.hidden, share.name.to_lowercase()));
    shares
}

/// Why a share list failed. `access_denied` is set when signing in would help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListError {
    pub message: String,
    pub access_denied: bool,
}

/// Maps a `NetShareEnum` error to the words of [`connect_error_text`].
pub fn list_error(code: u32) -> ListError {
    let words = match code {
        // ERROR_ACCESS_DENIED
        5 => 5,
        // The server was not found: bad network path, bad network name, RPC server unavailable.
        53 | 67 | 1722 => 53,
        // Not reachable: network down, host unreachable, unexpected network error.
        51 | 59 | 64 | 1203 | 1222 | 1231 => 1222,
        other => other,
    };
    ListError {
        message: match words {
            53 => "The server could not be found. Check the server name.".into(),
            other => connect_error_text(other),
        },
        access_denied: code == 5,
    }
}

/// The disk shares of `server`, hidden ones marked and last.
#[cfg(windows)]
pub fn list_shares(server: &str) -> Result<Vec<ShareInfo>, ListError> {
    use windows_sys::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
    use windows_sys::Win32::Storage::FileSystem::{NetShareEnum, SHARE_INFO_1};

    const MAX_PREFERRED: u32 = u32::MAX;
    const ERROR_MORE_DATA: u32 = 234;

    fn text(pointer: *const u16) -> String {
        if pointer.is_null() {
            return String::new();
        }
        // SAFETY: NetShareEnum fills the strings with NUL-terminated UTF-16 inside its buffer.
        let mut len = 0;
        while unsafe { *pointer.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len` units were just read from the same buffer.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, len) })
    }

    let name = server_name(server).ok_or_else(|| ListError {
        message: "Type a server name, for example fs01.".into(),
        access_denied: false,
    })?;
    let wide: Vec<u16> = format!(r"\\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut buffer: *mut u8 = std::ptr::null_mut();
    let (mut read, mut total, mut resume) = (0u32, 0u32, 0u32);
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointers are valid. The
    // buffer Windows allocates is read below and released with NetApiBufferFree.
    let code = unsafe {
        NetShareEnum(
            wide.as_ptr(),
            1,
            &mut buffer,
            MAX_PREFERRED,
            &mut read,
            &mut total,
            &mut resume,
        )
    };
    // With a preferred length of MAX_PREFERRED everything fits in one buffer.
    if code != 0 && code != ERROR_MORE_DATA {
        if !buffer.is_null() {
            // SAFETY: a buffer NetShareEnum allocated.
            unsafe { NetApiBufferFree(buffer.cast()) };
        }
        return Err(list_error(code));
    }
    let mut raw = Vec::with_capacity(read as usize);
    for index in 0..read as usize {
        // SAFETY: the buffer holds `read` SHARE_INFO_1 records.
        let entry = unsafe { *buffer.cast::<SHARE_INFO_1>().add(index) };
        raw.push(RawShare {
            name: text(entry.shi1_netname),
            kind: entry.shi1_type,
            remark: text(entry.shi1_remark),
        });
    }
    if !buffer.is_null() {
        // SAFETY: a buffer NetShareEnum allocated, no longer read.
        unsafe { NetApiBufferFree(buffer.cast()) };
    }
    Ok(disk_shares(raw))
}

#[cfg(not(windows))]
pub fn list_shares(_server: &str) -> Result<Vec<ShareInfo>, ListError> {
    Err(ListError {
        message: "Listing shares is only available on Windows.".into(),
        access_denied: false,
    })
}

/// Connects `share` (as returned by [`share_root`]) with the given account.
#[cfg(windows)]
pub fn connect(share: &str, username: &str, password: &str) -> Result<(), String> {
    use windows_sys::Win32::NetworkManagement::WNet::{
        WNetAddConnection2W, NETRESOURCEW, RESOURCETYPE_DISK,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    if share_root(share).as_deref() != Some(share) {
        return Err("That is not a network share path (\\\\server\\share).".into());
    }
    let mut remote = wide(share);
    let user = wide(username);
    let mut secret = wide(password);
    let resource = NETRESOURCEW {
        dwScope: 0,
        dwType: RESOURCETYPE_DISK,
        dwDisplayType: 0,
        dwUsage: 0,
        lpLocalName: std::ptr::null_mut(),
        lpRemoteName: remote.as_mut_ptr(),
        lpComment: std::ptr::null_mut(),
        lpProvider: std::ptr::null_mut(),
    };
    // SAFETY: every pointer refers to a NUL-terminated buffer that outlives the call; flags 0
    // means the connection is not saved to the profile.
    let code = unsafe {
        WNetAddConnection2W(
            &resource,
            secret.as_ptr(),
            if username.is_empty() {
                std::ptr::null()
            } else {
                user.as_ptr()
            },
            0,
        )
    };
    secret.iter_mut().for_each(|unit| *unit = 0);
    match code {
        0 => Ok(()),
        code => Err(connect_error_text(code)),
    }
}

#[cfg(not(windows))]
pub fn connect(_share: &str, _username: &str, _password: &str) -> Result<(), String> {
    Err("Signing in to a share is only available on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_root_cuts_a_unc_path_to_server_and_share() {
        assert_eq!(
            share_root(r"\\nas\backup\2024\Jan").as_deref(),
            Some(r"\\nas\backup")
        );
        assert_eq!(
            share_root(r"\\nas\backup").as_deref(),
            Some(r"\\nas\backup")
        );
        assert_eq!(
            share_root("//nas/backup/x").as_deref(),
            Some(r"\\nas\backup")
        );
        assert_eq!(
            share_root(r"\\?\UNC\nas\backup\x").as_deref(),
            Some(r"\\nas\backup")
        );
        assert_eq!(
            share_root(r"  \\nas\backup  ").as_deref(),
            Some(r"\\nas\backup")
        );
    }

    #[test]
    fn share_root_ignores_local_and_incomplete_paths() {
        assert_eq!(share_root(r"C:\Data"), None);
        assert_eq!(share_root(r"\\nas"), None);
        assert_eq!(share_root(r"\\nas\"), None);
        assert_eq!(share_root(r"\\?\C:\Data"), None);
        assert_eq!(share_root(r"\\.\PhysicalDrive0"), None);
        assert_eq!(share_root(""), None);
    }

    fn raw(name: &str, kind: u32, remark: &str) -> RawShare {
        RawShare {
            name: name.into(),
            kind,
            remark: remark.into(),
        }
    }

    #[test]
    fn share_root_covers_admin_shares() {
        assert_eq!(share_root(r"\\fs01\C$").as_deref(), Some(r"\\fs01\C$"));
        assert_eq!(
            share_root(r"\\fs01\D$\Data\2024").as_deref(),
            Some(r"\\fs01\D$")
        );
        assert_eq!(
            share_root(r"\\?\UNC\fs01\C$\Users").as_deref(),
            Some(r"\\fs01\C$")
        );
        assert_eq!(
            share_root(r"\\localhost\C$\Users\Public").as_deref(),
            Some(r"\\localhost\C$")
        );
        assert_eq!(
            share_root(r"\\fs01\ADMIN$").as_deref(),
            Some(r"\\fs01\ADMIN$")
        );
        assert_eq!(share_root(r"\\fs01\IPC$").as_deref(), Some(r"\\fs01\IPC$"));
    }

    #[test]
    fn server_name_takes_the_first_part_of_what_was_typed() {
        for (typed, name) in [
            ("fs01", "fs01"),
            (r"\\fs01", "fs01"),
            (r"  \\fs01\Finance\x ", "fs01"),
            ("//fs01/Finance", "fs01"),
            ("fs01.corp.local", "fs01.corp.local"),
            ("192.168.1.5", "192.168.1.5"),
        ] {
            assert_eq!(server_name(typed).as_deref(), Some(name), "{typed}");
        }
        for typed in [
            "",
            "  ",
            r"\\",
            r"\\?\C:\x",
            r"\\.\x",
            "C:\\Data",
            "a b",
            "fs*01",
        ] {
            assert_eq!(server_name(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn only_disk_shares_are_listed_normal_ones_first_hidden_ones_marked() {
        let special_disk = 0x8000_0000;
        let shares = disk_shares(vec![
            raw("print$", 0, "Printer Drivers"),
            raw("Zeta", 0, ""),
            raw("C$", special_disk, "Default share"),
            raw("IPC$", special_disk | 3, "Remote IPC"),
            raw("Laser", 1, "A printer"),
            raw("alpha", 0, "Finance"),
            raw("ADMIN$", special_disk, "Remote Admin"),
            raw("Dev", 2, "A device"),
            raw("Quiet$", 0, "Hidden by name only"),
            raw("", 0, "no name"),
        ]);
        let names: Vec<(&str, bool)> = shares
            .iter()
            .map(|share| (share.name.as_str(), share.hidden))
            .collect();
        assert_eq!(
            names,
            [
                ("alpha", false),
                ("Zeta", false),
                ("ADMIN$", true),
                ("C$", true),
                ("print$", true),
                ("Quiet$", true),
            ]
        );
        assert_eq!(shares[0].comment, "Finance");
    }

    #[test]
    fn a_share_list_error_uses_the_sign_in_words() {
        let denied = list_error(5);
        assert!(denied.access_denied);
        assert_eq!(denied.message, connect_error_text(5));
        for code in [53, 1722] {
            let missing = list_error(code);
            assert!(!missing.access_denied);
            assert!(missing.message.contains("server could not be found"));
        }
        for code in [64, 1231] {
            assert_eq!(list_error(code).message, connect_error_text(1222));
        }
        assert!(list_error(9999).message.contains("9999"));
    }

    #[cfg(not(windows))]
    #[test]
    fn listing_shares_is_unavailable_off_windows() {
        assert!(list_shares("fs01").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn listing_the_shares_of_this_pc_works_or_is_refused_cleanly() {
        match list_shares("localhost") {
            Ok(shares) => {
                // Elevated: the administrative shares are there and last. Otherwise the plain
                // shares (often none) are listed without them.
                let first_hidden = shares.iter().position(|share| share.hidden);
                if let Some(at) = first_hidden {
                    assert!(shares[at..].iter().all(|share| share.hidden));
                }
                assert!(shares.iter().all(|share| share.name != "IPC$"));
            }
            Err(err) => {
                assert!(err.access_denied, "{}", err.message);
                assert!(!err.message.is_empty());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn listing_shares_needs_a_server_name() {
        assert!(list_shares("").is_err());
        assert!(list_shares(r"C:\Data").is_err());
    }

    #[test]
    fn known_errors_get_plain_words() {
        assert!(connect_error_text(1326).contains("password"));
        assert!(connect_error_text(1219).contains("another account"));
        assert!(connect_error_text(9999).contains("9999"));
    }

    #[cfg(windows)]
    #[test]
    fn connect_refuses_a_path_that_is_not_a_share() {
        assert!(connect(r"C:\Data", "u", "p").is_err());
        assert!(connect(r"\\nas\backup\sub", "u", "p").is_err());
    }
}

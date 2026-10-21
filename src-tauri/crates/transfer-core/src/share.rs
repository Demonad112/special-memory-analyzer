//! Sign-in to a network share (`\\server\share`) with a user name and password, for shares the
//! signed-in Windows account cannot open. The connection lasts until sign-out and is never saved
//! to the Windows profile; the password is used for this one call and not kept.

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

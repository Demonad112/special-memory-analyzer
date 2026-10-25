//! Saved server profiles live in the webview (`analyzer-servers`); this is the one thing Rust
//! does for them: ask a server which disk shares it offers.

use serde::Serialize;
use transfer_core::share::{list_shares, server_name, ListError, ShareInfo};

/// The answer for one server. A failure is data, not an error, so the page can show the words
/// and, for access denied, offer the sign-in.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerShares {
    /// The server name as asked (`fs01`).
    server: String,
    shares: Vec<ShareInfo>,
    error: Option<String>,
    access_denied: bool,
}

fn answer(
    server: &str,
    lister: &dyn Fn(&str) -> Result<Vec<ShareInfo>, ListError>,
) -> ServerShares {
    let Some(name) = server_name(server) else {
        return ServerShares {
            server: server.trim().to_string(),
            shares: Vec::new(),
            error: Some("Type a server name, for example fs01.".into()),
            access_denied: false,
        };
    };
    match lister(&name) {
        Ok(shares) => ServerShares {
            server: name,
            shares,
            error: None,
            access_denied: false,
        },
        Err(err) => ServerShares {
            server: name,
            shares: Vec::new(),
            error: Some(err.message),
            access_denied: err.access_denied,
        },
    }
}

/// Lists the disk shares of `server` with the current Windows sign-in. Hidden (`$`) shares are
/// marked and come last. Access denied is reported so the page can offer the sign-in.
#[tauri::command]
pub async fn servers_list_shares(server: String) -> Result<ServerShares, String> {
    tauri::async_runtime::spawn_blocking(move || answer(&server, &list_shares))
        .await
        .map_err(|err| {
            let message = format!("Listing the shares stopped unexpectedly: {err}");
            report_core::applog::error(&message);
            message
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(name: &str, hidden: bool) -> ShareInfo {
        ShareInfo {
            name: name.into(),
            comment: String::new(),
            hidden,
        }
    }

    #[test]
    fn the_server_name_is_cut_from_what_was_typed_before_asking() {
        let asked = std::cell::RefCell::new(Vec::new());
        let lister = |name: &str| {
            asked.borrow_mut().push(name.to_string());
            Ok(vec![share("Finance", false), share("C$", true)])
        };
        let result = answer(r"  \\fs01\Finance ", &lister);
        assert_eq!(result.server, "fs01");
        assert_eq!(result.shares.len(), 2);
        assert!(result.error.is_none() && !result.access_denied);
        assert_eq!(asked.into_inner(), ["fs01"]);
    }

    #[test]
    fn a_bad_name_is_not_asked_of_the_network() {
        let lister = |_: &str| -> Result<Vec<ShareInfo>, ListError> { panic!("asked") };
        let result = answer(r"C:\Data", &lister);
        assert!(result.error.is_some());
        assert!(result.shares.is_empty());
    }

    #[test]
    fn access_denied_is_flagged_and_other_failures_are_not() {
        let denied = answer("fs01", &|_| Err(transfer_core::share::list_error(5)));
        assert!(denied.access_denied);
        assert_eq!(
            denied.error.as_deref(),
            Some("Access denied for that account.")
        );
        let gone = answer("fs01", &|_| Err(transfer_core::share::list_error(53)));
        assert!(!gone.access_denied);
        assert!(gone.error.unwrap().contains("could not be found"));
    }
}

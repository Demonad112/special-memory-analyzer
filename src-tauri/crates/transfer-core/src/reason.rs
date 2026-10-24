//! Why a file was not copied, worked out from the OS error code.

use serde::{Deserialize, Serialize};
use std::io;

/// Which end of the copy an error came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Source,
    Destination,
}

/// The reason a file is in the "Not copied" list. The UI groups by this; the English text is
/// [`FailureReason::title`] and [`FailureReason::explanation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureReason {
    AccessDenied,
    FileLocked,
    DiskFull,
    PathTooLong,
    InvalidName,
    /// Two source names differ only by letter case and would land on the same destination file.
    NameCollision,
    NetworkLost,
    DeviceRemoved,
    MediaError,
    SourceVanished,
    TargetExistsDifferent,
    VerifyMismatch,
    /// Watch mode: a source file never arrived at the destination.
    Missing,
    /// Watch mode: the destination file is still partial or held open by the copying tool.
    Incomplete,
    /// Copy mode: a OneDrive online-only file, left alone so the copy doesn't download it.
    CloudOnly,
    Cancelled,
    Unknown,
}

impl FailureReason {
    pub const ALL: [FailureReason; 17] = [
        FailureReason::AccessDenied,
        FailureReason::FileLocked,
        FailureReason::DiskFull,
        FailureReason::PathTooLong,
        FailureReason::InvalidName,
        FailureReason::NameCollision,
        FailureReason::NetworkLost,
        FailureReason::DeviceRemoved,
        FailureReason::MediaError,
        FailureReason::SourceVanished,
        FailureReason::TargetExistsDifferent,
        FailureReason::VerifyMismatch,
        FailureReason::Missing,
        FailureReason::Incomplete,
        FailureReason::CloudOnly,
        FailureReason::Cancelled,
        FailureReason::Unknown,
    ];

    /// Worth retrying automatically: the cause usually clears up by itself within seconds.
    pub fn is_transient(self) -> bool {
        matches!(self, FailureReason::FileLocked | FailureReason::NetworkLost)
    }

    /// Short English name, used in the client report.
    pub fn title(self) -> &'static str {
        match self {
            FailureReason::AccessDenied => "Access denied",
            FailureReason::FileLocked => "In use by another program",
            FailureReason::DiskFull => "Disk full",
            FailureReason::PathTooLong => "Path too long",
            FailureReason::InvalidName => "Name not allowed on Windows",
            FailureReason::NameCollision => "Name differs only by letter case",
            FailureReason::NetworkLost => "Network connection lost",
            FailureReason::DeviceRemoved => "Drive removed or not ready",
            FailureReason::MediaError => "Read or write error on the disk",
            FailureReason::SourceVanished => "Source file no longer exists",
            FailureReason::TargetExistsDifferent => {
                "A different file is already at the destination"
            }
            FailureReason::VerifyMismatch => "Copy does not match the source",
            FailureReason::Missing => "Never arrived at the destination",
            FailureReason::Incomplete => "Still incomplete at the destination",
            FailureReason::CloudOnly => "Online-only (not downloaded)",
            FailureReason::Cancelled => "Cancelled",
            FailureReason::Unknown => "Other error",
        }
    }

    /// One-sentence English explanation, used in the client report.
    pub fn explanation(self) -> &'static str {
        match self {
            FailureReason::AccessDenied => {
                "Windows refused access to the file or folder with the current account's permissions."
            }
            FailureReason::FileLocked => {
                "Another program had the file open and did not allow it to be read or replaced."
            }
            FailureReason::DiskFull => "The destination ran out of free space.",
            FailureReason::PathTooLong => {
                "The full path is longer than 260 characters. Deep Server Analyzer copies it, but Explorer and many older programs cannot open it."
            }
            FailureReason::InvalidName => {
                "The name is reserved on Windows or ends with a dot or space."
            }
            FailureReason::NameCollision => {
                "Another source file has the same name apart from letter case; Windows treats them as one file."
            }
            FailureReason::NetworkLost => "The network share stopped responding during the copy.",
            FailureReason::DeviceRemoved => {
                "The drive was disconnected or stopped responding during the copy."
            }
            FailureReason::MediaError => "The disk reported a hardware read or write error.",
            FailureReason::SourceVanished => {
                "The file was deleted or moved after the list of files was made."
            }
            FailureReason::TargetExistsDifferent => {
                "The destination already had a file with this name but a different size or date, so it was left alone."
            }
            FailureReason::VerifyMismatch => {
                "After copying, the destination file's size, date or content did not match the source."
            }
            FailureReason::Missing => {
                "The file was in the source but did not appear at the destination while it was watched."
            }
            FailureReason::Incomplete => {
                "The destination file was still partial or held open by the copying program."
            }
            FailureReason::CloudOnly => {
                "The file is only in OneDrive, not on this PC, and the copy was set not to download it."
            }
            FailureReason::Cancelled => "The run was stopped before this file was copied.",
            FailureReason::Unknown => "Windows reported an error that has no specific category.",
        }
    }
}

/// Maps an I/O error to a [`FailureReason`], from the Windows error code where there is one.
pub fn classify(error: &io::Error, side: Side) -> FailureReason {
    if let Some(code) = error.raw_os_error() {
        if let Some(reason) = classify_code(code, side) {
            return reason;
        }
    }
    match error.kind() {
        io::ErrorKind::PermissionDenied => FailureReason::AccessDenied,
        io::ErrorKind::NotFound if side == Side::Source => FailureReason::SourceVanished,
        io::ErrorKind::StorageFull => FailureReason::DiskFull,
        io::ErrorKind::InvalidFilename => FailureReason::InvalidName,
        io::ErrorKind::NetworkDown
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::HostUnreachable => FailureReason::NetworkLost,
        _ => FailureReason::Unknown,
    }
}

#[cfg(windows)]
fn classify_code(code: i32, side: Side) -> Option<FailureReason> {
    Some(match code {
        5 => FailureReason::AccessDenied,
        32 | 33 => FailureReason::FileLocked,
        39 | 112 => FailureReason::DiskFull,
        206 => FailureReason::PathTooLong,
        123 => FailureReason::InvalidName,
        51 | 53 | 59 | 64 | 67 | 121 | 240 | 1231 | 1236 => FailureReason::NetworkLost,
        21 | 1117 | 1167 => FailureReason::DeviceRemoved,
        23 => FailureReason::MediaError,
        2 | 3 if side == Side::Source => FailureReason::SourceVanished,
        _ => return None,
    })
}

#[cfg(not(windows))]
fn classify_code(_code: i32, _side: Side) -> Option<FailureReason> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn reason(code: i32, side: Side) -> FailureReason {
        classify(&io::Error::from_raw_os_error(code), side)
    }

    #[test]
    fn classifies_windows_error_codes() {
        let cases = [
            (5, FailureReason::AccessDenied),
            (32, FailureReason::FileLocked),
            (33, FailureReason::FileLocked),
            (39, FailureReason::DiskFull),
            (112, FailureReason::DiskFull),
            (206, FailureReason::PathTooLong),
            (123, FailureReason::InvalidName),
            (53, FailureReason::NetworkLost),
            (64, FailureReason::NetworkLost),
            (67, FailureReason::NetworkLost),
            (1231, FailureReason::NetworkLost),
            (51, FailureReason::NetworkLost),
            (121, FailureReason::NetworkLost),
            (240, FailureReason::NetworkLost),
            (1236, FailureReason::NetworkLost),
            (21, FailureReason::DeviceRemoved),
            (1117, FailureReason::DeviceRemoved),
            (23, FailureReason::MediaError),
            (1234, FailureReason::Unknown),
        ];
        for (code, expected) in cases {
            assert_eq!(reason(code, Side::Destination), expected, "code {code}");
        }
    }

    #[test]
    fn not_found_is_a_vanished_source_only_on_the_source_side() {
        assert_eq!(reason(2, Side::Source), FailureReason::SourceVanished);
        assert_eq!(reason(3, Side::Source), FailureReason::SourceVanished);
        assert_eq!(reason(2, Side::Destination), FailureReason::Unknown);
    }

    #[test]
    fn only_locks_and_network_drops_are_transient() {
        let transient: Vec<_> = FailureReason::ALL
            .into_iter()
            .filter(|reason| reason.is_transient())
            .collect();
        assert_eq!(
            transient,
            vec![FailureReason::FileLocked, FailureReason::NetworkLost]
        );
    }
}

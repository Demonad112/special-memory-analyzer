//! Name rules for the places a copy can land: what Windows (NTFS) cannot create, and what OneDrive
//! and SharePoint refuse on top of that. Record's pre-flight uses [`is_valid_windows_name`];
//! readiness checks use [`name_issues`], which lists every problem with a name.

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;

/// Paths longer than this trip up Explorer and many programs (MAX_PATH minus the terminator).
pub const LONG_PATH_LIMIT: usize = 259;
/// OneDrive and SharePoint refuse a decoded path longer than this.
pub const SHAREPOINT_PATH_LIMIT: usize = 400;

pub const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters Windows does not allow in a name. OneDrive and SharePoint refuse the same set.
const INVALID_CHARACTERS: [char; 9] = ['<', '>', ':', '"', '|', '?', '*', '\\', '/'];

// OneDrive and SharePoint, from Microsoft's "Restrictions and limitations in OneDrive and
// SharePoint" (support.microsoft.com, "Invalid file names and file types"; read 2026-10-24, the
// page's newest "What's new" entry is 2026-04-22). The 400-character limit on the decoded path is
// SharePoint Online's documented limit (decision D-03); that page now defers to "What are the file
// path length limits?" for per-app limits. Keep every SharePoint rule in this block.

/// Names SharePoint refuses outright (compared without case; the device names also with an
/// extension, as on Windows).
const SHAREPOINT_NAMES: [&str; 2] = [".lock", "desktop.ini"];
/// SharePoint also reserves `COM0` and `LPT0`.
const SHAREPOINT_DEVICE_NAMES: [&str; 2] = ["COM0", "LPT0"];
/// SharePoint refuses a name that contains this anywhere.
const SHAREPOINT_INFIX: &str = "_vti_";
/// Office's lock files start with this; SharePoint refuses them. Usually a leftover of an open
/// document, so it is a warning.
const SHAREPOINT_OFFICE_LOCK_PREFIX: &str = "~$";

/// Where the files are going: the rules differ.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetProfile {
    /// A Windows file server or drive (NTFS).
    #[default]
    FileServer,
    /// OneDrive or SharePoint Online: the NTFS rules plus Microsoft's own list.
    SharePoint,
}

impl TargetProfile {
    /// The longest path, in UTF-16 units, that the target takes without trouble.
    pub fn path_limit(self) -> usize {
        match self {
            Self::FileServer => LONG_PATH_LIMIT,
            Self::SharePoint => SHAREPOINT_PATH_LIMIT,
        }
    }
}

/// One problem with a name. A name can have several.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "rule")]
pub enum NameIssue {
    Empty,
    /// The name is not valid Unicode (an unpaired surrogate), so it cannot be opened by name.
    NotUnicode,
    /// A device name such as `CON` or `LPT1`, with or without an extension.
    Reserved {
        name: String,
    },
    TrailingSpace,
    TrailingDot,
    /// SharePoint only; Windows allows it.
    LeadingSpace,
    InvalidCharacter {
        character: char,
    },
    /// A character below U+0020, given as its code.
    ControlCharacter {
        code: u32,
    },
    /// A name on SharePoint's list (`.lock`, `desktop.ini`, `COM0`, `LPT0`).
    SharePointName {
        name: String,
    },
    /// The name contains `_vti_`.
    SharePointVti,
    /// The name starts with `~$`: SharePoint refuses Office lock files.
    SharePointOfficeLock,
    /// The whole path is longer than the target allows.
    PathTooLong {
        length: usize,
        limit: usize,
    },
}

impl NameIssue {
    /// Blockers stop the item from being created at the target; the rest are warnings.
    pub fn is_blocker(&self) -> bool {
        !matches!(self, Self::SharePointOfficeLock | Self::PathTooLong { .. })
    }

    /// A short fix hint for the person reading the report.
    pub fn hint(&self) -> String {
        match self {
            Self::Empty => "Give the item a name".to_owned(),
            Self::NotUnicode => {
                "Rename it on the source machine; the name has characters that are not valid Unicode"
                    .to_owned()
            }
            Self::Reserved { name } => format!("Rename it: {name} is a device name on Windows"),
            Self::TrailingSpace => "Remove the space at the end of the name".to_owned(),
            Self::TrailingDot => "Remove the dot at the end of the name".to_owned(),
            Self::LeadingSpace => "Remove the space at the start of the name".to_owned(),
            Self::InvalidCharacter { character } => {
                format!("Remove or replace the character {character}")
            }
            Self::ControlCharacter { code } => {
                format!("Remove the control character U+{code:04X}")
            }
            Self::SharePointName { name } => {
                format!("Rename it: SharePoint does not allow the name {name}")
            }
            Self::SharePointVti => "Rename it: SharePoint does not allow _vti_ in a name".to_owned(),
            Self::SharePointOfficeLock => {
                "An Office lock file; close the document or leave the file behind".to_owned()
            }
            Self::PathTooLong { length, limit } => {
                format!("Shorten by {} characters", length.saturating_sub(*limit))
            }
        }
    }
}

/// False for names Windows cannot create normally: reserved device names (also with an
/// extension), a trailing dot or space, and characters such as `<>:"|?*`.
pub fn is_valid_windows_name(name: &str) -> bool {
    if name.is_empty() || name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    if name
        .chars()
        .any(|ch| (ch as u32) < 32 || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\'))
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    !RESERVED_NAMES.contains(&stem.as_str())
}

/// Every problem with one name (not a path) for `target`, in a fixed order. Empty when it is fine.
/// Each invalid or control character is listed once, however often it appears.
pub fn name_issues(name: &str, target: TargetProfile) -> Vec<NameIssue> {
    let mut issues = Vec::new();
    if name.is_empty() {
        issues.push(NameIssue::Empty);
        return issues;
    }
    let stem = device_stem(name);
    if RESERVED_NAMES.contains(&stem.as_str()) {
        issues.push(NameIssue::Reserved { name: stem.clone() });
    }
    if name.ends_with(' ') {
        issues.push(NameIssue::TrailingSpace);
    }
    if name.ends_with('.') {
        issues.push(NameIssue::TrailingDot);
    }
    let mut seen = Vec::new();
    for ch in name.chars() {
        if seen.contains(&ch) {
            continue;
        }
        if (ch as u32) < 32 {
            seen.push(ch);
            issues.push(NameIssue::ControlCharacter { code: ch as u32 });
        } else if INVALID_CHARACTERS.contains(&ch) {
            seen.push(ch);
            issues.push(NameIssue::InvalidCharacter { character: ch });
        }
    }
    if target == TargetProfile::SharePoint {
        if name.starts_with(' ') {
            issues.push(NameIssue::LeadingSpace);
        }
        let lower = name.to_lowercase();
        if let Some(listed) = SHAREPOINT_NAMES.iter().find(|listed| **listed == lower) {
            issues.push(NameIssue::SharePointName {
                name: (*listed).to_owned(),
            });
        }
        if SHAREPOINT_DEVICE_NAMES.contains(&stem.as_str()) {
            issues.push(NameIssue::SharePointName { name: stem });
        }
        if lower.contains(SHAREPOINT_INFIX) {
            issues.push(NameIssue::SharePointVti);
        }
        if name.starts_with(SHAREPOINT_OFFICE_LOCK_PREFIX) {
            issues.push(NameIssue::SharePointOfficeLock);
        }
    }
    issues
}

/// [`name_issues`] for a name straight from the file system: one that is not valid Unicode is
/// reported as such and not checked further.
pub fn os_name_issues(name: &OsStr, target: TargetProfile) -> Vec<NameIssue> {
    match name.to_str() {
        Some(name) => name_issues(name, target),
        None => vec![NameIssue::NotUnicode],
    }
}

/// [`NameIssue::PathTooLong`] when `path` (measured in UTF-16 units, as Windows does) is over the
/// target's limit.
pub fn path_length_issue(path: &str, target: TargetProfile) -> Option<NameIssue> {
    let length = path.encode_utf16().count();
    let limit = target.path_limit();
    (length > limit).then_some(NameIssue::PathTooLong { length, limit })
}

/// The part before the first dot, without trailing spaces, upper case: what Windows matches
/// against the device names.
fn device_stem(name: &str) -> String {
    name.split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use TargetProfile::{FileServer, SharePoint};

    fn rules(name: &str, target: TargetProfile) -> Vec<NameIssue> {
        name_issues(name, target)
    }

    #[test]
    fn plain_names_have_no_issues() {
        for name in [
            "a.txt",
            "CONFIG.SYS",
            "con-notes.txt",
            ".gitignore",
            "COM10",
            "Résumé 2026.docx",
        ] {
            assert!(rules(name, FileServer).is_empty(), "{name}");
            assert!(rules(name, SharePoint).is_empty(), "{name}");
        }
    }

    #[test]
    fn reserved_names_with_and_without_an_extension() {
        for (name, stem) in [
            ("CON", "CON"),
            ("con.txt", "CON"),
            ("Nul", "NUL"),
            ("LPT1.log", "LPT1"),
            ("com9.tar.gz", "COM9"),
            ("AUX .txt", "AUX"),
        ] {
            assert_eq!(
                rules(name, FileServer),
                vec![NameIssue::Reserved {
                    name: stem.to_owned()
                }],
                "{name}"
            );
        }
        assert!(rules("CONSOLE.txt", FileServer).is_empty());
        assert!(rules("COM0", FileServer).is_empty(), "NTFS allows COM0");
    }

    #[test]
    fn trailing_space_and_dot() {
        assert_eq!(rules("notes ", FileServer), vec![NameIssue::TrailingSpace]);
        assert_eq!(rules("notes.", FileServer), vec![NameIssue::TrailingDot]);
        assert!(rules("notes.txt", FileServer).is_empty());
        assert!(rules(" notes", FileServer).is_empty(), "NTFS allows it");
    }

    #[test]
    fn every_problem_is_listed() {
        assert_eq!(
            rules("CON.", FileServer),
            vec![
                NameIssue::Reserved {
                    name: "CON".to_owned()
                },
                NameIssue::TrailingDot
            ]
        );
        assert_eq!(
            rules("a?b?c*", FileServer),
            vec![
                NameIssue::InvalidCharacter { character: '?' },
                NameIssue::InvalidCharacter { character: '*' }
            ],
            "each character once"
        );
    }

    #[test]
    fn each_invalid_character() {
        for character in INVALID_CHARACTERS {
            let name = format!("a{character}b");
            assert_eq!(
                rules(&name, FileServer),
                vec![NameIssue::InvalidCharacter { character }],
                "{name}"
            );
        }
        assert!(rules("a-b_c(d)[e]{f}#g%h&i'j,k;l=m+n!o@p$q^r`s~t", FileServer).is_empty());
    }

    #[test]
    fn control_characters() {
        for code in [0_u32, 1, 9, 10, 13, 31] {
            let name = format!("a{}b", char::from_u32(code).unwrap());
            assert_eq!(
                rules(&name, FileServer),
                vec![NameIssue::ControlCharacter { code }],
                "U+{code:04X}"
            );
        }
        assert!(rules("a\u{7f}b", FileServer).is_empty(), "DEL is allowed");
    }

    #[test]
    fn an_empty_name_is_one_issue() {
        assert_eq!(rules("", FileServer), vec![NameIssue::Empty]);
    }

    #[test]
    fn sharepoint_names() {
        for (name, listed) in [
            (".lock", ".lock"),
            (".LOCK", ".lock"),
            ("desktop.ini", "desktop.ini"),
            ("Desktop.INI", "desktop.ini"),
            ("COM0", "COM0"),
            ("lpt0.txt", "LPT0"),
        ] {
            assert_eq!(
                rules(name, SharePoint),
                vec![NameIssue::SharePointName {
                    name: listed.to_owned()
                }],
                "{name}"
            );
            assert!(rules(name, FileServer).is_empty(), "{name} is fine on NTFS");
        }
        assert!(rules("file.lock", SharePoint).is_empty());
        assert!(rules("desktop.ini.bak", SharePoint).is_empty());
    }

    #[test]
    fn sharepoint_vti_anywhere_in_the_name() {
        for name in ["_vti_", "a_vti_b.txt", "X_VTI_.doc"] {
            assert_eq!(
                rules(name, SharePoint),
                vec![NameIssue::SharePointVti],
                "{name}"
            );
            assert!(rules(name, FileServer).is_empty());
        }
        assert!(rules("_vti.txt", SharePoint).is_empty());
    }

    #[test]
    fn sharepoint_office_lock_files_are_a_warning() {
        let issues = rules("~$report.docx", SharePoint);
        assert_eq!(issues, vec![NameIssue::SharePointOfficeLock]);
        assert!(!issues[0].is_blocker());
        assert!(rules("~$report.docx", FileServer).is_empty());
        assert!(rules("report~$.docx", SharePoint).is_empty());
    }

    #[test]
    fn sharepoint_leading_space() {
        assert_eq!(rules(" notes", SharePoint), vec![NameIssue::LeadingSpace]);
    }

    #[test]
    fn sharepoint_also_applies_the_windows_rules() {
        assert_eq!(
            rules("CON", SharePoint),
            vec![NameIssue::Reserved {
                name: "CON".to_owned()
            }]
        );
        assert_eq!(rules("a|b", SharePoint).len(), 1);
    }

    #[test]
    fn path_limits_per_target() {
        let at_limit = "x".repeat(LONG_PATH_LIMIT);
        let over = "x".repeat(LONG_PATH_LIMIT + 1);
        assert_eq!(path_length_issue(&at_limit, FileServer), None);
        assert_eq!(
            path_length_issue(&over, FileServer),
            Some(NameIssue::PathTooLong {
                length: 260,
                limit: 259
            })
        );
        assert_eq!(path_length_issue(&over, SharePoint), None);
        let sharepoint_over = "x".repeat(SHAREPOINT_PATH_LIMIT + 1);
        assert_eq!(
            path_length_issue(&sharepoint_over, SharePoint),
            Some(NameIssue::PathTooLong {
                length: 401,
                limit: 400
            })
        );
        // Measured in UTF-16 units: a character outside the BMP counts twice.
        assert_eq!(
            path_length_issue(&"😀".repeat(130), FileServer),
            Some(NameIssue::PathTooLong {
                length: 260,
                limit: 259
            })
        );
        assert_eq!(
            NameIssue::PathTooLong {
                length: 293,
                limit: 259
            }
            .hint(),
            "Shorten by 34 characters"
        );
    }

    #[test]
    fn a_name_that_is_not_valid_unicode_is_reported() {
        #[cfg(windows)]
        let odd = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xD800, u16::from(b'a')])
        };
        #[cfg(unix)]
        let odd = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xFF, b'a'])
        };
        assert_eq!(
            os_name_issues(&odd, FileServer),
            vec![NameIssue::NotUnicode]
        );
        assert!(os_name_issues(OsStr::new("fine.txt"), SharePoint).is_empty());
    }

    #[test]
    fn severities() {
        assert!(NameIssue::TrailingDot.is_blocker());
        assert!(NameIssue::SharePointVti.is_blocker());
        assert!(!NameIssue::PathTooLong {
            length: 300,
            limit: 259
        }
        .is_blocker());
    }

    #[test]
    fn the_record_check_agrees_with_the_file_server_rules() {
        // Record splits paths on `/`, so its names never hold one.
        for name in [
            "a.txt", "CON", "con.txt", "x.", "x ", " x", "a:b", "a\tb", "", "COM0", "LPT9.x",
            "~$a.doc", "_vti_",
        ] {
            assert_eq!(
                is_valid_windows_name(name),
                rules(name, FileServer).is_empty(),
                "{name:?}"
            );
        }
    }
}

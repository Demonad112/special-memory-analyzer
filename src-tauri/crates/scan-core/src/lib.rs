//! Folder scanning and size-based folder comparison.
//!
//! [`scan`] walks a folder into a [`Tree`] whose folder rows carry recursive totals. [`compare`]
//! matches two trees by relative path and produces a [`DiffTree`] that the UI expands lazily.

mod diff;
pub mod hash;
pub mod inuse;
mod list;
pub mod names;
mod paths;
mod portable;
mod scan;
pub mod security;

pub use diff::{
    compare, DiffNode, DiffRow, DiffSide, DiffStatus, DiffSummary, DiffTree, MissingEntry, Side,
};
pub use names::{
    is_valid_windows_name, name_issues, os_name_issues, path_length_issue, NameIssue,
    TargetProfile, LONG_PATH_LIMIT,
};
pub use paths::{canonical_root, check_pair, clean_path, strip_verbatim};
pub use portable::{data_dir_in, portable_data_dir, PORTABLE_MARKER};
pub use scan::{scan, CancelToken, Kind, Node, NodeId, ScanError, ScanProgress, ScanState, Tree};

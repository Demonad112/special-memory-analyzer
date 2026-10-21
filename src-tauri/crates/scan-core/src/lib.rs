//! Folder scanning and size-based folder comparison.
//!
//! [`scan`] walks a folder into a [`Tree`] whose folder rows carry recursive totals. [`compare`]
//! matches two trees by relative path and produces a [`DiffTree`] that the UI expands lazily.

mod diff;
mod list;
mod paths;
mod portable;
mod scan;

pub use diff::{
    compare, DiffNode, DiffRow, DiffSide, DiffStatus, DiffSummary, DiffTree, MissingEntry, Side,
};
pub use paths::{check_pair, clean_path};
pub use portable::{data_dir_in, portable_data_dir, PORTABLE_MARKER};
pub use scan::{scan, CancelToken, Kind, Node, NodeId, ScanError, ScanProgress, ScanState, Tree};

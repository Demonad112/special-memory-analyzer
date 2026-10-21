//! The source listing of a run, from a `scan-core` scan: scan first (in parallel, with live
//! counts), then visit the tree depth-first in name order.

use crate::{CancelToken, Result, TransferError};
use scan_core::{scan, Kind, NodeId, ScanError, ScanProgress, ScanState, Tree};
use std::io;
use std::path::Path;
use std::thread;
use std::time::Duration;

pub(crate) enum Entry {
    Folder {
        relative_path: String,
    },
    File {
        relative_path: String,
        size: u64,
        modified_ms: Option<u64>,
        /// A OneDrive online-only file: reading it downloads it.
        cloud: bool,
    },
    /// A symlink or junction: never followed or copied.
    Link,
    /// A folder that could not be read.
    Unreadable {
        relative_path: String,
        error: io::Error,
    },
}

/// Scans `root`, calling `on_progress` every 100 ms while it runs.
pub(crate) fn scan_tree(
    root: &Path,
    ignore_junk: bool,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(&ScanProgress),
) -> Result<Tree> {
    let state = ScanState::new(cancel.clone()).ignoring_junk(ignore_junk);
    let scanned = thread::scope(|scope| {
        let job = scope.spawn(|| scan(root, &state));
        while !job.is_finished() {
            on_progress(&state.progress());
            thread::sleep(Duration::from_millis(100));
        }
        job.join()
    });
    match scanned {
        Ok(Ok(tree)) => Ok(tree),
        Ok(Err(ScanError::Cancelled)) => Err(TransferError::Cancelled),
        Ok(Err(error)) => Err(TransferError::InvalidPath(error.to_string())),
        Err(_) => Err(TransferError::InvalidPath(
            "The folder scan stopped unexpectedly.".to_owned(),
        )),
    }
}

/// Visits every entry below the root, depth-first, names in order. `visit` returns whether to
/// go into a folder.
pub(crate) fn visit(tree: &Tree, visit: &mut dyn FnMut(Entry) -> Result<bool>) -> Result<()> {
    visit_children(tree, 0, "", visit)
}

fn visit_children(
    tree: &Tree,
    id: NodeId,
    prefix: &str,
    visit: &mut dyn FnMut(Entry) -> Result<bool>,
) -> Result<()> {
    let mut children: Vec<NodeId> = tree.node(id).children().collect();
    children.sort_by(|a, b| tree.node(*a).name.cmp(&tree.node(*b).name));
    for child in children {
        let node = tree.node(child);
        let relative_path = if prefix.is_empty() {
            node.name.clone()
        } else {
            format!("{prefix}/{}", node.name)
        };
        match node.kind {
            Kind::Link => {
                visit(Entry::Link)?;
            }
            Kind::File => {
                visit(Entry::File {
                    relative_path,
                    size: node.size,
                    modified_ms: node.modified_ms,
                    cloud: node.cloud_files > 0,
                })?;
            }
            Kind::Dir if node.error.is_some() && !node.has_children() => {
                let error = match node.error_code {
                    Some(code) => io::Error::from_raw_os_error(code),
                    None => io::Error::other(node.error.clone().unwrap_or_default()),
                };
                visit(Entry::Unreadable {
                    relative_path,
                    error,
                })?;
            }
            Kind::Dir => {
                let enter = visit(Entry::Folder {
                    relative_path: relative_path.clone(),
                })?;
                if enter {
                    visit_children(tree, child, &relative_path, visit)?;
                }
            }
        }
    }
    Ok(())
}

//! Checks to run on a source folder before migrating it: names the target cannot hold, paths
//! that will be too long at the destination, and files another program holds open.
//!
//! Everything walks the tree [`scan_core::scan`] builds, so links are listed and never entered
//! and online-only files are checked by name only; the in-use pass never opens them.

pub mod acl_audit;
pub mod acl_diff;
pub mod readiness;

pub use acl_audit::{
    AuditStatus, AuditedItem, EntryRow, Finding, FindingCount, FindingKind, PermissionsReport,
};
pub use acl_diff::{
    compare_permissions, compare_rows, diff_descriptors, has_explicit_entries, AclDiff,
    CompareOptions, CompareRow, OwnerChange, PermCell, PermDetail, PermStatus, PermissionsCompare,
    PermissionsResult, SameNameEntry, StatusCount, DETAIL_LIMIT,
};
pub use readiness::{
    rule_label, run, InUseStatus, InUseSummary, Options, Phase, Problem, ProblemKind, Readiness,
    ReadinessProgress, ReadinessReport, ReadinessState, Row, RowProblem, RuleCount, PER_KIND_LIMIT,
};

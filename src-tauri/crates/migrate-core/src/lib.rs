//! Checks to run on a source folder before migrating it: names the target cannot hold, paths
//! that will be too long at the destination, and files another program holds open.
//!
//! Everything walks the tree [`scan_core::scan`] builds, so links are listed and never entered
//! and online-only files are checked by name only.

pub mod readiness;

pub use readiness::{
    rule_label, run, Options, Phase, Problem, ProblemKind, Readiness, ReadinessProgress,
    ReadinessReport, ReadinessState, Row, RowProblem, RuleCount, PER_KIND_LIMIT,
};

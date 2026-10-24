//! Migration readiness: one scan of the source, then a pass over the tree for everything that will
//! break the copy or the target.
//!
//! The name pass checks every name against the target's rules ([`scan_core::name_issues`]) and
//! every path's length as it will be at the destination. Links are listed as "not followed" (the
//! scan never enters them), and folders the scan could not read are listed with the reason.
//! Online-only files are checked by name only: nothing here opens a file.

use scan_core::{
    name_issues, scan, CancelToken, Kind, NameIssue, NodeId, ScanError, ScanProgress, ScanState,
    TargetProfile, Tree,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

/// Most problems of one kind a saved report keeps; the counts always cover every problem.
pub const PER_KIND_LIMIT: usize = 20_000;

/// What to check and where the files are going.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub target: TargetProfile,
    /// The planned destination folder (`D:\Shares\Finance`). Path lengths are measured as they
    /// will be there. Empty: measured at the source. It is only used as text, never opened.
    pub destination_root: String,
}

/// The groups the results are shown in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProblemKind {
    /// A name the target cannot hold (or SharePoint refuses).
    #[default]
    Name,
    /// The path is over the target's length limit.
    LongPath,
    /// A folder the scan could not read completely.
    Unreadable,
    /// A symlink or junction: listed, never entered.
    Link,
    /// Written by a newer version; shown as is.
    #[serde(other)]
    Other,
}

/// How many kinds [`ProblemKind::index`] numbers.
const KIND_COUNT: usize = 5;

impl ProblemKind {
    /// Every kind the checks produce, in the order the report shows them.
    pub const ALL: [ProblemKind; 4] = [Self::Name, Self::LongPath, Self::Unreadable, Self::Link];

    fn index(self) -> usize {
        match self {
            Self::Name => 0,
            Self::LongPath => 1,
            Self::Unreadable => 2,
            Self::Link => 3,
            Self::Other => 4,
        }
    }

    /// The heading of the group.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Name => "Names the target cannot hold",
            Self::LongPath => "Paths too long",
            Self::Unreadable => "Folders that could not be read",
            Self::Link => "Links and junctions (not followed)",
            Self::Other => "Other problems",
        }
    }
}

/// The rule's code for a name problem; saved as text so a newer rule never breaks an old reader.
fn rule_code(issue: &NameIssue) -> &'static str {
    match issue {
        NameIssue::Empty => "empty",
        NameIssue::NotUnicode => "notUnicode",
        NameIssue::Reserved { .. } => "reserved",
        NameIssue::TrailingSpace => "trailingSpace",
        NameIssue::TrailingDot => "trailingDot",
        NameIssue::LeadingSpace => "leadingSpace",
        NameIssue::InvalidCharacter { .. } => "invalidCharacter",
        NameIssue::ControlCharacter { .. } => "controlCharacter",
        NameIssue::SharePointName { .. } => "sharePointName",
        NameIssue::SharePointVti => "sharePointVti",
        NameIssue::SharePointOfficeLock => "sharePointOfficeLock",
        NameIssue::PathTooLong { .. } => "pathTooLong",
    }
}

const RULE_LINK: &str = "link";
const RULE_UNREADABLE: &str = "unreadable";

/// The rule in words, for a rule code saved in a report. Unknown codes come back as they are.
pub fn rule_label(rule: &str) -> &str {
    match rule {
        "empty" => "Empty name",
        "notUnicode" => "Name is not valid Unicode",
        "reserved" => "Reserved device name",
        "trailingSpace" => "Ends with a space",
        "trailingDot" => "Ends with a dot",
        "leadingSpace" => "Starts with a space",
        "invalidCharacter" => "Character Windows does not allow",
        "controlCharacter" => "Control character",
        "sharePointName" => "Name SharePoint does not allow",
        "sharePointVti" => "Contains _vti_",
        "sharePointOfficeLock" => "Office lock file (~$)",
        "pathTooLong" => "Path too long",
        RULE_LINK => "Link or junction, not followed",
        RULE_UNREADABLE => "Could not be read completely",
        other => other,
    }
}

/// One problem with one item, as the screen shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowProblem {
    pub kind: ProblemKind,
    /// A stable code (`reserved`, `pathTooLong`, ...).
    pub rule: String,
    pub label: String,
    pub hint: String,
    /// The item cannot be created at the target; the rest are warnings.
    pub blocker: bool,
}

impl RowProblem {
    fn new(kind: ProblemKind, rule: &str, hint: String, blocker: bool) -> Self {
        Self {
            kind,
            rule: rule.to_owned(),
            label: rule_label(rule).to_owned(),
            hint,
            blocker,
        }
    }

    fn from_issue(issue: &NameIssue) -> Self {
        let kind = match issue {
            NameIssue::PathTooLong { .. } => ProblemKind::LongPath,
            _ => ProblemKind::Name,
        };
        Self::new(kind, rule_code(issue), issue.hint(), issue.is_blocker())
    }
}

/// One saved problem.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Problem {
    /// Relative to the source, `\`-separated; empty for the source folder itself.
    pub path: String,
    pub dir: bool,
    pub kind: ProblemKind,
    pub rule: String,
    pub hint: String,
    pub blocker: bool,
}

/// How many items break one rule.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleCount {
    pub kind: ProblemKind,
    pub rule: String,
    pub count: u64,
    pub blocker: bool,
}

/// A finished readiness check: totals, counts per rule, and the problems (capped per kind).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReadinessReport {
    pub source: String,
    /// Empty when no destination root was given.
    pub destination_root: String,
    pub target: TargetProfile,
    pub files: u64,
    pub dirs: u64,
    pub size: u64,
    /// Online-only files: checked by name only.
    pub cloud_files: u64,
    pub links: u64,
    /// Folders that could not be read completely.
    pub unreadable: u64,
    /// Items with at least one name problem.
    pub name_problems: u64,
    pub long_paths: u64,
    /// Problems that stop an item from being created at the target.
    pub blockers: u64,
    pub problems_total: u64,
    pub elapsed_ms: u64,
    pub counts: Vec<RuleCount>,
    /// Grouped by kind, then by path; at most [`PER_KIND_LIMIT`] per kind.
    pub problems: Vec<Problem>,
}

impl ReadinessReport {
    /// Problems of `kind` the report lists (may be fewer than [`ReadinessReport::count_of`]).
    pub fn listed(&self, kind: ProblemKind) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(move |problem| problem.kind == kind)
    }

    /// Every problem of `kind`, listed or not.
    pub fn count_of(&self, kind: ProblemKind) -> u64 {
        self.counts
            .iter()
            .filter(|count| count.kind == kind)
            .map(|count| count.count)
            .sum()
    }

    /// Something will fail or needs a look before the copy (links alone do not count).
    pub fn has_problems(&self) -> bool {
        self.counts
            .iter()
            .any(|count| count.kind != ProblemKind::Link && count.count > 0)
    }
}

/// Which part of the check is running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Scan,
    Names,
}

/// Live state of one check. The check updates it; another thread reads
/// [`ReadinessState::progress`].
#[derive(Debug)]
pub struct ReadinessState {
    cancel: CancelToken,
    scan: ScanState,
    phase: AtomicU8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessProgress {
    pub phase: Phase,
    pub scan: ScanProgress,
}

impl ReadinessState {
    pub fn new(cancel: CancelToken) -> Self {
        Self {
            scan: ScanState::new(cancel.clone()),
            cancel,
            phase: AtomicU8::new(0),
        }
    }

    fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    pub fn progress(&self) -> ReadinessProgress {
        ReadinessProgress {
            phase: match self.phase.load(Ordering::Relaxed) {
                0 => Phase::Scan,
                _ => Phase::Names,
            },
            scan: self.scan.progress(),
        }
    }
}

/// A folder or file of the result that has problems at or below it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: NodeId,
    pub name: String,
    pub kind: Kind,
    /// Problems at or below this item (of the chosen kind).
    pub problems: u64,
    /// This item's own problems (of the chosen kind).
    pub own: Vec<RowProblem>,
    /// Items below it have problems: it can be opened.
    pub has_children: bool,
}

/// The result of [`run`]: the tree, every problem per item, and the report to save.
pub struct Readiness {
    tree: Tree,
    findings: HashMap<NodeId, Vec<RowProblem>>,
    /// Problems at or below each item, per kind; items without any are absent.
    below: HashMap<NodeId, [u64; KIND_COUNT]>,
    report: ReadinessReport,
}

/// Scans `root`, then checks every name and path in the tree.
pub fn run(root: &Path, options: &Options, state: &ReadinessState) -> Result<Readiness, ScanError> {
    let started = Instant::now();
    let tree = scan(root, &state.scan)?;
    state.set_phase(Phase::Names);
    let findings = name_pass(&tree, options);
    if state.cancel.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(Readiness::new(tree, findings, options, elapsed_ms))
}

/// What a path at the destination starts with: the destination root when given, else the source.
fn base_text(tree: &Tree, options: &Options) -> String {
    let destination = options.destination_root.trim();
    let base = if destination.is_empty() {
        tree.root().name.as_str()
    } else {
        destination
    };
    base.trim_end_matches(['\\', '/']).to_owned()
}

/// Every name, length, link and unreadable folder problem, per item.
fn name_pass(tree: &Tree, options: &Options) -> HashMap<NodeId, Vec<RowProblem>> {
    let limit = options.target.path_limit();
    let mut findings: HashMap<NodeId, Vec<RowProblem>> = HashMap::new();
    // UTF-16 length of each item's path at the destination. Parents come before their children
    // in the tree, so one pass in id order fills it.
    let mut lengths = vec![0usize; tree.len()];
    lengths[0] = base_text(tree, options).encode_utf16().count();
    for index in 0..tree.len() {
        let id = index as NodeId;
        let node = tree.node(id);
        let mut own = Vec::new();
        if let Some(parent) = tree.parent(id) {
            let length = lengths[parent as usize] + 1 + node.name.encode_utf16().count();
            lengths[index] = length;
            let issues = if node.lossy_name {
                // The name holds replacement characters; checking it further would mislead.
                vec![NameIssue::NotUnicode]
            } else {
                name_issues(&node.name, options.target)
            };
            own.extend(issues.iter().map(RowProblem::from_issue));
            if length > limit {
                own.push(RowProblem::from_issue(&NameIssue::PathTooLong {
                    length,
                    limit,
                }));
            }
            if node.kind == Kind::Link {
                own.push(RowProblem::new(
                    ProblemKind::Link,
                    RULE_LINK,
                    "Not followed: check where it points and whether the copy should include it"
                        .to_owned(),
                    false,
                ));
            }
        }
        if let Some(error) = &node.error {
            own.push(RowProblem::new(
                ProblemKind::Unreadable,
                RULE_UNREADABLE,
                format!("{error}. Items in it are not checked"),
                false,
            ));
        }
        if !own.is_empty() {
            findings.insert(id, own);
        }
    }
    findings
}

impl Readiness {
    fn new(
        tree: Tree,
        findings: HashMap<NodeId, Vec<RowProblem>>,
        options: &Options,
        elapsed_ms: u64,
    ) -> Self {
        let mut below: HashMap<NodeId, [u64; KIND_COUNT]> = HashMap::new();
        for (&id, own) in &findings {
            let mut add = [0u64; KIND_COUNT];
            for problem in own {
                add[problem.kind.index()] += 1;
            }
            let mut current = Some(id);
            while let Some(at) = current {
                let counts = below.entry(at).or_default();
                for (count, extra) in counts.iter_mut().zip(add) {
                    *count += extra;
                }
                current = tree.parent(at);
            }
        }
        let report = build_report(&tree, &findings, options, elapsed_ms, PER_KIND_LIMIT);
        Self {
            tree,
            findings,
            below,
            report,
        }
    }

    pub fn report(&self) -> &ReadinessReport {
        &self.report
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The problems of one item (all kinds).
    pub fn own(&self, id: NodeId) -> &[RowProblem] {
        self.findings.get(&id).map_or(&[], Vec::as_slice)
    }

    fn count(&self, id: NodeId, kind: Option<ProblemKind>) -> u64 {
        self.below.get(&id).map_or(0, |counts| match kind {
            Some(kind) => counts[kind.index()],
            None => counts.iter().sum(),
        })
    }

    /// The items directly below `id` with problems (of `kind`, or any), most problems first.
    /// `None` when `id` is not in the tree.
    pub fn children(&self, id: NodeId, kind: Option<ProblemKind>) -> Option<Vec<Row>> {
        if id as usize >= self.tree.len() {
            return None;
        }
        let mut rows: Vec<Row> = self
            .tree
            .live_children(id)
            .filter_map(|child| {
                let problems = self.count(child, kind);
                if problems == 0 {
                    return None;
                }
                let own: Vec<RowProblem> = self
                    .findings
                    .get(&child)
                    .into_iter()
                    .flatten()
                    .filter(|problem| kind.is_none_or(|kind| problem.kind == kind))
                    .cloned()
                    .collect();
                let node = self.tree.node(child);
                Some(Row {
                    id: child,
                    name: node.name.clone(),
                    kind: node.kind,
                    problems,
                    has_children: problems > own.len() as u64,
                    own,
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            b.problems
                .cmp(&a.problems)
                .then_with(|| a.name.cmp(&b.name))
        });
        Some(rows)
    }

    /// Full path of an item, for Explorer. `None` for an item whose path holds a name that is not
    /// valid Unicode (it would point somewhere else).
    pub fn full_path(&self, id: NodeId) -> Option<PathBuf> {
        if id as usize >= self.tree.len() {
            return None;
        }
        let mut current = Some(id);
        while let Some(at) = current {
            if self.tree.node(at).lossy_name {
                return None;
            }
            current = self.tree.parent(at);
        }
        Some(self.tree.path(id))
    }
}

/// `a\b\c` for an item, relative to the source.
fn relative(tree: &Tree, id: NodeId) -> String {
    let mut names = Vec::new();
    let mut current = id;
    while let Some(parent) = tree.parent(current) {
        names.push(tree.node(current).name.as_str());
        current = parent;
    }
    names.reverse();
    names.join("\\")
}

fn build_report(
    tree: &Tree,
    findings: &HashMap<NodeId, Vec<RowProblem>>,
    options: &Options,
    elapsed_ms: u64,
    per_kind_limit: usize,
) -> ReadinessReport {
    let root = tree.root();
    let mut report = ReadinessReport {
        source: root.name.clone(),
        destination_root: options.destination_root.trim().to_owned(),
        target: options.target,
        files: root.files,
        dirs: root.dirs,
        size: root.size,
        cloud_files: root.cloud_files,
        elapsed_ms,
        ..ReadinessReport::default()
    };
    let mut counts: HashMap<(ProblemKind, &str), (u64, bool)> = HashMap::new();
    let mut per_kind: HashMap<ProblemKind, Vec<(NodeId, &RowProblem)>> = HashMap::new();
    for (&id, own) in findings {
        if own.iter().any(|problem| problem.kind == ProblemKind::Name) {
            report.name_problems += 1;
        }
        for problem in own {
            let entry = counts
                .entry((problem.kind, problem.rule.as_str()))
                .or_insert((0, problem.blocker));
            entry.0 += 1;
            report.problems_total += 1;
            report.blockers += u64::from(problem.blocker);
            match problem.kind {
                ProblemKind::LongPath => report.long_paths += 1,
                ProblemKind::Link => report.links += 1,
                ProblemKind::Unreadable => report.unreadable += 1,
                _ => {}
            }
            per_kind
                .entry(problem.kind)
                .or_default()
                .push((id, problem));
        }
    }
    let mut counts: Vec<RuleCount> = counts
        .into_iter()
        .map(|((kind, rule), (count, blocker))| RuleCount {
            kind,
            rule: rule.to_owned(),
            count,
            blocker,
        })
        .collect();
    counts.sort_by(|a, b| {
        (a.kind.index(), std::cmp::Reverse(a.count), &a.rule).cmp(&(
            b.kind.index(),
            std::cmp::Reverse(b.count),
            &b.rule,
        ))
    });
    report.counts = counts;
    for kind in ProblemKind::ALL {
        let Some(items) = per_kind.remove(&kind) else {
            continue;
        };
        let mut listed: Vec<Problem> = items
            .into_iter()
            .map(|(id, problem)| Problem {
                path: relative(tree, id),
                dir: tree.node(id).kind == Kind::Dir,
                kind,
                rule: problem.rule.clone(),
                hint: problem.hint.clone(),
                blocker: problem.blocker,
            })
            .collect();
        listed.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.rule.cmp(&b.rule)));
        listed.truncate(per_kind_limit);
        report.problems.extend(listed);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn check(root: &Path, options: &Options) -> Readiness {
        run(root, options, &ReadinessState::new(CancelToken::new())).unwrap()
    }

    fn rules_for<'a>(report: &'a ReadinessReport, path: &str) -> Vec<&'a str> {
        report
            .problems
            .iter()
            .filter(|problem| problem.path == path)
            .map(|problem| problem.rule.as_str())
            .collect()
    }

    #[test]
    fn bad_names_are_listed_with_their_rule_and_hint() {
        let temp = tempfile::tempdir().unwrap();
        let sub = temp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("CON.txt"), b"x").unwrap();
        fs::write(sub.join("ends."), b"x").unwrap();
        fs::write(sub.join("a?b*.txt"), b"x").unwrap();
        fs::write(temp.path().join("fine.txt"), b"x").unwrap();

        let result = check(temp.path(), &Options::default());
        let report = result.report();
        assert_eq!(rules_for(report, r"sub\CON.txt"), ["reserved"]);
        assert_eq!(rules_for(report, r"sub\ends."), ["trailingDot"]);
        assert_eq!(
            rules_for(report, r"sub\a?b*.txt"),
            ["invalidCharacter", "invalidCharacter"]
        );
        assert!(rules_for(report, "fine.txt").is_empty());
        assert_eq!(report.name_problems, 3);
        assert_eq!(report.blockers, 4);
        assert_eq!(report.count_of(ProblemKind::Name), 4);
        assert!(report.has_problems());
        let reserved = report.listed(ProblemKind::Name).next().unwrap();
        assert_eq!(reserved.hint, "Rename it: CON is a device name on Windows");
        assert!(!reserved.dir);
        let invalid = report
            .counts
            .iter()
            .find(|count| count.rule == "invalidCharacter")
            .unwrap();
        assert_eq!((invalid.count, invalid.blocker), (2, true));
    }

    #[test]
    fn sharepoint_rules_apply_only_to_the_sharepoint_profile() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("desktop.ini"), b"x").unwrap();
        fs::write(temp.path().join("~$report.docx"), b"x").unwrap();

        let server = check(temp.path(), &Options::default());
        assert!(server.report().problems.is_empty());
        assert!(!server.report().has_problems());

        let options = Options {
            target: TargetProfile::SharePoint,
            ..Options::default()
        };
        let report = check(temp.path(), &options).report().clone();
        assert_eq!(rules_for(&report, "desktop.ini"), ["sharePointName"]);
        assert_eq!(
            rules_for(&report, "~$report.docx"),
            ["sharePointOfficeLock"]
        );
        // The lock file is a warning, not a blocker.
        assert_eq!(report.blockers, 1);
    }

    /// A tree whose one file has a path exactly `source_length` long at the source, under
    /// folders of 100 characters (names are capped at 255 bytes on Linux). Returns the file's
    /// path relative to the root.
    fn file_at_length(source_length: usize) -> (tempfile::TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        let root_length = temp.path().display().to_string().encode_utf16().count();
        let mut left = source_length - root_length - 1;
        let mut folder = temp.path().to_path_buf();
        let mut names = Vec::new();
        while left > 150 {
            let name = "d".repeat(100);
            folder.push(&name);
            names.push(name);
            left -= 101;
        }
        fs::create_dir_all(&folder).unwrap();
        let name = "n".repeat(left);
        fs::write(folder.join(&name), b"x").unwrap();
        names.push(name);
        (temp, names.join("\\"))
    }

    #[test]
    fn long_paths_are_measured_at_the_destination_when_one_is_given() {
        let (temp, name) = file_at_length(250);
        let at_source = check(temp.path(), &Options::default());
        assert_eq!(at_source.report().long_paths, 0);

        // 270 at the destination: the root is 20 characters longer than the source's.
        let root_length = temp.path().display().to_string().encode_utf16().count();
        let destination = format!(r"D:\{}", "d".repeat(root_length + 20 - 3));
        let options = Options {
            destination_root: format!("{destination}\\"),
            ..Options::default()
        };
        let result = check(temp.path(), &options);
        let report = result.report();
        assert_eq!(report.long_paths, 1);
        assert_eq!(report.destination_root, format!("{destination}\\"));
        let problem = report.listed(ProblemKind::LongPath).next().unwrap();
        assert_eq!(problem.path, name);
        assert_eq!(problem.rule, "pathTooLong");
        assert_eq!(problem.hint, "Shorten by 11 characters");
        assert!(!problem.blocker);
    }

    #[test]
    fn the_sharepoint_profile_allows_longer_paths() {
        let (temp, _) = file_at_length(300);
        assert_eq!(
            check(temp.path(), &Options::default()).report().long_paths,
            1
        );
        let options = Options {
            target: TargetProfile::SharePoint,
            ..Options::default()
        };
        assert_eq!(check(temp.path(), &options).report().long_paths, 0);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_listed_as_not_followed_and_never_entered() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("CON.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).unwrap();

        let result = check(temp.path(), &Options::default());
        let report = result.report();
        assert_eq!(rules_for(report, "link"), ["link"]);
        assert_eq!(report.links, 1);
        // Nothing behind the link was checked.
        assert_eq!(report.problems.len(), 1);
        // A link alone is not a reason to hold the copy.
        assert!(!report.has_problems());
        let rows = result.children(0, None).unwrap();
        assert_eq!(rows[0].kind, Kind::Link);
        assert!(!rows[0].has_children);
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_unicode_is_reported_and_not_built_into_a_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let bad = temp.path().join(OsStr::from_bytes(b"bad\xff dir"));
        fs::create_dir(&bad).unwrap();
        fs::write(bad.join("CON"), b"x").unwrap();

        let result = check(temp.path(), &Options::default());
        let report = result.report();
        // Named once, and listed as not read: the scan does not open such a folder.
        let rules: Vec<&str> = report.problems.iter().map(|p| p.rule.as_str()).collect();
        assert_eq!(rules, ["notUnicode", "unreadable"]);
        assert_eq!(report.problems[0].hint, NameIssue::NotUnicode.hint());
        let row = &result.children(0, None).unwrap()[0];
        assert_eq!(result.full_path(row.id), None);
        assert!(result.full_path(0).is_some());
    }

    #[test]
    fn children_open_one_level_at_a_time_and_filter_by_kind() {
        let temp = tempfile::tempdir().unwrap();
        let deep = temp.path().join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir(temp.path().join("clean")).unwrap();
        fs::write(temp.path().join("clean").join("ok.txt"), b"x").unwrap();
        fs::write(deep.join("NUL"), b"x").unwrap();
        fs::write(deep.join("x."), b"x").unwrap();
        fs::write(temp.path().join("top "), b"x").unwrap();

        let result = check(temp.path(), &Options::default());
        let top = result.children(0, None).unwrap();
        let names: Vec<&str> = top.iter().map(|row| row.name.as_str()).collect();
        // Folders without problems are left out; most problems first.
        assert_eq!(names, ["a", "top "]);
        assert_eq!(top[0].problems, 2);
        assert!(top[0].own.is_empty());
        assert!(top[0].has_children);
        assert_eq!(top[1].own[0].rule, "trailingSpace");
        assert_eq!(top[1].own[0].label, "Ends with a space");
        assert!(!top[1].has_children);

        let b = &result.children(top[0].id, None).unwrap()[0];
        let files = result.children(b.id, None).unwrap();
        assert_eq!(files.len(), 2);

        let only_long = result.children(0, Some(ProblemKind::LongPath)).unwrap();
        assert!(only_long.is_empty());
        assert_eq!(result.children(u32::MAX, None), None);
    }

    #[test]
    fn the_saved_list_is_capped_per_kind_but_the_counts_are_not() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..5 {
            fs::write(temp.path().join(format!("f{index}.")), b"x").unwrap();
        }
        let result = check(temp.path(), &Options::default());
        let report = build_report(result.tree(), &result.findings, &Options::default(), 0, 2);
        assert_eq!(report.problems.len(), 2);
        assert_eq!(report.problems[0].path, "f0.");
        assert_eq!(report.count_of(ProblemKind::Name), 5);
        assert_eq!(report.problems_total, 5);
    }

    #[test]
    fn a_cancelled_check_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let state = ReadinessState::new(cancel);
        assert!(matches!(
            run(temp.path(), &Options::default(), &state),
            Err(ScanError::Cancelled)
        ));
    }

    #[test]
    fn saved_reports_load_from_older_and_newer_versions() {
        assert_eq!(
            serde_json::from_str::<ReadinessReport>("{}").unwrap(),
            ReadinessReport::default()
        );
        let newer = r#"{"source":"S","futureField":1,"problems":[{"path":"a","kind":"hologram","rule":"r9"}]}"#;
        let report: ReadinessReport = serde_json::from_str(newer).unwrap();
        assert_eq!(report.problems[0].kind, ProblemKind::Other);
        assert_eq!(rule_label("r9"), "r9");
        let options: Options = serde_json::from_str("{}").unwrap();
        assert_eq!(options.target, TargetProfile::FileServer);
    }
}

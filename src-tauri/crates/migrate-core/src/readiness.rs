//! Migration readiness: one scan of the source, then a pass over the tree for everything that will
//! break the copy or the target.
//!
//! The name pass checks every name against the target's rules ([`scan_core::name_issues`]) and
//! every path's length as it will be at the destination. Links are listed as "not followed" (the
//! scan never enters them), and folders the scan could not read are listed with the reason.
//! Online-only files are checked by name only.
//!
//! The in-use pass (on unless turned off) opens every other file for read with full sharing
//! through [`scan_core::inuse::probe`], in parallel, and asks Restart Manager who holds the first
//! [`HOLDER_CAP`] files it finds in use. It never opens a link, an online-only file or a file whose
//! name is not valid Unicode, and it never reads a byte.

use scan_core::inuse::{self, Holder, Holders, InUse, HOLDER_CAP, HOLDER_DEADLINE};
use scan_core::{
    name_issues, scan, CancelToken, Kind, NameIssue, NodeId, ScanError, ScanProgress, ScanState,
    TargetProfile, Tree,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Most problems of one kind a saved report keeps; the counts always cover every problem.
pub const PER_KIND_LIMIT: usize = 20_000;

/// What to check and where the files are going.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub target: TargetProfile,
    /// The planned destination folder (`D:\Shares\Finance`). Path lengths are measured as they
    /// will be there. Empty: measured at the source. It is only used as text, never opened.
    pub destination_root: String,
    /// Open every file to find the ones another program holds (D-08). On by default.
    pub check_in_use: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            target: TargetProfile::default(),
            destination_root: String::new(),
            check_in_use: true,
        }
    }
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
    /// Another program holds the file open without sharing.
    InUse,
    /// A folder the scan could not read completely, or a file the in-use pass could not open.
    Unreadable,
    /// A symlink or junction: listed, never entered.
    Link,
    /// Written by a newer version; shown as is.
    #[serde(other)]
    Other,
}

/// How many kinds [`ProblemKind::index`] numbers.
const KIND_COUNT: usize = 6;

impl ProblemKind {
    /// Every kind the checks produce, in the order the report shows them.
    pub const ALL: [ProblemKind; 5] = [
        Self::Name,
        Self::LongPath,
        Self::InUse,
        Self::Unreadable,
        Self::Link,
    ];

    fn index(self) -> usize {
        match self {
            Self::Name => 0,
            Self::LongPath => 1,
            Self::Unreadable => 2,
            Self::Link => 3,
            Self::Other => 4,
            Self::InUse => 5,
        }
    }

    /// The heading of the group.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Name => "Names the target cannot hold",
            Self::LongPath => "Paths too long",
            Self::InUse => "Files in use",
            Self::Unreadable => "Could not be read",
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
const RULE_IN_USE: &str = "inUse";
const RULE_OPEN_FAILED: &str = "openFailed";

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
        RULE_IN_USE => "Open in another program",
        RULE_OPEN_FAILED => "Could not be opened",
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
    /// In use: the programs Restart Manager named.
    pub holders: Vec<Holder>,
}

impl RowProblem {
    fn new(kind: ProblemKind, rule: &str, hint: String, blocker: bool) -> Self {
        Self {
            kind,
            rule: rule.to_owned(),
            label: rule_label(rule).to_owned(),
            hint,
            blocker,
            holders: Vec::new(),
        }
    }

    fn in_use(holders: &Holders) -> Self {
        let mut problem = Self::new(ProblemKind::InUse, RULE_IN_USE, in_use_hint(holders), false);
        if let Holders::Found { holders } = holders {
            problem.holders = holders.clone();
        }
        problem
    }

    fn open_failed(code: i32) -> Self {
        Self::new(
            ProblemKind::Unreadable,
            RULE_OPEN_FAILED,
            format!("Windows error {code} when opening it; the copy will likely fail on it too"),
            false,
        )
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
    pub holders: Vec<Holder>,
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
    /// What the in-use pass did.
    pub in_use: InUseSummary,
}

/// Whether the in-use pass ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InUseStatus {
    /// Turned off for this check (and what reports from before the pass existed load as).
    #[default]
    Off,
    Checked,
    /// Not a Windows system: nothing was opened.
    NotSupported,
    /// Written by a newer version.
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InUseSummary {
    pub status: InUseStatus,
    /// Files opened.
    pub checked: u64,
    pub in_use: u64,
    /// Files that could not be opened for another reason.
    pub unreadable: u64,
    /// Links, online-only files and files whose name is not valid Unicode: never opened.
    pub skipped: u64,
    /// In-use files Restart Manager was asked about (local files, up to [`HOLDER_CAP`]).
    pub holders_asked: u64,
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
    InUse,
}

/// Live state of one check. The check updates it; another thread reads
/// [`ReadinessState::progress`].
#[derive(Debug)]
pub struct ReadinessState {
    cancel: CancelToken,
    scan: ScanState,
    phase: AtomicU8,
    to_check: AtomicU64,
    checked: AtomicU64,
    in_use: AtomicU64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessProgress {
    pub phase: Phase,
    pub scan: ScanProgress,
    /// In-use pass: files to open, opened so far, and found in use.
    pub to_check: u64,
    pub checked: u64,
    pub in_use: u64,
}

impl ReadinessState {
    pub fn new(cancel: CancelToken) -> Self {
        Self {
            scan: ScanState::new(cancel.clone()),
            cancel,
            phase: AtomicU8::new(0),
            to_check: AtomicU64::new(0),
            checked: AtomicU64::new(0),
            in_use: AtomicU64::new(0),
        }
    }

    fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    pub fn progress(&self) -> ReadinessProgress {
        ReadinessProgress {
            phase: match self.phase.load(Ordering::Relaxed) {
                0 => Phase::Scan,
                1 => Phase::Names,
                _ => Phase::InUse,
            },
            scan: self.scan.progress(),
            to_check: self.to_check.load(Ordering::Relaxed),
            checked: self.checked.load(Ordering::Relaxed),
            in_use: self.in_use.load(Ordering::Relaxed),
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

/// Scans `root`, then checks every name and path in the tree, then (when asked, on Windows)
/// which files are in use.
pub fn run(root: &Path, options: &Options, state: &ReadinessState) -> Result<Readiness, ScanError> {
    let probes = Probes {
        supported: cfg!(windows),
        probe: &|path: &Path| inuse::probe(&exact_path(path)),
        holders: &|path: &Path| {
            inuse::holders(&[path.to_path_buf()], 1, Duration::MAX)
                .pop()
                .unwrap_or(Holders::NotChecked)
        },
        cap: HOLDER_CAP,
        deadline: HOLDER_DEADLINE,
    };
    run_with(root, options, state, &probes)
}

/// The `\\?\` form of an absolute Windows path, so a name Win32 would change (`ends.`, `top `,
/// `CON`) is opened exactly as listed instead of as the file it normalizes to. Other paths, and
/// every path off Windows, are returned as they are.
fn exact_path(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    verbatim_form(&path.to_string_lossy()).map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// [`exact_path`] on text: `None` when the path is already verbatim or is not absolute.
fn verbatim_form(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        None
    } else if let Some(share) = text.strip_prefix(r"\\") {
        Some(format!(r"\\?\UNC\{share}"))
    } else if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == br":\" {
        Some(format!(r"\\?\{text}"))
    } else {
        None
    }
}

/// The Windows calls behind the in-use pass, passed in so the pass is tested everywhere.
struct Probes<'a> {
    supported: bool,
    probe: &'a (dyn Fn(&Path) -> InUse + Sync),
    /// Who holds one local file.
    holders: &'a dyn Fn(&Path) -> Holders,
    cap: usize,
    deadline: Duration,
}

fn run_with(
    root: &Path,
    options: &Options,
    state: &ReadinessState,
    probes: &Probes,
) -> Result<Readiness, ScanError> {
    let started = Instant::now();
    let tree = scan(root, &state.scan)?;
    state.set_phase(Phase::Names);
    let mut findings = name_pass(&tree, options);
    if state.cancel.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    let in_use = if !options.check_in_use {
        InUseSummary::default()
    } else if !probes.supported {
        InUseSummary {
            status: InUseStatus::NotSupported,
            ..InUseSummary::default()
        }
    } else {
        state.set_phase(Phase::InUse);
        in_use_pass(&tree, state, probes, &mut findings)?
    };
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut result = Readiness::new(tree, findings, options, elapsed_ms);
    result.report.in_use = in_use;
    Ok(result)
}

/// Files the in-use pass may open: never a link, an online-only file (that would download it) or
/// a name that is not valid Unicode (the path would point elsewhere).
fn is_probed(kind: Kind, cloud: bool, lossy_name: bool) -> bool {
    kind == Kind::File && !cloud && !lossy_name
}

/// How many threads open files at once. Opening is mostly waiting on the disk or the network.
const PROBE_THREADS: usize = 8;

/// Opens every file that may be opened, in parallel, and adds an in-use or could-not-open problem
/// to each file that needs one. Stops at the next file once cancelled.
fn in_use_pass(
    tree: &Tree,
    state: &ReadinessState,
    probes: &Probes,
    findings: &mut HashMap<NodeId, Vec<RowProblem>>,
) -> Result<InUseSummary, ScanError> {
    let mut summary = InUseSummary {
        status: InUseStatus::Checked,
        ..InUseSummary::default()
    };
    let mut files = Vec::new();
    for index in 1..tree.len() {
        let id = index as NodeId;
        let node = tree.node(id);
        if is_probed(node.kind, node.cloud_files > 0, node.lossy_name) {
            files.push(id);
        } else if node.kind != Kind::Dir {
            summary.skipped += 1;
        }
    }
    state.to_check.store(files.len() as u64, Ordering::Relaxed);
    let next = AtomicUsize::new(0);
    let found: Mutex<Vec<(NodeId, InUse)>> = Mutex::new(Vec::new());
    let threads = PROBE_THREADS.min(files.len()).max(1);
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut mine = Vec::new();
                while !state.cancel.is_cancelled() {
                    let Some(&id) = files.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let outcome = (probes.probe)(&tree.path(id));
                    state.checked.fetch_add(1, Ordering::Relaxed);
                    if outcome == InUse::InUse {
                        state.in_use.fetch_add(1, Ordering::Relaxed);
                    }
                    if matches!(outcome, InUse::InUse | InUse::Unreadable { .. }) {
                        mine.push((id, outcome));
                    }
                }
                if let Ok(mut found) = found.lock() {
                    found.extend(mine);
                }
            });
        }
    });
    if state.cancel.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    summary.checked = state.checked.load(Ordering::Relaxed);
    let mut found = found.into_inner().unwrap_or_default();
    // Walk order, so the first files asked about do not depend on thread timing.
    found.sort_unstable_by_key(|(id, _)| *id);
    let mut held = Vec::new();
    for (id, outcome) in found {
        match outcome {
            InUse::InUse => held.push(id),
            InUse::Unreadable { code } => {
                summary.unreadable += 1;
                findings
                    .entry(id)
                    .or_default()
                    .push(RowProblem::open_failed(code));
            }
            InUse::Free | InUse::NotChecked => {}
        }
    }
    summary.in_use = held.len() as u64;
    let paths: Vec<PathBuf> = held.iter().map(|&id| tree.path(id)).collect();
    let (holders, asked) = holder_pass(&paths, probes, &state.cancel)?;
    summary.holders_asked = asked;
    for (id, holders) in held.into_iter().zip(holders) {
        findings
            .entry(id)
            .or_default()
            .push(RowProblem::in_use(&holders));
    }
    Ok(summary)
}

/// Who holds each in-use file: Restart Manager for local files, one at a time, for at most
/// `probes.cap` files and `probes.deadline` (D-08); files on a share cost nothing. Returns the
/// holders in order and how many files were asked about.
fn holder_pass(
    paths: &[PathBuf],
    probes: &Probes,
    cancel: &CancelToken,
) -> Result<(Vec<Holders>, u64), ScanError> {
    let started = Instant::now();
    let mut asked = 0u64;
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        if cancel.is_cancelled() {
            return Err(ScanError::Cancelled);
        }
        out.push(if inuse::is_remote(path) {
            Holders::Remote
        } else if asked as usize >= probes.cap || started.elapsed() >= probes.deadline {
            Holders::NotChecked
        } else {
            asked += 1;
            (probes.holders)(path)
        });
    }
    Ok((out, asked))
}

/// What to tell the reader about one in-use file.
fn in_use_hint(holders: &Holders) -> String {
    const ADVICE: &str =
        "close it before the copy, or copy with another tool and record it in Watch mode";
    match holders {
        Holders::Found { holders } if !holders.is_empty() => {
            let names: Vec<String> = holders
                .iter()
                .map(|holder| format!("{} (PID {})", holder.app, holder.pid))
                .collect();
            format!("Held by {}: {ADVICE}", names.join(", "))
        }
        Holders::Found { .. } => {
            format!("Held by a program Windows could not name (perhaps over the network): {ADVICE}")
        }
        Holders::Remote => format!("Held on the server (program unknown): {ADVICE}"),
        Holders::NotChecked => {
            format!("Held by another program (only the first {HOLDER_CAP} are looked up): {ADVICE}")
        }
        Holders::Failed { code } => {
            format!("Held by another program (Restart Manager failed with error {code}): {ADVICE}")
        }
        Holders::NotSupported => format!("Held by another program: {ADVICE}"),
    }
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
                ProblemKind::Unreadable if problem.rule == RULE_UNREADABLE => {
                    report.unreadable += 1;
                }
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
                holders: problem.holders.clone(),
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

    /// Where a test writes names Win32 would change or drop (`CON`, `NUL`, `ends.`, `top `): the
    /// verbatim form of the temp folder on Windows (`\\?\C:\...`), under which a joined name
    /// stays exactly as written. The check itself scans the plain path, as the command does.
    fn verbatim(temp: &tempfile::TempDir) -> PathBuf {
        fs::canonicalize(temp.path()).unwrap()
    }

    /// Deletes the fixture through the verbatim root, which can also delete `CON` and `NUL`.
    /// A failure is only a leftover temp folder.
    fn remove(root: &Path) {
        let _ = fs::remove_dir_all(root);
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
    fn absolute_paths_are_opened_in_their_verbatim_form() {
        assert_eq!(
            verbatim_form(r"C:\data\ends.").as_deref(),
            Some(r"\\?\C:\data\ends.")
        );
        assert_eq!(
            verbatim_form(r"\\srv\share\top ").as_deref(),
            Some(r"\\?\UNC\srv\share\top ")
        );
        assert_eq!(verbatim_form(r"\\?\C:\data\CON"), None);
        assert_eq!(verbatim_form(r"data\file.txt"), None);
        assert_eq!(verbatim_form(""), None);
    }

    #[test]
    fn bad_names_are_listed_with_their_rule_and_hint() {
        let temp = tempfile::tempdir().unwrap();
        let root = verbatim(&temp);
        let sub = root.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("CON.txt"), b"x").unwrap();
        fs::write(sub.join("ends."), b"x").unwrap();
        fs::write(root.join("fine.txt"), b"x").unwrap();

        let result = check(temp.path(), &Options::default());
        let report = result.report().clone();
        remove(&root);
        // The names come back exactly as written.
        assert_eq!(rules_for(&report, r"sub\CON.txt"), ["reserved"]);
        assert_eq!(rules_for(&report, r"sub\ends."), ["trailingDot"]);
        assert!(rules_for(&report, "fine.txt").is_empty());
        assert_eq!(report.name_problems, 2);
        assert_eq!(report.blockers, 2);
        assert_eq!(report.count_of(ProblemKind::Name), 2);
        assert!(report.has_problems());
        let reserved = report.listed(ProblemKind::Name).next().unwrap();
        assert_eq!(reserved.hint, "Rename it: CON is a device name on Windows");
        assert!(!reserved.dir);
    }

    /// NTFS refuses `?` and `*` in a name even through `\\?\`, so this case only runs where the
    /// file can exist (a Linux or macOS source, or a NAS export seen from one). The rule itself
    /// is tested on every platform in `scan_core::names`.
    #[cfg(not(windows))]
    #[test]
    fn invalid_characters_are_listed_once_each() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a?b*?.txt"), b"x").unwrap();

        let result = check(temp.path(), &Options::default());
        let report = result.report();
        assert_eq!(
            rules_for(report, "a?b*?.txt"),
            ["invalidCharacter", "invalidCharacter"]
        );
        let invalid = report
            .counts
            .iter()
            .find(|count| count.rule == "invalidCharacter")
            .unwrap();
        assert_eq!((invalid.count, invalid.blocker), (2, true));
        assert_eq!(report.name_problems, 1);
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
        let root = verbatim(&temp);
        let deep = root.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir(root.join("clean")).unwrap();
        fs::write(root.join("clean").join("ok.txt"), b"x").unwrap();
        fs::write(deep.join("NUL"), b"x").unwrap();
        fs::write(deep.join("x."), b"x").unwrap();
        fs::write(root.join("top "), b"x").unwrap();

        let result = check(temp.path(), &Options::default());
        remove(&root);
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
        let mut names: Vec<&str> = files.iter().map(|row| row.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["NUL", "x."]);

        let only_long = result.children(0, Some(ProblemKind::LongPath)).unwrap();
        assert!(only_long.is_empty());
        assert_eq!(result.children(u32::MAX, None), None);
    }

    #[test]
    fn the_saved_list_is_capped_per_kind_but_the_counts_are_not() {
        let temp = tempfile::tempdir().unwrap();
        let root = verbatim(&temp);
        for index in 0..5 {
            fs::write(root.join(format!("f{index}.")), b"x").unwrap();
        }
        let result = check(temp.path(), &Options::default());
        remove(&root);
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
        assert!(options.check_in_use, "the in-use pass is on by default");
        assert_eq!(report.in_use, InUseSummary::default());
        let status: InUseStatus = serde_json::from_str(r#""someday""#).unwrap();
        assert_eq!(status, InUseStatus::Other);
    }

    /// Fake Windows calls: a file named `locked*` is in use, `denied*` cannot be opened. Every
    /// opened path is recorded.
    struct Fake {
        opened: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                opened: Mutex::new(Vec::new()),
            }
        }

        fn probe(&self, path: &Path) -> InUse {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            self.opened.lock().unwrap().push(name.clone());
            if name.starts_with("locked") {
                InUse::InUse
            } else if name.starts_with("denied") {
                InUse::Unreadable { code: 5 }
            } else {
                InUse::Free
            }
        }

        fn opened(&self) -> Vec<String> {
            let mut opened = self.opened.lock().unwrap().clone();
            opened.sort();
            opened
        }
    }

    fn outlook(_: &Path) -> Holders {
        Holders::Found {
            holders: vec![Holder {
                app: "OUTLOOK.EXE".into(),
                pid: 4120,
            }],
        }
    }

    fn check_with(root: &Path, options: &Options, fake: &Fake, supported: bool) -> Readiness {
        let probe = |path: &Path| fake.probe(path);
        let probes = Probes {
            supported,
            probe: &probe,
            holders: &outlook,
            cap: HOLDER_CAP,
            deadline: HOLDER_DEADLINE,
        };
        let state = ReadinessState::new(CancelToken::new());
        run_with(root, options, &state, &probes).unwrap()
    }

    #[test]
    fn files_in_use_are_listed_with_the_program_that_holds_them() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("mail")).unwrap();
        fs::write(temp.path().join("mail").join("locked.pst"), b"x").unwrap();
        fs::write(temp.path().join("denied.txt"), b"x").unwrap();
        fs::write(temp.path().join("free.txt"), b"x").unwrap();

        let fake = Fake::new();
        let result = check_with(temp.path(), &Options::default(), &fake, true);
        assert_eq!(fake.opened(), ["denied.txt", "free.txt", "locked.pst"]);
        let report = result.report();
        assert_eq!(
            report.in_use,
            InUseSummary {
                status: InUseStatus::Checked,
                checked: 3,
                in_use: 1,
                unreadable: 1,
                skipped: 0,
                holders_asked: 1,
            }
        );
        let held = report.listed(ProblemKind::InUse).next().unwrap();
        assert_eq!(held.path, r"mail\locked.pst");
        assert_eq!(
            held.hint,
            "Held by OUTLOOK.EXE (PID 4120): close it before the copy, or copy with another tool and record it in Watch mode"
        );
        assert_eq!(held.holders[0].pid, 4120);
        assert!(!held.blocker);
        let denied = report.listed(ProblemKind::Unreadable).next().unwrap();
        assert_eq!(denied.rule, "openFailed");
        // A file that could not be opened is not an unreadable folder.
        assert_eq!(report.unreadable, 0);
        assert!(report.has_problems());
        let mail = &result.children(0, Some(ProblemKind::InUse)).unwrap()[0];
        assert_eq!((mail.name.as_str(), mail.problems), ("mail", 1));
    }

    #[test]
    fn links_online_only_files_and_lossy_names_are_never_opened() {
        assert!(is_probed(Kind::File, false, false));
        assert!(!is_probed(Kind::File, true, false), "online-only");
        assert!(!is_probed(Kind::File, false, true), "not valid Unicode");
        assert!(!is_probed(Kind::Link, false, false));
        assert!(!is_probed(Kind::Dir, false, false));
    }

    #[cfg(unix)]
    #[test]
    fn the_in_use_pass_skips_links_and_lossy_names_in_a_real_tree() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("target.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(temp.path().join("target.txt"), temp.path().join("link"))
            .unwrap();
        fs::write(temp.path().join(OsStr::from_bytes(b"locked\xff")), b"x").unwrap();

        let fake = Fake::new();
        let result = check_with(temp.path(), &Options::default(), &fake, true);
        assert_eq!(fake.opened(), ["target.txt"]);
        assert_eq!(result.report().in_use.skipped, 2);
        assert_eq!(result.report().in_use.in_use, 0);
    }

    #[test]
    fn the_pass_is_off_when_asked_and_not_checked_off_windows() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("locked.pst"), b"x").unwrap();
        let fake = Fake::new();
        let off = Options {
            check_in_use: false,
            ..Options::default()
        };
        let report = check_with(temp.path(), &off, &fake, true).report().clone();
        assert_eq!(report.in_use.status, InUseStatus::Off);
        let report = check_with(temp.path(), &Options::default(), &fake, false)
            .report()
            .clone();
        assert_eq!(report.in_use.status, InUseStatus::NotSupported);
        assert!(fake.opened().is_empty());
        assert!(!report.has_problems());
    }

    /// Windows CI only: the real probe and Restart Manager on a file this test holds.
    #[cfg(windows)]
    #[test]
    fn a_file_held_without_sharing_is_in_use_and_held_by_this_process() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let held = temp.path().join("held.pst");
        fs::write(&held, b"mail").unwrap();
        fs::write(temp.path().join("free.txt"), b"text").unwrap();
        let _lock = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&held)
            .unwrap();

        let result = check(temp.path(), &Options::default());
        let report = result.report();
        assert_eq!(report.in_use.status, InUseStatus::Checked);
        assert_eq!((report.in_use.checked, report.in_use.in_use), (2, 1));
        let problem = report.listed(ProblemKind::InUse).next().unwrap();
        assert_eq!(problem.path, "held.pst");
        assert!(
            problem
                .holders
                .iter()
                .any(|holder| holder.pid == std::process::id()),
            "{problem:?}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_real_pass_reports_not_supported_here() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"x").unwrap();
        let report = check(temp.path(), &Options::default()).report().clone();
        assert_eq!(report.in_use.status, InUseStatus::NotSupported);
    }

    #[test]
    fn cancelling_stops_the_pass_at_the_next_file() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..200 {
            fs::write(temp.path().join(format!("f{index}.txt")), b"x").unwrap();
        }
        let cancel = CancelToken::new();
        let state = ReadinessState::new(cancel.clone());
        let calls = AtomicUsize::new(0);
        let probe = |_: &Path| {
            calls.fetch_add(1, Ordering::SeqCst);
            cancel.cancel();
            InUse::Free
        };
        let probes = Probes {
            supported: true,
            probe: &probe,
            holders: &outlook,
            cap: HOLDER_CAP,
            deadline: HOLDER_DEADLINE,
        };
        let outcome = run_with(temp.path(), &Options::default(), &state, &probes);
        assert!(matches!(outcome, Err(ScanError::Cancelled)));
        // Each thread finishes at most the file it had started.
        assert!(calls.load(Ordering::SeqCst) <= PROBE_THREADS);
        let progress = state.progress();
        assert_eq!(progress.phase, Phase::InUse);
        assert_eq!(progress.to_check, 200);
    }

    #[test]
    fn holders_are_asked_for_the_first_local_files_only() {
        let asked = Mutex::new(0);
        let lookup = |path: &Path| {
            *asked.lock().unwrap() += 1;
            outlook(path)
        };
        let probe = |_: &Path| InUse::Free;
        let mut probes = Probes {
            supported: true,
            probe: &probe,
            holders: &lookup,
            cap: 2,
            deadline: HOLDER_DEADLINE,
        };
        let paths: Vec<PathBuf> = [r"\\nas\share\a.pst", "/a", "/b", "/c"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let cancel = CancelToken::new();
        let (holders, count) = holder_pass(&paths, &probes, &cancel).unwrap();
        assert_eq!(holders[0], Holders::Remote);
        assert!(matches!(holders[1], Holders::Found { .. }));
        assert!(matches!(holders[2], Holders::Found { .. }));
        assert_eq!(holders[3], Holders::NotChecked);
        assert_eq!((count, *asked.lock().unwrap()), (2, 2));
        assert_eq!(
            in_use_hint(&holders[0]),
            "Held on the server (program unknown): close it before the copy, or copy with another tool and record it in Watch mode"
        );
        assert!(in_use_hint(&holders[3]).contains("only the first 500"));

        probes.deadline = Duration::ZERO;
        let (holders, count) = holder_pass(&paths[1..], &probes, &cancel).unwrap();
        assert!(holders.iter().all(|holder| *holder == Holders::NotChecked));
        assert_eq!(count, 0);

        cancel.cancel();
        assert!(matches!(
            holder_pass(&paths, &probes, &cancel),
            Err(ScanError::Cancelled)
        ));
    }
}

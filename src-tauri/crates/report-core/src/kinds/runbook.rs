//! The Migration runbook: one document from a readiness report, the Record runs of the copy and
//! the compare reports after it.
//!
//! A runbook saves ids and nothing else (the job fields are the report's own). The document is
//! built from the parts when it is opened or exported, so a part that was deleted since shows as
//! "Report no longer available" and the rest still exports.

use super::*;
use crate::header;
use std::collections::HashSet;

/// Most parts one runbook holds; the page it makes is already long at this size.
pub const MAX_PARTS: usize = 24;
/// Runs followed back along a delta chain before the table stops.
const MAX_CHAIN: usize = 25;

/// The ids of a runbook's parts, in the order they are shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RunbookReport {
    pub parts: Vec<RunbookPart>,
}

/// One report in a runbook: a readiness or compare report in the reports folder, or a Record run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunbookPart {
    pub kind: ReportKind,
    #[serde(default)]
    pub id: String,
}

/// Where a part sits in a migration, which is the order a runbook reads in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Before,
    Copy,
    After,
}

impl Stage {
    fn of(kind: ReportKind) -> Option<Stage> {
        match kind {
            ReportKind::Readiness => Some(Stage::Before),
            ReportKind::Record => Some(Stage::Copy),
            ReportKind::Compare => Some(Stage::After),
            ReportKind::DiskUsage | ReportKind::Cleanup | ReportKind::Runbook => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Stage::Before => "Before the copy",
            Stage::Copy => "The copy",
            Stage::After => "After the copy",
        }
    }
}

fn stage_label(kind: ReportKind) -> &'static str {
    Stage::of(kind).map_or("Other", Stage::label)
}

/// Checks the parts a runbook may hold (one readiness report at most, Record runs, compare
/// reports, none twice) and puts them in reading order; parts of one stage keep the order given.
pub(crate) fn order_parts(mut parts: Vec<RunbookPart>) -> Result<Vec<RunbookPart>, String> {
    if parts.is_empty() {
        return Err("Pick at least one report for the runbook.".into());
    }
    if parts.len() > MAX_PARTS {
        return Err(format!(
            "A runbook holds at most {MAX_PARTS} reports; this one has {}.",
            parts.len()
        ));
    }
    let mut seen = HashSet::new();
    for part in &parts {
        if Stage::of(part.kind).is_none() {
            return Err(
                "A runbook holds readiness reports, Record runs and compare reports only.".into(),
            );
        }
        if !seen.insert((part.kind, part.id.clone())) {
            return Err("The same report is in the runbook twice.".into());
        }
    }
    if parts
        .iter()
        .filter(|part| part.kind == ReportKind::Readiness)
        .count()
        > 1
    {
        return Err("A runbook has one readiness report.".into());
    }
    parts.sort_by_key(|part| Stage::of(part.kind));
    Ok(parts)
}

/// Whether the report or run behind `part` is there and is the kind the runbook says. Reads the
/// start of a report only.
pub(crate) fn part_exists(reports: &Path, records: &Path, part: &RunbookPart) -> bool {
    match part.kind {
        ReportKind::Record => RunStore::open(records, &part.id).is_ok(),
        ReportKind::Readiness | ReportKind::Compare => crate::report_path(reports, &part.id)
            .ok()
            .and_then(|path| header::read(&path, &part.id).ok())
            .is_some_and(|header| header.body.kind() == part.kind),
        _ => false,
    }
}

/// "Readiness · 2 copies · 1 compare": what a runbook holds.
pub(super) fn parts_summary(kinds: &[ReportKind]) -> String {
    let count = |wanted| kinds.iter().filter(|kind| **kind == wanted).count() as u64;
    let mut pieces = Vec::new();
    if count(ReportKind::Readiness) > 0 {
        pieces.push("Readiness".to_string());
    }
    match count(ReportKind::Record) {
        0 => {}
        1 => pieces.push("1 copy".to_string()),
        runs => pieces.push(format!("{} copies", format_count(runs))),
    }
    if count(ReportKind::Compare) > 0 {
        pieces.push(plural(count(ReportKind::Compare), "compare"));
    }
    if pieces.is_empty() {
        pieces.push("No reports".to_string());
    }
    pieces.join(" · ")
}

/// A part as the runbook shows it: its row on the Reports page and its own document.
pub(crate) struct PartDoc {
    pub entry: Entry,
    pub document: Document,
}

/// Where the parts come from; the folders in the app, a fake in tests.
pub(crate) trait Parts {
    fn part(&self, part: &RunbookPart) -> Option<PartDoc>;
}

/// No part can be read: every part shows as no longer available.
pub(crate) struct NoParts;

impl Parts for NoParts {
    fn part(&self, _: &RunbookPart) -> Option<PartDoc> {
        None
    }
}

struct Folders<'a> {
    reports: &'a Path,
    records: &'a Path,
}

impl Parts for Folders<'_> {
    fn part(&self, part: &RunbookPart) -> Option<PartDoc> {
        match part.kind {
            ReportKind::Record => {
                let store = RunStore::open(self.records, &part.id).ok()?;
                let run = store.summary().ok()?;
                let entry = record_entry(&run, record_job(&store));
                let mut document = record_document(self.records, &part.id).ok()?;
                let chain = chain_table(&chain_links(self.records, &run), &run.id);
                if let Some(chain) = chain {
                    document.tables.insert(0, chain);
                }
                Some(PartDoc { entry, document })
            }
            ReportKind::Readiness | ReportKind::Compare => {
                let saved = crate::load(self.reports, &part.id).ok()?;
                (saved.body.kind() == part.kind).then(|| PartDoc {
                    entry: entry(&saved),
                    document: document(&saved),
                })
            }
            _ => None,
        }
    }
}

pub(crate) fn runbook_document(
    reports: &Path,
    records: &Path,
    saved: &Saved,
) -> Result<Document, String> {
    let Body::Runbook(report) = &saved.body else {
        return Err("That is not a runbook.".into());
    };
    let meta = meta(&saved.job, saved.created_at_ms, &saved.machine, &saved.user);
    Ok(build(report, meta, &Folders { reports, records }))
}

fn verdict_text(verdict: Option<Verdict>) -> &'static str {
    match verdict {
        Some(Verdict::Good) => "All clear",
        Some(Verdict::Bad) => "Needs attention",
        None => "",
    }
}

fn verdict_tone(verdict: Option<Verdict>) -> Tone {
    match verdict {
        Some(Verdict::Good) => Tone::Good,
        Some(Verdict::Bad) => Tone::Bad,
        None => Tone::Plain,
    }
}

/// How one part came out.
enum Outcome {
    Verdict(Option<Verdict>),
    Missing,
}

pub(super) fn build(
    report: &RunbookReport,
    meta: Vec<(String, String)>,
    parts: &dyn Parts,
) -> Document {
    let mut stages: Vec<(Stage, Outcome)> = Vec::new();
    let mut summary_rows: Vec<Vec<String>> = Vec::new();
    let mut summary_tones: Vec<Tone> = Vec::new();
    let mut sections = Vec::new();
    let mut csv_rows: Vec<Vec<String>> = Vec::new();
    let mut missing = Vec::new();
    let mut recovery_left_out = false;

    for (index, part) in report.parts.iter().enumerate() {
        let number = index + 1;
        let stage_name = stage_label(part.kind);
        let Some(found) = parts.part(part) else {
            let label = format!("{number}. {stage_name}");
            summary_rows.push(vec![
                number.to_string(),
                stage_name.into(),
                "Report no longer available".into(),
                String::new(),
                "Missing".into(),
                String::new(),
            ]);
            summary_tones.push(Tone::Bad);
            sections.push(Section::Details {
                summary: format!("{label}: report no longer available"),
                open: true,
                body: vec![Section::Note(format!(
                    "Report no longer available. It was deleted after this runbook was made (id {}). The rest of the runbook is not affected.",
                    part.id
                ))],
            });
            csv_rows.push(vec![
                label,
                "Report no longer available".into(),
                part.id.clone(),
            ]);
            missing.push(part.id.clone());
            if let Some(stage) = Stage::of(part.kind) {
                stages.push((stage, Outcome::Missing));
            }
            continue;
        };
        let PartDoc { entry, document } = found;
        let verdict = document.verdict;
        let label = format!("{number}. {stage_name}: {}", document.title);
        let permissions = document
            .tiles
            .iter()
            .find(|tile| tile.label == "Permissions")
            .map(|tile| {
                if tile.detail.is_empty() {
                    tile.value.clone()
                } else {
                    format!("{} · {}", tile.value, tile.detail)
                }
            })
            .unwrap_or_default();
        summary_rows.push(vec![
            number.to_string(),
            stage_name.into(),
            format!("{} · {}", entry.title, entry.subject),
            entry.headline.clone(),
            verdict_text(verdict).into(),
            permissions.clone(),
        ]);
        summary_tones.push(verdict_tone(verdict));
        let csv_detail = [entry.headline.as_str(), verdict_text(verdict), &permissions]
            .into_iter()
            .filter(|cell| !cell.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        csv_rows.push(vec![
            "Summary".into(),
            format!("{label} ({})", entry.subject),
            csv_detail,
        ]);
        if let Some(stage) = Stage::of(part.kind) {
            stages.push((stage, Outcome::Verdict(verdict)));
        }
        csv_rows.extend(part_csv_rows(&label, &document));
        let headline = entry.headline;
        let (body, dropped) = part_body(document, &headline);
        recovery_left_out |= dropped;
        sections.push(Section::Details {
            summary: match verdict_text(verdict) {
                "" => label,
                text => format!("{label} · {text}"),
            },
            open: true,
            body,
        });
    }

    let mut notes = vec![
        "Each section is the saved report, read when this document was made. Deleting a report later does not change this runbook, but its section then says the report is gone."
            .to_string(),
    ];
    if !missing.is_empty() {
        notes.push(match missing.len() {
            1 => "1 report is no longer available: the runbook is incomplete.".to_string(),
            count => format!(
                "{} reports are no longer available: the runbook is incomplete.",
                format_count(count as u64)
            ),
        });
    }
    if recovery_left_out {
        notes.push(
            "The commands to copy missing files again are in the compare report itself."
                .to_string(),
        );
    }

    let kinds: Vec<ReportKind> = report.parts.iter().map(|part| part.kind).collect();
    let needs_attention = stages.iter().any(|(_, outcome)| match outcome {
        Outcome::Missing => true,
        Outcome::Verdict(verdict) => *verdict == Some(Verdict::Bad),
    });
    let tiles = [Stage::Before, Stage::Copy, Stage::After]
        .into_iter()
        .map(|stage| stage_tile(stage, &stages))
        .collect();

    let stages_table = Table {
        heading: "Stages".into(),
        columns: vec![
            "#".into(),
            "Stage".into(),
            "Report".into(),
            "Result".into(),
            "Verdict".into(),
            "Permissions".into(),
        ],
        numeric: vec![true, false, false, false, false, false],
        rows: summary_rows,
        row_tone: Some(summary_tones),
        ..Table::default()
    };
    // Summary rows first, then each part's own rows, so a filter on Section finds a stage.
    let (mut csv_rows, rest): (Vec<_>, Vec<_>) = csv_rows
        .into_iter()
        .partition(|row| row.first().is_some_and(|cell| cell == "Summary"));
    csv_rows.extend(rest);
    let csv_table = Table {
        heading: "Runbook".into(),
        columns: vec!["Section".into(), "Item".into(), "Details".into()],
        numeric: vec![false, false, false],
        rows: csv_rows,
        ..Table::default()
    };

    Document {
        title: "Migration runbook".into(),
        subtitle: parts_summary(&kinds),
        meta,
        verdict: Some(if needs_attention || stages.is_empty() {
            Verdict::Bad
        } else {
            Verdict::Good
        }),
        tiles,
        notes,
        tables: vec![stages_table],
        csv_table: Some(csv_table),
        sections,
        ..Document::default()
    }
}

/// One tile per stage on the summary page: its worst result.
fn stage_tile(stage: Stage, stages: &[(Stage, Outcome)]) -> Tile {
    let mine: Vec<&Outcome> = stages
        .iter()
        .filter(|(candidate, _)| *candidate == stage)
        .map(|(_, outcome)| outcome)
        .collect();
    if mine.is_empty() {
        return tile(
            stage.label(),
            "Not included".into(),
            "No report picked".into(),
            Tone::Plain,
        );
    }
    let detail = plural(mine.len() as u64, "report");
    if mine
        .iter()
        .any(|outcome| matches!(outcome, Outcome::Missing))
    {
        tile(stage.label(), "Missing".into(), detail, Tone::Bad)
    } else if mine
        .iter()
        .any(|outcome| matches!(outcome, Outcome::Verdict(Some(Verdict::Bad))))
    {
        tile(stage.label(), "Needs attention".into(), detail, Tone::Bad)
    } else {
        tile(stage.label(), "All clear".into(), detail, Tone::Good)
    }
}

/// A part's document laid out as sections inside the runbook: its own verdict tile, its tiles,
/// notes and tables. The recovery block of a compare is left out (its script and page data belong
/// to a page of their own); the flag says whether one was.
fn part_body(document: Document, headline: &str) -> (Vec<Section>, bool) {
    let mut body = Vec::new();
    let when: Vec<String> = document
        .meta
        .iter()
        .filter(|(label, _)| !crate::JOB_LABELS.contains(&label.as_str()))
        .map(|(label, value)| format!("{label}: {value}"))
        .collect();
    let mut intro = vec![document.subtitle.clone()];
    intro.extend(when);
    body.push(Section::Note(
        intro
            .into_iter()
            .filter(|piece| !piece.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
    ));
    let mut tiles = Vec::new();
    if document.verdict.is_some() {
        tiles.push(tile(
            "Verdict",
            verdict_text(document.verdict).into(),
            headline.to_string(),
            verdict_tone(document.verdict),
        ));
    }
    tiles.extend(document.tiles);
    body.push(Section::Tiles(tiles));
    if let Some(share) = document.share {
        body.push(Section::Note(format!("{}: {}", share.label, share.detail)));
    }
    body.extend(document.notes.into_iter().map(Section::Note));
    body.extend(document.tables.into_iter().map(Section::Table));
    let mut dropped = false;
    for section in document.sections {
        if matches!(section, Section::Recovery(_)) {
            dropped = true;
        } else {
            body.push(section);
        }
    }
    (body, dropped)
}

/// The rows of a part's own CSV table under the runbook's three columns: the column names first,
/// then each row as item and details.
fn part_csv_rows(label: &str, document: &Document) -> Vec<Vec<String>> {
    let Some(table) = document.csv_table.as_ref().or(document.tables.first()) else {
        return Vec::new();
    };
    let split = |cells: &[String]| -> Vec<String> {
        let mut cells = cells.iter();
        let item = cells.next().cloned().unwrap_or_default();
        let details = cells
            .filter(|cell| !cell.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        vec![label.to_string(), item, details]
    };
    std::iter::once(split(&table.columns))
        .chain(table.rows.iter().map(|row| split(row)))
        .collect()
}

/// A run in a delta chain: the run itself, or the id of one whose folder is gone.
enum Link {
    Run(Box<RunSummary>),
    Gone(String),
}

/// The run and the runs it follows, oldest first, stopping at a run that is gone, at a loop in
/// the saved ids, or after [`MAX_CHAIN`] runs.
fn chain_links(records: &Path, run: &RunSummary) -> Vec<Link> {
    let mut links = vec![Link::Run(Box::new(run.clone()))];
    let mut seen: HashSet<String> = HashSet::from([run.id.clone()]);
    let mut next = run.previous_run_id.clone();
    while let Some(id) = next.take() {
        if links.len() >= MAX_CHAIN || !seen.insert(id.clone()) {
            break;
        }
        match RunStore::open(records, &id).and_then(|store| store.summary()) {
            Ok(before) => {
                next = before.previous_run_id.clone();
                links.push(Link::Run(Box::new(before)));
            }
            Err(_) => links.push(Link::Gone(id)),
        }
    }
    links.reverse();
    links
}

fn run_state_label(state: RunState) -> &'static str {
    match state {
        RunState::Completed => "Completed",
        RunState::Cancelled => "Cancelled",
        RunState::Failed => "Failed",
        _ => "Not finished",
    }
}

/// The chain of delta runs ending at `current`, or `None` for an ordinary run.
fn chain_table(links: &[Link], current: &str) -> Option<Table> {
    if links.len() < 2 {
        return None;
    }
    let rows = links
        .iter()
        .map(|link| match link {
            Link::Run(run) => vec![
                if run.id == current {
                    format!("{} (this run)", run.id)
                } else {
                    run.id.clone()
                },
                format_local(run.created_at_ms),
                run_state_label(run.state).into(),
                format_count(run.totals.copied),
                format_count(run.totals.changed),
                format_count(run.totals.new),
                format_count(run.totals.retried),
                format_count(run.totals.not_copied),
            ],
            Link::Gone(id) => {
                let mut row = vec![id.clone(), "No longer on this computer".into()];
                row.resize(8, String::new());
                row
            }
        })
        .collect();
    Some(Table {
        heading: "Runs in this copy, oldest first".into(),
        columns: vec![
            "Run".into(),
            "Started".into(),
            "State".into(),
            "Copied".into(),
            "Changed".into(),
            "New".into(),
            "Retried".into(),
            "Not copied".into(),
        ],
        numeric: vec![false, false, false, true, true, true, true, true],
        rows,
        footnote: "Each run after the first copies only what changed since the run before it."
            .into(),
        ..Table::default()
    })
}

//! Record runs as report rows and documents.

use super::*;

pub(super) fn record_title(run: &RunSummary) -> &'static str {
    match run.settings.mode {
        TransferMode::Copy => "Copy for me",
        TransferMode::Watch => "Watched copy",
    }
}

pub(crate) fn record_entry(run: &RunSummary, job: JobInfo) -> Entry {
    let totals = &run.totals;
    let (headline, problem) = if totals.not_copied > 0 {
        (
            format!(
                "{} not copied ({})",
                plural(totals.not_copied, "file"),
                format_bytes(totals.not_copied_bytes)
            ),
            true,
        )
    } else {
        match run.state {
            RunState::Completed => (
                format!("All {} there", plural(totals.planned_files, "file")),
                false,
            ),
            RunState::Failed => (run.error.clone().unwrap_or_else(|| "Failed".into()), true),
            RunState::Cancelled => ("Cancelled".into(), true),
            _ => ("Not finished".into(), false),
        }
    };
    Entry {
        id: run.id.clone(),
        kind: ReportKind::Record,
        created_at_ms: run.created_at_ms,
        title: record_title(run).into(),
        subject: format!("{} → {}", run.settings.source, run.settings.destination),
        headline,
        problem,
        job,
    }
}

pub(crate) fn record_document(records: &Path, id: &str) -> Result<Document, String> {
    let store = RunStore::open(records, id).map_err(|err| err.to_string())?;
    let run = store.summary().map_err(|err| err.to_string())?;
    let results = store.latest_results().map_err(|err| err.to_string())?;
    let failed_items: Vec<_> = results
        .values()
        .filter(|item| item.status == ItemStatus::NotCopied)
        .collect();
    let row_bytes: Vec<u64> = failed_items.iter().map(|item| item.size).collect();
    let not_copied: Vec<Vec<String>> = failed_items
        .iter()
        .map(|item| {
            let reason = item
                .reason
                .map(|reason| reason.title())
                .unwrap_or("Other error");
            vec![
                item.relative_path.clone(),
                format_bytes(item.size),
                reason.to_string(),
                item.message.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let verify = run.settings.verify;
    let arrived_items: Vec<_> = results
        .values()
        .filter(|item| item.status != ItemStatus::NotCopied)
        .collect();
    let arrived_rows: Vec<Vec<String>> = arrived_items
        .iter()
        .map(|item| {
            vec![
                item.relative_path.clone(),
                format_bytes(item.size),
                check_label(item, verify).into(),
            ]
        })
        .collect();
    let arrived_table = Table {
        heading: "Arrived".into(),
        columns: vec![
            "Path (relative to the source)".into(),
            "Size".into(),
            "Check".into(),
        ],
        numeric: vec![false, true, false],
        rows: arrived_rows,
        size_col: Some(1),
        row_bytes: arrived_items.iter().map(|item| item.size).collect(),
        ..Table::default()
    }
    .toned(Tone::Good);
    // Every file with its status: the CSV holds the whole record, not just the failures.
    let all_items: Vec<_> = results.values().collect();
    let csv_table = Table {
        heading: "All files".into(),
        columns: vec![
            "Path (relative to the source)".into(),
            "Size".into(),
            "Status".into(),
            "Check or reason".into(),
            "Details".into(),
        ],
        numeric: vec![false, true, false, false, false],
        rows: all_items
            .iter()
            .map(|item| {
                let (status, note) = if item.status == ItemStatus::NotCopied {
                    let reason = item.reason.map_or("Other error", |reason| reason.title());
                    ("Not copied", reason)
                } else {
                    (status_label(item.status), check_label(item, verify))
                };
                vec![
                    item.relative_path.clone(),
                    format_bytes(item.size),
                    status.into(),
                    note.into(),
                    item.message.clone().unwrap_or_default(),
                ]
            })
            .collect(),
        size_col: Some(1),
        row_bytes: all_items.iter().map(|item| item.size).collect(),
        ..Table::default()
    };
    let arrived_count = arrived_items.len();
    let totals = &run.totals;
    let job = record_job(&store);
    let arrived = match run.settings.mode {
        TransferMode::Copy => "Copied",
        TransferMode::Watch => "Arrived",
    };
    let mut notes = Vec::new();
    if let Some(error) = &run.error {
        notes.push(format!("The record stopped: {error}"));
    }
    if run.settings.ignore_junk {
        notes.push(
            "System and temp files (Thumbs.db, desktop.ini, ~$ lock files, .tmp) were ignored."
                .into(),
        );
    }
    Ok(Document {
        title: record_title(&run).into(),
        subtitle: format!("{} → {}", run.settings.source, run.settings.destination),
        meta: meta(&job, run.created_at_ms, &run.machine, &run.user),
        verdict: Some(record_verdict(&run)),
        tiles: vec![
            tile(
                "Source",
                format_bytes(totals.planned_bytes),
                plural(totals.planned_files, "file"),
                Tone::Plain,
            ),
            tile(
                arrived,
                plural(totals.copied + totals.skipped_identical, "file"),
                format!(
                    "{} · {} already there",
                    format_bytes(totals.copied_bytes),
                    format_count(totals.skipped_identical)
                ),
                Tone::Good,
            ),
            if totals.not_copied > 0 {
                tile(
                    "Not copied",
                    plural(totals.not_copied, "file"),
                    format_bytes(totals.not_copied_bytes),
                    Tone::Bad,
                )
            } else {
                tile(
                    "Not copied",
                    "None".into(),
                    "Every file made it".into(),
                    Tone::Good,
                )
            },
        ],
        notes,
        share: (totals.planned_files > 0).then(|| {
            let there = (totals.copied + totals.skipped_identical).min(totals.planned_files);
            Share {
                label: format!("Source files {}", arrived.to_lowercase()),
                detail: format!(
                    "{} of {}",
                    format_count(there),
                    format_count(totals.planned_files)
                ),
                percent: there as f64 * 100.0 / totals.planned_files as f64,
                tone: if totals.not_copied == 0 {
                    Tone::Good
                } else {
                    Tone::Plain
                },
            }
        }),
        tables: vec![Table {
            heading: "Not copied".into(),
            columns: vec![
                "Path (relative to the source)".into(),
                "Size".into(),
                "Reason".into(),
                "Details".into(),
            ],
            numeric: vec![false, true, false, false],
            rows: not_copied,
            size_col: Some(1),
            row_bytes,
            ..Table::default()
        }
        .toned(Tone::Bad)],
        csv_table: Some(csv_table),
        sections: vec![Section::Details {
            summary: format!("Arrived ({})", format_count(arrived_count as u64)),
            open: false,
            body: vec![Section::Table(arrived_table)],
        }],
        ..Document::default()
    })
}

pub(super) fn status_label(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Copied => "Copied",
        ItemStatus::Arrived => "Arrived",
        ItemStatus::SkippedIdentical => "Already there",
        ItemStatus::NotCopied => "Not copied",
    }
}

/// How an arrived file was checked. A hash is shown only when both sides were hashed and agree;
/// a differing hash is a `NotCopied` item with its own reason, never listed as arrived.
pub(super) fn check_label(item: &ItemResult, verify: VerifyLevel) -> &'static str {
    match item.status {
        ItemStatus::SkippedIdentical => "Already there",
        _ if verify == VerifyLevel::Hash
            && item.source_hash.is_some()
            && item.source_hash == item.dest_hash =>
        {
            "Content verified (BLAKE3)"
        }
        _ => "Size + date match",
    }
}

/// "All clear" only when the run finished and every file made it: a run that failed or was
/// cancelled can have nothing in the not-copied list and still be unfinished.
pub(super) fn record_verdict(run: &RunSummary) -> Verdict {
    let unfinished = matches!(run.state, RunState::Failed | RunState::Cancelled);
    if unfinished || run.error.is_some() || run.totals.not_copied > 0 {
        Verdict::Bad
    } else {
        Verdict::Good
    }
}

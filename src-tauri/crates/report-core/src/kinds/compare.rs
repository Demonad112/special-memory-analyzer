//! The Compare report as a document.

use super::*;

pub(super) fn document(report: &CompareReport, meta: Vec<(String, String)>) -> Document {
    let listed = report.missing_paths.len() as u64;
    let mut notes = Vec::new();
    if report.unreadable > 0 {
        notes.push(format!(
            "{} could not be read completely; sizes there may be low.",
            plural(report.unreadable, "folder")
        ));
    }
    let plan = Plan::new(
        &report.source,
        &report.destination,
        &report.missing_paths,
        Options::default(),
    );
    let intro = format!(
                "Open PowerShell on a PC that can reach both folders, paste the command and press Enter. It copies {} again{}.",
                plan.as_ref().map_or_else(
                    || "the missing files".to_string(),
                    |plan| plural(plan.files, "missing file")
                ),
                if listed < report.missing {
                    format!(" (only the {} listed here)", format_count(listed))
                } else {
                    String::new()
                }
            );
    Document {
        title: "Compare".into(),
        subtitle: format!("{} → {}", report.source, report.destination),
        meta,
        verdict: Some(if report.missing > 0 {
            Verdict::Bad
        } else {
            Verdict::Good
        }),
        tiles: vec![
            tile(
                "Source",
                format_bytes(report.source_size),
                plural(report.source_files, "file"),
                Tone::Plain,
            ),
            tile(
                "Destination",
                format_bytes(report.destination_size),
                plural(report.destination_files, "file"),
                Tone::Plain,
            ),
            if report.missing > 0 {
                tile(
                    "Missing at destination",
                    plural(report.missing, "file"),
                    format!("{} not copied", format_bytes(report.missing_bytes)),
                    Tone::Bad,
                )
            } else {
                tile(
                    "Missing at destination",
                    "None".into(),
                    "Every file is there".into(),
                    Tone::Good,
                )
            },
            tile(
                "Other differences",
                plural(report.different, "size change"),
                format!("{} only at destination", plural(report.extra, "file")),
                Tone::Plain,
            ),
        ],
        notes,
        share: (report.source_files > 0).then(|| {
            let there = report.source_files.saturating_sub(report.missing);
            Share {
                label: "Source files found at the destination".into(),
                detail: format!(
                    "{} of {}",
                    format_count(there),
                    format_count(report.source_files)
                ),
                percent: there as f64 * 100.0 / report.source_files as f64,
                tone: if report.missing == 0 {
                    Tone::Good
                } else {
                    Tone::Plain
                },
            }
        }),
        tables: vec![Table {
            heading: "Missing at destination".into(),
            columns: vec!["Path (relative to the source)".into(), "Size".into()],
            numeric: vec![false, true],
            rows: report
                .missing_paths
                .iter()
                .map(|file| vec![file.path.clone(), format_bytes(file.size)])
                .collect(),
            size_col: Some(1),
            row_bytes: report.missing_paths.iter().map(|file| file.size).collect(),
            footnote: if listed < report.missing {
                format!(
                    "The first {} of {} are listed.",
                    format_count(listed),
                    format_count(report.missing)
                )
            } else {
                format!("Compared in {}.", seconds(report.elapsed_ms))
            },
            ..Table::default()
        }
        .toned(Tone::Bad)],
        sections: plan
            .iter()
            .map(|plan| plan.section(intro.clone()))
            .collect(),
        data: plan.as_ref().map(|plan| json!({ "rec": plan.data() })),
        ..Document::default()
    }
}

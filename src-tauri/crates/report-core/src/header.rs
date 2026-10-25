//! The part of a saved report the Reports list shows, read without loading the rest.
//!
//! A disk usage report carries an explorer of a few MB and a compare report up to 50,000
//! missing paths, and both are written last. [`read`] parses the start of the file and stops at
//! that field; anything it cannot be sure of falls back to a full parse that skips the field.

use crate::JobInfo;
use crate::{Body, ReportKind, Saved};
use migrate_core::AuditStatus;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::Deserialize;
use std::cell::RefCell;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Reports saved before ids, jobs or timestamps existed lack those fields; they all default.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Header {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub job: JobInfo,
    pub body: BodyHeader,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "data")]
pub(crate) enum BodyHeader {
    Compare(CompareHeader),
    DiskUsage(UsageHeader),
    Cleanup(CleanHeader),
    Readiness(ReadinessHeader),
    Runbook(RunbookHeader),
}

impl BodyHeader {
    pub(crate) fn kind(&self) -> ReportKind {
        match self {
            BodyHeader::Compare(_) => ReportKind::Compare,
            BodyHeader::DiskUsage(_) => ReportKind::DiskUsage,
            BodyHeader::Cleanup(_) => ReportKind::Cleanup,
            BodyHeader::Readiness(_) => ReportKind::Readiness,
            BodyHeader::Runbook(_) => ReportKind::Runbook,
        }
    }
}

/// Which parts a runbook holds; their ids are not needed for the row.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RunbookHeader {
    pub parts: Vec<PartHeader>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PartHeader {
    pub kind: ReportKind,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CompareHeader {
    pub source: String,
    pub destination: String,
    pub missing: u64,
    pub missing_bytes: u64,
    pub permission_mismatches: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UsageHeader {
    pub path: String,
    pub size: u64,
    pub removed: Vec<RemovedSize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RemovedSize {
    pub size: u64,
}

/// The readiness totals; the problem list after them is skipped, not kept.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ReadinessHeader {
    pub source: String,
    pub destination_root: String,
    pub name_problems: u64,
    pub long_paths: u64,
    pub unreadable: u64,
    pub links: u64,
    pub permission_issues: u64,
    pub in_use: InUseHeader,
    pub permissions: PermissionsHeader,
}

/// Whether the permissions audit ran: the permissions CSV exists only when it did.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PermissionsHeader {
    pub status: AuditStatus,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct InUseHeader {
    pub in_use: u64,
    pub unreadable: u64,
}

impl ReadinessHeader {
    /// Something to fix or look at before the copy; matches `ReadinessReport::has_problems`.
    pub(crate) fn has_problems(&self) -> bool {
        self.name_problems
            + self.long_paths
            + self.unreadable
            + self.in_use.in_use
            + self.in_use.unreadable
            + self.permission_issues
            > 0
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CleanHeader {
    pub items: Vec<CleanedHeader>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CleanedHeader {
    pub title: String,
    pub freed: u64,
}

impl Header {
    /// The header of a report that is already fully loaded.
    pub(crate) fn of(saved: &Saved) -> Self {
        let body = match &saved.body {
            Body::Compare(report) => BodyHeader::Compare(CompareHeader {
                source: report.source.clone(),
                destination: report.destination.clone(),
                missing: report.missing,
                missing_bytes: report.missing_bytes,
                permission_mismatches: report.permission_mismatches,
            }),
            Body::DiskUsage(report) => BodyHeader::DiskUsage(UsageHeader {
                path: report.path.clone(),
                size: report.size,
                removed: report
                    .removed
                    .iter()
                    .map(|item| RemovedSize { size: item.size })
                    .collect(),
            }),
            Body::Cleanup(report) => BodyHeader::Cleanup(CleanHeader {
                items: report
                    .items
                    .iter()
                    .map(|item| CleanedHeader {
                        title: item.title.clone(),
                        freed: item.freed,
                    })
                    .collect(),
            }),
            Body::Readiness(report) => BodyHeader::Readiness(ReadinessHeader {
                source: report.source.clone(),
                destination_root: report.destination_root.clone(),
                name_problems: report.name_problems,
                long_paths: report.long_paths,
                unreadable: report.unreadable,
                links: report.links,
                permission_issues: report.permission_issues,
                in_use: InUseHeader {
                    in_use: report.in_use.in_use,
                    unreadable: report.in_use.unreadable,
                },
                permissions: PermissionsHeader {
                    status: report.permissions.status,
                },
            }),
            Body::Runbook(report) => BodyHeader::Runbook(RunbookHeader {
                parts: report
                    .parts
                    .iter()
                    .map(|part| PartHeader { kind: part.kind })
                    .collect(),
            }),
        };
        Self {
            id: saved.id.clone(),
            created_at_ms: saved.created_at_ms,
            job: saved.job.clone(),
            body,
        }
    }
}

/// Reads the header of the report at `path`; `id` stands in for a missing id.
pub(crate) fn read(path: &Path, id: &str) -> Result<Header, String> {
    let gone = |_| "That report is no longer there.".to_string();
    let mut header = match read_prefix(path) {
        Some(header) => header,
        None => {
            let text = std::fs::read(path).map_err(gone)?;
            serde_json::from_slice(&text)
                .map_err(|err| format!("The report could not be read: {err}"))?
        }
    };
    if header.id.is_empty() {
        header.id = id.to_string();
    }
    Ok(header)
}

const STOP: &str = "header complete";

#[derive(Default)]
struct Found {
    id: Option<String>,
    created_at_ms: Option<u64>,
    job: Option<JobInfo>,
    body: Option<BodyHeader>,
    /// The parse was cut short on purpose, with every field the header needs already read.
    stopped: bool,
}

impl Found {
    fn envelope_read(&self) -> bool {
        self.id.is_some() && self.created_at_ms.is_some() && self.job.is_some()
    }
}

/// Streams the file and abandons the parse at the first big field, once everything the header
/// needs has been read. `None` means "use the full parse".
fn read_prefix(path: &Path) -> Option<Header> {
    let file = File::open(path).ok()?;
    let reader = BufReader::with_capacity(64 * 1024, file);
    let found = RefCell::new(Found::default());
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let outcome = EnvelopeSeed(&found).deserialize(&mut deserializer);
    let found = found.into_inner();
    // Run to the end, the parse must have succeeded; stopped early, only on purpose.
    if outcome.is_err() && !found.stopped {
        return None;
    }
    Some(Header {
        id: found.id?,
        created_at_ms: found.created_at_ms?,
        job: found.job?,
        body: found.body?,
    })
}

struct EnvelopeSeed<'a>(&'a RefCell<Found>);

impl<'de> DeserializeSeed<'de> for EnvelopeSeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for EnvelopeSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a saved report")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "id" => self.0.borrow_mut().id = Some(map.next_value()?),
                "createdAtMs" => self.0.borrow_mut().created_at_ms = Some(map.next_value()?),
                "job" => self.0.borrow_mut().job = Some(map.next_value()?),
                "body" => map.next_value_seed(BodySeed(self.0))?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

struct BodySeed<'a>(&'a RefCell<Found>);

impl<'de> DeserializeSeed<'de> for BodySeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for BodySeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a report body")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut kind: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match (key.as_str(), kind.as_deref()) {
                ("kind", _) => kind = Some(map.next_value()?),
                ("data", Some(kind)) => {
                    let data = DataSeed {
                        found: self.0,
                        kind,
                    };
                    map.next_value_seed(data)?;
                }
                // Data before its kind: let the full parse sort it out.
                ("data", None) => return Err(de::Error::custom("body out of order")),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

struct DataSeed<'a> {
    found: &'a RefCell<Found>,
    kind: &'a str,
}

impl<'de> DeserializeSeed<'de> for DataSeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        match self.kind {
            "cleanup" => {
                let data = CleanHeader::deserialize(deserializer)?;
                self.found.borrow_mut().body = Some(BodyHeader::Cleanup(data));
                Ok(())
            }
            // Read whole; the problem list is skipped field by field without being kept.
            "readiness" => {
                let data = ReadinessHeader::deserialize(deserializer)?;
                self.found.borrow_mut().body = Some(BodyHeader::Readiness(data));
                Ok(())
            }
            // Small: only ids and kinds.
            "runbook" => {
                let data = RunbookHeader::deserialize(deserializer)?;
                self.found.borrow_mut().body = Some(BodyHeader::Runbook(data));
                Ok(())
            }
            "compare" | "diskUsage" => deserializer.deserialize_map(self),
            other => Err(de::Error::custom(format!("unknown report kind {other}"))),
        }
    }
}

impl<'de> Visitor<'de> for DataSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a report")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let compare = self.kind == "compare";
        let mut head = CompareHeader::default();
        let mut usage = UsageHeader::default();
        // One bit per field the header needs; all set (compare: 4, usage: 3) means it is complete.
        let mut seen = 0u8;
        let (needed, big) = if compare {
            (0b1111, "missingPaths")
        } else {
            (0b111, "explorer")
        };
        while let Some(key) = map.next_key::<String>()? {
            let key = key.as_str();
            if key == big {
                let mut found = self.found.borrow_mut();
                if seen == needed && found.envelope_read() {
                    found.body = Some(finish(compare, head, usage));
                    found.stopped = true;
                    return Err(de::Error::custom(STOP));
                }
                drop(found);
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            seen |= match (compare, key) {
                (true, "source") => {
                    head.source = map.next_value()?;
                    0b0001
                }
                (true, "destination") => {
                    head.destination = map.next_value()?;
                    0b0010
                }
                (true, "missing") => {
                    head.missing = map.next_value()?;
                    0b0100
                }
                (true, "missingBytes") => {
                    head.missing_bytes = map.next_value()?;
                    0b1000
                }
                // Written ahead of the long lists, and only when there are any.
                (true, "permissionMismatches") => {
                    head.permission_mismatches = map.next_value()?;
                    0
                }
                (false, "path") => {
                    usage.path = map.next_value()?;
                    0b001
                }
                (false, "size") => {
                    usage.size = map.next_value()?;
                    0b010
                }
                (false, "removed") => {
                    usage.removed = map.next_value()?;
                    0b100
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                    0
                }
            };
        }
        self.found.borrow_mut().body = Some(finish(compare, head, usage));
        Ok(())
    }
}

fn finish(compare: bool, head: CompareHeader, usage: UsageHeader) -> BodyHeader {
    if compare {
        BodyHeader::Compare(head)
    } else {
        BodyHeader::DiskUsage(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::header_entry;
    use std::fs;

    fn read_text(text: &str) -> Result<Header, String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.json");
        fs::write(&path, text).unwrap();
        read(&path, "from-name")
    }

    const ENVELOPE: &str = r#""id":"r1","createdAtMs":5,"machine":"m","user":"u","job":{"client":"C","ticket":"","technician":""}"#;

    #[test]
    fn a_big_trailing_field_is_not_read() {
        // The explorer is cut off mid-array: only a reader that stops before it can succeed.
        let usage = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"diskUsage","data":{{"path":"C:\\","size":2048,"files":9,"removed":[{{"path":"a","size":10,"permanent":false}}],"explorer":{{"nodes":[{{"n":"x""#
        );
        let row = header_entry(read_text(&usage).unwrap());
        assert_eq!(row.id, "r1");
        assert_eq!(row.created_at_ms, 5);
        assert_eq!(row.job.client, "C");
        assert_eq!(row.subject, "C:\\");
        assert!(row.headline.contains("removed"), "{}", row.headline);

        let compare = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"compare","data":{{"source":"S","destination":"D","missing":3,"missingBytes":2048,"missingPaths":["a.txt","b"#
        );
        let row = header_entry(read_text(&compare).unwrap());
        assert_eq!(row.subject, "S → D");
        assert!(row.problem);
    }

    #[test]
    fn permission_mismatches_reach_the_row_without_reading_the_big_lists() {
        // Nothing missing, but the permissions differ: still a problem, read from the header.
        let text = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"compare","data":{{"source":"S","destination":"D","missing":0,"missingBytes":0,"elapsedMs":4,"permissionMismatches":3,"missingPaths":[],"permissions":{{"status":"checked","items":[{{"pa"#
        );
        let row = header_entry(read_text(&text).unwrap());
        assert!(row.problem);
        assert_eq!(row.headline, "Nothing missing · 3 permissions differ");
        // A report from before the option has no such key and reads as before.
        let old = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"compare","data":{{"source":"S","destination":"D","missing":0,"missingBytes":0,"missingPaths":[]}}}}}}"#
        );
        let row = header_entry(read_text(&old).unwrap());
        assert!(!row.problem);
        assert_eq!(row.headline, "Nothing missing");
    }

    #[test]
    fn a_big_field_ahead_of_the_fields_the_row_needs_is_still_handled() {
        let text = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"diskUsage","data":{{"explorer":{{"nodes":[]}},"path":"E:\\","size":2048,"removed":[]}}}}}}"#
        );
        assert_eq!(header_entry(read_text(&text).unwrap()).subject, "E:\\");
    }

    #[test]
    fn a_body_that_lists_its_data_before_its_kind_is_handled() {
        let text = format!(
            r#"{{{ENVELOPE},"body":{{"data":{{"path":"F:\\","size":1}},"kind":"diskUsage"}}}}"#
        );
        assert_eq!(header_entry(read_text(&text).unwrap()).subject, "F:\\");
    }

    #[test]
    fn a_report_without_an_envelope_takes_its_id_from_the_file_name() {
        let text = r#"{"body":{"kind":"diskUsage","data":{"path":"G:\\","size":1,"explorer":{"nodes":[]}}}}"#;
        let row = header_entry(read_text(text).unwrap());
        assert_eq!(row.id, "from-name");
        assert_eq!(row.subject, "G:\\");
    }

    #[test]
    fn a_readiness_report_with_files_in_use_is_a_problem() {
        let text = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"readiness","data":{{"source":"S","inUse":{{"status":"checked","inUse":2}}}}}}}}"#
        );
        let row = header_entry(read_text(&text).unwrap());
        assert_eq!(row.headline, "2 files in use");
        assert!(row.problem);
    }

    #[test]
    fn a_readiness_report_with_permission_issues_is_a_problem() {
        let text = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"readiness","data":{{"source":"S","permissionIssues":3,"permissions":{{"items":[{{"path":"a"}}]}}}}}}}}"#
        );
        let row = header_entry(read_text(&text).unwrap());
        assert_eq!(row.headline, "3 permission issues");
        assert!(row.problem);
    }

    #[test]
    fn a_readiness_report_lists_without_keeping_its_problems() {
        let text = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"readiness","data":{{"source":"S","problems":[{{"path":"a","rule":"reserved"}}],"nameProblems":1,"destinationRoot":"D:\\x"}}}}}}"#
        );
        let row = header_entry(read_text(&text).unwrap());
        assert_eq!(row.kind, crate::ReportKind::Readiness);
        assert_eq!(row.subject, r"S → D:\x");
        assert!(row.problem);
    }

    #[test]
    fn a_runbook_row_is_read_from_its_ids_and_a_readiness_row_knows_its_audit() {
        let text = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"runbook","data":{{"parts":[{{"kind":"readiness","id":"a"}},{{"kind":"record","id":"b"}}]}}}}}}"#
        );
        let row = header_entry(read_text(&text).unwrap());
        assert_eq!(row.kind, crate::ReportKind::Runbook);
        assert_eq!(row.subject, "Readiness · 1 copy");
        assert_eq!(row.headline, "2 parts");

        let audited = format!(
            r#"{{{ENVELOPE},"body":{{"kind":"readiness","data":{{"source":"S","permissions":{{"status":"checked","items":[{{"path":"a"}}]}}}}}}}}"#
        );
        assert_eq!(
            header_entry(read_text(&audited).unwrap()).csv_layouts,
            ["permissions"]
        );
        let off =
            format!(r#"{{{ENVELOPE},"body":{{"kind":"readiness","data":{{"source":"S"}}}}}}"#);
        assert!(header_entry(read_text(&off).unwrap())
            .csv_layouts
            .is_empty());
    }

    /// A build without the readiness kind (1.2.0, or this one reverted) skips such a report in
    /// the list: its header does not parse, and `list` drops what does not parse.
    #[test]
    fn unreadable_reports_are_errors() {
        assert!(read_text("{ nope").is_err());
        let unknown = format!(r#"{{{ENVELOPE},"body":{{"kind":"hologram","data":{{}}}}}}"#);
        assert!(read_text(&unknown).is_err());
    }
}

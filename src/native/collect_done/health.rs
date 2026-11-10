//! Deterministic archive health findings from one captured census.
//!
//! Findings describe the input vault. They never stamp dates, close open
//! tasks, or deduplicate history. Binding conflicts (`archive_duplicate` and
//! `archive_binding_conflict`) skip writes for the affected pair; the other
//! codes are warnings and do not change exit status on their own.

use super::super::markdown::non_content_line_indexes;
use super::super::ref_tasks::close_date;
use super::super::task_dependencies::is_archive_path;
use super::super::task_status_hooks::parse_tasks;
use super::*;

use super::snapshot::CapturedInputs;

pub(super) const CODE_ORPHAN: &str = "archive_orphan";
pub(super) const CODE_DUPLICATE: &str = "archive_duplicate";
pub(super) const CODE_OPEN_IN_ARCHIVE: &str = "open_task_in_archive";
pub(super) const CODE_DUPLICATE_ID: &str = "duplicate_block_id_in_done";
pub(super) const CODE_UNDATED: &str = "undated_closed_row";
pub(super) const CODE_BINDING_CONFLICT: &str = "archive_binding_conflict";

const ALL_CODES: [&str; 6] = [
    CODE_ORPHAN,
    CODE_DUPLICATE,
    CODE_OPEN_IN_ARCHIVE,
    CODE_DUPLICATE_ID,
    CODE_UNDATED,
    CODE_BINDING_CONFLICT,
];

/// One structured health finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Finding {
    pub(super) code: &'static str,
    pub(super) path: PathBuf,
    pub(super) line: Option<usize>,
    pub(super) block_id: Option<String>,
    pub(super) related: Vec<PathBuf>,
}

/// Census of archive health for one captured run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Diagnostics {
    /// False when capture did not finish, so an empty finding list is not a
    /// clean vault.
    pub(super) complete: bool,
    pub(super) findings: Vec<Finding>,
}

impl Diagnostics {
    pub(super) fn incomplete() -> Self {
        Self {
            complete: false,
            findings: Vec::new(),
        }
    }

    pub(super) fn blocks_writes(&self) -> bool {
        self.findings.iter().any(|finding| {
            finding.code == CODE_DUPLICATE
                || finding.code == CODE_BINDING_CONFLICT
        })
    }

    pub(super) fn conflict_paths(&self) -> Vec<PathBuf> {
        let mut paths = BTreeSet::new();
        for finding in &self.findings {
            if finding.code == CODE_DUPLICATE
                || finding.code == CODE_BINDING_CONFLICT
            {
                paths.insert(finding.path.clone());
                paths.extend(finding.related.iter().cloned());
            }
        }
        paths.into_iter().collect()
    }

    /// Source notes whose FilePlans and inferred historical repairs must skip.
    ///
    /// Related bound partners stay off this list so an extra claimant does
    /// not cancel an unrelated standing pair.
    pub(super) fn write_blocked_sources(&self) -> BTreeSet<PathBuf> {
        self.findings
            .iter()
            .filter(|finding| {
                (finding.code == CODE_DUPLICATE
                    || finding.code == CODE_BINDING_CONFLICT)
                    && !is_archive_path(&finding.path)
            })
            .map(|finding| finding.path.clone())
            .collect()
    }

    pub(super) fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for code in ALL_CODES {
            counts.insert(code, 0);
        }
        for finding in &self.findings {
            *counts.entry(finding.code).or_insert(0) += 1;
        }
        counts
    }
}

/// Walk the captured pair index and note contents once.
pub(super) fn collect_diagnostics(
    captured: &CapturedInputs,
    pairs: &ArchivePairIndex,
) -> Diagnostics {
    let mut findings = Vec::new();
    let mut duplicate_sources = BTreeSet::new();
    let mut duplicate_archives = BTreeSet::new();

    for (source, lookup) in pairs.source_entries() {
        let mut archives = Vec::new();
        match lookup {
            SourceLookup::Refused {
                reason, related, ..
            } if *reason == "duplicate_claim" => {
                archives.extend(
                    related
                        .iter()
                        .filter(|path| is_archive_path(path))
                        .cloned(),
                );
            }
            SourceLookup::Bound(bound) => {
                archives.push(bound.archive.clone());
                for (archive, archive_lookup) in pairs.archive_entries() {
                    let ArchiveLookup::Refused {
                        reason, related, ..
                    } = archive_lookup
                    else {
                        continue;
                    };
                    if *reason == "duplicate_claim"
                        && related.iter().any(|path| path == source)
                    {
                        archives.push(archive.to_path_buf());
                    }
                }
            }
            _ => continue,
        }
        archives.sort();
        archives.dedup();
        if archives.len() < 2 {
            continue;
        }
        duplicate_sources.insert(source.to_path_buf());
        duplicate_archives.extend(archives.iter().cloned());
        findings.push(Finding {
            code: CODE_DUPLICATE,
            path: source.to_path_buf(),
            line: None,
            block_id: None,
            related: archives,
        });
    }

    for (source, lookup) in pairs.source_entries() {
        if duplicate_sources.contains(source) {
            continue;
        }
        if let SourceLookup::Refused { related, .. } = lookup {
            findings.push(Finding {
                code: CODE_BINDING_CONFLICT,
                path: source.to_path_buf(),
                line: None,
                block_id: None,
                related: related.clone(),
            });
        }
    }

    for (archive, lookup) in pairs.archive_entries() {
        match lookup {
            ArchiveLookup::Orphan { .. } => findings.push(Finding {
                code: CODE_ORPHAN,
                path: archive.to_path_buf(),
                line: None,
                block_id: None,
                related: Vec::new(),
            }),
            ArchiveLookup::Refused { related, .. } => {
                if duplicate_archives.contains(archive) {
                    continue;
                }
                findings.push(Finding {
                    code: CODE_BINDING_CONFLICT,
                    path: archive.to_path_buf(),
                    line: None,
                    block_id: None,
                    related: related.clone(),
                });
            }
            ArchiveLookup::Bound(_) => {}
        }
    }

    for (relative, contents) in &captured.contents {
        if !is_archive_path(relative) {
            continue;
        }
        findings.extend(archive_content_findings(relative, contents, captured));
    }

    for path in &captured.source_paths {
        let Ok(relative) = vault_relative_path(&captured.vault, path, "note")
        else {
            continue;
        };
        let Ok(snapshot) = snapshot_for_path(&captured.snapshots, path) else {
            continue;
        };
        let Ok(contents) = captured.contents(&relative) else {
            continue;
        };
        let archive_exists =
            matches!(pairs.for_source(&relative), SourceLookup::Bound(_));
        match classify_source(
            &relative,
            snapshot,
            contents,
            captured,
            archive_exists,
        ) {
            SourceEligibility::Skip(_) => continue,
            SourceEligibility::Eligible { .. } => {}
        }
        findings.extend(undated_closed_rows(&relative, contents, captured));
    }

    findings.sort();
    findings.dedup();
    Diagnostics {
        complete: true,
        findings,
    }
}

fn archive_content_findings(
    relative: &Path,
    contents: &str,
    captured: &CapturedInputs,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let masked = non_content_line_indexes(&lines);

    for task in parse_tasks(contents, &captured.settings) {
        if !task.status_type.is_open() || masked.contains(&task.line_index) {
            continue;
        }
        let line = lines
            .get(task.line_index)
            .map(|segment| split_line_ending(segment).0)
            .unwrap_or("");
        if !has_task_tag(line) {
            continue;
        }
        findings.push(Finding {
            code: CODE_OPEN_IN_ARCHIVE,
            path: relative.to_path_buf(),
            line: Some(task.line_index + 1),
            block_id: task.block_id.clone(),
            related: Vec::new(),
        });
    }

    let mut id_lines: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for occurrence in content_anchor_occurrences(contents) {
        id_lines
            .entry(occurrence.id)
            .or_default()
            .push(occurrence.line_index + 1);
    }
    for (block_id, lines) in id_lines {
        if lines.len() < 2 {
            continue;
        }
        findings.push(Finding {
            code: CODE_DUPLICATE_ID,
            path: relative.to_path_buf(),
            line: Some(lines[1]),
            block_id: Some(block_id),
            related: Vec::new(),
        });
    }
    findings
}

fn undated_closed_rows(
    relative: &Path,
    contents: &str,
    captured: &CapturedInputs,
) -> Vec<Finding> {
    let transform =
        transform_markdown_with_settings(contents, &captured.settings);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let mut rows = Vec::new();
    for span in &transform.moved_roots {
        rows.push((span.source_line, span.root_id.clone()));
    }
    for hold in &transform.holds {
        rows.push((hold.line, hold.block_ids.first().cloned()));
    }
    let mut findings = Vec::new();
    for (line, block_id) in rows {
        let text = lines
            .get(line.saturating_sub(1))
            .map(|segment| split_line_ending(segment).0)
            .unwrap_or("");
        if close_date_is_valid(text) {
            continue;
        }
        findings.push(Finding {
            code: CODE_UNDATED,
            path: relative.to_path_buf(),
            line: Some(line),
            block_id,
            related: Vec::new(),
        });
    }
    findings
}

fn close_date_is_valid(line: &str) -> bool {
    close_date(line).is_some_and(|date| is_iso_date(&date))
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7) || byte.is_ascii_digit()
        })
}

//! Schema-version-1 album list/check report.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    grouping::{group_records, GroupingRecord, RecordGroup, RecordGrouping},
    registry::{AlbumEntry, AlbumRegistry, DiscoveryHealth},
    AlbumFinding, AlbumIssueSeverity,
};
use crate::native::{
    highlights_ref::humanize_stem,
    records::{
        catalog::IndexedRecord, schema::RecordField, IndexedFieldValue,
        RecordCatalog,
    },
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumIssue {
    pub(crate) code: &'static str,
    pub(crate) severity: AlbumIssueSeverity,
    pub(crate) line: Option<usize>,
    pub(crate) key_path: Option<String>,
    pub(crate) message: String,
    pub(crate) suggestions: Vec<String>,
}

impl AlbumIssue {
    fn from_finding(finding: &AlbumFinding) -> Self {
        Self {
            code: finding.code,
            severity: finding.severity,
            line: finding.line_number,
            key_path: finding.key_path.clone(),
            message: finding.message.clone(),
            suggestions: finding.suggestions.clone(),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "code": self.code,
            "severity": severity_name(self.severity),
            "line": self.line,
            "key_path": self.key_path,
            "message": self.message,
            "suggestions": self.suggestions,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumField {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: &'static str,
    pub(crate) required: bool,
    pub(crate) list: bool,
    pub(crate) choices: Vec<(String, String)>,
}

impl AlbumField {
    fn from_record_field(field: &RecordField) -> Self {
        let choices = match &field.kind {
            crate::native::records::schema::FieldKind::Choices(choices) => {
                choices
                    .iter()
                    .map(|choice| (choice.value.clone(), choice.label.clone()))
                    .collect()
            }
            _ => Vec::new(),
        };
        Self {
            key: field.key.clone(),
            label: field.label.clone(),
            kind: field.kind.kind_name(),
            required: field.required,
            list: field.list,
            choices,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "label": self.label,
            "kind": self.kind,
            "required": self.required,
            "list": self.list,
            "choices": self.choices.iter().map(|(value, label)| json!({
                "value": value,
                "label": label,
            })).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumReportEntry {
    pub(crate) stem: String,
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) emoji: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) noun: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) plural: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) parent_kind: Option<&'static str>,
    pub(crate) owner: Option<String>,
    pub(crate) root: Option<String>,
    pub(crate) breadcrumb: Vec<String>,
    pub(crate) fields: Vec<AlbumField>,
    pub(crate) group_field: Option<String>,
    pub(crate) index_by: Option<String>,
    pub(crate) index_facets: Vec<String>,
    pub(crate) records: usize,
    pub(crate) groups: Vec<RecordGroup>,
    pub(crate) needs_status: usize,
    pub(crate) last_record: Option<String>,
    pub(crate) issues: Vec<AlbumIssue>,
}

impl AlbumReportEntry {
    fn to_json(&self) -> Value {
        json!({
            "stem": self.stem,
            "path": self.path,
            "title": self.title,
            "emoji": self.emoji,
            "summary": self.summary,
            "noun": self.noun,
            "label": self.label,
            "plural": self.plural,
            "parent": self.parent,
            "parent_kind": self.parent_kind,
            "owner": self.owner,
            "root": self.root,
            "breadcrumb": self.breadcrumb,
            "fields": self.fields.iter().map(AlbumField::to_json).collect::<Vec<_>>(),
            "group_field": self.group_field,
            "index_by": self.index_by,
            "index_facets": self.index_facets,
            "records": self.records,
            "groups": self.groups.iter().map(|group| json!({
                "value": group.value,
                "label": group.label,
                "count": group.count,
                "peek": group.peek,
            })).collect::<Vec<_>>(),
            "needs_status": self.needs_status,
            "last_record": self.last_record,
            "issues": self.issues.iter().map(AlbumIssue::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumCounts {
    pub(crate) albums: usize,
    pub(crate) records: usize,
    pub(crate) roots: usize,
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumReport {
    pub(crate) albums: Vec<AlbumReportEntry>,
    pub(crate) counts: AlbumCounts,
    pub(crate) io_errors: Vec<String>,
}

impl AlbumReport {
    pub(crate) fn ok(&self) -> bool {
        self.counts.errors == 0
    }

    pub(crate) fn filter_root(&mut self, root: &str) {
        self.albums
            .retain(|entry| entry.root.as_deref() == Some(root));
        self.recount();
    }

    fn recount(&mut self) {
        let roots = self
            .albums
            .iter()
            .filter_map(|entry| entry.root.clone())
            .collect::<BTreeSet<_>>()
            .len();
        let records = self.albums.iter().map(|entry| entry.records).sum();
        let (mut errors, mut warnings) = (0, 0);
        for issue in self.albums.iter().flat_map(|entry| entry.issues.iter()) {
            match issue.severity {
                AlbumIssueSeverity::Error => errors += 1,
                AlbumIssueSeverity::Warning => warnings += 1,
                AlbumIssueSeverity::Info => {}
            }
        }
        self.counts = AlbumCounts {
            albums: self.albums.len(),
            records,
            roots,
            errors,
            warnings,
        };
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "schema_version": SCHEMA_VERSION,
            "albums": self.albums.iter().map(AlbumReportEntry::to_json).collect::<Vec<_>>(),
            "counts": {
                "albums": self.counts.albums,
                "records": self.counts.records,
                "roots": self.counts.roots,
                "errors": self.counts.errors,
                "warnings": self.counts.warnings,
            },
        })
    }
}

/// Build the shared list/check report from a discovered registry.
pub(crate) fn build_report(registry: &AlbumRegistry) -> AlbumReport {
    let mut catalog = RecordCatalog::new(&registry.vault_root);
    let mut albums = registry
        .albums
        .iter()
        .map(|entry| project_album(entry, &mut catalog))
        .collect::<Vec<_>>();
    albums.sort_by(|left, right| left.path.cmp(&right.path));
    let io_errors = match &registry.health {
        DiscoveryHealth::Partial(errors) => errors.clone(),
        DiscoveryHealth::Complete | DiscoveryHealth::TargetOnly => Vec::new(),
    };
    let mut report = AlbumReport {
        albums,
        counts: AlbumCounts {
            albums: 0,
            records: 0,
            roots: 0,
            errors: 0,
            warnings: 0,
        },
        io_errors,
    };
    report.recount();
    report
}

fn project_album(
    entry: &AlbumEntry,
    catalog: &mut RecordCatalog,
) -> AlbumReportEntry {
    let declaration = &entry.declaration;
    let record_type = declaration.record_type.as_ref().or_else(|| {
        entry
            .usable_type
            .as_ref()
            .map(|sourced| &sourced.record_type)
    });
    let fields: Vec<AlbumField> = record_type
        .map(|record_type| {
            record_type
                .fields
                .iter()
                .map(AlbumField::from_record_field)
                .collect()
        })
        .unwrap_or_default();
    let folder = catalog.folder(&declaration.stem);
    let members: Vec<GroupingRecord> =
        folder.notes().iter().map(grouping_record).collect();
    let grouping = if let Some(record_type) = record_type {
        group_records(
            &record_type.fields,
            declaration.index_by.as_deref(),
            &declaration.index_facets,
            &members,
        )
    } else {
        RecordGrouping {
            group_field: None,
            groups: Vec::new(),
            needs_status: 0,
            last_record: members
                .iter()
                .filter_map(|record| record.created)
                .max()
                .map(|date| date.format("%Y-%m-%d").to_string()),
            index_facets: declaration.index_facets.clone(),
        }
    };
    let mut issues: Vec<AlbumIssue> = entry
        .findings
        .iter()
        .map(AlbumIssue::from_finding)
        .collect();
    issues.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.code.cmp(right.code))
            .then_with(|| left.message.cmp(&right.message))
    });
    AlbumReportEntry {
        stem: declaration.stem.clone(),
        path: declaration.path.clone(),
        title: declaration.title.clone(),
        emoji: declaration.emoji.clone(),
        summary: declaration.summary.clone(),
        noun: declaration.noun_claim.clone(),
        label: declaration.label.clone(),
        plural: declaration.plural.clone(),
        parent: stem_of_parent(entry.parent_path.as_deref()),
        parent_kind: entry.parent_kind,
        owner: entry.owner.clone(),
        root: entry.root.clone(),
        breadcrumb: entry.breadcrumb.clone(),
        fields,
        group_field: grouping.group_field,
        index_by: declaration.index_by.clone(),
        index_facets: grouping.index_facets,
        records: members.len(),
        groups: grouping.groups,
        needs_status: grouping.needs_status,
        last_record: grouping.last_record,
        issues,
    }
}

fn grouping_record(note: &IndexedRecord) -> GroupingRecord {
    let title = note
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| humanize_stem(&note.stem));
    GroupingRecord {
        path: note.relative_path.to_string_lossy().replace('\\', "/"),
        title,
        created: created_date(note),
        fields: note
            .fields
            .iter()
            .map(|field| (field.key.clone(), field.value.clone()))
            .collect(),
    }
}

fn created_date(note: &IndexedRecord) -> Option<NaiveDate> {
    let field = note.fields.iter().find(|field| field.key == "created")?;
    let IndexedFieldValue::Scalar(text) = &field.value else {
        return None;
    };
    let date = text.get(..10)?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

fn stem_of_parent(path: Option<&Path>) -> Option<String> {
    path.and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
}

fn severity_name(severity: AlbumIssueSeverity) -> &'static str {
    match severity {
        AlbumIssueSeverity::Error => "error",
        AlbumIssueSeverity::Warning => "warning",
        AlbumIssueSeverity::Info => "info",
    }
}

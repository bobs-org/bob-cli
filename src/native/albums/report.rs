//! Schema-version-1 album list/check report.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use super::{
    catalog::{self, AlbumCatalogRecord},
    grouping::{group_records, GroupingRecord, RecordGroup},
    index::{self, IndexState},
    registry::{AlbumEntry, AlbumRegistry, DiscoveryHealth},
    write::folder_complete,
    AlbumDeclaration, AlbumFinding, AlbumIssueSeverity,
};
use crate::native::notebooks::snapshot::VaultSnapshot;
use crate::native::records::{
    schema::{
        album_label, album_schema_fingerprint, album_singular,
        RecordDiagnosticCode, RecordField, RecordType,
    },
    FolderIndex, RecordCatalog,
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumIssue {
    pub(crate) code: &'static str,
    pub(crate) severity: AlbumIssueSeverity,
    pub(crate) path: String,
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
            path: finding.path.clone(),
            line: finding.line_number,
            key_path: finding.key_path.clone(),
            message: finding.message.clone(),
            suggestions: finding.suggestions.clone(),
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "code": self.code,
            "severity": severity_name(self.severity),
            "path": self.path,
            "line": self.line,
            "key_path": self.key_path,
            "message": self.message,
            "suggestions": self.suggestions,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumField {
    value: Value,
}

impl AlbumField {
    fn from_record_field(
        field: &RecordField,
        line: Option<usize>,
        folder: &FolderIndex,
    ) -> Self {
        let choice_values: Vec<String> = match &field.kind {
            crate::native::records::schema::FieldKind::Choices(choices) => {
                choices.iter().map(|choice| choice.value.clone()).collect()
            }
            _ => Vec::new(),
        };
        let (filled, choice_counts) =
            folder.field_usage(&field.key, &choice_values);
        Self {
            value: field.schema_json(line, filled, &choice_counts),
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        self.value.clone()
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
    pub(crate) singular: Option<String>,
    pub(crate) dated_notes: Option<bool>,
    pub(crate) legacy: Option<Vec<String>>,
    pub(crate) schema_fingerprint: Option<String>,
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
    pub(crate) index: IndexState,
    pub(crate) issues: Vec<AlbumIssue>,
}

impl AlbumReportEntry {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "stem": self.stem,
            "path": self.path,
            "title": self.title,
            "emoji": self.emoji,
            "summary": self.summary,
            "noun": self.noun,
            "label": self.label,
            "plural": self.plural,
            "singular": self.singular,
            "dated_notes": self.dated_notes,
            "legacy": self.legacy,
            "schema_fingerprint": self.schema_fingerprint,
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
            "index": { "state": self.index.as_str() },
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
    pub(crate) catalogs: Vec<AlbumCatalogRecord>,
    pub(crate) counts: AlbumCounts,
    pub(crate) io_errors: Vec<String>,
}

pub(crate) struct AlbumProjection {
    pub(crate) entry: AlbumReportEntry,
    pub(crate) index_plan: index::IndexPlan,
    pub(crate) io_errors: Vec<String>,
}

impl AlbumReport {
    pub(crate) fn ok(&self) -> bool {
        self.counts.errors == 0 && self.io_errors.is_empty()
    }

    pub(crate) fn filter_root(&mut self, root: &str) {
        self.albums
            .retain(|entry| entry.root.as_deref() == Some(root));
        self.catalogs
            .retain(|catalog| catalog.root.as_deref() == Some(root));
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
        for issue in self
            .catalogs
            .iter()
            .flat_map(|catalog| catalog.issues.iter())
        {
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
            "catalogs": self.catalogs.iter().map(AlbumCatalogRecord::to_json).collect::<Vec<_>>(),
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
pub(crate) fn build_report(
    registry: &AlbumRegistry,
    snapshot: &VaultSnapshot,
) -> AlbumReport {
    let mut catalog = RecordCatalog::new(&registry.vault_root);
    let mut folder_io = Vec::new();
    let mut albums = registry
        .albums
        .iter()
        .map(|entry| {
            let projection = project_album(entry, &mut catalog);
            folder_io.extend(projection.io_errors);
            projection.entry
        })
        .collect::<Vec<_>>();
    albums.sort_by(|left, right| left.path.cmp(&right.path));
    let catalogs = catalog::plan_catalogs(snapshot, registry).records();
    let mut io_errors = match &registry.health {
        DiscoveryHealth::Partial(errors) => errors.clone(),
        DiscoveryHealth::Complete | DiscoveryHealth::TargetOnly => Vec::new(),
    };
    io_errors.extend(folder_io);
    let mut report = AlbumReport {
        albums,
        catalogs,
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

pub(crate) fn project_album(
    entry: &AlbumEntry,
    catalog: &mut RecordCatalog,
) -> AlbumProjection {
    let folder = catalog.folder(&entry.declaration.stem);
    project_album_from_folder(entry, folder)
}

pub(crate) fn project_album_from_folder(
    entry: &AlbumEntry,
    folder: &FolderIndex,
) -> AlbumProjection {
    let declaration = &entry.declaration;
    let record_type = declaration.record_type.as_ref().or_else(|| {
        entry
            .usable_type
            .as_ref()
            .map(|sourced| &sourced.record_type)
    });
    let mut io_errors = folder
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            format!("{}: {}", declaration.path, diagnostic.message)
        })
        .collect::<Vec<_>>();
    for note in folder.notes() {
        for diagnostic in &note.diagnostics {
            if diagnostic.code == RecordDiagnosticCode::NoteRead {
                io_errors.push(format!(
                    "{}: {}",
                    note.relative_path.to_string_lossy().replace('\\', "/"),
                    diagnostic.message
                ));
            }
        }
    }
    let members: Vec<GroupingRecord> = folder
        .notes()
        .iter()
        .map(GroupingRecord::from_indexed)
        .collect();
    let fields: Vec<AlbumField> = record_type
        .map(|record_type| {
            record_type
                .fields
                .iter()
                .map(|field| {
                    AlbumField::from_record_field(
                        field,
                        declaration.schema_field_lines.get(&field.key).copied(),
                        folder,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let schema_fields: &[RecordField] = record_type
        .map(|record_type| record_type.fields.as_slice())
        .unwrap_or(&[]);
    let grouping = group_records(
        schema_fields,
        declaration.index_by.as_deref(),
        &declaration.index_facets,
        &members,
    );
    let complete = folder_complete(folder);
    let plural = resolved_plural(declaration);
    let singular = resolved_singular(declaration);
    let nouns = index::RecordNouns {
        singular: &singular,
        plural: &plural,
    };
    let mut plan = index::plan_index(
        &declaration.path,
        entry.contents.as_deref(),
        &declaration.stem,
        &nouns,
        schema_fields,
        &grouping,
        declaration.index_by.as_deref(),
        members.len(),
        complete,
    );
    let album_invalid = entry
        .findings
        .iter()
        .any(|finding| finding.severity == AlbumIssueSeverity::Error);
    if album_invalid {
        plan.state = IndexState::Damaged;
        plan.edit = None;
        plan.findings
            .retain(|finding| finding.code != "album_index_stale");
        if !plan
            .findings
            .iter()
            .any(|finding| finding.code == "album_index_damaged")
        {
            plan.findings.push(AlbumFinding {
                code: "album_index_damaged",
                severity: AlbumIssueSeverity::Error,
                path: declaration.path.clone(),
                line_number: None,
                key_path: None,
                message: format!(
                    "{}: album is invalid; the records index was left unchanged",
                    declaration.path
                ),
                suggestions: vec![
                    "repair the album findings, then run `bob albums sync`".to_owned(),
                ],
            });
        }
    }
    let mut issues: Vec<AlbumIssue> = entry
        .findings
        .iter()
        .chain(plan.findings.iter())
        .map(AlbumIssue::from_finding)
        .collect();
    issues.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.code.cmp(right.code))
            .then_with(|| left.message.cmp(&right.message))
    });
    let schema_fingerprint = record_type.map(|record_type| {
        schema_fingerprint(record_type, declaration.plural.as_deref())
    });
    let singular = declaration
        .plural
        .as_ref()
        .map(|_| resolved_singular(declaration));
    AlbumProjection {
        entry: AlbumReportEntry {
            stem: declaration.stem.clone(),
            path: declaration.path.clone(),
            title: declaration.title.clone(),
            emoji: declaration.emoji.clone(),
            summary: declaration.summary.clone(),
            noun: declaration.noun_claim.clone(),
            label: declaration.label.clone(),
            plural: declaration.plural.clone(),
            singular,
            dated_notes: record_type.map(|record_type| record_type.dated_notes),
            legacy: record_type.map(|record_type| record_type.legacy.clone()),
            schema_fingerprint,
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
            index: plan.state,
            issues,
        },
        index_plan: plan,
        io_errors,
    }
}

/// Shared report and writer fingerprint calculation. A missing plural uses
/// the same empty-string argument as the schema's semantic hash.
pub(crate) fn schema_fingerprint(
    record_type: &RecordType,
    plural: Option<&str>,
) -> String {
    album_schema_fingerprint(
        &record_type.id,
        &record_type.label,
        plural.unwrap_or(""),
        record_type,
    )
}

pub(crate) fn resolved_plural(declaration: &AlbumDeclaration) -> String {
    declaration
        .plural
        .clone()
        .or_else(|| {
            declaration
                .noun_claim
                .as_deref()
                .map(|noun| format!("{noun}s"))
        })
        .unwrap_or_else(|| "records".to_owned())
}

pub(crate) fn resolved_singular(declaration: &AlbumDeclaration) -> String {
    if let Some(label) = declaration.label.as_deref() {
        return album_singular(label);
    }
    if let Some(noun) = declaration.noun_claim.as_deref() {
        return album_singular(&album_label(noun));
    }
    "record".to_owned()
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

#[cfg(test)]
mod tests {
    use super::album_schema_fingerprint;
    use crate::native::albums::parse_album_declaration;
    use std::path::Path;

    const SCHEMA: &str = concat!(
        "---\n",
        "type: [[album]]\n",
        "record:\n",
        "  noun: restaurant\n",
        "  label: Restaurant\n",
        "  plural: restaurants\n",
        "  dated_notes: true\n",
        "  fields:\n",
        "    status:\n",
        "      required: true\n",
        "      values:\n",
        "        to_try: Try\n",
        "        liked: Like\n",
        "    category: {suggest: true}\n",
        "  legacy: [lid, source_*]\n",
        "---\n",
    );

    fn fingerprint(source: &str) -> String {
        let declaration = parse_album_declaration(Path::new("eat.md"), source)
            .expect("album declaration");
        let record_type = declaration.record_type.as_ref().expect("schema");
        album_schema_fingerprint(
            &record_type.id,
            &record_type.label,
            declaration.plural.as_deref().expect("resolved plural"),
            record_type,
        )
    }

    #[test]
    fn album_schema_fingerprint_ignores_yaml_formatting() {
        let flow_style = concat!(
            "---\n",
            "type: [[album]]\n",
            "record: {noun: restaurant, label: Restaurant, plural: restaurants, ",
            "dated_notes: true, fields: {status: {required: true, values: ",
            "{to_try: Try, liked: Like}}, category: {suggest: true}}, ",
            "legacy: [lid, source_*]}\n",
            "---\n",
        );
        assert_eq!(fingerprint(SCHEMA), fingerprint(flow_style));
    }

    #[test]
    fn album_schema_fingerprint_tracks_order_and_semantic_changes() {
        let baseline = fingerprint(SCHEMA);
        let reordered = SCHEMA.replace(
            "    status:\n      required: true\n      values:\n        to_try: Try\n        liked: Like\n    category: {suggest: true}\n",
            "    category: {suggest: true}\n    status:\n      required: true\n      values:\n        to_try: Try\n        liked: Like\n",
        );
        assert_ne!(baseline, fingerprint(&reordered));

        assert_ne!(
            baseline,
            fingerprint(&SCHEMA.replace("label: Restaurant", "label: Eatery"))
        );
        assert_ne!(
            baseline,
            fingerprint(&SCHEMA.replace("required: true", "required: false"))
        );
        assert_ne!(
            baseline,
            fingerprint(&SCHEMA.replace("liked: Like", "enjoyed: Like"))
        );
        assert_ne!(
            baseline,
            fingerprint(&SCHEMA.replace("liked: Like", "liked: Love"))
        );
    }
}

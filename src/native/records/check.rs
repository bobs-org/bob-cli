//! Validation of Record Note folders against config or album schemas.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Serialize;

use super::{
    catalog::{FolderIndex, IndexedFieldValue, IndexedRecord},
    context::AlbumRecordIdentity,
    schema::{
        validate_field_input, FieldInputState, RecordDiagnostic, RecordField,
        RecordType, ValidatedFieldInput,
    },
};
use crate::native::{
    notebooks::rules,
    projects::{
        frontmatter_type_names, parse_frontmatter, wikilink_path_target,
    },
    vault_links::{LinkResolution, VaultLinkResolver},
};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Finding {
    pub(crate) severity: &'static str,
    pub(crate) code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) field: Option<String>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypeReport {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) dir: String,
    pub(crate) count: usize,
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) album: Option<AlbumRecordIdentity>,
    pub(crate) findings: Vec<Finding>,
}

/// Album membership context for one collection under check.
#[derive(Clone, Copy)]
pub(crate) struct AlbumMembership<'a> {
    pub(crate) album_path: &'a Path,
    pub(crate) stem: &'a str,
    pub(crate) noun: &'a str,
    pub(crate) resolver: &'a VaultLinkResolver,
}

impl TypeReport {
    pub(crate) fn new(record_type: &RecordType, folder: &FolderIndex) -> Self {
        Self::for_folder(
            record_type.id.clone(),
            record_type.label.clone(),
            record_type.dir.clone(),
            folder,
        )
    }

    pub(crate) fn for_folder(
        id: impl Into<String>,
        label: impl Into<String>,
        dir: impl Into<String>,
        folder: &FolderIndex,
    ) -> Self {
        let mut report = Self {
            id: id.into(),
            label: label.into(),
            dir: dir.into(),
            count: folder.notes().len(),
            errors: 0,
            warnings: 0,
            source: None,
            album: None,
            findings: Vec::new(),
        };
        for diagnostic in folder.diagnostics() {
            report.push_diagnostic(None, diagnostic, "error");
        }
        report
    }

    pub(crate) fn orphan_bucket() -> Self {
        Self {
            id: "record".to_owned(),
            label: "Record".to_owned(),
            dir: String::new(),
            count: 0,
            errors: 0,
            warnings: 0,
            source: None,
            album: None,
            findings: Vec::new(),
        }
    }

    pub(crate) fn with_source(mut self, source: &'static str) -> Self {
        self.source = Some(source);
        self
    }

    pub(crate) fn with_album(
        mut self,
        album: Option<AlbumRecordIdentity>,
    ) -> Self {
        self.album = album;
        self
    }

    pub(crate) fn check_note(
        &mut self,
        record_type: &RecordType,
        note: &IndexedRecord,
        today: NaiveDate,
        membership: Option<AlbumMembership<'_>>,
        contents: Option<&str>,
    ) {
        let file = note.relative_path.display().to_string();
        for diagnostic in &note.diagnostics {
            self.push_diagnostic(Some(&file), diagnostic, "error");
        }

        match membership {
            Some(membership) => {
                self.check_album_type(note, membership.noun, contents);
                self.check_album_parent(note, membership);
                if let Some(contents) = contents {
                    self.check_checkboxes(note, contents);
                }
            }
            None => self.check_config_type(record_type, note),
        }

        for field in &record_type.fields {
            match field_state(note, field) {
                FieldInputState::Absent if field.required => self.push(
                    "error",
                    "record_missing_field",
                    Some(&file),
                    Some(&field.key),
                    format!("missing required field {}", field.key),
                ),
                state => match validate_field_input(field, &state, today) {
                    Ok(ValidatedFieldInput::Absent) => {}
                    Ok(ValidatedFieldInput::Empty) if field.required => self
                        .push(
                            "error",
                            "record_missing_field",
                            Some(&file),
                            Some(&field.key),
                            format!("required field {} is empty", field.key),
                        ),
                    Ok(
                        ValidatedFieldInput::Empty
                        | ValidatedFieldInput::Value(_),
                    ) => {}
                    Err(diagnostic) => {
                        self.push_diagnostic(Some(&file), &diagnostic, "error")
                    }
                },
            }
        }

        for field in &note.fields {
            if matches!(
                field.key.as_str(),
                "parent" | "type" | "title" | "created"
            ) || record_type.field(&field.key).is_some()
                || legacy_field(&field.key, &record_type.legacy)
            {
                continue;
            }
            self.push(
                "error",
                "record_unknown_field",
                Some(&file),
                Some(&field.key),
                format!("unknown field {}", field.key),
            );
        }
    }

    /// Membership, type, and checkbox checks that do not need a usable schema.
    pub(crate) fn check_album_membership(
        &mut self,
        note: &IndexedRecord,
        membership: AlbumMembership<'_>,
        contents: Option<&str>,
    ) {
        let file = note.relative_path.display().to_string();
        for diagnostic in &note.diagnostics {
            self.push_diagnostic(Some(&file), diagnostic, "error");
        }
        self.check_album_type(note, membership.noun, contents);
        self.check_album_parent(note, membership);
        if let Some(contents) = contents {
            self.check_checkboxes(note, contents);
        }
    }

    pub(crate) fn check_suggested_values(
        &mut self,
        record_type: &RecordType,
        folder: &FolderIndex,
    ) {
        for field in record_type.fields.iter().filter(|field| field.suggest) {
            let mut values: BTreeMap<String, (BTreeSet<String>, String)> =
                BTreeMap::new();
            for note in folder.notes() {
                let Some(value) = note.field(&field.key) else {
                    continue;
                };
                let members: Vec<&str> = match value {
                    IndexedFieldValue::Scalar(value) => vec![value],
                    IndexedFieldValue::List(values) => {
                        values.iter().map(String::as_str).collect()
                    }
                    IndexedFieldValue::Null
                    | IndexedFieldValue::Unsupported => Vec::new(),
                };
                for member in members {
                    let display = suggestion_display(member);
                    let fold = display.to_lowercase();
                    let entry = values.entry(fold).or_insert_with(|| {
                        (
                            BTreeSet::new(),
                            note.relative_path.display().to_string(),
                        )
                    });
                    entry.0.insert(member.to_owned());
                }
            }
            for (_, (spellings, file)) in values {
                if spellings.len() < 2 {
                    continue;
                }
                self.push(
                    "warning",
                    "record_near_duplicate_value",
                    Some(&file),
                    Some(&field.key),
                    format!(
                        "{} has equivalent spellings: {}",
                        field.label,
                        spellings.into_iter().collect::<Vec<_>>().join(" · ")
                    ),
                );
            }
        }
    }

    pub(crate) fn push_diagnostic(
        &mut self,
        file: Option<&str>,
        diagnostic: &RecordDiagnostic,
        severity: &'static str,
    ) {
        self.push(
            severity,
            diagnostic.diagnostic_code(),
            file,
            diagnostic.field.as_deref(),
            diagnostic.message.clone(),
        );
    }

    pub(crate) fn push(
        &mut self,
        severity: &'static str,
        code: &'static str,
        file: Option<&str>,
        field: Option<&str>,
        message: impl Into<String>,
    ) {
        match severity {
            "warning" => self.warnings += 1,
            _ => self.errors += 1,
        }
        self.findings.push(Finding {
            severity,
            code,
            file: file.map(str::to_owned),
            field: field.map(str::to_owned),
            message: message.into(),
        });
    }

    fn check_config_type(
        &mut self,
        record_type: &RecordType,
        note: &IndexedRecord,
    ) {
        let file = note.relative_path.display().to_string();
        let expected_type = format!("[[{}]]", record_type.id);
        match note.field("type") {
            Some(IndexedFieldValue::Scalar(value))
                if value == &expected_type => {}
            Some(_) => self.push(
                "error",
                "record_wrong_type",
                Some(&file),
                Some("type"),
                format!("expected type [[{}]]", record_type.id),
            ),
            None => self.push(
                "error",
                "record_missing_type",
                Some(&file),
                Some("type"),
                format!("missing type [[{}]]", record_type.id),
            ),
        }
    }

    fn check_album_type(
        &mut self,
        note: &IndexedRecord,
        noun: &str,
        contents: Option<&str>,
    ) {
        let file = note.relative_path.display().to_string();
        let names = type_names(note, contents);
        if names.is_empty() {
            self.push(
                "error",
                "record_missing_type",
                Some(&file),
                Some("type"),
                "missing type [[record]]".to_owned(),
            );
            return;
        }
        if names.iter().any(|name| name == "record") {
            return;
        }
        if names.iter().any(|name| name == noun) {
            self.push(
                "warning",
                "record_type_legacy",
                Some(&file),
                Some("type"),
                format!(
                    "album records use type [[record]]; [[{noun}]] is accepted as a legacy spelling"
                ),
            );
            return;
        }
        self.push(
            "error",
            "record_wrong_type",
            Some(&file),
            Some("type"),
            "expected type [[record]]".to_owned(),
        );
    }

    fn check_album_parent(
        &mut self,
        note: &IndexedRecord,
        membership: AlbumMembership<'_>,
    ) {
        let file = note.relative_path.display().to_string();
        let expected = format!("[[{}]]", membership.stem);
        match parent_values(note) {
            ParentValues::Absent => self.push(
                "error",
                "record_parent_mismatch",
                Some(&file),
                Some("parent"),
                format!("missing parent {expected}"),
            ),
            ParentValues::Ambiguous => self.push(
                "error",
                "record_parent_mismatch",
                Some(&file),
                Some("parent"),
                format!(
                    "parent is ambiguous; records must parent to {expected}"
                ),
            ),
            ParentValues::One(raw) => {
                let Some(target) = wikilink_path_target(&raw) else {
                    self.push(
                        "error",
                        "record_parent_mismatch",
                        Some(&file),
                        Some("parent"),
                        format!("parent does not resolve to {expected}"),
                    );
                    return;
                };
                match membership
                    .resolver
                    .resolve(&note.relative_path, &target)
                {
                    LinkResolution::Found(path)
                        if paths_match(&path, membership.album_path) => {}
                    LinkResolution::Ambiguous => self.push(
                        "error",
                        "record_parent_mismatch",
                        Some(&file),
                        Some("parent"),
                        format!(
                            "parent is ambiguous; records must parent to {expected}"
                        ),
                    ),
                    LinkResolution::Found(_) | LinkResolution::Missing => self
                        .push(
                            "error",
                            "record_parent_mismatch",
                            Some(&file),
                            Some("parent"),
                            format!("parent does not resolve to {expected}"),
                        ),
                }
            }
        }
    }

    fn check_checkboxes(&mut self, note: &IndexedRecord, contents: &str) {
        let file = note.relative_path.display().to_string();
        let lines = rules::physical_lines(contents);
        let body_start = rules::body_start_line(contents);
        let mut fence = None;
        for (index, line) in lines.iter().enumerate() {
            let line_number = index + 1;
            if line_number < body_start || rules::track_fence(line, &mut fence)
            {
                continue;
            }
            if fence.is_none() && rules::is_checkbox_line(line) {
                self.push(
                    "error",
                    "record_checkbox",
                    Some(&file),
                    None,
                    format!(
                        "checkbox on line {line_number}; records hold facts, never tasks"
                    ),
                );
            }
        }
    }
}

enum ParentValues {
    Absent,
    Ambiguous,
    One(String),
}

fn parent_values(note: &IndexedRecord) -> ParentValues {
    match note.field("parent") {
        None | Some(IndexedFieldValue::Null) => ParentValues::Absent,
        Some(IndexedFieldValue::Unsupported) => ParentValues::Ambiguous,
        Some(IndexedFieldValue::Scalar(value)) => {
            if value.trim().is_empty() {
                ParentValues::Absent
            } else {
                ParentValues::One(value.clone())
            }
        }
        Some(IndexedFieldValue::List(values)) => {
            let members: Vec<String> = values
                .iter()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            match members.len() {
                0 => ParentValues::Absent,
                1 => ParentValues::One(
                    members.into_iter().next().expect("len 1"),
                ),
                _ => ParentValues::Ambiguous,
            }
        }
    }
}

fn type_names(note: &IndexedRecord, contents: Option<&str>) -> Vec<String> {
    if let Some(contents) = contents
        && let Some(frontmatter) = parse_frontmatter(contents)
    {
        return frontmatter_type_names(&frontmatter);
    }
    match note.field("type") {
        Some(IndexedFieldValue::Scalar(value)) => yaml_type_names(value),
        Some(IndexedFieldValue::List(values)) => values
            .iter()
            .flat_map(|value| yaml_type_names(value))
            .collect(),
        _ => Vec::new(),
    }
}

fn yaml_type_names(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    // Reconstruct a tiny frontmatter so album checks share the vault's
    // type-name parser (quoted, bare, flow-list, and nested `[["name"]]`).
    let reconstructed = format!("---\ntype: {trimmed}\n---\n");
    parse_frontmatter(&reconstructed)
        .map(|frontmatter| frontmatter_type_names(&frontmatter))
        .unwrap_or_default()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

fn suggestion_display(original: &str) -> String {
    let trimmed = original.trim();
    if let Some(inner) = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        && !inner.contains("[[")
    {
        crate::native::capture_block_ids::wikilink_phrase(inner)
    } else {
        original.to_owned()
    }
}

fn field_state(note: &IndexedRecord, field: &RecordField) -> FieldInputState {
    match note.field(&field.key) {
        None => FieldInputState::Absent,
        Some(IndexedFieldValue::Null) => FieldInputState::SuppliedEmpty,
        Some(IndexedFieldValue::Scalar(value)) => {
            FieldInputState::SuppliedValues(vec![value.clone()])
        }
        Some(IndexedFieldValue::List(values)) => {
            FieldInputState::SuppliedValues(values.clone())
        }
        Some(IndexedFieldValue::Unsupported) => {
            FieldInputState::SuppliedValues(vec![
                "<unsupported YAML value>".to_owned()
            ])
        }
    }
}

fn legacy_field(key: &str, legacy: &[String]) -> bool {
    legacy.iter().any(|pattern| {
        if let Some(prefix) = pattern.strip_suffix('*') {
            key.starts_with(prefix)
        } else if let Some(suffix) = pattern.strip_prefix('*') {
            key.ends_with(suffix)
        } else {
            key == pattern
        }
    })
}

/// Walk the vault for `[[record]]` notes that sit outside every album folder.
pub(crate) fn collect_orphan_records(
    vault: &Path,
    album_dirs: &BTreeSet<String>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_markdown(vault, vault, album_dirs, &mut files);
    let mut orphans = Vec::new();
    for path in files {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            continue;
        };
        if !frontmatter_type_names(&frontmatter)
            .iter()
            .any(|name| name == "record")
        {
            continue;
        }
        orphans.push(path);
    }
    orphans
}

fn collect_markdown(
    root: &Path,
    directory: &Path,
    skip_dirs: &BTreeSet<String>,
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let Some(name) = path.file_name() else {
                continue;
            };
            if crate::native::is_always_excluded_note_directory_name(name)
                || (directory == root
                    && skip_dirs.iter().any(|dir| {
                        name.to_string_lossy().eq_ignore_ascii_case(dir)
                    }))
            {
                continue;
            }
            collect_markdown(root, &path, skip_dirs, files);
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
}

/// Config kinds keep the legacy outside-folder scan, skipping every config
/// collection folder. Album folders are never treated as config exclusions.
pub(crate) fn collect_markdown_outside_config_folders(
    vault: &Path,
    skip_dirs: &BTreeSet<String>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_markdown(vault, vault, skip_dirs, &mut files);
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::records::catalog::RecordCatalog;
    use crate::native::records::schema::{FieldKind, RecordField};

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, text).expect("write");
    }

    fn restaurant_type() -> RecordType {
        RecordType {
            id: "restaurant".to_owned(),
            dir: "eat".to_owned(),
            label: "Restaurant".to_owned(),
            emoji: None,
            parent: None,
            dated_notes: false,
            fields: vec![RecordField {
                key: "status".to_owned(),
                label: "Status".to_owned(),
                kind: FieldKind::Text,
                required: true,
                list: false,
                default: None,
                suggest: false,
                min: None,
                max: None,
            }],
            legacy: Vec::new(),
        }
    }

    #[test]
    fn album_accepts_canonical_type_and_warns_on_legacy_noun() {
        let temp = tempfile::tempdir().expect("temp");
        let vault = temp.path();
        write(
            &vault.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\n---\n# Eat\n",
        );
        write(&vault.join("body.md"), "---\ntype: [[area]]\n---\n# Body\n");
        write(
            &vault.join("eat/nagi.md"),
            "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Nagi\nstatus: liked\n---\n# Nagi\n",
        );
        write(
            &vault.join("eat/legacy.md"),
            "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: Legacy\nstatus: liked\n---\n# Legacy\n",
        );
        let mut catalog = RecordCatalog::new(vault);
        let folder = catalog.folder("eat");
        let resolver = VaultLinkResolver::new(vault);
        let membership = AlbumMembership {
            album_path: Path::new("eat.md"),
            stem: "eat",
            noun: "restaurant",
            resolver: &resolver,
        };
        let record_type = restaurant_type();
        let mut report = TypeReport::new(&record_type, folder);
        for note in folder.notes() {
            let contents =
                std::fs::read_to_string(vault.join(&note.relative_path)).ok();
            report.check_note(
                &record_type,
                note,
                NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
                Some(membership),
                contents.as_deref(),
            );
        }
        assert_eq!(report.errors, 0, "{:?}", report.findings);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == "record_type_legacy"),
            "{:?}",
            report.findings
        );
    }

    #[test]
    fn album_flags_wrong_parent_and_real_checkboxes_not_fenced() {
        let temp = tempfile::tempdir().expect("temp");
        let vault = temp.path();
        write(
            &vault.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\n---\n# Eat\n",
        );
        write(&vault.join("body.md"), "---\ntype: [[area]]\n---\n# Body\n");
        write(
            &vault.join("eat/wrong.md"),
            "---\nparent: \"[[body]]\"\ntype: \"[[record]]\"\ntitle: Wrong\nstatus: liked\n---\n# Wrong\n\n- [ ] book a table\n\n```\n- [ ] fenced example\n```\n",
        );
        let mut catalog = RecordCatalog::new(vault);
        let folder = catalog.folder("eat");
        let resolver = VaultLinkResolver::new(vault);
        let membership = AlbumMembership {
            album_path: Path::new("eat.md"),
            stem: "eat",
            noun: "restaurant",
            resolver: &resolver,
        };
        let mut report =
            TypeReport::for_folder("restaurant", "Restaurant", "eat", folder);
        for note in folder.notes() {
            let contents =
                std::fs::read_to_string(vault.join(&note.relative_path)).ok();
            report.check_album_membership(
                note,
                membership,
                contents.as_deref(),
            );
        }
        let codes: Vec<_> =
            report.findings.iter().map(|finding| finding.code).collect();
        assert!(codes.contains(&"record_parent_mismatch"), "{codes:?}");
        assert!(codes.contains(&"record_checkbox"), "{codes:?}");
        assert_eq!(
            report
                .findings
                .iter()
                .filter(|finding| finding.code == "record_checkbox")
                .count(),
            1,
            "fenced checkbox is ignored: {:?}",
            report.findings
        );
    }
}

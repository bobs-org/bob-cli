//! Per-record `bob records show TYPE STEM` read.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};
use serde_yaml::Value as YamlValue;

use super::{
    catalog::{staged_record, RecordCatalog},
    check::{legacy_field, AlbumMembership, TypeReport},
    context::{RecordContext, ResolvedRecordKind},
    fingerprint::{
        empty_record_fingerprint, parse_frontmatter_mapping,
        record_fingerprint, strip_bom, yaml_to_canonical_json,
    },
    plan::{
        display_value, plan_update, resolve_update_target, yaml_to_inputs,
        PlanUpdateRequest, RecordPropertyState, RecordUpdateTargetError,
        RecordUpdateTargetErrorCode, ResolvedUpdateTarget,
    },
    schema::{
        record_value_json, validate_field_input, FieldInputState, FieldKind,
        RecordType, ValidatedFieldInput, IDENTITY_FIELD_NAMES,
    },
};
use crate::native::{
    albums::{
        grouping::group_field, registry::RecordTypeSource,
        report::schema_fingerprint, walk as album_walk, write as album_write,
        AlbumDeclaration,
    },
    env as bob_env,
    notebooks::rules,
    style::{pad_right, Styler},
    vault_links::VaultLinkResolver,
};

const SCHEMA_VERSION: u32 = 1;
const COMMAND: &str = "bob records show";

pub(crate) fn execute_show_record(
    json_output: bool,
    vault: &Path,
    context: &RecordContext,
    kind: &ResolvedRecordKind,
    stem: &str,
) -> i32 {
    let mut catalog = RecordCatalog::new(vault);
    let config = super::cli::config_from_kind(kind);
    let target = match resolve_update_target(
        &config,
        &mut catalog,
        &kind.record_type().id,
        stem,
    ) {
        Ok(target) => target,
        Err(error) => {
            return refuse_unresolved(json_output, vault, kind, stem, error)
        }
    };
    let absolute = vault.join(&target.path);
    let original = match fs::read_to_string(&absolute) {
        Ok(original) => original,
        Err(error) => {
            return emit_error(
                json_output,
                None,
                &format!(
                    "could not read record {}: {error}",
                    target.path.display()
                ),
                1,
            )
        }
    };
    let contents = strip_bom(&original);
    match parent_mismatch(vault, kind, &target, contents, &mut catalog) {
        Ok(()) => {}
        Err(message) => {
            return emit_error(
                json_output,
                Some("record_parent_mismatch"),
                &message,
                2,
            )
        }
    }
    match build_payload(vault, context, kind, &target, contents, &mut catalog) {
        Ok(payload) => {
            print_record(&payload, kind, json_output);
            0
        }
        Err(message) => emit_error(json_output, None, &message, 1),
    }
}

fn refuse_unresolved(
    json_output: bool,
    vault: &Path,
    kind: &ResolvedRecordKind,
    stem: &str,
    error: RecordUpdateTargetError,
) -> i32 {
    if matches!(
        error.code,
        RecordUpdateTargetErrorCode::UnknownStem
            | RecordUpdateTargetErrorCode::UnsafeTarget
    ) && let Some(path) =
        record_outside_folder(vault, &kind.record_type().dir, stem)
    {
        return emit_error(
            json_output,
            Some("record_outside_album"),
            &format!(
                "record `{stem}` is outside {}: {}",
                kind.record_type().dir,
                path.display()
            ),
            2,
        );
    }
    emit_error(
        json_output,
        Some(error.code.diagnostic_code()),
        &error.to_string(),
        2,
    )
}

fn parent_mismatch(
    vault: &Path,
    kind: &ResolvedRecordKind,
    target: &ResolvedUpdateTarget,
    contents: &str,
    catalog: &mut RecordCatalog,
) -> Result<(), String> {
    let record_type = kind.record_type();
    let folder = catalog.folder(&record_type.dir);
    let note =
        staged_record(target.path.clone(), target.stem.clone(), contents);
    let RecordTypeSource::Album { path } = &kind.sourced.source;
    let resolver = VaultLinkResolver::new(vault);
    let membership = AlbumMembership {
        album_path: Path::new(path),
        stem: record_type.dir.as_str(),
        noun: record_type.id.as_str(),
        resolver: &resolver,
    };
    let mut report = TypeReport::for_folder(
        record_type.id.clone(),
        record_type.label.clone(),
        record_type.dir.clone(),
        folder,
    );
    report.findings.clear();
    report.errors = 0;
    report.warnings = 0;
    report.check_note(
        record_type,
        &note,
        bob_env::current_datetime().date(),
        Some(membership),
        Some(contents),
    );
    if let Some(finding) = report
        .findings
        .iter()
        .find(|finding| finding.code == "record_parent_mismatch")
    {
        return Err(finding.message.clone());
    }
    Ok(())
}

/// Current semantic fingerprint for the note bytes plus the album schema
/// fingerprint it was hashed with. Shared by `show` and the guarded
/// `update -i` expect check so the card's fingerprint always matches.
pub(crate) fn current_record_fingerprint(
    context: &RecordContext,
    kind: &ResolvedRecordKind,
    contents: &str,
) -> (String, String) {
    let record_type = kind.record_type();
    let schema_fp = album_schema_fingerprint(context, kind, record_type);
    let fingerprint = match parse_frontmatter_mapping(contents) {
        Some(mapping) => record_fingerprint(&mapping, &schema_fp),
        None => empty_record_fingerprint(&schema_fp),
    };
    (fingerprint, schema_fp)
}

/// The `record` object from the per-record read, cloned for the
/// `stale_record` refusal so the Record Card repaints in one call. Unlike
/// `execute_show_record` this never refuses a parent mismatch: the update
/// path resolves exactly like `update` and only needs current values.
pub(crate) fn record_json_for(
    vault: &Path,
    context: &RecordContext,
    kind: &ResolvedRecordKind,
    target: &ResolvedUpdateTarget,
    contents: &str,
    catalog: &mut RecordCatalog,
) -> Result<Value, String> {
    let payload =
        build_payload(vault, context, kind, target, contents, catalog)?;
    Ok(payload["record"].clone())
}

fn build_payload(
    vault: &Path,
    context: &RecordContext,
    kind: &ResolvedRecordKind,
    target: &ResolvedUpdateTarget,
    contents: &str,
    catalog: &mut RecordCatalog,
) -> Result<Value, String> {
    let record_type = kind.record_type();
    let today = bob_env::current_datetime().date();
    let plan = plan_update(PlanUpdateRequest {
        target,
        original: contents,
        fields: &[],
        notes: &[],
        today,
    })
    .ok();
    let mapping = parse_frontmatter_mapping(contents);
    let key_lines = top_level_key_lines(contents);
    let (fingerprint, schema_fp) =
        current_record_fingerprint(context, kind, contents);
    let (fields, count) = {
        let folder = catalog.folder(&record_type.dir);
        let fields: Vec<Value> = record_type
            .fields
            .iter()
            .map(|field| {
                let choice_values: Vec<String> = match &field.kind {
                    FieldKind::Choices(choices) => choices
                        .iter()
                        .map(|choice| choice.value.clone())
                        .collect(),
                    _ => Vec::new(),
                };
                let (filled, choice_counts) =
                    folder.field_usage(&field.key, &choice_values);
                let line = kind.album.as_ref().and_then(|album| {
                    album.schema_field_lines.get(&field.key).copied()
                });
                let mut field_json =
                    field.schema_json(line, filled, &choice_counts);
                let top_values: Vec<Value> = if field.suggest {
                    folder
                        .value_suggestions(&field.key)
                        .into_iter()
                        .take(5)
                        .map(|value| {
                            json!({
                                "value": value.insert,
                                "count": value.count,
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let values: Vec<Value> = if field.suggest {
                    folder
                        .value_suggestions(&field.key)
                        .into_iter()
                        .map(|value| {
                            json!({
                                "value": value.insert,
                                "count": value.count,
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                field_json["top_values"] = json!(top_values);
                field_json["values"] = json!(values);
                field_json
            })
            .collect();
        (fields, folder.notes().len())
    };

    let properties = record_properties(
        record_type,
        mapping.as_ref(),
        &key_lines,
        plan.as_ref().map(|plan| plan.properties.as_slice()),
        today,
    );
    let identity = identity_keys(mapping.as_ref(), &key_lines);
    let other = other_keys(record_type, mapping.as_ref(), &key_lines);
    let findings =
        record_findings(vault, kind, target, contents, catalog, &key_lines);
    let group = group_json(record_type, &properties);
    let title = mapping
        .as_ref()
        .and_then(|mapping| yaml_string(mapping, "title"))
        .filter(|title| !title.is_empty())
        .or_else(|| {
            plan.as_ref()
                .map(|plan| plan.title.clone())
                .filter(|title| !title.is_empty())
        })
        .unwrap_or_else(|| target.title.clone());
    let created = mapping
        .as_ref()
        .and_then(|mapping| yaml_string(mapping, "created"));
    let heading = first_heading(contents);
    let path = target.path.to_string_lossy().replace('\\', "/");
    let walk = {
        let folder = catalog.folder(&record_type.dir);
        record_walk_value(vault, context, kind, folder, &path, contents)
    };
    let mut payload = json!({
        "schema_version": SCHEMA_VERSION,
        "type": record_type.id,
        "label": record_type.label,
        "dir": record_type.dir,
        "emoji": record_type.emoji,
        "dated_notes": record_type.dated_notes,
        "count": count,
        "fields": fields,
        "record": {
            "stem": target.stem,
            "path": path,
            "title": title,
            "heading": heading,
            "created": created,
            "fingerprint": fingerprint,
            "schema_fingerprint": schema_fp,
            "frontmatter_end_line": frontmatter_end_line(contents),
            "group": group,
            "properties": properties,
            "identity": identity,
            "other": other,
            "findings": findings,
            "walk": walk,
        },
    });
    super::cli::merge_kind_metadata(&mut payload, kind);
    Ok(payload)
}

/// The `record.walk` object for one record, or null when the album is held.
/// Computed from the album's `scope: All` walk plan over current disk bytes,
/// with the shown note's in-memory bytes standing in for its own path.
fn record_walk_value(
    vault: &Path,
    context: &RecordContext,
    kind: &ResolvedRecordKind,
    folder: &super::catalog::FolderIndex,
    target_path: &str,
    target_contents: &str,
) -> Value {
    let declaration = album_declaration_for(context, kind);
    let Some(declaration) = declaration else {
        return Value::Null;
    };
    if !album_write::folder_complete(folder) {
        return Value::Null;
    }
    let members = album_write::overlay_folder_members(folder, &[]);
    if members.is_empty() {
        return Value::Null;
    }
    let mut texts: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for member in &members {
        let slash = member.relative_path.to_string_lossy().replace('\\', "/");
        if slash == target_path {
            texts.insert(slash, target_contents.to_owned());
            continue;
        }
        let Ok(text) = fs::read_to_string(vault.join(&member.relative_path))
        else {
            return Value::Null;
        };
        texts.insert(slash, text);
    }
    let plan = album_walk::plan_album_walk(
        &declaration,
        folder,
        &[],
        |path| texts.get(path).cloned(),
        album_walk::WalkScope::All,
    );
    if matches!(
        plan.state,
        album_walk::AlbumWalkState::Held | album_walk::AlbumWalkState::Empty
    ) {
        return Value::Null;
    }
    let Some(record) = plan
        .records
        .iter()
        .find(|record| record.slot.member.path == target_path)
    else {
        return Value::Null;
    };
    let prev = record.slot.prev.as_ref().map(|prev| {
        json!({
            "stem": prev.stem,
            "path": prev.path,
            "title": prev.title,
        })
    });
    let next = record.slot.next.as_ref().map(|next| {
        json!({
            "stem": next.stem,
            "path": next.path,
            "title": next.title,
        })
    });
    let reason = record
        .reason
        .map(|reason| Value::String(reason.as_str().to_owned()))
        .unwrap_or(Value::Null);
    json!({
        "state": record.state.as_str(),
        "reason": reason,
        "position": record.slot.position,
        "total": record.slot.total,
        "prev": prev.unwrap_or(Value::Null),
        "next": next.unwrap_or(Value::Null),
    })
}

fn album_declaration_for(
    context: &RecordContext,
    kind: &ResolvedRecordKind,
) -> Option<AlbumDeclaration> {
    let RecordTypeSource::Album { path } = &kind.sourced.source;
    context.album_claims().iter().find_map(|entry| {
        (entry.declaration.path == *path).then(|| entry.declaration.clone())
    })
}

fn record_properties(
    record_type: &RecordType,
    mapping: Option<&serde_yaml::Mapping>,
    key_lines: &BTreeMap<String, usize>,
    planned: Option<&[super::plan::PlannedUpdateProperty]>,
    today: chrono::NaiveDate,
) -> Vec<Value> {
    record_type
        .fields
        .iter()
        .map(|field| {
            let yaml_key = YamlValue::String(field.key.clone());
            let raw_yaml = mapping.and_then(|mapping| mapping.get(&yaml_key));
            let line = key_lines.get(&field.key).copied();
            let raw =
                raw_yaml.map(yaml_to_canonical_json).unwrap_or(Value::Null);
            let state_input = yaml_field_state(raw_yaml);
            let planned_row = planned
                .and_then(|rows| rows.iter().find(|row| row.key == field.key));
            let (state, value, display) = match &state_input {
                FieldInputState::Absent | FieldInputState::SuppliedEmpty => {
                    if field.required {
                        (RecordPropertyState::Missing, Value::Null, None)
                    } else {
                        (RecordPropertyState::None, Value::Null, None)
                    }
                }
                FieldInputState::SuppliedValues(_)
                | FieldInputState::SuppliedLiteralValues(_) => {
                    match validate_field_input(field, &state_input, today) {
                        Ok(ValidatedFieldInput::Value(validated)) => {
                            let display = planned_row
                                .and_then(|row| {
                                    row.current_display
                                        .clone()
                                        .or(row.display.clone())
                                })
                                .unwrap_or_else(|| {
                                    display_value(field, &validated, today)
                                });
                            (
                                RecordPropertyState::Set,
                                record_value_json(&validated),
                                Some(display),
                            )
                        }
                        Ok(
                            ValidatedFieldInput::Empty
                            | ValidatedFieldInput::Absent,
                        ) => {
                            if field.required {
                                (
                                    RecordPropertyState::Missing,
                                    Value::Null,
                                    None,
                                )
                            } else {
                                (RecordPropertyState::None, Value::Null, None)
                            }
                        }
                        Err(_) => {
                            (RecordPropertyState::Invalid, Value::Null, None)
                        }
                    }
                }
            };
            json!({
                "key": field.key,
                "label": field.label,
                "kind": field.kind.kind_name(),
                "required": field.required,
                "list": field.list,
                "state": state.as_str(),
                "value": value,
                "display": display,
                "raw": raw,
                "line": line,
            })
        })
        .collect()
}

fn yaml_field_state(raw: Option<&YamlValue>) -> FieldInputState {
    match raw {
        None => FieldInputState::Absent,
        Some(YamlValue::Null) => FieldInputState::SuppliedEmpty,
        Some(value) => match yaml_to_inputs(value) {
            None => FieldInputState::SuppliedValues(vec![
                "<unsupported YAML value>".to_owned(),
            ]),
            Some(inputs) if inputs.is_empty() => FieldInputState::SuppliedEmpty,
            // YAML has already delimited list members. Only CLI `-s`
            // occurrences use the comma-separated input grammar.
            Some(inputs) => FieldInputState::SuppliedLiteralValues(inputs),
        },
    }
}

fn identity_keys(
    mapping: Option<&serde_yaml::Mapping>,
    key_lines: &BTreeMap<String, usize>,
) -> Vec<Value> {
    let Some(mapping) = mapping else {
        return Vec::new();
    };
    IDENTITY_FIELD_NAMES
        .iter()
        .filter_map(|key| {
            let yaml_key = YamlValue::String((*key).to_owned());
            let raw = mapping.get(&yaml_key)?;
            Some(json!({
                "key": key,
                "line": key_lines.get(*key).copied(),
                "value": yaml_to_canonical_json(raw),
            }))
        })
        .collect()
}

fn other_keys(
    record_type: &RecordType,
    mapping: Option<&serde_yaml::Mapping>,
    key_lines: &BTreeMap<String, usize>,
) -> Vec<Value> {
    let Some(mapping) = mapping else {
        return Vec::new();
    };
    mapping
        .iter()
        .filter_map(|(key, value)| {
            let name = key.as_str()?;
            if IDENTITY_FIELD_NAMES.contains(&name)
                || record_type.field(name).is_some()
            {
                return None;
            }
            Some(json!({
                "key": name,
                "line": key_lines.get(name).copied(),
                "value": yaml_to_canonical_json(value),
                "legacy": legacy_field(name, &record_type.legacy),
            }))
        })
        .collect()
}

fn record_findings(
    vault: &Path,
    kind: &ResolvedRecordKind,
    target: &ResolvedUpdateTarget,
    contents: &str,
    catalog: &mut RecordCatalog,
    key_lines: &BTreeMap<String, usize>,
) -> Vec<Value> {
    let record_type = kind.record_type();
    let folder = catalog.folder(&record_type.dir);
    let note =
        staged_record(target.path.clone(), target.stem.clone(), contents);
    let RecordTypeSource::Album { path } = &kind.sourced.source;
    let resolver = VaultLinkResolver::new(vault);
    let membership = AlbumMembership {
        album_path: Path::new(path),
        stem: record_type.dir.as_str(),
        noun: record_type.id.as_str(),
        resolver: &resolver,
    };
    let mut report = TypeReport::for_folder(
        record_type.id.clone(),
        record_type.label.clone(),
        record_type.dir.clone(),
        folder,
    );
    report.findings.clear();
    report.errors = 0;
    report.warnings = 0;
    report.check_note(
        record_type,
        &note,
        bob_env::current_datetime().date(),
        Some(membership),
        Some(contents),
    );
    let path = target.path.to_string_lossy().replace('\\', "/");
    report
        .findings
        .into_iter()
        .filter(|finding| {
            finding.file.as_deref() == Some(path.as_str())
                || finding.file.is_none()
        })
        .map(|finding| {
            let line = finding
                .field
                .as_ref()
                .and_then(|field| key_lines.get(field).copied())
                .or(finding.line);
            json!({
                "code": finding.code,
                "severity": finding.severity,
                "message": finding.message,
                "field": finding.field,
                "line": line,
            })
        })
        .collect()
}

fn group_json(record_type: &RecordType, properties: &[Value]) -> Value {
    let Some(field) = group_field(&record_type.fields) else {
        return Value::Null;
    };
    let Some(row) = properties.iter().find(|row| row["key"] == field.key)
    else {
        return Value::Null;
    };
    if row["state"] != "set" {
        return Value::Null;
    }
    let value = row["value"].as_str().unwrap_or("");
    if value.is_empty() {
        return Value::Null;
    }
    let label = row["display"].as_str().unwrap_or(value);
    json!({
        "field": field.key,
        "value": value,
        "label": label,
    })
}

fn album_schema_fingerprint(
    context: &RecordContext,
    kind: &ResolvedRecordKind,
    record_type: &RecordType,
) -> String {
    let RecordTypeSource::Album { path } = &kind.sourced.source;
    let plural = context
        .album_claims()
        .iter()
        .find(|entry| entry.declaration.path == *path)
        .and_then(|entry| entry.declaration.plural.as_deref());
    schema_fingerprint(record_type, plural)
}

fn yaml_string(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping
        .get(YamlValue::String(key.to_owned()))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .filter(|text| !text.is_empty())
}

fn record_outside_folder(
    vault: &Path,
    dir: &str,
    stem: &str,
) -> Option<PathBuf> {
    let folded = stem.to_ascii_lowercase();
    let mut nested = None;
    collect_stem_matches(vault, &vault.join(dir), dir, &folded, &mut nested);
    if nested.is_some() {
        return nested;
    }
    let root = vault.join(format!("{stem}.md"));
    if root.is_file() {
        return Some(PathBuf::from(format!("{stem}.md")));
    }
    None
}

fn collect_stem_matches(
    vault: &Path,
    directory: &Path,
    album_dir: &str,
    folded_stem: &str,
    found: &mut Option<PathBuf>,
) {
    if found.is_some() {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
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
            collect_stem_matches(vault, &path, album_dir, folded_stem, found);
            continue;
        }
        if !file_type.is_file()
            || !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.to_ascii_lowercase() != folded_stem {
            continue;
        }
        let Ok(relative) = path.strip_prefix(vault) else {
            continue;
        };
        let relative =
            PathBuf::from(relative.to_string_lossy().replace('\\', "/"));
        let immediate = PathBuf::from(format!("{album_dir}/{folded_stem}.md"));
        let immediate_orig = PathBuf::from(format!("{album_dir}/{name}.md"));
        if relative == immediate || relative == immediate_orig {
            continue;
        }
        *found = Some(relative);
        return;
    }
}

fn top_level_key_lines(contents: &str) -> BTreeMap<String, usize> {
    let contents = strip_bom(contents);
    let lines = rules::physical_lines(contents);
    let mut map = BTreeMap::new();
    if lines.first().map(|line| line.trim()) != Some("---") {
        return map;
    }
    for (index, line) in lines.iter().enumerate().skip(1) {
        let line_number = index + 1;
        if line.trim() == "---" {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((raw_key, _)) = trimmed.split_once(':') else {
            continue;
        };
        let key = raw_key.trim().trim_matches(['\'', '"']);
        if key.is_empty() {
            continue;
        }
        map.entry(key.to_owned()).or_insert(line_number);
    }
    map
}

fn frontmatter_end_line(contents: &str) -> Option<usize> {
    let contents = strip_bom(contents);
    let lines = rules::physical_lines(contents);
    if lines.first().map(|line| line.trim()) != Some("---") {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim() == "---").then_some(index + 1))
}

fn first_heading(contents: &str) -> Option<String> {
    let contents = strip_bom(contents);
    let lines = rules::physical_lines(contents);
    let mut in_frontmatter = false;
    let mut seen_open = false;
    let mut fence = None;
    for line in lines {
        if !seen_open {
            if line.trim() == "---" {
                seen_open = true;
                in_frontmatter = true;
                continue;
            }
            seen_open = true;
        } else if in_frontmatter {
            if line.trim() == "---" {
                in_frontmatter = false;
            }
            continue;
        }
        if rules::track_fence(line, &mut fence) || fence.is_some() {
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(title) = parse_h1(trimmed) {
            return Some(title);
        }
        if trimmed.starts_with("## ") || trimmed == "##" {
            break;
        }
    }
    None
}

/// Human `N of M` summary for the card. Singleton rings show only the
/// count; larger rings name both neighbours with sanitized labels.
fn walk_summary(walk: &Value) -> Option<String> {
    let position = walk.get("position")?.as_u64()?;
    let total = walk.get("total")?.as_u64()?;
    let prev = walk.get("prev").and_then(|prev| {
        let title = prev.get("title")?.as_str()?;
        let stem = prev.get("stem")?.as_str().unwrap_or("");
        Some(album_walk::sanitize_walk_label(title, stem))
    });
    let next = walk.get("next").and_then(|next| {
        let title = next.get("title")?.as_str()?;
        let stem = next.get("stem")?.as_str().unwrap_or("");
        Some(album_walk::sanitize_walk_label(title, stem))
    });
    match (prev, next) {
        (Some(prev), Some(next)) => {
            Some(format!("{position} of {total} · ‹ {prev} · {next} ›"))
        }
        _ => Some(format!("{position} of {total}")),
    }
}

fn parse_h1(line: &str) -> Option<String> {
    if !line.starts_with("# ") && line != "#" {
        return None;
    }
    let title = line.strip_prefix('#').unwrap_or("").trim();
    (!title.is_empty()).then(|| title.to_owned())
}

fn print_record(payload: &Value, kind: &ResolvedRecordKind, json_output: bool) {
    if json_output {
        println!("{payload}");
        return;
    }
    let styler = Styler::detect();
    let record = &payload["record"];
    let title = record["title"].as_str().unwrap_or("");
    let heading = record["heading"].as_str().unwrap_or("");
    let shown_title = if heading.is_empty() { title } else { heading };
    let record_type = kind.record_type();
    let emoji = record_type.emoji.as_deref().unwrap_or("•");
    let album_title = kind
        .album
        .as_ref()
        .map(|album| album.title.as_str())
        .unwrap_or(record_type.dir.as_str());
    println!("{}", styler.bold(shown_title));
    println!(
        "{} {} › {}",
        emoji,
        styler.dim(album_title),
        record_type.label
    );
    println!("{}", styler.dim(record["path"].as_str().unwrap_or("")));
    if let Some(summary) = walk_summary(&record["walk"]) {
        println!("  {summary}");
        if let Some(state) = record["walk"]["state"].as_str()
            && state != "current"
        {
            println!(
                "  {}",
                styler.dim(&format!("walk line {state} — bob albums sync"))
            );
        }
    }
    println!();
    let properties =
        record["properties"].as_array().cloned().unwrap_or_default();
    let label_width = properties
        .iter()
        .map(|row| {
            let label = row["label"].as_str().unwrap_or("");
            let star = if row["required"].as_bool().unwrap_or(false) {
                2
            } else {
                0
            };
            label.chars().count() + star
        })
        .max()
        .unwrap_or(0);
    for row in &properties {
        let label = row["label"].as_str().unwrap_or("");
        let required = row["required"].as_bool().unwrap_or(false);
        let named = if required {
            format!("{label} *")
        } else {
            label.to_owned()
        };
        let state = row["state"].as_str().unwrap_or("none");
        let value = match state {
            "missing" => styler.yellow("missing"),
            "invalid" => {
                let raw = row["raw"]
                    .as_str()
                    .map_or_else(|| row["raw"].to_string(), str::to_owned);
                styler.yellow(&raw)
            }
            "none" => styler.dim("—"),
            _ => row["display"].as_str().unwrap_or("").to_owned(),
        };
        println!("  {}  {value}", pad_right(&named, label_width));
    }
    let findings = record["findings"].as_array().cloned().unwrap_or_default();
    let other = record["other"].as_array().map_or(0, Vec::len);
    if !findings.is_empty() {
        println!();
        for finding in findings {
            let severity = finding["severity"].as_str().unwrap_or("error");
            let tag = if severity == "warning" {
                styler.yellow("WARN")
            } else {
                styler.red("ERROR")
            };
            let field = finding["field"]
                .as_str()
                .map(|field| format!(" · {field}"))
                .unwrap_or_default();
            println!(
                "  {tag}{field}: {}",
                finding["message"].as_str().unwrap_or("")
            );
        }
    }
    println!();
    let other_label = if other == 1 {
        "1 other property".to_owned()
    } else {
        format!("{other} other properties")
    };
    println!("  {}", styler.dim(&other_label));
}

fn emit_error(
    json_output: bool,
    error_code: Option<&str>,
    message: &str,
    exit: i32,
) -> i32 {
    if json_output {
        let error = match error_code {
            Some(code) => json!({ "code": code, "message": message }),
            None => json!({ "message": message }),
        };
        println!(
            "{}",
            json!({
                "schema_version": SCHEMA_VERSION,
                "error": error,
            })
        );
    } else {
        eprintln!("{COMMAND}: {message}");
    }
    exit
}

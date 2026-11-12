//! Read-only, snapshot-bound album schema and index planning.
//!
//! The planner resolves one root album by stem, checks the report's
//! semantic fingerprint, applies the typed operation batch, overlays the
//! edited declaration in memory, reruns registry validation, and composes
//! schema and generated-index changes into one album-file edit.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::{
    ops::{self, ApplyInputs, IndexRefs, OpError, OpOutcome, OtherAlbumField},
    protocol::Envelope,
    spans::{self, SchemaSpanEdit, SpanError},
};
use crate::native::{
    albums::{
        catalog::catalog_read_dependencies,
        index::IndexState,
        registry::{confine_vault_file, AlbumRegistry, DiscoveryHealth},
        report::{self, schema_fingerprint},
        write::{folder_complete, plan_album_write},
        AlbumFinding, AlbumIssueSeverity,
    },
    host_catalog::{FileEdit, ReadDependencies},
    notebooks::snapshot::VaultSnapshot,
    records::catalog::{FolderIndex, RecordCatalog},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdatePlanError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) key_path: Option<String>,
    pub(crate) affected: Option<usize>,
    pub(crate) op_index: Option<usize>,
    pub(crate) field: Option<String>,
    pub(crate) choice: Option<String>,
}

impl std::fmt::Display for UpdatePlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "[{}]: {}", self.code, self.message)
    }
}

impl std::error::Error for UpdatePlanError {}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UpdatePlan {
    pub(crate) stem: String,
    pub(crate) path: PathBuf,
    pub(crate) before: String,
    pub(crate) after: String,
    pub(crate) before_fingerprint: String,
    pub(crate) after_fingerprint: String,
    pub(crate) changed: bool,
    pub(crate) op_outcome: OpOutcome,
    pub(crate) schema_edit: SchemaSpanEdit,
    pub(crate) edit: Option<FileEdit>,
    pub(crate) deps: ReadDependencies,
    pub(crate) index_state: &'static str,
}

/// Build a complete read-only plan from one snapshot, registry, and folder
/// catalog. The planner never writes or performs Git/sync operations.
#[allow(clippy::result_large_err)] // Structured refusals retain actionable fields.
pub(crate) fn plan_update(
    vault: &Path,
    snapshot: &VaultSnapshot,
    registry: &AlbumRegistry,
    requested_stem: &str,
    envelope: &Envelope,
    today: NaiveDate,
) -> Result<UpdatePlan, UpdatePlanError> {
    let stem = super::super::confined_album_route(requested_stem).ok_or_else(|| {
        plan_error(
            "album_stem_invalid",
            format!("album stem {requested_stem:?} is not a confined root route"),
            Some("use one root album stem such as `eat`".to_owned()),
        )
    })?;
    if !matches!(registry.health, DiscoveryHealth::Complete)
        || !snapshot.io_errors.is_empty()
    {
        return Err(plan_error(
            "discovery_incomplete",
            format!(
                "the vault scan is incomplete: {}",
                snapshot.io_errors.join("; ")
            ),
            Some("resolve the read errors, then retry the preview".to_owned()),
        ));
    }
    let relative = PathBuf::from(format!("{stem}.md"));
    let target = vault.join(&relative);
    confine_vault_file(vault, &target)
        .map_err(|message| plan_error("album_path_unsafe", message, None))?;
    let candidates = registry
        .albums
        .iter()
        .filter(|entry| entry.declaration.stem == stem)
        .collect::<Vec<_>>();
    let entry = match candidates.as_slice() {
        [entry] if entry.declaration.path == format!("{stem}.md") => *entry,
        [] => {
            return Err(plan_error(
                "album_not_found",
                format!("no root album named {stem:?} exists"),
                Some("a stem selects an album note; it does not fall back to a record noun".to_owned()),
            ));
        }
        _ => {
            return Err(plan_error(
                "album_ambiguous",
                format!("more than one album claims root stem {stem:?}"),
                Some(
                    "repair the competing album declarations first".to_owned(),
                ),
            ));
        }
    };
    if !entry.declaration.path.ends_with(".md") {
        return Err(plan_error(
            "album_path_unsafe",
            "album updates are limited to root Markdown notes",
            None,
        ));
    }
    let before = entry.contents.clone().ok_or_else(|| {
        plan_error(
            "album_unreadable",
            format!(
                "{} was not retained in the complete snapshot",
                entry.declaration.path
            ),
            None,
        )
    })?;
    if let Some(finding) = entry
        .findings
        .iter()
        .find(|finding| finding.severity == AlbumIssueSeverity::Error)
    {
        return Err(finding_error(finding));
    }
    let before_type =
        entry.declaration.record_type.as_ref().ok_or_else(|| {
            plan_error(
                "album_schema_invalid",
                "the selected album has no valid record schema",
                Some(
                    "repair the schema in YAML (`y`) before updating it"
                        .to_owned(),
                ),
            )
        })?;

    let mut catalog = RecordCatalog::new(vault);
    let folder = catalog.folder(&stem);
    if !folder_complete(folder) {
        return Err(plan_error(
            "incomplete_scan",
            format!("record folder {stem}/ could not be read completely"),
            Some(
                "resolve unreadable or missing record frontmatter, then retry"
                    .to_owned(),
            ),
        ));
    }
    let before_fingerprint =
        schema_fingerprint(before_type, entry.declaration.plural.as_deref());
    if envelope.expect != before_fingerprint {
        return Err(plan_error(
            "stale_schema",
            format!(
                "schema fingerprint changed: expected {}, current {}",
                envelope.expect, before_fingerprint
            ),
            Some(
                "refresh the album report and preview the operation again"
                    .to_owned(),
            ),
        ));
    }

    let index_refs = IndexRefs {
        index_by: entry.declaration.index_by.clone(),
        index_facets: entry.declaration.index_facets.clone(),
    };
    let other_fields =
        registry
            .albums
            .iter()
            .filter(|other| other.declaration.path != entry.declaration.path)
            .flat_map(|other| {
                other.declaration.record_type.iter().flat_map(
                    move |record_type| {
                        record_type.fields.iter().map(move |field| {
                            OtherAlbumField {
                                path: other.declaration.path.clone(),
                                key: field.key.clone(),
                                storage: field.kind.storage_kind(),
                                list: field.list,
                            }
                        })
                    },
                )
            })
            .collect::<Vec<_>>();
    let raw = spans::parse_record_mapping(&before).map_err(span_error)?.1;
    let inputs = ApplyInputs {
        before_raw: &raw,
        before_type,
        before_plural: entry.declaration.plural.as_deref(),
        stem: &stem,
        folder,
        index_refs: &index_refs,
        other_fields: &other_fields,
        today,
    };
    let op_outcome =
        ops::apply_ops(&envelope.ops, &inputs).map_err(op_error)?;
    let schema_edit = spans::apply_schema_ops(
        &before,
        &envelope.ops,
        &envelope.restorations,
        &op_outcome,
        &inputs,
    )
    .map_err(span_error)?;

    let mut deps = read_dependencies(
        vault, snapshot, registry, folder, &relative, &before,
    )?;
    if !schema_edit.changed {
        // Semantic no-ops return before inspecting or repairing the current
        // index. In particular, an empty op list cannot become a sync path.
        return Ok(UpdatePlan {
            stem,
            path: relative,
            before: before.clone(),
            after: before,
            before_fingerprint: before_fingerprint.clone(),
            after_fingerprint: before_fingerprint,
            changed: false,
            op_outcome,
            schema_edit,
            edit: None,
            deps,
            index_state: report::project_album_from_folder(entry, folder)
                .entry
                .index
                .as_str(),
        });
    }

    let overlay = snapshot
        .with_overlay([(relative.clone(), schema_edit.contents.clone())]);
    let overlay_registry = AlbumRegistry::from_snapshot(vault, &overlay);
    if !matches!(overlay_registry.health, DiscoveryHealth::Complete) {
        return Err(plan_error(
            "discovery_incomplete",
            "the proposed album overlay made registry discovery incomplete",
            None,
        ));
    }
    let proposed = overlay_registry
        .albums
        .iter()
        .find(|candidate| candidate.declaration.path == entry.declaration.path)
        .ok_or_else(|| {
            plan_error(
                "album_overlay_missing",
                "the proposed album declaration disappeared from registry validation",
                None,
            )
        })?;
    if let Some(finding) = proposed
        .findings
        .iter()
        .find(|finding| finding.severity == AlbumIssueSeverity::Error)
    {
        return Err(finding_error(finding));
    }
    let after_type =
        proposed.declaration.record_type.as_ref().ok_or_else(|| {
            plan_error(
                "album_schema_invalid",
                "the edited schema did not parse",
                None,
            )
        })?;
    let after_fingerprint =
        schema_fingerprint(after_type, proposed.declaration.plural.as_deref());
    let index = plan_album_write(
        &proposed.declaration,
        &schema_edit.contents,
        folder,
        &[],
    );
    if index.state == IndexState::Damaged {
        let reason = index
            .findings
            .first()
            .map(|finding| finding.message.clone())
            .unwrap_or_else(|| {
                "records index cannot be rebuilt safely".to_owned()
            });
        return Err(plan_error(
            "album_index_damaged",
            reason,
            Some(
                "repair the records index markers in YAML/Markdown, then retry"
                    .to_owned(),
            ),
        ));
    }
    let after = index
        .edit
        .as_ref()
        .map(|edit| edit.after.clone())
        .unwrap_or_else(|| schema_edit.contents.clone());
    let changed = after != before;
    let edit = changed.then(|| FileEdit {
        path: relative.clone(),
        before: before.clone(),
        after: after.clone(),
    });
    // The staged album file is already guarded by the transaction path, but
    // keeping its exact preimage in dependencies makes a plan independently
    // auditable and protects read-only consumers of this structure too.
    deps.insert(relative.clone(), Some(before.clone()));
    Ok(UpdatePlan {
        stem,
        path: relative,
        before,
        after,
        before_fingerprint,
        after_fingerprint,
        changed,
        op_outcome,
        schema_edit,
        edit,
        deps,
        index_state: index.state.as_str(),
    })
}

#[allow(clippy::result_large_err)] // Propagate the planner's structured refusal.
fn read_dependencies(
    vault: &Path,
    snapshot: &VaultSnapshot,
    registry: &AlbumRegistry,
    folder: &FolderIndex,
    album_path: &Path,
    album_before: &str,
) -> Result<ReadDependencies, UpdatePlanError> {
    let mut dependencies = catalog_read_dependencies(snapshot, registry);
    dependencies
        .insert(album_path.to_path_buf(), Some(album_before.to_owned()));
    dependencies.insert_directory_membership(
        PathBuf::from(folder.directory()),
        folder.present().then(|| folder.membership()),
    );
    for note in folder.notes() {
        dependencies.insert_frontmatter(
            note.relative_path.clone(),
            note.frontmatter.clone(),
        );
    }
    let namespace = fs::read_dir(vault)
        .map_err(|error| {
            plan_error(
                "dependency_read_failed",
                format!("could not read the album namespace: {error}"),
                None,
            )
        })?
        .map(|entry| {
            entry.map(|entry| entry.file_name().to_string_lossy().into_owned())
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            plan_error(
                "dependency_read_failed",
                format!("could not read an album namespace entry: {error}"),
                None,
            )
        })?
        .into_iter()
        .filter(|name| {
            crate::native::host_catalog::txn::is_album_namespace_name(name)
        })
        .collect::<Vec<_>>();
    dependencies.insert_namespace(PathBuf::from("."), namespace);
    Ok(dependencies)
}

fn finding_error(finding: &AlbumFinding) -> UpdatePlanError {
    UpdatePlanError {
        code: finding.code,
        message: finding.message.clone(),
        hint: finding.suggestions.first().cloned(),
        path: Some(finding.path.clone()),
        line: finding.line_number,
        key_path: finding.key_path.clone(),
        affected: None,
        op_index: None,
        field: None,
        choice: None,
    }
}

fn op_error(error: OpError) -> UpdatePlanError {
    UpdatePlanError {
        code: error.code,
        message: error.message,
        hint: error.hint,
        path: None,
        line: None,
        key_path: error.key_path,
        affected: error.affected,
        op_index: error.op_index,
        field: error.field,
        choice: error.choice,
    }
}

fn span_error(error: SpanError) -> UpdatePlanError {
    UpdatePlanError {
        code: error.code,
        message: error.message,
        hint: Some("edit the schema in YAML (`y`) or simplify the unsupported YAML construct".to_owned()),
        path: None,
        line: error.line,
        key_path: error.key_path,
        affected: None,
        op_index: None,
        field: None,
        choice: None,
    }
}

fn plan_error(
    code: &'static str,
    message: impl Into<String>,
    hint: Option<String>,
) -> UpdatePlanError {
    UpdatePlanError {
        code,
        message: message.into(),
        hint,
        path: None,
        line: None,
        key_path: None,
        affected: None,
        op_index: None,
        field: None,
        choice: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = concat!(
        "---\n",
        "type: [[album]]\n",
        "parent: \"[[body]]\"\n",
        "record:\n",
        "  noun: restaurant\n",
        "  label: Restaurant\n",
        "  fields:\n",
        "    status:\n",
        "      values:\n",
        "        ready: Ready\n",
        "---\n\n",
        "# Restaurants\n",
    );

    fn setup() -> (tempfile::TempDir, PathBuf, VaultSnapshot, AlbumRegistry) {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path().to_path_buf();
        fs::create_dir_all(vault.join("eat")).expect("record folder");
        fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body\n",
        )
        .expect("area");
        fs::write(vault.join("eat.md"), SCHEMA).expect("album");
        fs::write(
            vault.join("eat/ninja.md"),
            "---\nstatus: ready\n---\n\n# Ninja\n",
        )
        .expect("record");
        let snapshot = VaultSnapshot::load(&vault);
        assert!(snapshot.io_errors.is_empty(), "{:?}", snapshot.io_errors);
        let registry = AlbumRegistry::from_snapshot(&vault, &snapshot);
        (temp, vault, snapshot, registry)
    }

    #[test]
    fn schema_change_composes_with_index_and_guards_record_inputs() {
        let (_temp, vault, snapshot, registry) = setup();
        let record_type =
            registry.albums[0].declaration.record_type.as_ref().unwrap();
        let expect = schema_fingerprint(
            record_type,
            registry.albums[0].declaration.plural.as_deref(),
        );
        let request = format!(
            r#"{{"schema_version":1,"expect":"{expect}","ops":[{{"op":"choice.set","field":"status","value":"ready","set":{{"label":"Available"}}}}]}}"#
        );
        let envelope =
            super::super::protocol::parse_envelope(&request).unwrap();
        let plan = plan_update(
            &vault,
            &snapshot,
            &registry,
            "eat",
            &envelope,
            NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        )
        .unwrap();
        assert!(plan.changed);
        assert!(plan.after.contains("Available"));
        assert!(plan.after.contains("[!records]"));
        plan.deps.validate(&vault).expect("fresh dependencies");
        fs::write(vault.join("eat/ninja.md"), "---\nstatus: other\n---\n")
            .unwrap();
        assert!(plan
            .deps
            .validate(&vault)
            .unwrap_err()
            .message
            .contains("frontmatter changed"));
    }

    #[test]
    fn empty_operations_do_not_repair_a_missing_index() {
        let (_temp, vault, snapshot, registry) = setup();
        let record_type =
            registry.albums[0].declaration.record_type.as_ref().unwrap();
        let expect = schema_fingerprint(
            record_type,
            registry.albums[0].declaration.plural.as_deref(),
        );
        let request =
            format!(r#"{{"schema_version":1,"expect":"{expect}","ops":[]}}"#);
        let envelope =
            super::super::protocol::parse_envelope(&request).unwrap();
        let plan = plan_update(
            &vault,
            &snapshot,
            &registry,
            "eat",
            &envelope,
            NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        )
        .unwrap();
        assert!(!plan.changed);
        assert!(plan.edit.is_none());
        assert_eq!(plan.after, SCHEMA);
    }

    #[test]
    fn incomplete_record_scan_refuses_before_index_planning() {
        let (_temp, vault, _snapshot, _registry) = setup();
        fs::write(vault.join("eat/bad.md"), [0xff, 0xfe]).unwrap();
        let snapshot = VaultSnapshot::load(&vault);
        let registry = AlbumRegistry::from_snapshot(&vault, &snapshot);
        let record_type =
            registry.albums[0].declaration.record_type.as_ref().unwrap();
        let expect = schema_fingerprint(record_type, None);
        let request =
            format!(r#"{{"schema_version":1,"expect":"{expect}","ops":[]}}"#);
        let envelope =
            super::super::protocol::parse_envelope(&request).unwrap();
        let error = plan_update(
            &vault,
            &snapshot,
            &registry,
            "eat",
            &envelope,
            NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.code, "discovery_incomplete");
    }
}

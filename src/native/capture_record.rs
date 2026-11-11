//! Native adapter between capture parsing and the records engine.
//!
//! Parse and execution build one immutable route policy through
//! [`record_policy_for_draft`]. Parse resolves claimed intents through the
//! existing planner and one invocation-local folder catalog; ordinary task
//! drafts avoid both config loading and record-index reads.

use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

use super::records::context::{RecordContext, RecordKindResolution};
use super::{
    albums::index::IndexState,
    capture_language::{
        candidate_collection_routes, candidate_dirs, AlbumRouteMeta,
        DiagnosticRewrite, PerRouteDecision, RecordRoutePolicy,
    },
};
use super::{
    capture_language::record::RecordNoteIntent,
    config,
    records::{
        self, FieldInputState, PlannedRecordProperty, PlannedUpdateProperty,
        RecordCatalog, RecordDiagnostic, RecordFieldInput, RecordNoteInput,
        RecordPlanError, RecordPlanErrorCode, RecordStemSource, RecordType,
        ResolvedUpdateTarget,
    },
};

/// Native state shared by every record item in one capture invocation.
/// The immutable route policy owns the already-loaded schema; the catalog
/// is constructed on first record execution and indexes each folder lazily.
pub(crate) struct CaptureRecordContext {
    policy: RecordRoutePolicy,
    vault_root: PathBuf,
    catalog: Option<RecordCatalog>,
    album_edits: Vec<AlbumIndexEdit>,
}

/// One staged album records-index edit reported for batch output.
/// Repeated items re-render the same album; the latest postimage wins.
#[derive(Debug, Clone)]
pub(crate) struct AlbumIndexEdit {
    pub(crate) relative: PathBuf,
    pub(crate) state: IndexState,
    pub(crate) before: String,
    pub(crate) after: String,
}

impl CaptureRecordContext {
    pub(crate) fn new(policy: RecordRoutePolicy, vault_root: &Path) -> Self {
        Self {
            policy,
            vault_root: vault_root.to_path_buf(),
            catalog: None,
            album_edits: Vec::new(),
        }
    }

    pub(crate) fn policy(&self) -> &RecordRoutePolicy {
        &self.policy
    }

    pub(crate) fn catalog_mut(&mut self) -> &mut RecordCatalog {
        let vault_root = self.vault_root.clone();
        self.catalog
            .get_or_insert_with(|| RecordCatalog::new(&vault_root))
    }

    /// Usable record schema for one route: album-valid and config-valid
    /// routes resolve here; every other explicit per-route outcome returns
    /// `None` so callers refuse instead of planning against a fallback. Only
    /// the legacy `None` path (no per-route decision) consults the
    /// transitional config map.
    pub(crate) fn route_kind_for(
        &self,
        dir: &str,
    ) -> Option<(RecordType, &'static str, Option<AlbumRouteMeta>)> {
        match self.policy.route_decision(dir) {
            Some(PerRouteDecision::AlbumValid { record_type, album }) => {
                Some((record_type.clone(), "album", Some(album.clone())))
            }
            Some(PerRouteDecision::ConfigValid { record_type }) => {
                Some((record_type.clone(), "config", None))
            }
            Some(
                PerRouteDecision::AlbumInvalid { .. }
                | PerRouteDecision::Unclaimed { .. }
                | PerRouteDecision::DiscoveryIncomplete { .. },
            ) => None,
            None => self
                .policy
                .records_config()
                .and_then(|config| config.by_directory(dir))
                .map(|record_type| (record_type.clone(), "config", None)),
        }
    }

    /// Additive per-route warnings (for example `records_config_deprecated`)
    /// resolved alongside the route.
    pub(crate) fn route_warnings_for(
        &self,
        dir: &str,
    ) -> &[crate::native::albums::AlbumFinding] {
        self.policy.route_warnings(dir)
    }

    pub(crate) fn config_and_catalog(
        &mut self,
    ) -> Option<(&super::records::RecordsConfig, &mut RecordCatalog)> {
        let config = self.policy.records_config()?;
        let catalog = self
            .catalog
            .get_or_insert_with(|| RecordCatalog::new(&self.vault_root));
        Some((config, catalog))
    }

    #[cfg(test)]
    pub(crate) fn catalog_is_loaded(&self, directory: &str) -> bool {
        self.catalog
            .as_ref()
            .is_some_and(|catalog| catalog.is_loaded(directory))
    }

    #[cfg(test)]
    pub(crate) fn catalog_files_opened(&self) -> usize {
        self.catalog.as_ref().map_or(0, RecordCatalog::files_opened)
    }

    #[cfg(test)]
    pub(crate) fn catalog_stats(&self) -> (Vec<String>, usize) {
        self.catalog.as_ref().map_or((Vec::new(), 0), |catalog| {
            (catalog.loaded_directories(), catalog.files_opened())
        })
    }

    /// Report one staged album edit, replacing any earlier postimage for
    /// the same album so the batch carries exactly one edit per album.
    pub(crate) fn record_album_edit(&mut self, edit: AlbumIndexEdit) {
        if let Some(existing) = self
            .album_edits
            .iter_mut()
            .find(|existing| existing.relative == edit.relative)
        {
            *existing = edit;
        } else {
            self.album_edits.push(edit);
        }
    }

    pub(crate) fn take_album_edits(&mut self) -> Vec<AlbumIndexEdit> {
        std::mem::take(&mut self.album_edits)
    }
}

/// A capture item's record metadata after the existing planner has resolved
/// its schema fields, defaults, stem, and destination. Album-backed items
/// carry their source and album presentation so parse, execution, item
/// JSON, note_files, previews, and human success lines agree.
#[derive(Debug, Clone)]
pub(crate) struct RecordParseEntry {
    pub(crate) index: usize,
    pub(crate) intent: RecordNoteIntent,
    pub(crate) action: RecordParseAction,
    pub(crate) record_type: RecordType,
    pub(crate) source: &'static str,
    pub(crate) album: Option<AlbumRouteMeta>,
    pub(crate) title: Option<String>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) stem: Option<String>,
    pub(crate) stem_source: Option<RecordStemSource>,
    pub(crate) properties: Vec<PlannedRecordProperty>,
    pub(crate) update_properties: Vec<PlannedUpdateProperty>,
    pub(crate) missing: Vec<String>,
    pub(crate) invalid: Vec<RecordDiagnostic>,
    pub(crate) warnings: Vec<RecordDiagnostic>,
    pub(crate) plan_issue: Option<RecordParseIssue>,
    pub(crate) near_values: Vec<records::NearValueHint>,
    /// Exact existing or staged target for a safe `record_exists` rewrite.
    pub(crate) duplicate_target_stem: Option<String>,
}

/// Build the item-scoped create-to-update fix shared by `record_exists` and
/// the similar-record picker. The splice covers only the create route and
/// title on the parent line, leaving bullets and surrounding draft bytes
/// untouched. An explicit-new route is a create too, so it is rewritten in
/// exactly the same way when its target is known.
pub(crate) fn create_to_update_rewrite(
    input: &str,
    intent: &RecordNoteIntent,
    target_dir: &str,
    target_stem: &str,
) -> Option<DiagnosticRewrite> {
    if intent.action != super::capture_language::record::RecordAction::Create
        || !intent.dir.eq_ignore_ascii_case(target_dir)
        || !super::capture_language::is_addressable_update_stem(target_stem)
    {
        return None;
    }
    let title = intent.title_range?;
    let (start, end) = (
        intent.route_range.0.min(title.0),
        intent.route_range.1.max(title.1),
    );
    if start > end
        || end > input.len()
        || !input.is_char_boundary(start)
        || !input.is_char_boundary(end)
        || input[start..end].contains('\r')
        || input[start..end].contains('\n')
    {
        return None;
    }
    Some(DiagnosticRewrite {
        start,
        end,
        replacement: format!("@{target_dir}/{target_stem}"),
        label: "Add to it".to_owned(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordParseAction {
    Create,
    Update,
}

#[derive(Debug, Clone)]
pub(crate) struct RecordParseIssue {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) range: Option<Range<usize>>,
}

/// Build the record route policy for one draft: lexical candidates
/// first, then config load, then `is_dir` stats for candidates only.
/// No candidates means no config or filesystem reads at all, so
/// ordinary task drafts pay nothing for record support.
pub(crate) fn record_policy_for_draft(
    vault_root: &Path,
    config_path: Option<&Path>,
    raw_text: &str,
) -> RecordRoutePolicy {
    let folder_candidates = candidate_dirs(raw_text);
    let mut candidates = folder_candidates.clone();
    for candidate in candidate_collection_routes(raw_text) {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    if candidates.is_empty() {
        return RecordRoutePolicy::absent().with_vault_root(vault_root);
    }
    // Effective config path: the caller's explicit path wins, else the
    // established config location. Per-route album resolution reads at most
    // one album head plus its owner chain; full discovery runs only to
    // prove a matching config kind is unclaimed.
    let path = config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(config::config_path);
    // Legacy config maps stay for transition callers built without
    // per-route outcomes (tests, older adapters). Per-route decisions win
    // wherever present.
    let legacy = match config::records::load_records_config_from(&path) {
        Ok(records) => RecordRoutePolicy::from_config(&records, Some(&path)),
        Err(diagnostic) => {
            RecordRoutePolicy::invalid(&path, diagnostic.message)
        }
    };
    let mut merged = legacy.with_vault_root(vault_root);
    for candidate in &candidates {
        let resolution = RecordContext::resolve_route_head(
            vault_root, &path, candidate, None,
        );
        let (decision, warnings) = match resolution {
            Ok(RecordKindResolution::Valid { kind, warnings, .. }) => {
                if kind.source_name() == "album" {
                    match kind.album.clone() {
                        Some(identity) => (
                            PerRouteDecision::AlbumValid {
                                record_type: kind.record_type().clone(),
                                album: AlbumRouteMeta {
                                    stem: identity.stem,
                                    title: identity.title,
                                    owner: identity.owner,
                                    breadcrumb: identity.breadcrumb,
                                },
                            },
                            warnings,
                        ),
                        None => (
                            PerRouteDecision::Unclaimed {
                                config_diagnostic: None,
                            },
                            Vec::new(),
                        ),
                    }
                } else {
                    (
                        PerRouteDecision::ConfigValid {
                            record_type: kind.record_type().clone(),
                        },
                        warnings,
                    )
                }
            }
            Ok(RecordKindResolution::Invalid {
                paths, findings, ..
            }) => (
                PerRouteDecision::AlbumInvalid { paths, findings },
                Vec::new(),
            ),
            Ok(RecordKindResolution::Unclaimed {
                config_diagnostic, ..
            }) => (
                PerRouteDecision::Unclaimed { config_diagnostic },
                Vec::new(),
            ),
            Ok(RecordKindResolution::DiscoveryIncomplete { errors }) => (
                PerRouteDecision::DiscoveryIncomplete {
                    errors: errors.clone(),
                },
                Vec::new(),
            ),
            Err(error) => (
                PerRouteDecision::DiscoveryIncomplete {
                    errors: vec![format!(
                        "could not read album head for `{candidate}`: {error}"
                    )],
                },
                Vec::new(),
            ),
        };
        merged = merged
            .insert_route(candidate.clone(), decision)
            .insert_route_warnings(candidate.clone(), warnings);
    }
    let mut existing = HashSet::new();
    for dir in &folder_candidates {
        if merged.configured_dir(dir).is_some()
            || merged.is_usable_record_route(dir)
        {
            continue;
        }
        if vault_root.join(dir).is_dir() {
            existing.insert(dir.clone());
        }
    }
    merged.with_existing(existing)
}

/// Resolve claimed record items through the records engine. A single lazy
/// folder catalog is shared across this parse invocation, and earlier
/// planned paths reserve their stems for later items in the same draft.
/// Ordinary task drafts return before creating or reading a catalog.
pub(crate) fn resolve_record_intents(
    intents: &[(usize, RecordNoteIntent)],
    policy: Option<&RecordRoutePolicy>,
) -> Vec<RecordParseEntry> {
    if intents.is_empty() {
        return Vec::new();
    }
    let Some(policy) = policy else {
        return Vec::new();
    };
    let Some(vault_root) = policy.vault_root() else {
        return Vec::new();
    };

    let mut catalog = RecordCatalog::new(vault_root);
    resolve_record_intents_with_catalog(intents, Some(policy), &mut catalog)
}

/// Resolve record intents against a caller-owned catalog so a parse or
/// capture invocation can share the same lazy folder indexes.
pub(crate) fn resolve_record_intents_with_catalog(
    intents: &[(usize, RecordNoteIntent)],
    policy: Option<&RecordRoutePolicy>,
    catalog: &mut RecordCatalog,
) -> Vec<RecordParseEntry> {
    if intents.is_empty() {
        return Vec::new();
    }
    let Some(policy) = policy else {
        return Vec::new();
    };
    let legacy_config = policy.records_config();
    if legacy_config.is_none() && !policy.has_route_decisions() {
        return Vec::new();
    }
    let Some(vault_root) = policy.vault_root() else {
        return Vec::new();
    };

    let now = super::env::current_datetime();
    let timezone = super::day_timezone().ok().flatten();
    let today = now.date();
    let mut reserved_paths = Vec::new();
    let mut overlay = HashMap::new();
    let mut entries = Vec::with_capacity(intents.len());

    for (index, intent) in intents {
        // Per-route album/config outcomes win. Policies without them
        // (legacy fixtures) fall back to the config directory map.
        let (record_type_owned, source, album, render_identity): (
            RecordType,
            &'static str,
            Option<AlbumRouteMeta>,
            super::records::RecordRenderIdentity,
        ) = match policy.route_decision(&intent.dir) {
            Some(PerRouteDecision::AlbumValid { record_type, album }) => (
                record_type.clone(),
                "album",
                Some(album.clone()),
                super::records::RecordRenderIdentity::Album,
            ),
            Some(PerRouteDecision::ConfigValid { record_type }) => (
                record_type.clone(),
                "config",
                None,
                super::records::RecordRenderIdentity::Config,
            ),
            Some(PerRouteDecision::AlbumInvalid { .. })
            | Some(PerRouteDecision::Unclaimed { .. })
            | Some(PerRouteDecision::DiscoveryIncomplete { .. }) => {
                // The grammar already refused these routes; no intent
                // should exist. Skip rather than planning against a
                // fallback the album owns entirely.
                continue;
            }
            None => {
                let Some(config) = legacy_config else {
                    continue;
                };
                let Some(record_type) = config.by_directory(&intent.dir) else {
                    continue;
                };
                (
                    record_type.clone(),
                    "config",
                    None,
                    super::records::RecordRenderIdentity::Config,
                )
            }
        };
        let record_type = &record_type_owned;
        // Album kinds are not in the transitional config. Plan them
        // against a single-type synthetic config so the existing
        // planner, catalog, overlay, and collision logic stay unchanged.
        let synthetic;
        let config: &records::RecordsConfig = if source == "album" {
            synthetic = records::RecordsConfig {
                types: vec![record_type.clone()],
            };
            &synthetic
        } else {
            legacy_config.expect("config route needs legacy config")
        };
        let field_inputs = field_inputs_from_intent(intent);
        let notes = record_note_inputs(intent);
        let near_values =
            collect_near_value_hints(catalog, record_type, intent);
        match intent.action {
            super::capture_language::record::RecordAction::Create => {
                let request = records::RecordCreateRequest {
                    config,
                    record_type: &record_type.id,
                    title: &intent.title,
                    explicit_stem: intent.explicit_stem.as_deref(),
                    fields: &field_inputs,
                    notes: &notes,
                    now,
                    timezone,
                    vault_root,
                    reserved_paths: &reserved_paths,
                };
                match records::plan_create_with_identity(
                    request,
                    catalog,
                    render_identity,
                ) {
                    Ok(plan) => {
                        reserved_paths.push(plan.path.clone());
                        if let Some(markdown) = plan.markdown.as_ref() {
                            overlay.insert(
                                overlay_key(&intent.dir, &plan.stem),
                                OverlaySnapshot {
                                    path: plan.path.clone(),
                                    stem: plan.stem.clone(),
                                    title: plan.title.clone(),
                                    markdown: markdown.clone(),
                                },
                            );
                        }
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Create,
                            record_type: record_type.clone(),
                            source,
                            album: album.clone(),
                            title: Some(plan.title.clone()),
                            path: Some(plan.path),
                            stem: Some(plan.stem),
                            stem_source: Some(plan.stem_source),
                            properties: plan.properties,
                            update_properties: Vec::new(),
                            missing: plan.missing,
                            invalid: plan.invalid,
                            warnings: Vec::new(),
                            plan_issue: None,
                            near_values,
                            duplicate_target_stem: None,
                        });
                    }
                    Err(error) => {
                        let field_plan = records::plan_fields(
                            record_type,
                            &field_inputs,
                            today,
                        );
                        let (stem, stem_source, path) =
                            resolved_destination(intent, record_type);
                        let duplicate_target_stem = safe_duplicate_target_stem(
                            config,
                            catalog,
                            vault_root,
                            record_type,
                            stem.as_deref(),
                            &error,
                            &overlay,
                        );
                        if error.code != RecordPlanErrorCode::PathCollision
                            && let Some(path) = path.as_ref()
                        {
                            reserved_paths.push(path.clone());
                        }
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Create,
                            record_type: record_type.clone(),
                            source,
                            album: album.clone(),
                            title: Some(intent.title.clone()),
                            path,
                            stem,
                            stem_source,
                            properties: field_plan.properties,
                            update_properties: Vec::new(),
                            missing: field_plan.missing,
                            invalid: field_plan.invalid,
                            warnings: Vec::new(),
                            plan_issue: Some(plan_issue(
                                error,
                                intent,
                                record_type,
                            )),
                            near_values,
                            duplicate_target_stem,
                        });
                    }
                }
            }
            super::capture_language::record::RecordAction::Update => {
                let stem = intent.update_stem.as_deref().unwrap_or_default();
                let resolved = match resolve_update_snapshot(
                    config,
                    catalog,
                    record_type,
                    vault_root,
                    &overlay,
                    stem,
                ) {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Update,
                            record_type: record_type.clone(),
                            source,
                            album: album.clone(),
                            title: error.title,
                            path: error.path,
                            stem: Some(
                                error.stem.unwrap_or_else(|| stem.to_owned()),
                            ),
                            stem_source: Some(RecordStemSource::Existing),
                            properties: Vec::new(),
                            update_properties: Vec::new(),
                            missing: Vec::new(),
                            invalid: Vec::new(),
                            warnings: Vec::new(),
                            plan_issue: Some(RecordParseIssue {
                                code: error.code,
                                message: error.message,
                                range: intent
                                    .stem_range
                                    .map(|(start, end)| start..end),
                            }),
                            near_values,
                            duplicate_target_stem: None,
                        });
                        continue;
                    }
                };
                match records::plan_update(records::PlanUpdateRequest {
                    target: &resolved.target,
                    original: &resolved.original,
                    fields: &field_inputs,
                    notes: &notes,
                    today,
                }) {
                    Ok(plan) => {
                        if let Some(markdown) = plan.markdown.as_ref() {
                            overlay.insert(
                                overlay_key(&intent.dir, &plan.stem),
                                OverlaySnapshot {
                                    path: plan.path.clone(),
                                    stem: plan.stem.clone(),
                                    title: plan.title.clone(),
                                    markdown: markdown.clone(),
                                },
                            );
                        }
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Update,
                            record_type: record_type.clone(),
                            source,
                            album: album.clone(),
                            title: Some(plan.title.clone()),
                            path: Some(plan.path),
                            stem: Some(plan.stem),
                            stem_source: Some(RecordStemSource::Existing),
                            properties: Vec::new(),
                            update_properties: plan.properties,
                            missing: plan.missing,
                            invalid: plan.invalid,
                            warnings: plan.warnings,
                            plan_issue: None,
                            near_values,
                            duplicate_target_stem: None,
                        });
                    }
                    Err(error) => {
                        let range = error
                            .field
                            .as_deref()
                            .and_then(|key| {
                                intent
                                    .fields
                                    .iter()
                                    .find(|field| field.key == key)
                                    .map(|field| field.value_range)
                            })
                            .or(intent.stem_range);
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Update,
                            record_type: record_type.clone(),
                            source,
                            album: album.clone(),
                            title: Some(resolved.target.title),
                            path: Some(resolved.target.path),
                            stem: Some(resolved.target.stem),
                            stem_source: Some(RecordStemSource::Existing),
                            properties: Vec::new(),
                            update_properties: Vec::new(),
                            missing: Vec::new(),
                            invalid: Vec::new(),
                            warnings: Vec::new(),
                            plan_issue: Some(RecordParseIssue {
                                code: "record_invalid_value",
                                message: error.to_string(),
                                range: range.map(|(start, end)| start..end),
                            }),
                            near_values,
                            duplicate_target_stem: None,
                        });
                    }
                }
            }
        }
    }
    entries
}

#[derive(Clone)]
struct OverlaySnapshot {
    path: PathBuf,
    stem: String,
    title: String,
    markdown: String,
}

struct OverlayResolveError {
    code: &'static str,
    message: String,
    title: Option<String>,
    path: Option<PathBuf>,
    stem: Option<String>,
}

struct OverlayResolved {
    target: ResolvedUpdateTarget,
    original: String,
}

fn overlay_key(dir: &str, stem: &str) -> String {
    format!("{}/{}", dir.to_lowercase(), stem.to_lowercase())
}

fn safe_duplicate_target_stem(
    config: &records::RecordsConfig,
    catalog: &mut RecordCatalog,
    vault_root: &Path,
    record_type: &RecordType,
    stem: Option<&str>,
    error: &RecordPlanError,
    overlay: &HashMap<String, OverlaySnapshot>,
) -> Option<String> {
    if error.code != RecordPlanErrorCode::PathCollision {
        return None;
    }
    let [collision_path] = error.paths.as_slice() else {
        return None;
    };
    let stem = stem?;
    if let Some(snapshot) = overlay.get(&overlay_key(&record_type.dir, stem))
        && snapshot.path == *collision_path
    {
        let candidate = snapshot.stem.clone();
        if !super::capture_language::is_addressable_update_stem(&candidate) {
            return None;
        }
        // A staged target that shadows a symlinked note is not a safe
        // update: the real update would refuse the alias.
        if let Ok(meta) =
            std::fs::symlink_metadata(vault_root.join(collision_path))
            && meta.file_type().is_symlink()
        {
            return None;
        }
        return Some(candidate);
    }
    let candidate = match catalog.folder(&record_type.dir).lookup_stem(stem) {
        records::StemLookup::Exact(note)
            if note.relative_path == *collision_path =>
        {
            note.stem.clone()
        }
        records::StemLookup::Exact(_)
        | records::StemLookup::Ambiguous(_)
        | records::StemLookup::None => return None,
    };
    if !super::capture_language::is_addressable_update_stem(&candidate) {
        return None;
    }
    // Offer the rewrite only when a real update resolves to the same path,
    // so symlinked aliases and other unsafe targets have no rewrite.
    match records::resolve_update_target(
        config,
        catalog,
        &record_type.id,
        &candidate,
    ) {
        Ok(target) if target.path == *collision_path => Some(candidate),
        Ok(_) | Err(_) => None,
    }
}

fn resolve_update_snapshot(
    config: &records::RecordsConfig,
    catalog: &mut RecordCatalog,
    record_type: &RecordType,
    vault_root: &Path,
    overlay: &HashMap<String, OverlaySnapshot>,
    stem: &str,
) -> Result<OverlayResolved, OverlayResolveError> {
    if let Some(snapshot) = overlay.get(&overlay_key(&record_type.dir, stem)) {
        return Ok(OverlayResolved {
            target: ResolvedUpdateTarget {
                record_type: record_type.clone(),
                path: snapshot.path.clone(),
                stem: snapshot.stem.clone(),
                title: snapshot.title.clone(),
            },
            original: snapshot.markdown.clone(),
        });
    }
    let target = match records::resolve_update_target(
        config,
        catalog,
        &record_type.id,
        stem,
    ) {
        Ok(target) => target,
        Err(error) => {
            return Err(OverlayResolveError {
                code: error.code.diagnostic_code(),
                message: error.message,
                title: None,
                path: None,
                stem: Some(stem.to_owned()),
            });
        }
    };
    match std::fs::read_to_string(vault_root.join(&target.path)) {
        Ok(original) => Ok(OverlayResolved { target, original }),
        Err(error) => Err(OverlayResolveError {
            code: "record_invalid_value",
            message: format!(
                "cannot read existing record {}: {error}",
                target.path.display()
            ),
            title: Some(target.title),
            path: Some(target.path),
            stem: Some(target.stem),
        }),
    }
}

fn collect_near_value_hints(
    catalog: &mut RecordCatalog,
    record_type: &RecordType,
    intent: &RecordNoteIntent,
) -> Vec<records::NearValueHint> {
    let needs_index = record_type.fields.iter().any(|field| {
        field.suggest && matches!(field.kind, records::FieldKind::Text)
    });
    if !needs_index {
        return Vec::new();
    }
    let folder = catalog.folder(&intent.dir);
    let mut hints = Vec::new();
    for field in &intent.fields {
        let Some(schema) = record_type.field(&field.key) else {
            continue;
        };
        if !schema.suggest || !matches!(schema.kind, records::FieldKind::Text) {
            continue;
        }
        if field.value.is_empty() {
            continue;
        }
        if schema.list {
            for (member, range) in
                list_member_ranges(&field.value, field.value_range.0)
            {
                if let Some(hint) =
                    folder.near_value_hint(&field.key, &member, range)
                {
                    hints.push(hint);
                }
            }
        } else if let Some(hint) = folder.near_value_hint(
            &field.key,
            &field.value,
            field.value_range.0..field.value_range.1,
        ) {
            hints.push(hint);
        }
    }
    hints
}

fn list_member_ranges(
    value: &str,
    value_start: usize,
) -> Vec<(String, std::ops::Range<usize>)> {
    let Ok(members) = records::schema::split_list_members(value) else {
        return vec![(
            value.to_string(),
            value_start..value_start + value.len(),
        )];
    };
    let mut ranges = Vec::new();
    let mut search_from = 0;
    for member in members {
        let Some(relative) = value[search_from..].find(&member) else {
            continue;
        };
        let start = value_start + search_from + relative;
        ranges.push((member.clone(), start..start + member.len()));
        search_from += relative + member.len();
    }
    ranges
}

fn resolved_destination(
    intent: &RecordNoteIntent,
    record_type: &RecordType,
) -> (Option<String>, Option<RecordStemSource>, Option<PathBuf>) {
    let stem = if let Some(explicit) = intent.explicit_stem.as_deref() {
        records::stem::is_valid_explicit_stem(explicit)
            .then(|| explicit.to_owned())
    } else {
        records::stem::normalize_stem(&intent.title)
    };
    let source = stem.as_ref().map(|_| {
        if intent.explicit_stem.is_some() {
            RecordStemSource::Explicit
        } else {
            RecordStemSource::Title
        }
    });
    let path = stem
        .as_ref()
        .map(|stem| PathBuf::from(&record_type.dir).join(format!("{stem}.md")));
    (stem, source, path)
}

/// Convert parsed field bullets into engine inputs. Repeated list
/// bullets stay separate so `plan_create` can combine them.
pub(crate) fn field_inputs_from_intent(
    intent: &RecordNoteIntent,
) -> Vec<RecordFieldInput> {
    intent
        .fields
        .iter()
        .map(|field| RecordFieldInput {
            key: field.key.clone(),
            state: if field.value.is_empty() {
                FieldInputState::SuppliedEmpty
            } else {
                FieldInputState::SuppliedValues(vec![field.value.clone()])
            },
            source_range: Some(field.value_range.0..field.value_range.1),
        })
        .collect()
}

/// Convert grouped capture notes into engine note inputs. ALL-CAPS
/// sections become one section input with flattened children; ordinary
/// groups become one input per first-level line.
pub(crate) fn record_note_inputs(
    intent: &RecordNoteIntent,
) -> Vec<RecordNoteInput> {
    let mut inputs = Vec::new();
    if intent.action == super::capture_language::record::RecordAction::Update
        && !intent.title.is_empty()
    {
        inputs.push(RecordNoteInput {
            text: intent.title.clone(),
            children: Vec::new(),
        });
    }
    for group in &intent.notes {
        if let Some(section) = group.section.as_deref() {
            let mut children = Vec::new();
            for line in &group.lines {
                children.push(line.text.clone());
                children.extend(note_child_texts(&line.children));
            }
            inputs.push(RecordNoteInput::section(section, children));
        } else {
            for line in &group.lines {
                inputs.push(RecordNoteInput {
                    text: line.text.clone(),
                    children: note_child_texts(&line.children),
                });
            }
        }
    }
    inputs
}

/// Convert an engine value to its machine-readable JSON form. Parse output
/// and `capture` note_files share this so canonical values retain their
/// numeric, list, choice, date, and explicit-none representations.
pub(crate) fn record_value_json(
    value: &super::records::RecordValue,
) -> serde_json::Value {
    use super::records::{RecordDate, RecordScalar, RecordValue};

    fn scalar(value: &RecordScalar) -> serde_json::Value {
        match value {
            RecordScalar::Text(value) | RecordScalar::Choice(value) => {
                json!(value)
            }
            RecordScalar::Date(RecordDate::Resolved(date)) => {
                json!(date.format("%Y-%m-%d").to_string())
            }
            RecordScalar::Date(RecordDate::Today) => json!("today"),
            RecordScalar::Date(RecordDate::Yesterday) => json!("yesterday"),
            RecordScalar::Link(value) => json!(value),
            RecordScalar::Url(value) => json!(value.to_string()),
            RecordScalar::Number(value) => json!(value),
        }
    }

    match value {
        RecordValue::Scalar(value) => scalar(value),
        RecordValue::List(values) => {
            serde_json::Value::Array(values.iter().map(scalar).collect())
        }
    }
}

fn note_child_texts(
    lines: &[crate::native::capture_language::record::RecordNoteLine],
) -> Vec<String> {
    lines.iter().map(|line| line.text.clone()).collect()
}

/// Teaching copy for a path collision: name the existing file and offer
/// the working update spelling.
pub(crate) fn collision_message(
    error: &RecordPlanError,
    intent: &RecordNoteIntent,
    record_type: &RecordType,
) -> String {
    let path = error
        .paths
        .first()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "the requested path".to_string());
    let stem = error
        .paths
        .first()
        .and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .or(intent.explicit_stem.as_deref())
        .unwrap_or("<stem>");
    format!(
        "record already exists: {path}; add to it with `@{}/{stem}`",
        record_type.dir,
    )
}

fn plan_issue(
    error: RecordPlanError,
    intent: &RecordNoteIntent,
    record_type: &RecordType,
) -> RecordParseIssue {
    let code = error.code.diagnostic_code();
    let message = match error.code {
        RecordPlanErrorCode::PathCollision => {
            collision_message(&error, intent, record_type)
        }
        _ => error.message,
    };
    let range = match error.code {
        RecordPlanErrorCode::PathCollision
        | RecordPlanErrorCode::ExplicitStemRequired => intent.title_range,
        RecordPlanErrorCode::EmptyTitle
        | RecordPlanErrorCode::FolderUnavailable => Some(intent.route_range),
        RecordPlanErrorCode::InvalidExplicitStem => intent.stem_range,
        RecordPlanErrorCode::UnknownRecordType => intent.route_range.into(),
    };
    RecordParseIssue {
        code,
        message,
        range: range.map(|(start, end)| start..end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::records::{FieldInputState, RecordCreateRequest};
    use chrono::NaiveDate;

    const CONFIG: &str = r#"records:
  restaurant:
    dir: eat
    label: Restaurant
    fields:
      status:
        required: true
        values: {liked: Liked}
"#;

    #[test]
    fn invocation_context_reuses_schema_and_folder_index() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        std::fs::create_dir_all(root.join("eat")).expect("record folder");
        std::fs::write(
            root.join("eat/existing.md"),
            "---\ntitle: Existing\nstatus: liked\n---\n# Existing\n",
        )
        .expect("existing record");
        let config_path = root.join("config.yml");
        std::fs::write(&config_path, CONFIG).expect("config");
        let config = crate::native::records::schema::parse_records_config(
            CONFIG,
            &config_path,
        )
        .expect("valid config");
        let policy =
            RecordRoutePolicy::from_config(&config, Some(&config_path))
                .with_vault_root(root);
        let mut context = CaptureRecordContext::new(policy, root);
        assert_eq!(context.catalog_files_opened(), 0);
        assert!(!context.catalog_is_loaded("eat"));

        let fields = vec![RecordFieldInput {
            key: "status".to_string(),
            state: FieldInputState::SuppliedValues(vec!["liked".to_string()]),
            source_range: None,
        }];
        let now = NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(13, 40, 0)
            .unwrap();
        {
            let (config, catalog) = context.config_and_catalog().unwrap();
            let config_address = config as *const _;
            let catalog_address = catalog as *mut _;
            let first = records::plan_create(
                RecordCreateRequest {
                    config,
                    record_type: "restaurant",
                    title: "Ramen",
                    explicit_stem: None,
                    fields: &fields,
                    notes: &[],
                    now,
                    timezone: None,
                    vault_root: root,
                    reserved_paths: &[],
                },
                catalog,
            )
            .expect("first record plan");
            assert_eq!(first.path, PathBuf::from("eat/ramen.md"));
            assert_eq!(catalog.files_opened(), 1);
            assert!(catalog.is_loaded("eat"));

            let (same_config, same_catalog) =
                context.config_and_catalog().unwrap();
            assert_eq!(same_config as *const _, config_address);
            assert_eq!(same_catalog as *mut _, catalog_address);
        }
        {
            let (config, catalog) = context.config_and_catalog().unwrap();
            let second = records::plan_create(
                RecordCreateRequest {
                    config,
                    record_type: "restaurant",
                    title: "Udon",
                    explicit_stem: None,
                    fields: &fields,
                    notes: &[],
                    now,
                    timezone: None,
                    vault_root: root,
                    reserved_paths: &[],
                },
                catalog,
            )
            .expect("second record plan");
            assert_eq!(second.path, PathBuf::from("eat/udon.md"));
            assert_eq!(catalog.files_opened(), 1);
        }
        assert!(context.catalog_is_loaded("eat"));
        assert_eq!(context.catalog_files_opened(), 1);

        // An ordinary-task lexical preflight returns before config or index
        // reads, leaving its invocation context catalog completely lazy.
        let absent = record_policy_for_draft(
            root,
            Some(&root.join("missing-config.yml")),
            "Buy milk",
        );
        assert!(absent.records_config().is_none());
        let task_context = CaptureRecordContext::new(absent, root);
        assert!(!task_context.catalog_is_loaded("eat"));
        assert_eq!(task_context.catalog_files_opened(), 0);
    }

    fn album_vault(root: &std::path::Path) {
        std::fs::write(
            root.join("body.md"),
            "---\ntype: [[area]]\n---\n# Body\n",
        )
        .expect("area note");
        std::fs::write(
            root.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places\nrecord:\n  noun: restaurant\n  label: Restaurant\n  fields: {}\n---\n# Restaurants and Cafes\n",
        )
        .expect("album note");
        std::fs::create_dir_all(root.join("eat")).expect("record folder");
    }

    fn create_intent(dir: &str, title: &str) -> RecordNoteIntent {
        RecordNoteIntent {
            action:
                super::super::capture_language::record::RecordAction::Create,
            dir: dir.to_owned(),
            title: title.to_owned(),
            explicit_stem: None,
            update_stem: None,
            fields: Vec::new(),
            notes: Vec::new(),
            route_range: (0, 5),
            title_range: Some((6, 6 + title.len())),
            stem_range: None,
        }
    }

    #[test]
    fn album_create_carries_source_metadata_and_canonical_storage() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        album_vault(root);
        // No config file: albums resolve without transitional config.
        let policy = record_policy_for_draft(
            root,
            Some(&root.join("missing-config.yml")),
            "Ani Ramen House @eat/",
        );
        let entries = resolve_record_intents(
            &[(0, create_intent("eat", "Ani Ramen House"))],
            Some(&policy),
        );
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.source, "album");
        assert_eq!(entry.record_type.id, "restaurant");
        assert_eq!(entry.record_type.dir, "eat");
        let album = entry.album.as_ref().expect("album metadata");
        assert_eq!(album.stem, "eat");
        assert_eq!(album.title, "Restaurants and Cafes");
        assert_eq!(album.owner.as_deref(), Some("body"));
        assert!(album.breadcrumb.contains(&album.title));
        // Byte-exact storage spelling: album parent plus canonical type.
        let synthetic = records::RecordsConfig {
            types: vec![entry.record_type.clone()],
        };
        let mut catalog = RecordCatalog::new(root);
        let now = chrono::NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(13, 40, 0)
            .unwrap();
        let plan = records::plan_create_with_identity(
            records::RecordCreateRequest {
                config: &synthetic,
                record_type: "restaurant",
                title: "Ani Ramen House",
                explicit_stem: None,
                fields: &[],
                notes: &[],
                now,
                timezone: None,
                vault_root: root,
                reserved_paths: &[],
            },
            &mut catalog,
            records::RecordRenderIdentity::Album,
        )
        .expect("album plan");
        let markdown = plan.markdown.expect("complete plan renders");
        assert!(markdown.contains("parent: \"[[eat]]\""));
        assert!(markdown.contains("type: \"[[record]]\""));
        assert!(!markdown.contains("[[restaurant]]"));
        // Parse and execution share the same resolution: re-resolving with
        // a caller-owned catalog agrees byte for byte.
        let mut catalog = RecordCatalog::new(root);
        let again = resolve_record_intents_with_catalog(
            &[(0, create_intent("eat", "Ani Ramen House"))],
            Some(&policy),
            &mut catalog,
        );
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].source, "album");
        assert_eq!(again[0].path, entries[0].path);
        assert_eq!(again[0].stem, entries[0].stem);
    }

    #[test]
    fn broken_album_never_falls_back_to_config() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        std::fs::write(
            root.join("body.md"),
            "---\ntype: [[area]]\n---\n# Body\n",
        )
        .expect("area note");
        std::fs::write(
            root.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\nrecord: {noun: restaurant, fields: []}\n---\n# Restaurants\n",
        )
        .expect("broken album");
        std::fs::write(
            root.join("drink.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Drinks\nrecord:\n  noun: drink\n  fields: {}\n---\n# Drinks\n",
        )
        .expect("healthy album");
        std::fs::create_dir_all(root.join("drink")).expect("record folder");
        let config_path = root.join("config.yml");
        std::fs::write(
            &config_path,
            "records:\n  restaurant:\n    dir: eat\n    label: Restaurant\n    fields: {}\n",
        )
        .expect("config");
        let policy = record_policy_for_draft(
            root,
            Some(&config_path),
            "Ramen @eat/\n\nCola @drink/",
        );
        // The broken album owns its noun and folder: no config fallback.
        assert!(matches!(
            policy.route_decision("eat"),
            Some(PerRouteDecision::AlbumInvalid { .. })
        ));
        assert!(matches!(
            policy.route_decision("drink"),
            Some(PerRouteDecision::AlbumValid { .. })
        ));
        // The grammar already refused the broken route, so execution sees
        // no intent for it; the healthy album still plans.
        let entries = resolve_record_intents(
            &[
                (0, create_intent("eat", "Ramen")),
                (1, create_intent("drink", "Cola")),
            ],
            Some(&policy),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].intent.dir, "drink");
        assert_eq!(entries[0].source, "album");
    }

    #[test]
    fn staged_create_then_update_shares_one_batch_overlay() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        album_vault(root);
        let policy = record_policy_for_draft(
            root,
            Some(&root.join("missing-config.yml")),
            "Ramen Shop @eat/^ramen\n\n@eat/ramen",
        );
        let create = RecordNoteIntent {
            action:
                super::super::capture_language::record::RecordAction::Create,
            dir: "eat".to_owned(),
            title: "Ramen Shop".to_owned(),
            explicit_stem: Some("ramen".to_owned()),
            update_stem: None,
            fields: Vec::new(),
            notes: Vec::new(),
            route_range: (0, 5),
            title_range: Some((6, 16)),
            stem_range: None,
        };
        let update = RecordNoteIntent {
            action:
                super::super::capture_language::record::RecordAction::Update,
            dir: "eat".to_owned(),
            title: String::new(),
            explicit_stem: None,
            update_stem: Some("ramen".to_owned()),
            fields: Vec::new(),
            notes: Vec::new(),
            route_range: (0, 5),
            title_range: None,
            stem_range: Some((6, 11)),
        };
        let mut catalog = RecordCatalog::new(root);
        let entries = resolve_record_intents_with_catalog(
            &[(0, create), (1, update)],
            Some(&policy),
            &mut catalog,
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source, "album");
        assert_eq!(
            entries[0].path.as_ref().expect("create path"),
            &PathBuf::from("eat/ramen.md")
        );
        // The later update resolves against the staged create, so one
        // draft can add a record and then amend it atomically.
        assert!(matches!(entries[1].action, RecordParseAction::Update));
        assert_eq!(entries[1].source, "album");
        assert_eq!(
            entries[1].path.as_ref().expect("update path"),
            &PathBuf::from("eat/ramen.md")
        );
        assert_eq!(entries[1].stem.as_deref(), Some("ramen"));
    }

    #[test]
    fn ordinary_drafts_do_no_album_or_folder_reads() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        album_vault(root);
        let policy = record_policy_for_draft(root, None, "Buy milk");
        assert!(!policy.has_route_decisions());
        assert!(policy.route_decision("eat").is_none());
        let context = CaptureRecordContext::new(policy, root);
        assert!(!context.catalog_is_loaded("eat"));
    }

    #[test]
    fn suppressed_config_kind_never_resurrects_legacy_route() {
        // eat.md claims noun `restaurant`; transitional config declares the
        // same noun with dir `old_eat`. The registry suppresses that entire
        // config kind, so `Place @old_eat/` must not resurrect its schema.
        for broken in [false, true] {
            for with_legacy_folder in [false, true] {
                let temp = tempfile::tempdir().expect("temporary vault");
                let root = temp.path();
                std::fs::write(
                    root.join("body.md"),
                    "---\ntype: [[area]]\n---\n# Body\n",
                )
                .expect("area note");
                let album_frontmatter = if broken {
                    "---\ntype: [[album]]\nparent: \"[[body]]\"\nrecord: {noun: restaurant, fields: []}\n---\n"
                } else {
                    "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places\nrecord:\n  noun: restaurant\n  label: Restaurant\n  fields: {}\n---\n"
                };
                std::fs::write(
                    root.join("eat.md"),
                    format!("{album_frontmatter}# Eat\n"),
                )
                .expect("claiming album");
                if with_legacy_folder {
                    std::fs::create_dir_all(root.join("old_eat"))
                        .expect("legacy folder");
                }
                let config_path = root.join("config.yml");
                std::fs::write(
                    &config_path,
                    "records:\n  restaurant:\n    dir: old_eat\n    label: Restaurant\n    fields: {}\n",
                )
                .expect("transitional config");
                let policy = record_policy_for_draft(
                    root,
                    Some(&config_path),
                    "Place @old_eat/",
                );
                // A valid claim suppresses config: no usable schema for the
                // legacy dir. A broken claim also suppresses it entirely.
                assert!(
                    policy.route_decision("old_eat").is_none()
                        || matches!(
                            policy.route_decision("old_eat"),
                            Some(PerRouteDecision::Unclaimed { .. })
                        ),
                    "legacy route stays unclaimed (broken={broken}, folder={with_legacy_folder}): {:?}",
                    policy.route_decision("old_eat"),
                );
                assert!(
                    CaptureRecordContext::new(policy, root)
                        .route_kind_for("old_eat")
                        .is_none(),
                    "no fallback schema for suppressed kind (broken={broken})",
                );
            }
        }
        // Ordinary unknown routes keep their literal meaning (unclaimed,
        // never a fallback schema).
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        album_vault(root);
        let policy = record_policy_for_draft(
            root,
            Some(&root.join("missing-config.yml")),
            "Buy milk @nowhere/",
        );
        assert!(matches!(
            policy.route_decision("nowhere"),
            Some(PerRouteDecision::Unclaimed { .. }) | None
        ));
        assert!(CaptureRecordContext::new(policy, root)
            .route_kind_for("nowhere")
            .is_none());
    }

    #[test]
    fn bad_byte_album_head_is_explicit_discovery_incomplete() {
        // Deterministic bad-byte fixture (works as root, unlike
        // permission-only tests): invalid UTF-8 in the album head makes
        // route-local resolution return an I/O error, which the policy keeps
        // as explicit `DiscoveryIncomplete` instead of unclaimed fallback.
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        std::fs::write(
            root.join("body.md"),
            "---\ntype: [[area]]\n---\n# Body\n",
        )
        .expect("area note");
        std::fs::write(
            root.join("eat.md"),
            b"---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: \xff\xfe places\nrecord:\n  noun: restaurant\n  fields: {}\n---\n# Eat\n",
        )
        .expect("bad-byte album");
        std::fs::create_dir_all(root.join("eat")).expect("record folder");
        let policy = record_policy_for_draft(
            root,
            Some(&root.join("missing-config.yml")),
            "Ramen @eat/",
        );
        assert!(
            matches!(
                policy.route_decision("eat"),
                Some(PerRouteDecision::DiscoveryIncomplete { .. })
            ),
            "bad-byte head is incomplete, not unclaimed: {:?}",
            policy.route_decision("eat")
        );
        // No fallback schema and no planned intent for the incomplete route.
        assert!(CaptureRecordContext::new(policy, root)
            .route_kind_for("eat")
            .is_none());
        let entries = resolve_record_intents(
            &[(0, create_intent("eat", "Ramen"))],
            Some(&record_policy_for_draft(
                root,
                Some(&root.join("missing-config.yml")),
                "Ramen @eat/",
            )),
        );
        assert!(entries.is_empty(), "incomplete route plans nothing");
    }

    #[test]
    fn healthy_album_stays_usable_despite_unrelated_bad_file() {
        // A healthy directly resolved album remains usable even when an
        // unrelated file has bad bytes, as long as the bounded route does
        // not need global discovery.
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        album_vault(root);
        std::fs::create_dir_all(root.join("eat")).expect("record folder");
        std::fs::write(root.join("unrelated.md"), b"\xff\xfe bad\n")
            .expect("unrelated bad file");
        let policy = record_policy_for_draft(
            root,
            Some(&root.join("missing-config.yml")),
            "Ramen @eat/",
        );
        assert!(matches!(
            policy.route_decision("eat"),
            Some(PerRouteDecision::AlbumValid { .. })
        ));
        let entries = resolve_record_intents(
            &[(0, create_intent("eat", "Ramen"))],
            Some(&policy),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "album");
    }

    #[test]
    fn create_to_update_rewrite_is_item_scoped_for_both_route_positions() {
        let cases = [
            (
                "@eat/^ninja_sushi Ninja Sushi\n- keep Café\n\nSibling",
                "@eat/^ninja_sushi",
                "Ninja Sushi",
                "ninja_sushi",
                "@eat/ninja_sushi\n- keep Café\n\nSibling",
            ),
            (
                "Café Nagi @eat/^nagi-shop\n- keep this bullet\n\nNext item",
                "@eat/^nagi-shop",
                "Café Nagi",
                "nagi-shop",
                "@eat/nagi-shop\n- keep this bullet\n\nNext item",
            ),
        ];
        for (input, route, title, target_stem, expected) in cases {
            let route_start = input.find(route).expect("route");
            let title_start = input.find(title).expect("title");
            let intent = RecordNoteIntent {
                action:
                    super::super::capture_language::record::RecordAction::Create,
                dir: "eat".to_owned(),
                title: title.to_owned(),
                explicit_stem: Some("new-stem".to_owned()),
                update_stem: None,
                fields: Vec::new(),
                notes: Vec::new(),
                route_range: (route_start, route_start + route.len()),
                title_range: Some((title_start, title_start + title.len())),
                stem_range: None,
            };
            let rewrite =
                create_to_update_rewrite(input, &intent, "eat", target_stem)
                    .expect("target rewrite");
            assert_eq!(rewrite.label, "Add to it");
            let result = format!(
                "{}{}{}",
                &input[..rewrite.start],
                rewrite.replacement,
                &input[rewrite.end..]
            );
            assert_eq!(result, expected);
        }
    }
}

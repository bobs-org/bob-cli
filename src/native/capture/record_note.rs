//! Record-note planning: engine create and update plans staged through
//! the capture batch overlay.

use std::collections::HashSet;

use super::*;
use crate::native::albums::{parse_album_declaration, write as album_write};
use crate::native::capture_language::record::RecordNoteIntent;
use crate::native::capture_record::{
    collision_message, field_inputs_from_intent, record_note_inputs,
    record_value_json, CaptureRecordContext,
};
use crate::native::records::{
    self, FieldKind, PlanUpdateRequest, RecordCatalog, RecordCreatePlan,
    RecordCreateRequest, RecordDiagnostic, RecordPlanErrorCode,
    RecordPropertySource, RecordType, RecordUpdatePlan, ResolvedUpdateTarget,
};
use serde_json::{json, Map};

pub(super) fn plan_record_note_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    record_context: &mut CaptureRecordContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    match parsed_item.parsed.kind.clone() {
        CaptureKind::RecordNote { intent } => plan_record_create_item(
            request,
            intent,
            now,
            today,
            planner,
            record_context,
            warnings,
        ),
        CaptureKind::RecordUpdate { intent } => plan_record_update_item(
            request,
            intent,
            today,
            planner,
            record_context,
            warnings,
        ),
        _ => Err(CaptureError::io(
            "record-note capture invariant failed: wrong capture kind",
        )),
    }
}

fn plan_record_create_item(
    request: &CaptureRequest,
    intent: RecordNoteIntent,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    record_context: &mut CaptureRecordContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    let Some((record_type, source, album)) =
        record_context.route_kind_for(&intent.dir)
    else {
        return Err(CaptureError::usage(format!(
            "`{}/` is not an album record folder",
            intent.dir
        )));
    };
    for finding in record_context.route_warnings_for(&intent.dir) {
        warnings.push(CaptureWarning::Text(format!(
            "{}: {}",
            finding.code, finding.message
        )));
    }
    let render_identity = records::RecordRenderIdentity::Album;
    // The records planner consumes a single-type schema view supplied by the
    // album route.
    let synthetic;
    synthetic = records::RecordsConfig {
        types: vec![record_type.clone()],
    };
    let config = &synthetic;
    let catalog = record_context.catalog_mut();
    let field_inputs = field_inputs_from_intent(&intent);
    let notes = record_note_inputs(&intent);
    let reserved_paths = reserved_paths(planner, &request.bob_dir);
    let timezone = crate::native::day_timezone().ok().flatten();
    let plan = match records::plan_create_with_identity(
        RecordCreateRequest {
            config,
            record_type: &record_type.id,
            title: &intent.title,
            explicit_stem: intent.explicit_stem.as_deref(),
            fields: &field_inputs,
            notes: &notes,
            now,
            timezone,
            vault_root: &request.bob_dir,
            reserved_paths: &reserved_paths,
        },
        catalog,
        render_identity,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let message = match error.code {
                RecordPlanErrorCode::PathCollision => {
                    collision_message(&error, &intent, &record_type)
                }
                _ => error.message.clone(),
            };
            return Err(CaptureError::usage_code(
                message,
                error.code.diagnostic_code(),
            ));
        }
    };

    let Some(markdown) = plan.markdown.clone() else {
        return Err(incomplete_error(&intent, &record_type, &plan, catalog));
    };
    let target = request.bob_dir.join(&plan.path);
    planner.stage(&target, markdown.clone())?;
    if source == "album" {
        stage_album_index(request, planner, record_context, &record_type)?;
    }
    let relative = plan.path.display().to_string().replace('\\', "/");
    let properties = plan
        .properties
        .iter()
        .map(note_file_property)
        .collect::<Vec<_>>();
    let note_count = ordinary_note_count(&intent);
    let body = plan.body.clone().unwrap_or_default();
    let album_json = album.as_ref().map(|album| {
        crate::native::capture::output::RecordAlbumJson {
            stem: album.stem.clone(),
            title: album.title.clone(),
            owner: album.owner.clone(),
            breadcrumb: album.breadcrumb.clone(),
        }
    });
    let source_json = (source == "album").then_some(source);
    let note_file = NoteFileJson {
        path: relative.clone(),
        action: "create",
        record_type: plan.record_type.clone(),
        source: source_json,
        album: album_json.clone(),
        properties: properties.clone(),
        body,
        markdown: markdown.clone(),
        lines: Vec::new(),
    };
    let summary = RecordNoteSummary {
        action: "create",
        emoji: record_type.emoji.clone(),
        label: record_type.label.clone(),
        path: relative.clone(),
        notes: note_count,
        properties,
        source: source_json,
        album: album_json,
        markdown,
        note_file,
    };
    Ok(PlannedCaptureItem {
        result: record_item_result(
            request,
            &intent,
            &plan.title,
            &target,
            &relative,
            summary,
            today,
            "record_note",
            Placement::Created,
        ),
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
        project_move_landings: Vec::new(),
    })
}

fn plan_record_update_item(
    request: &CaptureRequest,
    intent: RecordNoteIntent,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    record_context: &mut CaptureRecordContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    let Some((record_type, source, album)) =
        record_context.route_kind_for(&intent.dir)
    else {
        return Err(CaptureError::usage(format!(
            "`{}/` is not an album record folder",
            intent.dir
        )));
    };
    for finding in record_context.route_warnings_for(&intent.dir) {
        warnings.push(CaptureWarning::Text(format!(
            "{}: {}",
            finding.code, finding.message
        )));
    }
    let synthetic = records::RecordsConfig {
        types: vec![record_type.clone()],
    };
    let config = &synthetic;
    let catalog = record_context.catalog_mut();
    let stem = intent.update_stem.as_deref().unwrap_or_default();
    let (target, original) = match staged_update_snapshot(
        planner,
        &request.bob_dir,
        &record_type,
        stem,
    ) {
        Some(snapshot) => snapshot,
        None => {
            let target = match records::resolve_update_target(
                config,
                catalog,
                &record_type.id,
                stem,
            ) {
                Ok(target) => target,
                Err(error) => {
                    return Err(CaptureError::usage_code(
                        error.message,
                        error.code.diagnostic_code(),
                    ));
                }
            };
            let absolute = request.bob_dir.join(&target.path);
            let original = planner.read_existing(&absolute)?;
            (target, original)
        }
    };
    let field_inputs = field_inputs_from_intent(&intent);
    let notes = record_note_inputs(&intent);
    let plan = match records::plan_update(PlanUpdateRequest {
        target: &target,
        original: &original,
        fields: &field_inputs,
        notes: &notes,
        today,
    }) {
        Ok(plan) => plan,
        Err(error) => {
            return Err(CaptureError::usage_code(
                error.to_string(),
                "record_invalid_value",
            ));
        }
    };
    let Some(markdown) = plan.markdown.clone() else {
        return Err(incomplete_update_error(
            &intent,
            &record_type,
            &plan,
            catalog,
        ));
    };
    for warning in &plan.warnings {
        warnings.push(CaptureWarning::Text(existing_record_warning(warning)));
    }
    let absolute = request.bob_dir.join(&plan.path);
    if markdown != original {
        planner.stage(&absolute, markdown.clone())?;
    } else {
        let _ = planner.ensure_loaded(&absolute)?;
    }
    if source == "album" {
        stage_album_index(request, planner, record_context, &record_type)?;
    }
    let relative = plan.path.display().to_string().replace('\\', "/");
    let properties = plan
        .properties
        .iter()
        .map(update_note_file_property)
        .collect::<Vec<_>>();
    let note_count =
        ordinary_note_count(&intent) + usize::from(!intent.title.is_empty());
    let body = plan.body.clone().unwrap_or_default();
    let lines = record_update_lines(&original, &markdown);
    let album_json = album.as_ref().map(|album| {
        crate::native::capture::output::RecordAlbumJson {
            stem: album.stem.clone(),
            title: album.title.clone(),
            owner: album.owner.clone(),
            breadcrumb: album.breadcrumb.clone(),
        }
    });
    let source_json = (source == "album").then_some(source);
    let note_file = NoteFileJson {
        path: relative.clone(),
        action: "update",
        record_type: plan.record_type.clone(),
        source: source_json,
        album: album_json.clone(),
        properties: properties.clone(),
        body,
        markdown: markdown.clone(),
        lines,
    };
    let summary = RecordNoteSummary {
        action: "update",
        emoji: record_type.emoji.clone(),
        label: record_type.label.clone(),
        path: relative.clone(),
        notes: note_count,
        properties,
        source: source_json,
        album: album_json,
        markdown,
        note_file,
    };
    Ok(PlannedCaptureItem {
        result: record_item_result(
            request,
            &intent,
            &plan.title,
            &absolute,
            &relative,
            summary,
            today,
            "record_update",
            Placement::Updated,
        ),
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
        project_move_landings: Vec::new(),
    })
}

fn staged_update_snapshot(
    planner: &CaptureBatchPlanner,
    vault: &Path,
    record_type: &RecordType,
    stem: &str,
) -> Option<(ResolvedUpdateTarget, String)> {
    let folded = stem.to_lowercase();
    let mut matches = Vec::new();
    for (path, contents) in planner.staged_snapshot() {
        let Some(contents) = contents else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(vault) else {
            continue;
        };
        let Some(parent) = relative.parent() else {
            continue;
        };
        if parent != Path::new(&record_type.dir) {
            continue;
        }
        if relative.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(file_stem) =
            relative.file_stem().and_then(|name| name.to_str())
        else {
            continue;
        };
        if file_stem.to_lowercase() != folded {
            continue;
        }
        matches.push((relative.to_path_buf(), file_stem.to_owned(), contents));
    }
    let (path, stem, markdown) = match matches.len() {
        1 => matches.pop()?,
        _ => return None,
    };
    Some((
        ResolvedUpdateTarget {
            record_type: record_type.clone(),
            path,
            stem,
            title: String::new(),
        },
        markdown,
    ))
}

fn record_update_lines(original: &str, updated: &str) -> Vec<BlockLineJson> {
    let before: Vec<String> = original.lines().map(str::to_owned).collect();
    let after: Vec<String> = updated.lines().map(str::to_owned).collect();
    let before_refs: Vec<&str> = before.iter().map(String::as_str).collect();
    let after_refs: Vec<&str> = after.iter().map(String::as_str).collect();
    let before_depths = block_depths(&before_refs, 0, 0..before.len());
    let after_depths = block_depths(&after_refs, 0, 0..after.len());
    pair_lines(&before, &before_depths, &after, &after_depths, false)
}

fn existing_record_warning(warning: &RecordDiagnostic) -> String {
    let field = warning
        .field
        .as_deref()
        .map(|field| format!(" `{field}`"))
        .unwrap_or_default();
    format!(
        "existing record{field}: {}; review it with `bob records check`",
        warning.message
    )
}

/// Render the album's records index from the current batch overlay and
/// stage the single final album edit. Repeated items re-render from the
/// same in-memory postimages, so `into_staged_files` emits one album file
/// change per affected album. Every record route belongs to an album, so the
/// record write and index stay in the same transaction.
fn stage_album_index(
    request: &CaptureRequest,
    planner: &mut CaptureBatchPlanner,
    record_context: &mut CaptureRecordContext,
    record_type: &RecordType,
) -> Result<(), CaptureError> {
    let dir = record_type.dir.as_str();
    let album_relative = PathBuf::from(format!("{dir}.md"));
    let album_absolute = request.bob_dir.join(&album_relative);
    // Load through the planner so this exact preimage guards the commit
    // and survives repeated batch stages unchanged.
    let before = match planner.current_contents(&album_absolute)? {
        Some(contents) => contents,
        None => {
            return Err(stale_album_error(
                &album_relative,
                "was deleted on disk after planning",
            ));
        }
    };
    let declaration = match parse_album_declaration(&album_relative, &before) {
        Some(declaration) => declaration,
        None => {
            return Err(stale_album_error(
                &album_relative,
                "is no longer a readable album declaration",
            ));
        }
    };
    // Bind the schema to this same preimage: a declaration change after
    // route resolution refuses instead of rendering with a stale schema.
    if declaration.record_type.as_ref() != Some(record_type) {
        return Err(stale_album_error(
            &album_relative,
            "changed on disk after planning",
        ));
    }
    let overlays = record_overlays(planner, &request.bob_dir);
    let catalog = record_context.catalog_mut();
    let folder = catalog.folder(dir);
    let planned =
        album_write::plan_album_write(&declaration, &before, folder, &overlays);
    if planned.state == crate::native::albums::index::IndexState::Damaged {
        let reason = planned
            .findings
            .first()
            .map(|finding| finding.message.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}: records index is damaged; the records index was left unchanged",
                    album_relative.display()
                )
            });
        return Err(CaptureError::io(reason));
    }
    match planned.edit {
        Some(edit) => {
            debug_assert_eq!(edit.before, before);
            record_context.record_album_edit(
                crate::native::capture_record::AlbumIndexEdit {
                    relative: album_relative.clone(),
                    state: planned.state,
                    before: before.clone(),
                    after: edit.after.clone(),
                },
            );
            planner.stage(&album_absolute, edit.after)?;
        }
        None => planner.guard_merge_read(&album_absolute, None),
    }
    Ok(())
}

/// Staged record postimages keyed by vault-relative path for the album
/// overlay. The planner already holds every item staged so far, so a
/// create followed by an update renders the final fields exactly once.
fn record_overlays(
    planner: &CaptureBatchPlanner,
    vault: &Path,
) -> Vec<(PathBuf, String)> {
    planner
        .staged_snapshot()
        .into_iter()
        .filter_map(|(absolute, contents)| {
            let contents = contents?;
            let relative = absolute.strip_prefix(vault).ok()?.to_path_buf();
            Some((relative, contents))
        })
        .collect()
}

fn stale_album_error(relative: &Path, detail: &str) -> CaptureError {
    CaptureError::io(format!(
        "refusing to update records index in {}: album {detail}",
        relative.display(),
    ))
    .with_typed(CaptureFailureKind::NoteChanged)
}

fn reserved_paths(planner: &CaptureBatchPlanner, vault: &Path) -> Vec<PathBuf> {
    planner
        .staged_snapshot()
        .into_iter()
        .filter_map(|(path, contents)| {
            contents?;
            path.strip_prefix(vault)
                .ok()
                .map(|relative| relative.to_path_buf())
        })
        .collect()
}

fn ordinary_note_count(intent: &RecordNoteIntent) -> usize {
    intent
        .notes
        .iter()
        .filter(|group| group.section.is_none())
        .map(|group| group.lines.len())
        .sum()
}

fn note_file_property(
    property: &records::PlannedRecordProperty,
) -> NoteFilePropertyJson {
    NoteFilePropertyJson {
        key: property.key.clone(),
        label: property.label.clone(),
        value: property.value.as_ref().map(record_value_json),
        display: property.display.clone(),
        source: property.source.map(|source| match source {
            RecordPropertySource::Typed => "typed",
            RecordPropertySource::Default => "default",
        }),
        before: None,
        before_display: None,
    }
}

fn update_note_file_property(
    property: &records::PlannedUpdateProperty,
) -> NoteFilePropertyJson {
    NoteFilePropertyJson {
        key: property.key.clone(),
        label: property.label.clone(),
        value: property.value.as_ref().map(record_value_json),
        display: property.display.clone(),
        source: property.source.map(|_| "typed"),
        before: property.current.as_ref().map(record_value_json),
        before_display: property.current_display.clone(),
    }
}

fn incomplete_error(
    intent: &RecordNoteIntent,
    record_type: &RecordType,
    plan: &RecordCreatePlan,
    catalog: &mut RecordCatalog,
) -> CaptureError {
    incomplete_plan_error(
        intent,
        record_type,
        &plan.title,
        &plan.missing,
        &plan.invalid,
        catalog,
    )
}

fn incomplete_update_error(
    intent: &RecordNoteIntent,
    record_type: &RecordType,
    plan: &RecordUpdatePlan,
    catalog: &mut RecordCatalog,
) -> CaptureError {
    incomplete_plan_error(
        intent,
        record_type,
        &plan.title,
        &plan.missing,
        &plan.invalid,
        catalog,
    )
}

fn incomplete_plan_error(
    intent: &RecordNoteIntent,
    record_type: &RecordType,
    title: &str,
    missing: &[String],
    invalid: &[RecordDiagnostic],
    catalog: &mut RecordCatalog,
) -> CaptureError {
    let draft = incomplete_draft(intent, record_type, title, missing, catalog);
    let label = record_type.label.as_str();
    let message = if !missing.is_empty() {
        format!(
            "a {label} note needs {} more required field{}",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
        )
    } else {
        format!("the {label} note has invalid field values")
    };
    let mut extra = Map::new();
    extra.insert("draft".to_string(), json!(draft));
    extra.insert("missing".to_string(), json!(missing));
    extra.insert(
        "invalid".to_string(),
        json!(invalid.iter().map(invalid_json).collect::<Vec<_>>()),
    );
    let mut error = match invalid.first() {
        Some(diagnostic) => {
            CaptureError::usage_code(message, diagnostic.diagnostic_code())
        }
        None => CaptureError::usage(message),
    };
    error.extra = Some(extra);
    error
}

fn invalid_json(diagnostic: &RecordDiagnostic) -> serde_json::Value {
    json!({
        "field": diagnostic.field,
        "code": diagnostic.diagnostic_code(),
        "message": diagnostic.message,
    })
}

fn incomplete_draft(
    intent: &RecordNoteIntent,
    record_type: &RecordType,
    title: &str,
    missing: &[String],
    catalog: &mut RecordCatalog,
) -> String {
    let is_update = matches!(
        intent.action,
        crate::native::capture_language::record::RecordAction::Update
    );
    let mut draft = match intent.action {
        crate::native::capture_language::record::RecordAction::Update => {
            let stem = intent.update_stem.as_deref().unwrap_or_default();
            let route = format!("@{}/{}", record_type.dir, stem);
            if intent.title.is_empty() {
                route
            } else {
                format!("{} {route}", intent.title)
            }
        }
        crate::native::capture_language::record::RecordAction::Create => {
            if let Some(stem) = intent.explicit_stem.as_deref() {
                format!("{} @{}/^{stem}", title, record_type.dir)
            } else {
                format!("{} @{}/", title, record_type.dir)
            }
        }
    };
    let mut written = HashSet::new();
    for field in &intent.fields {
        written.insert(field.key.clone());
        let hint = record_type
            .field(&field.key)
            .map(field_hint)
            .unwrap_or_default();
        if field.value.is_empty() {
            draft.push_str(&format!("\n- {}:: …", field.key));
            if !hint.is_empty() {
                draft.push_str(&format!("    {hint}"));
            }
        } else {
            draft.push_str(&format!("\n- {}:: {}", field.key, field.value));
        }
    }
    for key in missing {
        if !written.insert(key.clone()) {
            continue;
        }
        let hint = record_type.field(key).map(field_hint).unwrap_or_default();
        draft.push_str(&format!("\n- {key}:: …"));
        if !hint.is_empty() {
            draft.push_str(&format!("    {hint}"));
        }
    }
    // On an update, an unsupplied optional `suggest` line would overwrite
    // stored data once filled in, so only supplied fields and missing keys
    // are offered.
    if !is_update {
        let folder = catalog.folder(&record_type.dir);
        for field in &record_type.fields {
            if field.required || written.contains(&field.key) || !field.suggest
            {
                continue;
            }
            let suggestions = folder.value_suggestions(&field.key);
            if suggestions.is_empty() {
                continue;
            }
            let hint = suggestions
                .iter()
                .map(|suggestion| suggestion.insert.as_str())
                .take(5)
                .collect::<Vec<_>>()
                .join(" · ");
            draft.push_str(&format!("\n- {}:: …    {hint}", field.key));
            written.insert(field.key.clone());
        }
    }
    for group in &intent.notes {
        if let Some(section) = &group.section {
            draft.push_str(&format!("\n- {section}"));
        }
        for line in &group.lines {
            draft.push_str(&format!("\n- {}", line.text));
            append_nested_notes(&mut draft, &line.children, 1);
        }
    }
    draft
}

fn append_nested_notes(
    draft: &mut String,
    lines: &[crate::native::capture_language::record::RecordNoteLine],
    depth: usize,
) {
    for line in lines {
        draft.push_str(&format!("\n{}- {}", "\t".repeat(depth), line.text));
        append_nested_notes(draft, &line.children, depth + 1);
    }
}

fn field_hint(field: &records::RecordField) -> String {
    match &field.kind {
        FieldKind::Choices(values) => values
            .iter()
            .map(|choice| choice.value.as_str())
            .collect::<Vec<_>>()
            .join(" · "),
        FieldKind::Date => "today · yesterday".to_string(),
        _ => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn record_item_result(
    request: &CaptureRequest,
    intent: &RecordNoteIntent,
    title: &str,
    target: &Path,
    relative: &str,
    summary: RecordNoteSummary,
    today: NaiveDate,
    kind: &'static str,
    placement: Placement,
) -> CaptureItemResult {
    CaptureItemResult {
        note_pull: None,
        record_note: Some(summary),
        notebook: None,
        task_theme: None,
        type_relocation: None,
        ok: true,
        dry_run: request.dry_run,
        routed: true,
        route: Some(intent.dir.clone()),
        route_label: relative.to_string(),
        relative_target: relative.to_string(),
        target: target.display().to_string(),
        text: title.to_owned(),
        task_line: String::new(),
        kind,
        fanout: None,
        created: super::plan::date_string(today),
        scheduled: None,
        priority: None,
        priority_label: None,
        placement,
        sub_bullets: Vec::new(),
        notes: Vec::new(),
        drawers: Vec::new(),
        clip: None,
        schedule_log: None,
        block_id: None,
        day_file: None,
        block_link: None,
        pomodoro_link_placement: None,
        parent_line: None,
        parent_text: None,
        drawer: None,
        parent_status_symbol: None,
        parent_status_name: None,
        toggle_direction: None,
        previous_task_line: None,
        status_symbol: None,
        status_name: None,
        previous_status_symbol: None,
        previous_status_name: None,
        pomodoro_name: None,
        creates_pomodoro: None,
        pomodoro_already_linked: None,
        removed_pomodoro_links: None,
        removed_scheduled: None,
        pomodoro_selector_unused: None,
        toggle_behavior: None,
        status_changed: None,
        pomodoro_link_action: None,
        pomodoro_link_source: None,
        pomodoro_link_destination: None,
        project_note: None,
        pomodoro_start: None,
        pomodoro_adjust: None,
        pomodoro_shift: None,
        pomodoro_split: None,
        pomodoro_merge: None,
        pomodoro_close: None,
        pomodoro_past: None,
        pomodoro_reset: None,
        dependency_update: None,
        consumed_links: Vec::new(),
        task_complete: None,
        r#ref: None,
        gmail: Vec::new(),
        toggle_task_description: None,
        link_guard: None,
        dropped_dependencies: Vec::new(),
        blocks_line_updates: Vec::new(),
        minted_dependent_block_id: None,
        dependency_updates: Vec::new(),
        minted_blocker_block_id: None,
        relation_updates: Vec::new(),
        relation_cancels: Vec::new(),
        relation_completes: Vec::new(),
        relation_merge_output: Default::default(),
        transfer_move_output: Default::default(),
        linked_from_line_updates: Vec::new(),
        minted_relation_block_ids: Vec::new(),
        mirror_skipped: Vec::new(),
        list: None,
        relation_label_hides: Vec::new(),
        relation_successors: Vec::new(),
        relation_held: Vec::new(),
        relation_check: None,
        relation_groups: Vec::new(),
    }
}

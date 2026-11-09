//! Record-note planning: engine create plans staged as new files.

use std::collections::HashSet;

use super::*;
use crate::native::capture_language::record::RecordNoteIntent;
use crate::native::capture_record::{
    collision_message, field_inputs_from_intent, record_note_inputs,
    record_policy_for_draft,
};
use crate::native::records::{
    self, FieldKind, RecordCatalog, RecordCreatePlan, RecordCreateRequest,
    RecordDiagnostic, RecordPlanErrorCode, RecordPropertySource, RecordType,
};
use serde_json::{json, Map};

pub(super) fn plan_record_note_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    config_path: Option<&Path>,
) -> Result<PlannedCaptureItem, CaptureError> {
    let CaptureKind::RecordNote { intent } = parsed_item.parsed.kind.clone()
    else {
        return Err(CaptureError::io(
            "record-note capture invariant failed: wrong capture kind",
        ));
    };
    let policy = record_policy_for_draft(
        &request.bob_dir,
        config_path,
        &request.raw_text,
    );
    let Some(config) = policy.records_config() else {
        return Err(CaptureError::usage(
            "record notes need a valid records config",
        ));
    };
    let Some(record_type) = config.by_directory(&intent.dir).cloned() else {
        return Err(CaptureError::usage(format!(
            "`{}/` is not a configured record folder",
            intent.dir
        )));
    };
    let field_inputs = field_inputs_from_intent(&intent);
    let notes = record_note_inputs(&intent);
    let reserved_paths = reserved_paths(planner, &request.bob_dir);
    let timezone = crate::native::day_timezone().ok().flatten();
    let mut catalog = RecordCatalog::new(&request.bob_dir);
    let plan = match records::plan_create(
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
        &mut catalog,
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
        return Err(incomplete_error(
            &intent,
            &record_type,
            &plan,
            &mut catalog,
        ));
    };
    let target = request.bob_dir.join(&plan.path);
    planner.stage(&target, markdown.clone())?;
    let relative = plan.path.display().to_string().replace('\\', "/");
    let properties = plan
        .properties
        .iter()
        .map(note_file_property)
        .collect::<Vec<_>>();
    let note_count = ordinary_note_count(&intent);
    let body = plan.body.clone().unwrap_or_default();
    let note_file = NoteFileJson {
        path: relative.clone(),
        action: "create",
        record_type: plan.record_type.clone(),
        properties: properties.clone(),
        body,
        markdown: markdown.clone(),
    };
    let summary = RecordNoteSummary {
        emoji: record_type.emoji.clone(),
        label: record_type.label.clone(),
        path: relative.clone(),
        notes: note_count,
        properties,
        markdown,
        note_file,
    };
    Ok(PlannedCaptureItem {
        result: record_item_result(
            request, &intent, &plan, &target, &relative, summary, today,
        ),
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
        project_move_landings: Vec::new(),
    })
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
        value: property.display.clone(),
        display: property.display.clone(),
        source: property.source.map(|source| match source {
            RecordPropertySource::Typed => "typed",
            RecordPropertySource::Default => "default",
        }),
    }
}

fn incomplete_error(
    intent: &RecordNoteIntent,
    record_type: &RecordType,
    plan: &RecordCreatePlan,
    catalog: &mut RecordCatalog,
) -> CaptureError {
    let draft = incomplete_draft(intent, record_type, plan, catalog);
    let label = record_type.label.as_str();
    let message = if !plan.missing.is_empty() {
        format!(
            "a {label} note needs {} more required field{}",
            plan.missing.len(),
            if plan.missing.len() == 1 { "" } else { "s" },
        )
    } else {
        format!("the {label} note has invalid field values")
    };
    let mut extra = Map::new();
    extra.insert("draft".to_string(), json!(draft));
    extra.insert("missing".to_string(), json!(plan.missing));
    extra.insert(
        "invalid".to_string(),
        json!(plan.invalid.iter().map(invalid_json).collect::<Vec<_>>()),
    );
    let mut error = match plan.invalid.first() {
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
    plan: &RecordCreatePlan,
    catalog: &mut RecordCatalog,
) -> String {
    let mut draft = if let Some(stem) = intent.explicit_stem.as_deref() {
        format!("{} @{}/^{stem}", plan.title, record_type.dir)
    } else {
        format!("{} @{}/", plan.title, record_type.dir)
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
    for key in &plan.missing {
        if !written.insert(key.clone()) {
            continue;
        }
        let hint = record_type.field(key).map(field_hint).unwrap_or_default();
        draft.push_str(&format!("\n- {key}:: …"));
        if !hint.is_empty() {
            draft.push_str(&format!("    {hint}"));
        }
    }
    let folder = catalog.folder(&record_type.dir);
    for field in &record_type.fields {
        if field.required || written.contains(&field.key) || !field.suggest {
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
    plan: &RecordCreatePlan,
    target: &Path,
    relative: &str,
    summary: RecordNoteSummary,
    today: NaiveDate,
) -> CaptureItemResult {
    CaptureItemResult {
        note_pull: None,
        record_note: Some(summary),
        task_theme: None,
        type_relocation: None,
        ok: true,
        dry_run: request.dry_run,
        routed: true,
        route: Some(intent.dir.clone()),
        route_label: relative.to_string(),
        relative_target: relative.to_string(),
        target: target.display().to_string(),
        text: plan.title.clone(),
        task_line: String::new(),
        kind: "record_note",
        fanout: None,
        created: super::plan::date_string(today),
        scheduled: None,
        priority: None,
        priority_label: None,
        placement: Placement::Created,
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

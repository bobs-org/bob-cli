//! Capture `@parent^id@ %n` notebook creation, composed into the batch.

use super::output::NoteFileJson;
use super::*;
use crate::native::capture_language::notebook::{
    strip_trailing_caret_id, NotebookRouteInfo, NotebookRoutePolicy,
    NotebookSection,
};
use crate::native::capture_project_note::{
    format_project_section_title, is_project_section_title,
};
use crate::native::notebooks::{plan_create, CreateRefusal, CreateRequest};

pub(super) fn plan_notebook_create_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    policy: &mut NotebookRoutePolicy,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    let parsed = parsed_item.parsed;
    let CaptureKind::NotebookCreate {
        parent,
        notebook_id,
    } = parsed.kind.clone()
    else {
        return Err(CaptureError::io(
            "notebook-create capture invariant failed: wrong capture kind",
        ));
    };
    let created = plan::date_string(today);
    let seed = seed_body(&parsed.sub_bullets, &created);
    if crate::native::notebooks::rules::contains_checkbox_rows(&seed) {
        return Err(CaptureError::usage_code(
            capture_language::notebook_holds_no_tasks_error(
                &parent,
                policy.get(&parent).and_then(|info| info.owner.as_deref()),
            ),
            "notebook_holds_no_tasks",
        ));
    }
    let snapshot = planner.overlay_notebook_snapshot(&request.bob_dir)?;
    let plan = plan_create(
        &snapshot,
        &CreateRequest {
            parent: parent.clone(),
            id: notebook_id.clone(),
            summary: parsed.body.clone(),
            title: None,
            body: (!seed.trim().is_empty()).then_some(seed.clone()),
            now,
        },
    )
    .map_err(map_create_refusal)?;
    for warning in &plan.warnings {
        warnings.push(CaptureWarning::Text(warning.clone()));
    }

    let new_abs = request.bob_dir.join(&plan.path);
    planner.stage_snapshot_postimage(&new_abs, None, plan.contents.clone())?;
    for edit in &plan.edits {
        let abs = request.bob_dir.join(&edit.path);
        let before = planner
            .peek_text(&abs)
            .or_else(|| Some(edit.before.clone()));
        planner.stage_snapshot_postimage(&abs, before, edit.after.clone())?;
    }
    for (relative, expected) in plan.deps.iter() {
        planner.guard_merge_read(
            &request.bob_dir.join(relative),
            expected.map(str::to_string),
        );
    }

    let relative = plan.path.to_string_lossy().replace('\\', "/");
    let sections = seed_sections(&parsed.sub_bullets);
    let parent_info = policy.get(&parent);
    let owner = parent_info
        .and_then(|info| info.owner.clone())
        .or_else(|| Some(parent.clone()));
    let breadcrumb = match parent_info {
        Some(info) => {
            let mut crumb = info.breadcrumb.clone();
            crumb.push(plan.title.clone());
            crumb
        }
        None => vec![parent.clone(), plan.title.clone()],
    };
    policy.insert_staged(
        plan.stem.clone(),
        NotebookRouteInfo {
            title: plan.title.clone(),
            summary: Some(plan.summary.clone()),
            owner,
            breadcrumb: breadcrumb.clone(),
            sections,
        },
    );

    let note_file = NoteFileJson {
        path: relative.clone(),
        action: "create",
        record_type: String::new(),
        properties: Vec::new(),
        body: seed,
        markdown: plan.contents.clone(),
        lines: Vec::new(),
    };
    let summary = super::notebook_note::NotebookSummary {
        route: plan.stem.clone(),
        title: plan.title.clone(),
        breadcrumb,
        section: None,
        section_action: None,
        block_id: Some(notebook_id.clone()),
        parent_block_id: None,
        note_file: Some(note_file),
    };
    let mut result = super::notebook_note::notebook_item_result(
        request,
        &plan.stem,
        &relative,
        &new_abs,
        &parsed.body,
        &parsed.body,
        Vec::new(),
        None,
        summary,
        "notebook_create",
    );
    result.placement = Placement::Created;
    result.created = created;
    Ok(PlannedCaptureItem {
        result,
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
        project_move_landings: Vec::new(),
    })
}

pub(super) fn refresh_create_markdown(
    items: &mut [PlannedCaptureItem],
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
) {
    for item in items {
        let Some(notebook) = item.result.notebook.as_mut() else {
            continue;
        };
        let Some(note_file) = notebook.note_file.as_mut() else {
            continue;
        };
        let path = bob_dir.join(&note_file.path);
        if let Some(contents) = planner.peek_text(&path) {
            note_file.markdown = contents;
        }
    }
}

fn map_create_refusal(refusal: CreateRefusal) -> CaptureError {
    let message = if refusal.code == "notebook_summary_required" {
        "a notebook needs a one-line summary".to_string()
    } else {
        refusal.message
    };
    CaptureError::usage_code(message, refusal.code)
}

fn seed_body(sub_bullets: &[AuthoredSubBullet], created: &str) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < sub_bullets.len() {
        let bullet = &sub_bullets[index];
        let is_section = bullet.depth == capture_language::AuthoredDepth::First
            && is_project_section_title(&bullet.body)
            && sub_bullets.get(index + 1).is_some_and(|next| {
                next.depth == capture_language::AuthoredDepth::Nested
            });
        if is_section {
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(&format!(
                "## {}\n\n",
                format_project_section_title(&bullet.body)
            ));
            index += 1;
            while index < sub_bullets.len()
                && sub_bullets[index].depth
                    == capture_language::AuthoredDepth::Nested
            {
                out.push_str(&format_seed_note(
                    &sub_bullets[index].body,
                    created,
                    true,
                ));
                index += 1;
            }
            continue;
        }
        if bullet.depth == capture_language::AuthoredDepth::First {
            out.push_str(&format_seed_note(&bullet.body, created, true));
            index += 1;
            while index < sub_bullets.len()
                && sub_bullets[index].depth
                    == capture_language::AuthoredDepth::Nested
            {
                out.push_str(&format_seed_note(
                    &sub_bullets[index].body,
                    created,
                    false,
                ));
                index += 1;
            }
            continue;
        }
        out.push_str(&format_seed_note(&bullet.body, created, true));
        index += 1;
    }
    out
}

fn format_seed_note(body: &str, created: &str, stamp: bool) -> String {
    let (text, block_id) = strip_trailing_caret_id(body.trim());
    let mut line = if stamp {
        format!("- {text} [created::{created}]")
    } else {
        format!("\t- {text}")
    };
    if let Some(block_id) = block_id {
        plan::append_block_id(&mut line, &block_id);
    }
    line.push('\n');
    line
}

fn seed_sections(sub_bullets: &[AuthoredSubBullet]) -> Vec<NotebookSection> {
    let mut bullets = Vec::new();
    let mut index = 0;
    while index < sub_bullets.len() {
        let bullet = &sub_bullets[index];
        let has_children = sub_bullets.get(index + 1).is_some_and(|next| {
            next.depth == capture_language::AuthoredDepth::Nested
        });
        if bullet.depth == capture_language::AuthoredDepth::First {
            bullets.push((bullet.body.clone(), has_children));
        }
        index += 1;
    }
    crate::native::capture_language::notebook::notebook_create_seed_sections(
        bullets,
    )
}

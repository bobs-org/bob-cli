use std::collections::BTreeMap;
use std::path::Path;

use super::{
    candidates::{
        active_task_candidates, apply_relationship_picker,
        blocked_task_candidates, dependency_candidates, link_candidates,
        relation_candidates, relation_label_suggestions, route_candidates,
        section_candidates, task_candidates, task_complete_candidates,
        task_goto_candidates, task_link_candidates, task_parent_candidates,
        TaskSearch,
    },
    model::{
        Candidates, CaptureCompleteResult, CompleteError, CompletionOwner,
        CompletionRelation, DrawerTarget, OverrideCompletion, PickerDescriptor,
        PickerKind, PickerScope, Replacement, SCHEMA_VERSION,
    },
    pomodoros::{
        apply_bare_start_name_replacements, pomodoro_candidates,
        running_pomodoro_session,
    },
};
use crate::native::{
    capture, capture_active_tasks, capture_block_ids, capture_dependency_tasks,
    capture_language::{
        self, CompletionContext, DependencyTarget, DependencyTargetKind,
        RelationDirection,
    },
    capture_links, capture_task_wikilinks, pomodoro,
};

/// `true` when `cursor` sits in Work Log text: a bullet line or an inline
/// entry on the close line. Block-link completion is suppressed there
/// because entries reject block links, while note and heading completion
/// keeps working.
pub(super) fn cursor_in_close_log_text(raw_text: &str, cursor: usize) -> bool {
    capture_language::cursor_in_close_log_text(raw_text, cursor)
}

pub(super) struct CompleteRequest<'a> {
    pub(super) bob_dir: &'a Path,
    pub(super) raw_text: &'a str,
    pub(super) cursor: usize,
    pub(super) all_tasks: bool,
    pub(super) closed_tasks: bool,
    pub(super) next_missing_field: bool,
    pub(super) record_field: Option<&'a str>,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn build_result(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
    all_tasks: bool,
    closed_tasks: bool,
) -> Result<CaptureCompleteResult, CompleteError> {
    build_result_for(&CompleteRequest {
        bob_dir,
        raw_text,
        cursor,
        all_tasks,
        closed_tasks,
        next_missing_field: false,
        record_field: None,
    })
}

pub(super) fn build_result_for(
    request: &CompleteRequest<'_>,
) -> Result<CaptureCompleteResult, CompleteError> {
    let directive_owns_cursor =
        capture_language::completion_field_at(request.raw_text, request.cursor)
            .is_some_and(|field| field.context == CompletionContext::Directive);
    if !directive_owns_cursor
        && let Some(mut result) = template_completion_gate(request)?
    {
        result.initial_selection = initial_selection(request.raw_text, &result);
        return Ok(result);
    }
    if request.next_missing_field {
        let mut result = super::records::next_missing_field(request)?;
        result.initial_selection = initial_selection(request.raw_text, &result);
        return Ok(result);
    }
    if request.record_field.is_some() {
        let mut result = super::records::named_field(request)?;
        result.initial_selection = initial_selection(request.raw_text, &result);
        return Ok(result);
    }
    let mut result = build_result_inner(request)?;
    result.initial_selection = initial_selection(request.raw_text, &result);
    Ok(result)
}

fn build_result_inner(
    request: &CompleteRequest<'_>,
) -> Result<CaptureCompleteResult, CompleteError> {
    let bob_dir = request.bob_dir;
    let raw_text = request.raw_text;
    let cursor = request.cursor;
    let all_tasks = request.all_tasks;
    let closed_tasks = request.closed_tasks;
    if let Some(result) = super::records::try_complete(request)? {
        return Ok(result);
    }
    let current_route = capture_language::editor_item_at(raw_text, cursor)
        .and_then(|item| item.route);
    let current_note_path = current_route
        .as_deref()
        .map(capture::route_label)
        .unwrap_or_else(|| capture::route_label(capture::inbox_route()));
    if let Some(field) = capture_links::completion_field_at(
        raw_text,
        cursor,
        current_note_path.clone(),
    ) {
        // In Work Log text, block-link candidates are suppressed because
        // entries reject block links; note and heading completion keeps
        // working.
        if matches!(field.context, CompletionContext::WikilinkBlock)
            && cursor_in_close_log_text(raw_text, cursor)
        {
            return Ok(CaptureCompleteResult::empty(cursor));
        }
        let index = capture_links::NoteIndex::read(bob_dir)
            .map_err(CompleteError::io)?;
        let candidates = match field.context {
            CompletionContext::WikilinkNote => Candidates::WikilinkNote(
                capture_links::note_candidates(&field, &index),
            ),
            CompletionContext::WikilinkHeading => Candidates::WikilinkHeading(
                capture_links::heading_candidates(&field, &index),
            ),
            CompletionContext::WikilinkBlock => Candidates::WikilinkBlock(
                capture_links::block_candidates(&field, &index),
            ),
            CompletionContext::Route
            | CompletionContext::Section
            | CompletionContext::PomodoroBlockId
            | CompletionContext::TaskBlockId
            | CompletionContext::ProjectTaskBlockId
            | CompletionContext::ProjectTaskMove
            | CompletionContext::PomodoroName
            | CompletionContext::PomodoroStartName
            | CompletionContext::PomodoroPastName
            | CompletionContext::PomodoroPastSegment
            | CompletionContext::Task
            | CompletionContext::Drawer
            | CompletionContext::ActiveTask
            | CompletionContext::TaskLink
            | CompletionContext::TaskGoto
            | CompletionContext::TaskParent
            | CompletionContext::TaskDependency
            | CompletionContext::BlockedTask
            | CompletionContext::RelationTo
            | CompletionContext::RelationFrom
            | CompletionContext::RelationLabel
            | CompletionContext::TaskComplete
            | CompletionContext::TaskType
            | CompletionContext::Directive
            | CompletionContext::PomodoroSplitBase
            | CompletionContext::PomodoroSplitType
            | CompletionContext::PomodoroMergeSrc
            | CompletionContext::PomodoroMergeDest
            | CompletionContext::WikilinkTask
            | CompletionContext::RecordTarget
            | CompletionContext::RecordField
            | CompletionContext::RecordValue => {
                unreachable!("link field context")
            }
        };

        return Ok(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            grammar: capture_language::GRAMMAR_REVISION,
            cursor,
            replacement: Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            },
            context: Some(field.context),
            initial_selection: None,
            subject: None,
            candidates,
            target: None,
            theme_suggestion: None,
            r#override: None,
            block_id: None,
            warnings: index.warnings(),
            query: None,
            owner: None,
            relation: None,
            relation_cancel: None,
            relation_complete: None,
            relation_merge: None,
            relation_successor: None,
            picker: None,
            hidden_count: None,
            one_off: None,
            project_task_move: None,
            record: None,
            diagnostics: Vec::new(),
        });
    }

    // A valid edge `#` owns the caret even when it sits inside a
    // `((query` Task Wikilink; `((` itself still completes at its own
    // positions.
    let type_owns_cursor =
        capture_language::task_type_completion_at(raw_text, cursor).is_some();
    if !type_owns_cursor
        && let Some(field) = capture_task_wikilinks::completion_field_at(
            raw_text,
            cursor,
            &current_note_path,
        )
    {
        let (rows, warnings) = capture_task_wikilinks::candidates(
            bob_dir,
            &field,
            &current_note_path,
        );
        let marker_range = Replacement {
            start: field.replacement.0,
            end: field.replacement.1,
        };
        return Ok(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            grammar: capture_language::GRAMMAR_REVISION,
            cursor,
            replacement: marker_range,
            context: Some(CompletionContext::WikilinkTask),
            initial_selection: None,
            subject: None,
            candidates: Candidates::WikilinkTask(rows),
            target: None,
            theme_suggestion: None,
            r#override: None,
            block_id: None,
            warnings,
            query: Some(field.query),
            owner: None,
            relation: None,
            relation_cancel: None,
            relation_complete: None,
            relation_merge: None,
            relation_successor: None,
            picker: Some(PickerDescriptor {
                kind: PickerKind::WikilinkTask,
                scope: PickerScope::Vault,
                scope_token: "((".to_string(),
                note_target: None,
                marker_range,
                trigger_removal_range: Replacement {
                    start: field.replacement.0 + 1,
                    end: field.replacement.1,
                },
                destination: None,
                continuation: None,
                action_continuation_keys: None,
                action_continuation_aliases: None,
                relationship: None,
            }),
            hidden_count: None,
            one_off: None,
            project_task_move: None,
            record: None,
            diagnostics: Vec::new(),
        });
    }

    let Some(field) = capture_language::completion_field_at(raw_text, cursor)
    else {
        return Ok(CaptureCompleteResult::empty(cursor));
    };
    let transfer_role =
        capture_language::transfer_completion_role(raw_text, cursor);
    let merge_in_role =
        capture_language::merge_in_completion_role(raw_text, cursor);
    let include_missing_task_rows = all_tasks
        || (field.context == CompletionContext::Task
            && (merge_in_role.is_some()
                || matches!(
                    &transfer_role,
                    Some(
                        capture_language::TransferCompletionRole::Source { .. }
                    )
                )));

    if field.context == CompletionContext::ProjectTaskMove {
        let detail = capture_language::project_task_move_detail(
            raw_text,
            cursor,
            field.replacement,
        )
        .expect("project_task_move field has matching detail");
        let built = super::candidates::project_task_move_candidates(
            bob_dir,
            &detail,
            &field.query,
        );
        let marker_range = Replacement {
            start: detail.marker_range.0,
            end: detail.marker_range.1,
        };
        let initial_selection =
            (!built.rows.iter().any(|row| row.disabled_reason.is_none()))
                .then_some("none");
        // A lone `- ^` with an empty query offers `^` as a continuation
        // key so a second `^` leaves `- ^^`. The key is not offered for
        // `- :`, for a non-empty query, or outside a sealed project-note
        // item (the detail only exists there). At `- ^^` there is no
        // `project_task_move` context at all. The `.` alias is offered
        // alongside `^` so an empty-filter `.` also leaves `- ^^`.
        let action_continuation_keys = (detail.sigil == '^'
            && field.query.is_empty())
        .then(|| vec!["^".to_string()]);
        let action_continuation_aliases = action_continuation_keys
            .as_ref()
            .map(|_| BTreeMap::from([(".".to_string(), "^".to_string())]));
        return Ok(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            grammar: capture_language::GRAMMAR_REVISION,
            cursor,
            replacement: Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            },
            context: Some(field.context),
            initial_selection,
            subject: None,
            candidates: Candidates::ProjectTaskMove(built.rows),
            target: None,
            theme_suggestion: None,
            r#override: None,
            block_id: None,
            warnings: built.warnings,
            query: Some(field.query),
            owner: None,
            relation: None,
            relation_cancel: None,
            relation_complete: None,
            relation_merge: None,
            relation_successor: None,
            one_off: None,
            picker: Some(PickerDescriptor {
                kind: PickerKind::ProjectTaskMove,
                scope: PickerScope::Note,
                scope_token: detail.sigil.to_string(),
                note_target: detail.parent.clone(),
                marker_range,
                trigger_removal_range: marker_range,
                destination: detail.destination.clone(),
                continuation: Some(detail.continuation),
                action_continuation_keys,
                action_continuation_aliases,
                relationship: None,
            }),
            project_task_move: Some(built.context),
            hidden_count: None,
            record: None,
            diagnostics: Vec::new(),
        });
    }

    if matches!(
        field.context,
        CompletionContext::PomodoroBlockId
            | CompletionContext::TaskBlockId
            | CompletionContext::ProjectTaskBlockId
    ) {
        let route = field.route.as_deref().expect("route resolved");
        let block_field = capture_block_ids::build_block_id_field(
            bob_dir,
            raw_text,
            cursor,
            &capture_block_ids::BlockIdRequest {
                route,
                replacement: field.replacement,
                context: field.context,
            },
        );
        let (candidates, mut warnings) = match block_field.intent {
            capture_block_ids::BlockIdIntent::Link => {
                if matches!(field.context, CompletionContext::PomodoroBlockId) {
                    link_candidates(bob_dir, route, &field.query)?
                } else {
                    (Candidates::Task(Vec::new()), Vec::new())
                }
            }
            capture_block_ids::BlockIdIntent::New
            | capture_block_ids::BlockIdIntent::ProjectNote => {
                (Candidates::Task(Vec::new()), Vec::new())
            }
        };
        // Surface bounded ledger warnings for link candidates.
        if matches!(block_field.intent, capture_block_ids::BlockIdIntent::Link)
            && matches!(field.context, CompletionContext::PomodoroBlockId)
        {
            let day_file = pomodoro::day_file_for(bob_dir);
            let mut ledger_warnings = Vec::new();
            let _ = capture_active_tasks::read_ledger(
                &day_file,
                &mut ledger_warnings,
            );
            for warning in ledger_warnings {
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
        return Ok(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            grammar: capture_language::GRAMMAR_REVISION,
            cursor,
            replacement: Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            },
            context: Some(field.context),
            initial_selection: None,
            subject: None,
            candidates,
            target: None,
            theme_suggestion: None,
            r#override: None,
            block_id: Some(block_field),
            warnings,
            query: None,
            owner: None,
            relation: None,
            relation_cancel: None,
            relation_complete: None,
            relation_merge: None,
            relation_successor: None,
            picker: None,
            hidden_count: None,
            one_off: None,
            project_task_move: None,
            record: None,
            diagnostics: Vec::new(),
        });
    }

    let editor_item = capture_language::editor_item_at(raw_text, cursor);
    // A drawer field at a first-level bullet start carries its own target
    // and accept continuation; the glued `@route+id#` field has neither.
    let bullet = (field.context == CompletionContext::Drawer)
        .then(|| {
            capture_language::drawer_bullet_completion_at(raw_text, cursor)
        })
        .flatten();
    let mut hidden_count = None;
    let mut relation_cancel = None;
    let mut relation_complete = None;
    let mut relation_merge = None;
    let mut relation_successor = None;
    let mut subject = None;
    // A `||` probe is the `relation_label` context with `one_off`: the
    // scan behind the field already classified the separator, so this
    // never re-lexes the draft.
    let one_off = field.context == CompletionContext::RelationLabel
        && capture_language::relation_label_token_at(
            raw_text,
            field.replacement.0,
        )
        .is_some_and(|(_, _, hidden)| hidden);
    let mut theme_suggestion = None;
    let (mut candidates, mut warnings) = match field.context {
        CompletionContext::Route => {
            (route_candidates(bob_dir, &field.query)?, Vec::new())
        }
        CompletionContext::Section => {
            let route = field.route.as_deref().expect("route resolved");
            (
                section_candidates(bob_dir, route, &field.query)?,
                Vec::new(),
            )
        }
        CompletionContext::PomodoroBlockId
        | CompletionContext::TaskBlockId
        | CompletionContext::ProjectTaskBlockId => {
            unreachable!("block-id handled above")
        }
        CompletionContext::Task => {
            let route = field.route.as_deref().expect("route resolved");
            (
                task_candidates(
                    bob_dir,
                    route,
                    &field.query,
                    include_missing_task_rows,
                    TaskSearch::MultiField,
                )?,
                Vec::new(),
            )
        }
        CompletionContext::Drawer => {
            let quoted = raw_text[field.replacement.0..]
                .strip_prefix('#')
                .is_some_and(|name| name.starts_with(['"', '“']));
            if let Some(bullet) = bullet.as_ref() {
                super::drawers::bullet_drawer_candidates(
                    bob_dir,
                    &bullet.target,
                    &field.query,
                    quoted,
                )?
            } else {
                let route = field.route.as_deref().expect("route resolved");
                super::drawers::drawer_candidates(
                    bob_dir,
                    route,
                    field.block_id.as_deref(),
                    &field.query,
                    quoted,
                )?
            }
        }
        CompletionContext::PomodoroName => {
            let (candidates, warnings, suggestion) =
                super::pomodoros::pomodoro_name_candidates_for_task(
                    bob_dir,
                    &field.query,
                    raw_text[field.replacement.0..].starts_with(['"', '“']),
                    field.route.as_deref().expect("route resolved"),
                    field.block_id.as_deref(),
                    editor_item
                        .as_ref()
                        .is_some_and(|item| !item.body.trim().is_empty()),
                )?;
            theme_suggestion = Some(suggestion);
            (candidates, warnings)
        }
        CompletionContext::PomodoroStartName => pomodoro_candidates(
            bob_dir,
            &field.query,
            true,
            raw_text[field.replacement.0..].starts_with(['"', '“']),
        )?,
        CompletionContext::PomodoroPastName => {
            super::pomodoros::pomodoro_past_candidates(
                bob_dir,
                &field.query,
                raw_text[field.replacement.0..].starts_with(['"', '“']),
            )?
        }
        CompletionContext::PomodoroPastSegment => {
            super::pomodoros::pomodoro_past_segment_candidates(
                bob_dir,
                &field.query,
            )?
        }
        CompletionContext::ActiveTask => {
            active_task_candidates(bob_dir, &field.query)
        }
        CompletionContext::TaskLink => {
            task_link_candidates(bob_dir, &field.query)
        }
        CompletionContext::TaskGoto => {
            task_goto_candidates(bob_dir, &field.query, closed_tasks)
        }
        CompletionContext::TaskParent => {
            task_parent_candidates(bob_dir, &field.query)
        }
        CompletionContext::ProjectTaskMove => {
            unreachable!("project task move handled above")
        }
        CompletionContext::TaskComplete => {
            // The unfiltered snapshot comes back when the cursor sits
            // at `replacement.start`: the field reports an empty query
            // there, exactly like the `:` picker.
            task_complete_candidates(bob_dir, &field.query, raw_text, cursor)
        }
        CompletionContext::TaskType => {
            let context = super::task_types::build(
                bob_dir,
                raw_text,
                &field,
                editor_item.as_ref(),
            )?;
            subject = Some(context.subject);
            theme_suggestion = Some(context.suggestion);
            (Candidates::TaskType(context.candidates), context.warnings)
        }
        CompletionContext::Directive => (
            Candidates::Directive(super::directives::candidates(
                bob_dir,
                raw_text,
                &field,
                editor_item.as_ref(),
            )?),
            Vec::new(),
        ),
        CompletionContext::TaskDependency => {
            // Vault-wide prerequisite scan: the lexical owner of the
            // modifier under the cursor plus every complete `&note:id`
            // already typed in its item feed the already-added guards,
            // so picked rows and typed rows agree.
            let typed: Vec<(String, String)> = editor_item
                .as_ref()
                .map(|found| {
                    found
                        .dependencies
                        .iter()
                        .map(|entry| {
                            (entry.note.clone(), entry.block_id.clone())
                        })
                        .collect()
                })
                .unwrap_or_default();
            dependency_candidates(
                bob_dir,
                &field.query,
                editor_item
                    .as_ref()
                    .and_then(|found| found.dependency_target.as_ref()),
                &typed,
            )
        }
        CompletionContext::BlockedTask => {
            let typed_dependencies: Vec<(String, String)> = editor_item
                .as_ref()
                .map(|found| {
                    found
                        .dependencies
                        .iter()
                        .map(|entry| {
                            (entry.note.clone(), entry.block_id.clone())
                        })
                        .collect()
                })
                .unwrap_or_default();
            let typed_blocked: Vec<(String, String)> = editor_item
                .as_ref()
                .map(|found| {
                    found
                        .blocked_tasks
                        .iter()
                        .map(|entry| {
                            (entry.note.clone(), entry.block_id.clone())
                        })
                        .collect()
                })
                .unwrap_or_default();
            let owner_note = editor_item
                .as_ref()
                .and_then(|found| found.route.as_deref())
                .or_else(|| {
                    editor_item
                        .as_ref()
                        .and_then(|found| found.dependency_target.as_ref())
                        .and_then(|target| target.route.as_deref())
                })
                .unwrap_or_else(|| capture::inbox_route());
            blocked_task_candidates(
                bob_dir,
                &field.query,
                editor_item
                    .as_ref()
                    .and_then(|found| found.dependency_target.as_ref()),
                owner_note,
                &typed_dependencies,
                &typed_blocked,
            )
        }
        CompletionContext::RelationTo | CompletionContext::RelationFrom => {
            let item = editor_item.as_ref();
            let owner_note = item
                .and_then(|found| found.route.as_deref())
                .or_else(|| {
                    item.and_then(|found| found.dependency_target.as_ref())
                        .and_then(|target| target.route.as_deref())
                })
                .unwrap_or_else(|| capture::inbox_route());
            relation_candidates(
                bob_dir,
                &field.query,
                if field.context == CompletionContext::RelationTo {
                    RelationDirection::To
                } else {
                    RelationDirection::From
                },
                item.and_then(|found| found.dependency_target.as_ref()),
                &capture::route_label(owner_note),
                item.map(|found| found.relations.as_slice()).unwrap_or(&[]),
                item.map(|found| found.dependencies.as_slice())
                    .unwrap_or(&[]),
            )
        }
        CompletionContext::RelationLabel => {
            let policy = super::cancel::load_policy();
            let (mut candidates, warnings, count) = relation_label_suggestions(
                bob_dir,
                &field.query,
                one_off,
                &policy,
            );
            hidden_count = Some(count);
            if !policy.cancel_labels.is_empty()
                && let Some(plan) = super::cancel::plan_relation_cancel(
                    bob_dir,
                    raw_text,
                    field.replacement.0,
                    editor_item.as_ref(),
                    &field.query,
                    &policy,
                )
            {
                if let Candidates::RelationLabel(rows) = &mut candidates
                    && let Some(reason) = plan.row_disable.as_deref()
                {
                    for row in rows.iter_mut() {
                        if row.cancels_source {
                            row.disabled_reason = Some(reason.to_string());
                            row.replacement.clear();
                        }
                    }
                }
                relation_cancel = Some(plan.response);
            }
            if !policy.complete_labels.is_empty()
                && let Some(plan) = super::complete::plan_relation_complete(
                    bob_dir,
                    raw_text,
                    field.replacement.0,
                    editor_item.as_ref(),
                    &field.query,
                    &policy,
                )
            {
                if let Candidates::RelationLabel(rows) = &mut candidates
                    && let Some(reason) = plan.row_disable.as_deref()
                {
                    for row in rows.iter_mut() {
                        if row.completes_source {
                            row.disabled_reason = Some(reason.to_string());
                            row.replacement.clear();
                        }
                    }
                }
                relation_complete = Some(plan.response);
            }
            if !policy.merge_labels.is_empty()
                && let Some(plan) = super::merge::plan_relation_merge(
                    bob_dir,
                    raw_text,
                    field.replacement.0,
                    editor_item.as_ref(),
                    &field.query,
                    &policy,
                )
            {
                if let Candidates::RelationLabel(rows) = &mut candidates
                    && let Some(reason) = plan.row_disable.as_deref()
                {
                    for row in rows.iter_mut() {
                        if row.merges_source {
                            row.disabled_reason = Some(reason.to_string());
                            row.replacement.clear();
                        }
                    }
                }
                relation_merge = Some(plan.response);
            }
            if !policy.successor_labels.is_empty()
                && let Some(plan) = super::successor::plan_relation_successor(
                    bob_dir,
                    raw_text,
                    field.replacement.0,
                    editor_item.as_ref(),
                    &field.query,
                    &policy,
                )
            {
                relation_successor = Some(plan);
            }
            (candidates, warnings)
        }
        CompletionContext::PomodoroSplitBase => {
            let (candidates, warnings) =
                super::pomodoros::pomodoro_split_base_candidates(
                    bob_dir,
                    &field.query,
                )?;
            (candidates, warnings)
        }
        CompletionContext::PomodoroSplitType => {
            let (candidates, warnings) =
                super::pomodoros::pomodoro_split_type_candidates(
                    bob_dir,
                    raw_text,
                    cursor,
                    &field.query,
                )?;
            (candidates, warnings)
        }
        CompletionContext::PomodoroMergeSrc => {
            let (candidates, warnings) =
                super::pomodoros::pomodoro_merge_src_candidates(
                    bob_dir,
                    &field.query,
                )?;
            (candidates, warnings)
        }
        CompletionContext::PomodoroMergeDest => {
            let (candidates, warnings) =
                super::pomodoros::pomodoro_merge_dest_candidates(
                    bob_dir,
                    raw_text,
                    cursor,
                    &field.query,
                )?;
            (candidates, warnings)
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock
        | CompletionContext::WikilinkTask
        | CompletionContext::RecordTarget
        | CompletionContext::RecordField
        | CompletionContext::RecordValue => {
            unreachable!("marker field context")
        }
    };

    // Transfer-assist scoping: a `>` line reuses the ordinary contexts,
    // so narrow them here — movable-or-mergeable sources, minus-source
    // merge targets with a merge consequence, the `+` hint on complete
    // bare destinations, and section creation plus the §3.4 refusal.
    let mix_record_create = transfer_role.is_none() && merge_in_role.is_none();
    if let Some(role) = transfer_role {
        super::transfer_assist::apply(
            bob_dir,
            &field,
            role,
            &mut candidates,
            &mut warnings,
        )?;
    }
    if let Some(role) = merge_in_role {
        super::transfer_assist::apply_merge_in(
            bob_dir,
            &field,
            role,
            &mut candidates,
        )?;
    }

    let mut replacement = Replacement {
        start: field.replacement.0,
        end: field.replacement.1,
    };
    if field.context == CompletionContext::Route {
        super::candidates::annotate_record_route_continuations(
            &mut candidates,
            replacement.start,
        );
    }
    if field.context == CompletionContext::Section
        && mix_record_create
        && let Some(expanded) = super::candidates::mix_record_section_create(
            bob_dir,
            raw_text,
            &field,
            &mut candidates,
        )
    {
        replacement = expanded;
    }

    // Bob owns the search query for task pickers and relation completion.
    // Relation target queries are decoded here so clients never parse a
    // quoted note component; relation target and label contexts carry
    // their lexical owner.
    let (query, owner) = if matches!(
        field.context,
        CompletionContext::TaskDependency
            | CompletionContext::BlockedTask
            | CompletionContext::Task
            | CompletionContext::TaskParent
            | CompletionContext::TaskComplete
            | CompletionContext::TaskType
            | CompletionContext::Directive
            | CompletionContext::Drawer
            | CompletionContext::RelationTo
            | CompletionContext::RelationFrom
            | CompletionContext::RelationLabel
    ) {
        (
            Some(field.query.clone()),
            if matches!(
                field.context,
                CompletionContext::TaskDependency
                    | CompletionContext::BlockedTask
                    | CompletionContext::RelationTo
                    | CompletionContext::RelationFrom
                    | CompletionContext::RelationLabel
            ) {
                editor_item
                    .as_ref()
                    .and_then(|item| item.dependency_target.clone())
                    .map(|target| {
                        completion_owner(bob_dir, editor_item.as_ref(), target)
                    })
            } else {
                None
            },
        )
    } else {
        (None, None)
    };
    // The relation token behind a label field: its direction plus the
    // sigil-to-`|` range, derived from the same scan that produced the
    // field, never by re-lexing bytes here.
    let relation = if field.context == CompletionContext::RelationLabel {
        capture_language::relation_label_token_at(raw_text, field.replacement.0)
            .map(|(start, direction, _)| CompletionRelation {
                direction,
                range: Replacement {
                    start,
                    end: field.replacement.0,
                },
            })
    } else {
        None
    };
    let mut picker = picker_descriptor(raw_text, &field);
    if matches!(
        field.context,
        CompletionContext::TaskParent | CompletionContext::Task
    ) && let (Some(picker), Some(item)) =
        (picker.as_mut(), editor_item.as_ref())
    {
        picker.relationship =
            apply_relationship_picker(bob_dir, &mut candidates, item);
    }
    // A named-start name field carries override context when exactly one
    // session runs and no earlier session operator is in the draft.
    let override_info = (field.context == CompletionContext::PomodoroStartName)
        .then(|| capture_language::pomodoro_start_override_at(raw_text, cursor))
        .flatten()
        .and_then(|keeps_ledger| {
            running_pomodoro_session(bob_dir).map(|running| {
                OverrideCompletion {
                    keeps_ledger,
                    running,
                }
            })
        });
    if field.bare_name {
        let replaced = raw_text
            .get(field.replacement.0..field.replacement.1)
            .unwrap_or("");
        apply_bare_start_name_replacements(&mut candidates, replaced);
    }
    if bullet.as_ref().is_some_and(|bullet| bullet.continuation) {
        super::drawers::append_continuation(&mut candidates);
    }
    if field.context == CompletionContext::Drawer {
        super::drawers::place_cursor_after(
            &mut candidates,
            field.replacement.0,
        );
    }

    let drawer_target = if field.context == CompletionContext::Drawer {
        bullet
            .as_ref()
            .and_then(|bullet| {
                super::drawers::fresh_bullet_target(&bullet.target)
            })
            .or_else(|| drawer_completion_target(bob_dir, &field, &candidates))
    } else {
        None
    };
    Ok(CaptureCompleteResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        grammar: capture_language::GRAMMAR_REVISION,
        cursor,
        replacement,
        context: Some(field.context),
        initial_selection: None,
        subject,
        candidates,
        target: drawer_target,
        theme_suggestion,
        r#override: override_info,
        block_id: None,
        warnings,
        query,
        owner,
        relation,
        relation_cancel,
        relation_complete,
        relation_merge,
        relation_successor,
        picker,
        hidden_count,
        one_off: one_off.then_some(true),
        project_task_move: None,
        record: None,
        diagnostics: Vec::new(),
    })
}

/// Template completion uses a same-length masked draft so template syntax and
/// row values never become picker queries.
fn template_completion_gate(
    request: &CompleteRequest<'_>,
) -> Result<Option<CaptureCompleteResult>, CompleteError> {
    let raw_text = request.raw_text;
    let cursor = request.cursor;
    let analysis = crate::native::capture_language::template::analyze(raw_text);
    if analysis.directive.is_none() {
        return Ok(None);
    }

    let (directive_range, _) =
        analysis.directive.as_ref().expect("checked directive");
    if (directive_range.start..=directive_range.end).contains(&cursor)
        || crate::native::capture_language::template::cursor_on_template_row(
            &analysis, cursor,
        )
        || analysis.placeholders.iter().any(|placeholder| {
            placeholder.range.start <= cursor && cursor <= placeholder.range.end
        })
    {
        return Ok(Some(CaptureCompleteResult::empty(cursor)));
    }

    let masked = crate::native::capture_language::template::masked(raw_text);
    let masked_request = CompleteRequest {
        raw_text: &masked,
        ..*request
    };
    let result = build_result_inner(&masked_request)?;
    if matches!(
        result.context,
        Some(
            CompletionContext::PomodoroBlockId
                | CompletionContext::TaskBlockId
                | CompletionContext::ProjectTaskBlockId
        )
    ) || analysis.placeholders.iter().any(|placeholder| {
        result.replacement.start < placeholder.range.end
            && placeholder.range.start < result.replacement.end
    }) {
        return Ok(Some(CaptureCompleteResult::empty(cursor)));
    }
    Ok(Some(result))
}

fn drawer_completion_target(
    bob_dir: &Path,
    field: &capture_language::CompletionField,
    candidates: &Candidates,
) -> Option<DrawerTarget> {
    let route = field.route.clone()?;
    let route_label = capture::route_label(&route);
    let block_id = field.block_id.clone();
    // Prefer the parent text already resolved for task rows.
    if let Candidates::Drawer(rows) = candidates
        && let Some(first) = rows.first()
    {
        return Some(DrawerTarget {
            route: route_label,
            block_id,
            text: first.text.clone(),
        });
    }
    // Fall back to reading the parent directly so `target` is present
    // even when the drawer list is empty.
    let note_path = bob_dir.join(&route_label);
    let content = std::fs::read_to_string(&note_path).ok()?;
    let settings = crate::native::note_tasks::read_settings(bob_dir);
    let scan = crate::native::note_tasks::scan(&content, &settings);
    let parent =
        block_id
            .as_deref()
            .and_then(|id| match scan.by_block_id(id) {
                crate::native::note_tasks::BlockIdLookup::Found(task) => {
                    Some(task)
                }
                _ => None,
            });
    Some(DrawerTarget {
        route: route_label,
        block_id,
        text: parent
            .map(|task| task.description.clone())
            .unwrap_or_default(),
    })
}

fn initial_selection(
    raw_text: &str,
    result: &CaptureCompleteResult,
) -> Option<&'static str> {
    if matches!(result.context, Some(CompletionContext::ProjectTaskMove)) {
        return match &result.candidates {
            Candidates::ProjectTaskMove(rows)
                if rows.iter().any(|row| row.disabled_reason.is_none()) =>
            {
                None
            }
            Candidates::ProjectTaskMove(_) => Some("none"),
            _ => None,
        };
    }
    if matches!(result.context, Some(CompletionContext::PomodoroMergeSrc))
        && result.replacement.start == result.replacement.end
    {
        return Some("none");
    }
    if result.query.as_deref() != Some("") {
        return None;
    }
    // An empty drawer query (`@route+id#`) opens with nothing selected,
    // even though the `#` is glued to the ID.
    if matches!(result.context, Some(CompletionContext::Drawer)) {
        return Some("none");
    }
    let marker =
        raw_text.get(result.replacement.start..result.replacement.end)?;
    let bare = match result.context? {
        CompletionContext::TaskType => marker == "#",
        CompletionContext::Directive => marker == "%",
        _ => false,
    };
    if !bare {
        return None;
    }
    let left_edge = raw_text[..result.replacement.start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    let right_edge = raw_text[result.replacement.end..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace);
    (left_edge && right_edge).then_some("none")
}

fn completion_owner(
    bob_dir: &Path,
    item: Option<&capture_language::EditorItemParse>,
    target: DependencyTarget,
) -> CompletionOwner {
    let mut enriched = CompletionOwner {
        target,
        text: None,
        note_path: None,
        status_symbol: None,
        status_name: None,
    };
    match enriched.target.kind {
        DependencyTargetKind::NewTask => {
            if let Some(item) = item.filter(|item| !item.body.trim().is_empty())
            {
                enriched.text = Some(item.body.clone());
            }
        }
        DependencyTargetKind::ExistingTask => {
            let (Some(route), Some(block_id)) = (
                enriched.target.route.as_deref(),
                enriched.target.block_id.as_deref(),
            ) else {
                return enriched;
            };
            let discovered = capture_dependency_tasks::discover(bob_dir);
            let note = capture::route_label(route);
            let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
                bob_dir,
                &discovered,
                &note,
            ) else {
                return enriched;
            };
            let note_path = capture_dependency_tasks::display_path(&path);
            let mut matches = discovered.tasks.iter().filter(|task| {
                task.note_path == note_path
                    && task.block_id.as_deref() == Some(block_id)
            });
            let Some(task) = matches.next() else {
                return enriched;
            };
            if matches.next().is_some() {
                return enriched;
            }
            enriched.text = Some(task.text.clone());
            enriched.note_path = Some(task.note_path.clone());
            enriched.status_symbol = Some(task.status_symbol);
            enriched.status_name = Some(task.status_name.clone());
        }
    }
    enriched
}

fn picker_descriptor(
    raw_text: &str,
    field: &capture_language::CompletionField,
) -> Option<PickerDescriptor> {
    match field.context {
        CompletionContext::TaskParent => {
            let marker_range = Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            };
            let selector =
                raw_text.get(marker_range.start..marker_range.end)?;
            let action_continuation_keys = if selector == "+"
                && capture_language::editor_item_at(
                    raw_text,
                    field.replacement.1,
                )
                .is_some_and(|item| {
                    item.line_start == item.line_end
                        && raw_text
                            .get(item.start..item.end)
                            .is_some_and(|text| text.trim() == "+")
                }) {
                Some(
                    (0..=9)
                        .map(|digit| digit.to_string())
                        .chain(std::iter::once("+".to_string()))
                        .collect(),
                )
            } else {
                None
            };
            let continuation =
                parent_task_comma_continuation(raw_text, marker_range, "@x+y");
            let action_continuation_aliases =
                action_continuation_keys.as_ref().map(|_| {
                    BTreeMap::from([(String::from(","), String::from("+"))])
                });
            Some(PickerDescriptor {
                kind: PickerKind::ParentTask,
                scope: PickerScope::Vault,
                scope_token: "+".to_string(),
                note_target: None,
                marker_range,
                trigger_removal_range: marker_range,
                destination: None,
                continuation,
                action_continuation_keys,
                action_continuation_aliases,
                relationship: None,
            })
        }
        CompletionContext::ProjectTaskMove => {
            unreachable!("project task move handled above")
        }
        CompletionContext::TaskComplete => {
            // The replacement already covers the whole `!` token,
            // sigil included, so both ranges are the token. The
            // continuation keys apply only to a bare whole-item `!`:
            // an editor hands `!!` and `![[` straight back to prose
            // and embeds.
            let marker_range = Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            };
            let bare = raw_text
                .get(marker_range.start..marker_range.end)
                .is_some_and(|token| token == "!");
            Some(PickerDescriptor {
                kind: PickerKind::TaskComplete,
                scope: PickerScope::Vault,
                scope_token: "!".to_string(),
                note_target: None,
                marker_range,
                trigger_removal_range: marker_range,
                destination: None,
                continuation: None,
                action_continuation_keys: bare
                    .then(|| vec!["!".to_string(), "[".to_string()]),
                action_continuation_aliases: None,
                relationship: None,
            })
        }
        CompletionContext::Task => {
            let route = field.route.as_deref()?;
            let line = capture_language::split_physical_lines(raw_text)
                .into_iter()
                .find(|line| {
                    field.replacement.0 >= line.start
                        && field.replacement.0 <= line.end
                })?;
            let token = capture_language::tokenize_line_with_spans(&line)
                .into_iter()
                .find(|token| {
                    token.start <= field.replacement.0
                        && token.end >= field.replacement.1
                })?;
            let plus = token.text.find('+')?;
            let plus_start = token.start + plus;
            if plus_start >= field.replacement.0 {
                return None;
            }
            let scope_token = format!("@{route}+");
            let marker_range = Replacement {
                start: token.start,
                end: field.replacement.1,
            };
            let continuation = parent_task_comma_continuation(
                raw_text,
                Replacement {
                    start: field.replacement.0,
                    end: field.replacement.1,
                },
                "y",
            );
            Some(PickerDescriptor {
                kind: PickerKind::ParentTask,
                scope: PickerScope::Note,
                scope_token,
                note_target: Some(capture::route_label(route)),
                marker_range,
                trigger_removal_range: Replacement {
                    start: plus_start,
                    end: field.replacement.1,
                },
                destination: None,
                continuation,
                action_continuation_keys: None,
                action_continuation_aliases: None,
                relationship: None,
            })
        }
        _ => None,
    }
}

/// Advertise accept-and-continue only when Bob's comma rewrite lane would
/// extend the row inserted by this picker. The simulation keeps rewrite
/// eligibility as the single source of truth for marker placement,
/// transfer exclusions, and trailing text.
fn parent_task_comma_continuation(
    raw_text: &str,
    replacement: Replacement,
    inserted_member: &str,
) -> Option<String> {
    let mut simulated = raw_text.to_string();
    simulated
        .replace_range(replacement.start..replacement.end, inserted_member);
    let comma_start = replacement.start + inserted_member.len();
    simulated.insert(comma_start, ',');
    let comma_end = comma_start + 1;
    let rewrite = capture_language::rewrite_draft(&simulated, Some(comma_end));
    let edit = rewrite.edits.as_slice();
    (rewrite.rule == Some(capture_language::RewriteRule::ExpandSigilAlias)
        && edit.len() == 1
        && edit[0].start == comma_start
        && edit[0].end == comma_end
        && edit[0].replacement == " +")
        .then(|| " +".to_string())
}

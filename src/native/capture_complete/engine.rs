use std::path::Path;

use super::{
    candidates::{
        active_task_candidates, link_candidates, route_candidates,
        section_candidates, task_candidates, task_link_candidates,
        task_section_candidates, TaskSearch,
    },
    model::{
        Candidates, CaptureCompleteResult, CompleteError, Replacement,
        SCHEMA_VERSION,
    },
    pomodoros::{pomodoro_name_candidates, pomodoro_start_name_candidates},
};
use crate::native::{
    capture, capture_active_tasks, capture_block_ids,
    capture_language::{self, CompletionContext},
    capture_links, pomodoro,
};

/// `true` when `cursor` sits in Work Log text: a bullet line or an inline
/// entry on the close line. Block-link completion is suppressed there
/// because entries reject block links, while note and heading completion
/// keeps working.
pub(super) fn cursor_in_close_log_text(raw_text: &str, cursor: usize) -> bool {
    capture_language::cursor_in_close_log_text(raw_text, cursor)
}

pub(super) fn build_result(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
    all_tasks: bool,
) -> Result<CaptureCompleteResult, CompleteError> {
    let current_route = capture_language::editor_item_at(raw_text, cursor)
        .and_then(|item| item.route);
    let current_note_path = current_route
        .as_deref()
        .map(capture::route_label)
        .unwrap_or_else(|| capture::route_label(capture::inbox_route()));
    if let Some(field) =
        capture_links::completion_field_at(raw_text, cursor, current_note_path)
    {
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
            | CompletionContext::PomodoroName
            | CompletionContext::PomodoroStartName
            | CompletionContext::Task
            | CompletionContext::TaskSection
            | CompletionContext::ActiveTask
            | CompletionContext::TaskLink => {
                unreachable!("link field context")
            }
        };

        return Ok(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor,
            replacement: Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            },
            context: Some(field.context),
            candidates,
            block_id: None,
            warnings: index.warnings(),
        });
    }

    let Some(field) = capture_language::completion_field_at(raw_text, cursor)
    else {
        return Ok(CaptureCompleteResult::empty(cursor));
    };

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
            cursor,
            replacement: Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            },
            context: Some(field.context),
            candidates,
            block_id: Some(block_field),
            warnings,
        });
    }

    let (candidates, warnings) = match field.context {
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
                    all_tasks,
                    TaskSearch::MultiField,
                )?,
                Vec::new(),
            )
        }
        CompletionContext::TaskSection => {
            let route = field.route.as_deref().expect("route resolved");
            task_section_candidates(
                bob_dir,
                route,
                field.block_id.as_deref(),
                &field.query,
            )?
        }
        CompletionContext::PomodoroName => {
            pomodoro_name_candidates(bob_dir, &field.query)?
        }
        CompletionContext::PomodoroStartName => {
            pomodoro_start_name_candidates(bob_dir, &field.query)?
        }
        CompletionContext::ActiveTask => {
            active_task_candidates(bob_dir, &field.query)
        }
        CompletionContext::TaskLink => {
            task_link_candidates(bob_dir, &field.query)
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock => {
            unreachable!("marker field context")
        }
    };

    Ok(CaptureCompleteResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        cursor,
        replacement: Replacement {
            start: field.replacement.0,
            end: field.replacement.1,
        },
        context: Some(field.context),
        candidates,
        block_id: None,
        warnings,
    })
}

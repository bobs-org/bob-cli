use std::path::Path;

use super::{
    candidates::{
        active_task_candidates, dependency_candidates, link_candidates,
        route_candidates, section_candidates, task_candidates,
        task_complete_candidates, task_link_candidates, task_parent_candidates,
        task_section_candidates, TaskSearch,
    },
    model::{
        Candidates, CaptureCompleteResult, CompleteError, OverrideCompletion,
        PickerDescriptor, PickerKind, PickerScope, Replacement, SCHEMA_VERSION,
    },
    pomodoros::{
        pomodoro_name_candidates, pomodoro_start_name_candidates,
        running_pomodoro_session,
    },
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
            | CompletionContext::TaskLink
            | CompletionContext::TaskParent
            | CompletionContext::TaskDependency
            | CompletionContext::TaskComplete => {
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
            r#override: None,
            block_id: None,
            warnings: index.warnings(),
            query: None,
            owner: None,
            picker: None,
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
            r#override: None,
            block_id: Some(block_field),
            warnings,
            query: None,
            owner: None,
            picker: None,
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
        CompletionContext::TaskParent => {
            task_parent_candidates(bob_dir, &field.query)
        }
        CompletionContext::TaskComplete => {
            // The unfiltered snapshot comes back when the cursor sits
            // at `replacement.start`: the field reports an empty query
            // there, exactly like the `:` picker.
            task_complete_candidates(bob_dir, &field.query, raw_text, cursor)
        }
        CompletionContext::TaskDependency => {
            // Vault-wide prerequisite scan: the lexical owner of the
            // modifier under the cursor plus every complete `&note:id`
            // already typed in its item feed the already-added guards,
            // so picked rows and typed rows agree.
            let item = capture_language::editor_item_at(raw_text, cursor);
            let typed: Vec<(String, String)> = item
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
                item.as_ref()
                    .and_then(|found| found.dependency_target.as_ref()),
                &typed,
            )
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock => {
            unreachable!("marker field context")
        }
    };

    // Bob owns the search query for dependency, scoped task,
    // parent-task, and task-complete pickers. Dependency and
    // task-complete queries are decoded here so clients never parse a
    // quoted note component; only the dependency context also carries
    // its owner.
    let (query, owner) = if matches!(
        field.context,
        CompletionContext::TaskDependency
            | CompletionContext::Task
            | CompletionContext::TaskParent
            | CompletionContext::TaskComplete
    ) {
        (
            Some(field.query.clone()),
            if field.context == CompletionContext::TaskDependency {
                capture_language::editor_item_at(raw_text, cursor)
                    .and_then(|item| item.dependency_target)
            } else {
                None
            },
        )
    } else {
        (None, None)
    };
    let picker = picker_descriptor(raw_text, &field);
    // A `==` name field carries the additive override context (keeps_ledger
    // plus the running session); plain `=` output stays byte-identical.
    let override_info = (field.context == CompletionContext::PomodoroStartName)
        .then(|| capture_language::pomodoro_start_override_at(raw_text, cursor))
        .flatten()
        .map(|keeps_ledger| OverrideCompletion {
            keeps_ledger,
            running: running_pomodoro_session(bob_dir),
        });

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
        r#override: override_info,
        block_id: None,
        warnings,
        query,
        owner,
        picker,
    })
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
            Some(PickerDescriptor {
                kind: PickerKind::ParentTask,
                scope: PickerScope::Vault,
                scope_token: "+".to_string(),
                note_target: None,
                marker_range,
                trigger_removal_range: marker_range,
                action_continuation_keys,
            })
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
                action_continuation_keys: bare
                    .then(|| vec!["!".to_string(), "[".to_string()]),
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
            let prefix = if token.text.starts_with("@@") {
                "@@"
            } else {
                "@"
            };
            let scope_token = format!("{prefix}{route}+");
            let marker_range = Replacement {
                start: token.start,
                end: field.replacement.1,
            };
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
                action_continuation_keys: None,
            })
        }
        _ => None,
    }
}

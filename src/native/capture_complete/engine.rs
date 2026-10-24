use std::path::Path;

use super::{
    candidates::{
        active_task_candidates, apply_relationship_picker,
        blocked_task_candidates, dependency_candidates, link_candidates,
        relation_candidates, relation_label_suggestions, route_candidates,
        section_candidates, task_candidates, task_complete_candidates,
        task_goto_candidates, task_link_candidates, task_parent_candidates,
        task_section_candidates, TaskSearch,
    },
    model::{
        Candidates, CaptureCompleteResult, CompleteError, CompletionOwner,
        CompletionRelation, OverrideCompletion, PickerDescriptor, PickerKind,
        PickerScope, Replacement, RouteCandidate, RouteCandidateKind,
        SCHEMA_VERSION,
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
    if let Some(result) =
        template_completion_gate(bob_dir, raw_text, cursor, all_tasks)?
    {
        return Ok(result);
    }
    build_result_inner(bob_dir, raw_text, cursor, all_tasks)
}

fn build_result_inner(
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
            | CompletionContext::TaskGoto
            | CompletionContext::TaskParent
            | CompletionContext::TaskDependency
            | CompletionContext::BlockedTask
            | CompletionContext::RelationTo
            | CompletionContext::RelationFrom
            | CompletionContext::RelationLabel
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
            relation: None,
            picker: None,
            hidden_count: None,
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
            relation: None,
            picker: None,
            hidden_count: None,
        });
    }

    let editor_item = capture_language::editor_item_at(raw_text, cursor);
    let mut hidden_count = None;
    let (mut candidates, warnings) = match field.context {
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
        CompletionContext::PomodoroName => pomodoro_candidates(
            bob_dir,
            &field.query,
            false,
            raw_text[field.replacement.0..].starts_with(['"', '“']),
        )?,
        CompletionContext::PomodoroStartName => pomodoro_candidates(
            bob_dir,
            &field.query,
            true,
            raw_text[field.replacement.0..].starts_with(['"', '“']),
        )?,
        CompletionContext::ActiveTask => {
            active_task_candidates(bob_dir, &field.query)
        }
        CompletionContext::TaskLink => {
            task_link_candidates(bob_dir, &field.query)
        }
        CompletionContext::TaskGoto => {
            task_goto_candidates(bob_dir, &field.query)
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
            let (candidates, warnings, count) =
                relation_label_suggestions(bob_dir, &field.query);
            hidden_count = Some(count);
            (candidates, warnings)
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock => {
            unreachable!("marker field context")
        }
    };

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
            .map(|(start, direction)| CompletionRelation {
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
        apply_bare_start_name_replacements(&mut candidates);
    }

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
        relation,
        picker,
        hidden_count,
    })
}

/// Template completion uses a same-length masked draft so template syntax and
/// row values never become picker queries. Partial first-line headers remain
/// ordinary route completion, with a synthetic `template:` candidate.
fn template_completion_gate(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
    all_tasks: bool,
) -> Result<Option<CaptureCompleteResult>, CompleteError> {
    let partial_header = partial_template_header(raw_text, cursor);
    let analysis = crate::native::capture_language::template::analyze(raw_text);
    if analysis.header.is_none() {
        if let Some((start, end, query)) = partial_header {
            let mut result =
                build_result_inner(bob_dir, raw_text, cursor, all_tasks)?;
            if result.context == Some(CompletionContext::Route)
                && let Candidates::Route(ref mut candidates) = result.candidates
            {
                let template = RouteCandidate {
                    replacement: "template:".to_string(),
                    route: "template:".to_string(),
                    label: "Bulk template: one capture per row below"
                        .to_string(),
                    kind: RouteCandidateKind::Template,
                    status: None,
                    match_kind: None,
                };
                if "template:".starts_with(&query.to_ascii_lowercase()) {
                    let at = if query.is_empty() {
                        candidates.len()
                    } else {
                        0
                    };
                    candidates.insert(at, template);
                    result.replacement = Replacement { start, end };
                }
            }
            return Ok(Some(result));
        }
        return Ok(None);
    }

    let (header_range, _) = analysis.header.as_ref().expect("checked header");
    if (header_range.start..=header_range.end).contains(&cursor)
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
    let result = build_result_inner(bob_dir, &masked, cursor, all_tasks)?;
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

fn partial_template_header(
    raw: &str,
    cursor: usize,
) -> Option<(usize, usize, String)> {
    if cursor > raw.len() || !raw.is_char_boundary(cursor) {
        return None;
    }
    let lines = crate::native::capture_language::split_physical_lines(raw);
    let first_nonblank_index = lines
        .iter()
        .position(|line| !line.text.trim_matches([' ', '\t']).is_empty())?;
    let cursor_index = lines
        .iter()
        .position(|line| line.start <= cursor && cursor <= line.end)?;
    if cursor_index != first_nonblank_index {
        return None;
    }
    let cursor_line = &lines[cursor_index];
    let line_start = cursor_line.start;
    let line_end = cursor_line.end;
    let line = &raw[line_start..line_end];
    let trimmed_start = line.len() - line.trim_start_matches([' ', '\t']).len();
    let content = &line[trimmed_start..];
    if !"@@template:".starts_with(&content.to_ascii_lowercase())
        || content.len() < 2
        || content.to_ascii_lowercase() == "@@template:"
    {
        return None;
    }
    let query = content.strip_prefix("@@")?;
    Some((
        line_start + trimmed_start + 2,
        cursor.min(line_end),
        query.to_string(),
    ))
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
            Some(PickerDescriptor {
                kind: PickerKind::ParentTask,
                scope: PickerScope::Vault,
                scope_token: "+".to_string(),
                note_target: None,
                marker_range,
                trigger_removal_range: marker_range,
                action_continuation_keys,
                relationship: None,
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
                relationship: None,
            })
        }
        _ => None,
    }
}

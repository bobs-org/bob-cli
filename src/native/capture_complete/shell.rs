use std::path::Path;

use super::{
    candidates::{
        active_task_candidates, link_candidates, route_candidates,
        section_candidates, task_candidates, task_link_candidates,
        task_section_candidates, TaskSearch,
    },
    model::Candidates,
    pomodoros::{pomodoro_name_candidates, pomodoro_start_name_candidates},
};
use crate::native::{
    capture, capture_block_ids,
    capture_language::{self, CompletionContext},
    capture_links,
    capture_targets::CaptureTargetKind,
};

/// One shell-completion row: the full marker text to insert (from the
/// marker's `@`/`^`/`:`/`=` sigil through the candidate), a description,
/// a human group, and whether the shell should keep typing (`nospace`)
/// because the value ends in a continuation character (`:` `+` `#` `=`
/// `^`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellRow {
    pub full: String,
    pub description: String,
    pub group: String,
    pub nospace: bool,
}

/// Shell completion for one capture marker: the marker's byte start in
/// `raw_text` plus safe rows. `None` means no marker (or a deferred
/// wikilink): the caller falls back or shows nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellCompletion {
    pub marker_start: usize,
    pub rows: Vec<ShellRow>,
}

/// Extract shell completion from `capture_complete` without re-implementing
/// grammar: wikilinks return `None` before `NoteIndex::read` (about 300 ms,
/// deferred), safe rows only (no `requires_block_id` / `requires_name`;
/// `creates_pomodoro` rows stay as `new Pomodoro`), and nothing beyond
/// completion (no `capture-task-id`, `capture-pomodoro-name`, writes, or
/// dry runs — only `build_result`'s read-only scans with `all_tasks`
/// off).
pub(crate) fn shell_completion(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
) -> Result<Option<ShellCompletion>, String> {
    if raw_text.contains(['\n', '\r']) {
        return Ok(None);
    }
    if cursor > raw_text.len() || !raw_text.is_char_boundary(cursor) {
        return Ok(None);
    }
    // Wikilinks are deferred: return before the ~300 ms `NoteIndex::read`.
    let current_route = capture_language::editor_item_at(raw_text, cursor)
        .and_then(|item| item.route);
    let current_note_path = current_route
        .as_deref()
        .map(capture::route_label)
        .unwrap_or_else(|| capture::route_label(capture::inbox_route()));
    if capture_links::completion_field_at(raw_text, cursor, current_note_path)
        .is_some()
    {
        return Ok(None);
    }
    let Some(field) = capture_language::completion_field_at(raw_text, cursor)
    else {
        return Ok(None);
    };
    // End of the active word only: the replacement must end at the cursor.
    // A non-empty `--suffix` never reaches here with end == cursor, since
    // the caller builds `raw_text` ending at the cursor.
    if field.replacement.1 != cursor {
        return Ok(None);
    }
    if field.replacement.0 > field.replacement.1 {
        return Ok(None);
    }
    let Some(prefix) = raw_text.get(..field.replacement.0) else {
        return Ok(None);
    };
    let marker_start = prefix
        .rfind([' ', '\t'])
        .map(|index| index + 1)
        .unwrap_or(0);
    if !raw_text.is_char_boundary(marker_start) {
        return Ok(None);
    }
    let Some(marker_prefix) = raw_text.get(marker_start..field.replacement.0)
    else {
        return Ok(None);
    };
    let marker_prefix = marker_prefix.to_string();
    // The shell filters, not bob: always serve the slot's full set with an
    // empty query, so `matcher-list` keeps working (`@ca` lists every route).
    let route_for_group = field.route.clone().unwrap_or_default();
    let mut rows = Vec::new();
    match field.context {
        CompletionContext::Route => {
            let candidates = route_candidates(bob_dir, "")
                .map_err(|error| error.message.clone())?;
            let Candidates::Route(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                let (group, description) = match item.kind {
                    CaptureTargetKind::Inbox => (
                        "inbox",
                        "inbox \u{00B7} default capture target".to_string(),
                    ),
                    CaptureTargetKind::Area => ("areas", "area".to_string()),
                    CaptureTargetKind::Project => (
                        "projects",
                        item.status
                            .as_deref()
                            .map(|status| format!("project \u{00B7} {status}"))
                            .unwrap_or_else(|| "project".to_string()),
                    ),
                };
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: group.to_string(),
                });
            }
        }
        CompletionContext::Section => {
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let candidates = section_candidates(bob_dir, route, "")
                .map_err(|error| error.message.clone())?;
            let Candidates::Section(items) = candidates else {
                return Ok(None);
            };
            let group = format!("sections in {route_for_group}");
            for item in items {
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: format!("H{}", item.level),
                    group: group.clone(),
                });
            }
        }
        CompletionContext::Task => {
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let candidates = task_candidates(
                bob_dir,
                route,
                "",
                false,
                TaskSearch::MultiField,
            )
            .map_err(|error| error.message.clone())?;
            let Candidates::Task(items) = candidates else {
                return Ok(None);
            };
            let group = format!("tasks in {route_for_group}");
            for item in items {
                if item.requires_block_id || item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: group.clone(),
                });
            }
        }
        CompletionContext::TaskSection => {
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let (candidates, _) = task_section_candidates(
                bob_dir,
                route,
                field.block_id.as_deref(),
                "",
            )
            .map_err(|error| error.message.clone())?;
            let Candidates::TaskSection(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "task sections".to_string(),
                });
            }
        }
        CompletionContext::PomodoroName
        | CompletionContext::PomodoroStartName => {
            let (candidates, _) =
                if field.context == CompletionContext::PomodoroName {
                    pomodoro_name_candidates(bob_dir, "")
                        .map_err(|error| error.message.clone())?
                } else {
                    pomodoro_start_name_candidates(bob_dir, "")
                        .map_err(|error| error.message.clone())?
                };
            let Candidates::PomodoroName(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.requires_name || item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                let description = if item.creates_pomodoro {
                    "new Pomodoro".to_string()
                } else {
                    pomodoro_description(&item.time_range, &item.name)
                };
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: "open Pomodoros".to_string(),
                });
            }
        }
        CompletionContext::ActiveTask => {
            let (candidates, _) = active_task_candidates(bob_dir, "");
            let Candidates::ActiveTask(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "active tasks".to_string(),
                });
            }
        }
        CompletionContext::TaskDependency => {
            // Contract phase: the vault-wide prerequisite scan lands in
            // the discovery phase, so the shell offers no rows yet.
        }
        CompletionContext::TaskLink => {
            let (candidates, _) = task_link_candidates(bob_dir, "");
            let Candidates::TaskLink(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.requires_block_id || item.replacement.is_empty() {
                    continue;
                }
                // The `:` field covers the whole token including the sigil,
                // so `marker_prefix` is empty and the full value is the
                // `@route:id` candidate itself.
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "active tasks".to_string(),
                });
            }
        }
        CompletionContext::PomodoroBlockId => {
            // A `+` after a `:` block ID is the retired project-note
            // form, which offers nothing.
            if raw_text
                .get(field.replacement.1..)
                .is_some_and(|rest| rest.starts_with('+'))
            {
                return Ok(Some(ShellCompletion { marker_start, rows }));
            }
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            // Follow capture-complete's new-ID intent exactly as
            // `build_result` does: link intent (a solo `@route:` item)
            // offers existing linkable tasks, while new or project-note
            // intent offers the field's suggestions as new block IDs.
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
            if !matches!(
                block_field.intent,
                capture_block_ids::BlockIdIntent::Link
            ) {
                for suggestion in &block_field.suggestions {
                    if suggestion.is_empty() {
                        continue;
                    }
                    // Built like the link rows below: the kept marker
                    // prefix already carries `@<route>:`.
                    let full = format!("{marker_prefix}{suggestion}");
                    if full.is_empty() {
                        continue;
                    }
                    rows.push(ShellRow {
                        nospace: ends_in_continuation(&full),
                        full,
                        description: "new block ID".to_string(),
                        group: "new task ID".to_string(),
                    });
                }
                return Ok(Some(ShellCompletion { marker_start, rows }));
            }
            let (candidates, _) = link_candidates(bob_dir, route, "")
                .map_err(|error| error.message.clone())?;
            let Candidates::Task(items) = candidates else {
                return Ok(None);
            };
            let group = format!("tasks in {route_for_group}");
            for item in items {
                if item.requires_block_id || item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: group.clone(),
                });
            }
        }
        CompletionContext::TaskBlockId
        | CompletionContext::ProjectTaskBlockId => {
            // A new ID (`@route^id`), a project-note `+`, or a trailing
            // ` :id` / ` ^id`: no safe rows (suggestions live in the
            // additive `block_id` object, not candidates).
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock => return Ok(None),
    }
    Ok(Some(ShellCompletion { marker_start, rows }))
}

/// A value ending in a continuation character expects more typing.
pub(super) fn ends_in_continuation(value: &str) -> bool {
    value.ends_with([':', '+', '#', '=', '^'])
}

/// `time · name` for Pomodoro rows, mirroring the vault provider.
pub(super) fn pomodoro_description(
    time_range: &Option<String>,
    name: &Option<String>,
) -> String {
    match (time_range, name) {
        (Some(time), Some(name)) => format!("{time} \u{00B7} {name}"),
        (Some(time), None) => format!("{time} \u{00B7} open"),
        (None, Some(name)) => name.clone(),
        (None, None) => "open".to_string(),
    }
}

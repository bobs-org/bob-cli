use std::path::Path;

use super::{
    candidates::{
        active_task_candidates, blocked_task_candidates, link_candidates,
        project_task_move_candidates, relation_candidates,
        relation_label_suggestions, route_candidates, section_candidates,
        task_candidates, task_complete_candidates, task_link_candidates,
        task_parent_candidates, TaskSearch,
    },
    engine::CompleteRequest,
    model::{Candidates, RouteCandidateKind, SectionMenuRow},
    pomodoros::{apply_bare_start_name_replacements, pomodoro_candidates},
};
use crate::native::{
    capture, capture_block_ids, capture_completable_tasks,
    capture_language::{self, CompletionContext, RelationDirection},
    capture_links, capture_task_wikilinks,
};

use super::candidates::dependency_candidates;

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
    let template = crate::native::capture_language::template::analyze(raw_text)
        .directive
        .is_some();
    if raw_text.contains(['\n', '\r']) {
        return Ok(None);
    }
    if cursor > raw_text.len() || !raw_text.is_char_boundary(cursor) {
        return Ok(None);
    }
    let directive_owns_cursor =
        capture_language::completion_field_at(raw_text, cursor)
            .is_some_and(|field| field.context == CompletionContext::Directive);
    if template && !directive_owns_cursor {
        return Ok(None);
    }
    let record_request = CompleteRequest {
        bob_dir,
        raw_text,
        cursor,
        all_tasks: false,
        closed_tasks: false,
        next_missing_field: false,
        record_field: None,
    };
    if let Some(result) = super::records::try_complete(&record_request)
        .map_err(|error| error.message)?
        && let Some(completion) = shell_from_record_result(result)
    {
        return Ok(Some(completion));
    }
    // Wikilinks are deferred: return before the ~300 ms `NoteIndex::read`.
    let current_route = capture_language::editor_item_at(raw_text, cursor)
        .and_then(|item| item.route);
    let current_note_path = current_route
        .as_deref()
        .map(capture::route_label)
        .unwrap_or_else(|| capture::route_label(capture::inbox_route()));
    if capture_links::completion_field_at(
        raw_text,
        cursor,
        current_note_path.clone(),
    )
    .is_some()
    {
        return Ok(None);
    }
    let type_owns_cursor =
        capture_language::task_type_completion_at(raw_text, cursor).is_some();
    if !type_owns_cursor
        && capture_task_wikilinks::completion_field_at(
            raw_text,
            cursor,
            &current_note_path,
        )
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
    // Most slots serve the full set so shell matcher-list keeps working.
    // Pomodoro names use PT7 query-aware caps and lossless replacements.
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
                    RouteCandidateKind::Inbox => (
                        "inbox",
                        "inbox \u{00B7} default capture target".to_string(),
                    ),
                    RouteCandidateKind::Area => ("areas", "area".to_string()),
                    RouteCandidateKind::Project => (
                        "projects",
                        item.status
                            .as_deref()
                            .map(|status| format!("project \u{00B7} {status}"))
                            .unwrap_or_else(|| "project".to_string()),
                    ),
                    RouteCandidateKind::Record => {
                        ("records", "record folder".to_string())
                    }
                    RouteCandidateKind::Notebook => {
                        let title =
                            item.title.as_deref().unwrap_or(&item.route);
                        let trail = item
                            .breadcrumb
                            .as_deref()
                            .map(|crumbs| crumbs.join(" › "))
                            .filter(|trail| !trail.is_empty());
                        let mut description = match trail {
                            Some(trail) => format!("📓 {title} · {trail}"),
                            None => format!("📓 {title}"),
                        };
                        if let Some(summary) = item.summary.as_deref()
                            && !summary.is_empty()
                        {
                            description.push_str(&format!(" · {summary}"));
                        }
                        ("notebooks", description)
                    }
                };
                let full = if item.kind == RouteCandidateKind::Record {
                    format!("{}{}", marker_prefix, item.replacement.trim_end())
                } else {
                    full
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
            if field.query.is_empty()
                && let Some(create) = record_section_create_shell_row(
                    bob_dir,
                    raw_text,
                    route,
                    marker_start,
                )
            {
                rows.push(create);
            }
            for item in items {
                let SectionMenuRow::Section(item) = item else {
                    continue;
                };
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
            let mut candidates = task_candidates(
                bob_dir,
                route,
                "",
                false,
                TaskSearch::MultiField,
            )
            .map_err(|error| error.message.clone())?;
            if let Some(item) =
                capture_language::editor_item_at(raw_text, cursor)
            {
                super::candidates::apply_relationship_picker(
                    bob_dir,
                    &mut candidates,
                    &item,
                );
            }
            let Candidates::Task(items) = candidates else {
                return Ok(None);
            };
            let group = format!("tasks in {route_for_group}");
            for item in items {
                if item.requires_block_id
                    || item.disabled_reason.is_some()
                    || item.replacement.is_empty()
                {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                let description = match item.consequence.as_deref() {
                    Some(consequence) => {
                        format!("{}  · {consequence}", item.text)
                    }
                    None => item.text.clone(),
                };
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: group.clone(),
                });
            }
        }
        CompletionContext::Drawer => {
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let (candidates, _) = super::drawers::target_drawer_candidates(
                bob_dir,
                route,
                field.block_id.as_deref(),
            )
            .map_err(|error| error.message.clone())?;
            let Candidates::Drawer(items) = candidates else {
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
                    group: "drawers".to_string(),
                });
            }
        }
        // Shell completion serves one-line drafts only, so the lexical
        // field never yields a section bullet; the arm exists for
        // exhaustiveness.
        CompletionContext::NotebookSectionBullet => {
            return Ok(None);
        }
        CompletionContext::PomodoroName
        | CompletionContext::PomodoroStartName
        | CompletionContext::PomodoroPastName => {
            let (mut candidates, _) = if field.context
                == CompletionContext::PomodoroName
            {
                let route = field.route.as_deref().expect("route resolved");
                let (candidates, warnings, _) =
                    super::pomodoros::pomodoro_name_candidates_for_task(
                        bob_dir,
                        &field.query,
                        raw_text[field.replacement.0..].starts_with(['"', '“']),
                        route,
                        field.block_id.as_deref(),
                        capture_language::editor_item_at(raw_text, cursor)
                            .is_some_and(|item| !item.body.trim().is_empty()),
                    )
                    .map_err(|error| error.message.clone())?;
                (candidates, warnings)
            } else if field.context == CompletionContext::PomodoroPastName {
                super::pomodoros::pomodoro_past_candidates(
                    bob_dir,
                    &field.query,
                    raw_text[field.replacement.0..].starts_with(['"', '“']),
                )
                .map_err(|error| error.message.clone())?
            } else {
                pomodoro_candidates(
                    bob_dir,
                    &field.query,
                    true,
                    raw_text[field.replacement.0..].starts_with(['"', '“']),
                )
                .map_err(|error| error.message.clone())?
            };
            if field.bare_name {
                let replaced = raw_text
                    .get(field.replacement.0..field.replacement.1)
                    .unwrap_or("");
                apply_bare_start_name_replacements(&mut candidates, replaced);
            }
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
                let description = if item.suggested {
                    format!(
                        "suggested · {}",
                        item.suggestion_label.as_deref().unwrap_or("task type")
                    )
                } else if item.origin == Some("history") {
                    if item.last_used.is_none() {
                        "your type · never used".to_string()
                    } else {
                        let date = item
                            .last_used
                            .as_deref()
                            .and_then(|date| {
                                chrono::NaiveDate::parse_from_str(
                                    date, "%Y-%m-%d",
                                )
                                .ok()
                            })
                            .map(|date| date.format("%b %-d").to_string())
                            .unwrap_or_default();
                        let days = item.days_used.unwrap_or_default();
                        format!(
                            "recent · {date} · {days} {}",
                            if days == 1 { "day" } else { "days" }
                        )
                    }
                } else if item.creates_pomodoro {
                    if item
                        .themes
                        .iter()
                        .any(|theme| theme.configured == Some(false))
                    {
                        "new · not one of your types".to_string()
                    } else {
                        "new Pomodoro".to_string()
                    }
                } else {
                    pomodoro_description(&item.time_range, &item.name)
                };
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: if item.suggested {
                        "Suggested"
                    } else if item.origin == Some("history") {
                        "Recent"
                    } else {
                        "open Pomodoros"
                    }
                    .to_string(),
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
            // Safe rows only: identified, unguarded prerequisites the
            // shell can insert verbatim. ID-less and guarded rows stay
            // in the interactive picker, which owns the Add block ID
            // flow.
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
            let (candidates, _) = dependency_candidates(
                bob_dir,
                "",
                item.as_ref()
                    .and_then(|found| found.dependency_target.as_ref()),
                &typed,
            );
            let Candidates::Dependency(items) = candidates else {
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
                    group: "prerequisite tasks".to_string(),
                });
            }
        }
        CompletionContext::BlockedTask => {
            let item = capture_language::editor_item_at(raw_text, cursor);
            let typed_dependencies: Vec<(String, String)> = item
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
            let typed_blocked: Vec<(String, String)> = item
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
            let owner_note = item
                .as_ref()
                .and_then(|found| found.route.as_deref())
                .or_else(|| {
                    item.as_ref()
                        .and_then(|found| found.dependency_target.as_ref())
                        .and_then(|target| target.route.as_deref())
                })
                .unwrap_or_else(|| capture::inbox_route());
            let (candidates, _) = blocked_task_candidates(
                bob_dir,
                "",
                item.as_ref()
                    .and_then(|found| found.dependency_target.as_ref()),
                owner_note,
                &typed_dependencies,
                &typed_blocked,
            );
            let Candidates::BlockedTask(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{marker_prefix}{}", item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "blocked tasks".to_string(),
                });
            }
        }
        CompletionContext::RelationTo | CompletionContext::RelationFrom => {
            let item = capture_language::editor_item_at(raw_text, cursor);
            let owner_note = item
                .as_ref()
                .and_then(|found| found.route.as_deref())
                .or_else(|| {
                    item.as_ref()
                        .and_then(|found| found.dependency_target.as_ref())
                        .and_then(|target| target.route.as_deref())
                })
                .unwrap_or_else(|| capture::inbox_route());
            let (candidates, _) = relation_candidates(
                bob_dir,
                "",
                if field.context == CompletionContext::RelationTo {
                    RelationDirection::To
                } else {
                    RelationDirection::From
                },
                item.as_ref()
                    .and_then(|found| found.dependency_target.as_ref()),
                &capture::route_label(owner_note),
                item.as_ref()
                    .map(|found| found.relations.as_slice())
                    .unwrap_or(&[]),
                item.as_ref()
                    .map(|found| found.dependencies.as_slice())
                    .unwrap_or(&[]),
            );
            let Candidates::Relation(items) = candidates else {
                return Ok(None);
            };
            let group = if field.context == CompletionContext::RelationTo {
                "links to"
            } else {
                "linked from"
            };
            for item in items {
                if item.replacement.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&item.replacement),
                    full: format!("{marker_prefix}{}", item.replacement),
                    description: item.text,
                    group: group.to_string(),
                });
            }
        }
        CompletionContext::RelationLabel => {
            // A `||` probe keeps its pipe count in every row, so the
            // shell inserts the one-off spelling the draft typed.
            let one_off = capture_language::relation_label_token_at(
                raw_text,
                field.replacement.0,
            )
            .is_some_and(|(_, _, hidden)| hidden);
            let policy = super::cancel::load_policy();
            let (candidates, _, _) = relation_label_suggestions(
                bob_dir,
                &field.query,
                one_off,
                &policy,
            );
            let Candidates::RelationLabel(items) = candidates else {
                return Ok(None);
            };
            let last_sigil = raw_text
                .get(..field.replacement.0)
                .and_then(|prefix| prefix.rfind('~'))
                .unwrap_or(field.replacement.0);
            let relation_start = if last_sigil > 0
                && raw_text.as_bytes().get(last_sigil - 1) == Some(&b'~')
            {
                last_sigil - 1
            } else {
                last_sigil
            };
            // A refused cancel source disables its cancel-label rows
            // here too, so zsh never inserts a label that would refuse.
            let cancel_refused = if policy.cancel_labels.is_empty() {
                false
            } else {
                capture_language::editor_item_at(raw_text, cursor)
                    .and_then(|editor_item| {
                        super::cancel::plan_relation_cancel(
                            bob_dir,
                            raw_text,
                            field.replacement.0,
                            Some(&editor_item),
                            &field.query,
                            &policy,
                        )
                    })
                    .is_some_and(|plan| plan.row_disable.is_some())
            };
            let complete_refused = if policy.complete_labels.is_empty() {
                false
            } else {
                capture_language::editor_item_at(raw_text, cursor)
                    .and_then(|editor_item| {
                        super::complete::plan_relation_complete(
                            bob_dir,
                            raw_text,
                            field.replacement.0,
                            Some(&editor_item),
                            &field.query,
                            &policy,
                        )
                    })
                    .is_some_and(|plan| plan.row_disable.is_some())
            };
            let merge_refused = if policy.merge_labels.is_empty() {
                false
            } else {
                capture_language::editor_item_at(raw_text, cursor)
                    .and_then(|editor_item| {
                        super::merge::plan_relation_merge(
                            bob_dir,
                            raw_text,
                            field.replacement.0,
                            Some(&editor_item),
                            &field.query,
                            &policy,
                        )
                    })
                    .is_some_and(|plan| plan.row_disable.is_some())
            };
            for item in items {
                if item.disabled_reason.is_some() || item.replacement.is_empty()
                {
                    continue;
                }
                if cancel_refused && item.cancels_source {
                    continue;
                }
                if complete_refused && item.completes_source {
                    continue;
                }
                if merge_refused && item.merges_source {
                    continue;
                }
                let Some(target_prefix) =
                    raw_text.get(relation_start..field.replacement.0)
                else {
                    continue;
                };
                let full = format!("{target_prefix}{}", item.replacement);
                let mut description =
                    format!("{} links", item.count.unwrap_or_default());
                if item.cancels_source {
                    description.push_str(" · cancels source");
                }
                if item.completes_source {
                    description.push_str(" · completes source");
                }
                if item.merges_source {
                    description.push_str(" · merges source");
                }
                if item.offers_source {
                    description.push_str(" · 🪢 offers");
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: "relation labels".to_string(),
                });
            }
            return Ok(Some(ShellCompletion {
                marker_start: relation_start,
                rows,
            }));
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
        CompletionContext::TaskGoto => {
            // Nothing is insertable: Go To rows open the task instead of
            // rewriting the draft, so the shell serves no rows.
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::TaskParent => {
            let (mut candidates, _) = task_parent_candidates(bob_dir, "");
            if let Some(item) =
                capture_language::editor_item_at(raw_text, cursor)
            {
                super::candidates::apply_relationship_picker(
                    bob_dir,
                    &mut candidates,
                    &item,
                );
            }
            let Candidates::TaskParent(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.requires_block_id
                    || item.disabled_reason.is_some()
                    || item.replacement.is_empty()
                {
                    continue;
                }
                let full = format!("{marker_prefix}{}", item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "parent tasks".to_string(),
                });
            }
        }
        CompletionContext::ProjectTaskMove => {
            let detail = capture_language::project_task_move_detail(
                raw_text,
                cursor,
                field.replacement,
            )
            .ok_or_else(|| {
                "invalid project_task_move completion field".to_string()
            })?;
            let built = project_task_move_candidates(bob_dir, &detail, "");
            for item in built.rows {
                if item.requires_block_id
                    || item.disabled_reason.is_some()
                    || item.replacement.is_empty()
                {
                    continue;
                }
                let full = format!("{marker_prefix}{}", item.replacement);
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text,
                    group: "project task moves".to_string(),
                });
            }
        }
        CompletionContext::TaskComplete => {
            // Safe rows only: identified, enabled completions the
            // shell can insert verbatim, today rows first. Guarded
            // (recurring, already-selected) and ID-less rows stay in
            // the interactive picker, which owns the Add block ID
            // flow. The shell filters, so the full empty-query order
            // is served here.
            let (candidates, _) =
                task_complete_candidates(bob_dir, "", raw_text, cursor);
            let Candidates::TaskComplete(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.replacement.is_empty() {
                    continue;
                }
                // Values never carry the `!` sigil itself: a leading
                // `!` would parse as a protocol directive and the
                // adapters would drop the row. The marker starts past
                // the sigil instead, so the presenter keeps the sigil
                // (plus any leading whitespace) with `!prefix` and the
                // inserted text is still the full `!note:block-id`.
                // The word prefix stays out of the value: the kept
                // slice already covers everything before the marker.
                let full = item
                    .replacement
                    .strip_prefix('!')
                    .unwrap_or(item.replacement.as_str())
                    .to_string();
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: capture_completable_tasks::group_label(item.group)
                        .to_string(),
                });
            }
            return Ok(Some(ShellCompletion {
                marker_start: field.replacement.0 + 1,
                rows,
            }));
        }
        CompletionContext::TaskType => {
            let editor_item =
                capture_language::editor_item_at(raw_text, cursor);
            let context = super::task_types::build_for_shell(
                bob_dir,
                raw_text,
                &field,
                editor_item.as_ref(),
            )
            .map_err(|error| error.message)?;
            for item in context.candidates {
                if item.refused.is_some() {
                    continue;
                }
                let full = item.replacement.trim_end().to_string();
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: false,
                    full,
                    description: task_type_description(&item),
                    group: "task types".to_string(),
                });
            }
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::Directive => {
            let context = super::directives::candidates(
                bob_dir,
                raw_text,
                &field,
                capture_language::editor_item_at(raw_text, cursor).as_ref(),
            )
            .map_err(|error| error.message)?;
            for item in context {
                if item.disabled_reason.is_some() || item.replacement.is_empty()
                {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: false,
                    full: item.replacement.trim_end().to_string(),
                    description: item
                        .detail
                        .map(|detail| format!("{} · {detail}", item.summary))
                        .unwrap_or_else(|| item.summary.to_string()),
                    group: "directives".to_string(),
                });
            }
            return Ok(Some(ShellCompletion { marker_start, rows }));
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
            let notebook_policy =
                crate::native::capture_notebooks::notebook_policy_for_draft(
                    bob_dir, raw_text,
                );
            let block_field =
                capture_block_ids::build_block_id_field_with_notebooks(
                    bob_dir,
                    raw_text,
                    cursor,
                    &capture_block_ids::BlockIdRequest {
                        route,
                        replacement: field.replacement,
                        context: field.context,
                    },
                    Some(&notebook_policy),
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
            // A new ID (`@route^id`), a project-note `@` seal, or a
            // trailing ` :id` / ` ^id`: no safe rows (suggestions live in
            // the additive `block_id` object, not candidates).
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::PomodoroSplitBase => {
            let (candidates, _) =
                super::pomodoros::pomodoro_split_base_candidates(
                    bob_dir,
                    &field.query,
                )
                .map_err(|error| error.message.clone())?;
            let Candidates::PomodoroName(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                let full = format!("{marker_prefix}{}", item.replacement);
                if full.is_empty() {
                    continue;
                }
                let description = item.name.unwrap_or(item.replacement.clone());
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: "pomodoro".to_string(),
                });
            }
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::PomodoroSplitType => {
            let (candidates, _) =
                super::pomodoros::pomodoro_split_type_candidates(
                    bob_dir,
                    raw_text,
                    cursor,
                    &field.query,
                )
                .map_err(|error| error.message.clone())?;
            let Candidates::PomodoroName(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                let full = format!("{marker_prefix}{}", item.replacement);
                if full.is_empty() {
                    continue;
                }
                let description = item.name.unwrap_or(item.replacement.clone());
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: "pomodoro".to_string(),
                });
            }
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::PomodoroMergeSrc => {
            let (candidates, _) =
                super::pomodoros::pomodoro_merge_src_candidates(
                    bob_dir,
                    &field.query,
                )
                .map_err(|error| error.message.clone())?;
            let Candidates::PomodoroName(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                let full = format!("{marker_prefix}{}", item.replacement);
                if full.is_empty() {
                    continue;
                }
                let description = item.name.unwrap_or(item.replacement.clone());
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: "pomodoro".to_string(),
                });
            }
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::PomodoroMergeDest => {
            let (candidates, _) =
                super::pomodoros::pomodoro_merge_dest_candidates(
                    bob_dir,
                    raw_text,
                    cursor,
                    &field.query,
                )
                .map_err(|error| error.message.clone())?;
            let Candidates::PomodoroName(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                let full = format!("{marker_prefix}{}", item.replacement);
                if full.is_empty() {
                    continue;
                }
                let description = item.name.unwrap_or(item.replacement.clone());
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: "pomodoro".to_string(),
                });
            }
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::PomodoroPastSegment => {
            let (candidates, _) =
                super::pomodoros::pomodoro_past_segment_candidates(
                    bob_dir,
                    &field.query,
                )
                .map_err(|error| error.message.clone())?;
            let Candidates::PastSegment(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                let full = format!("{marker_prefix}{}", item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.label.clone(),
                    group: "segments".to_string(),
                });
            }
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock
        | CompletionContext::WikilinkTask
        | CompletionContext::RecordTarget
        | CompletionContext::RecordField
        | CompletionContext::RecordValue => return Ok(None),
    }
    Ok(Some(ShellCompletion { marker_start, rows }))
}

/// A value ending in a continuation character expects more typing.
pub(super) fn ends_in_continuation(value: &str) -> bool {
    value.ends_with([':', '+', '#', '=', '^', '@'])
}

fn shell_from_record_result(
    result: super::model::CaptureCompleteResult,
) -> Option<ShellCompletion> {
    let marker_start = result.replacement.start;
    let mut rows = Vec::new();
    match result.candidates {
        Candidates::RecordTarget(items) => {
            for item in items {
                let mut description = item.title;
                if let Some(status) = item.status {
                    description.push_str(" · ");
                    description.push_str(&status);
                }
                rows.push(ShellRow {
                    nospace: false,
                    full: item.replacement,
                    description,
                    group: "record targets".to_string(),
                });
            }
        }
        Candidates::RecordField(items) => {
            for item in items {
                rows.push(ShellRow {
                    nospace: ends_in_continuation(item.replacement.trim_end()),
                    full: item.replacement,
                    description: item.label,
                    group: "record fields".to_string(),
                });
            }
        }
        Candidates::RecordValue(items) => {
            for item in items {
                rows.push(ShellRow {
                    nospace: ends_in_continuation(item.replacement.trim_end()),
                    full: item.replacement,
                    description: item.display,
                    group: "record values".to_string(),
                });
            }
        }
        _ => return None,
    }
    Some(ShellCompletion { marker_start, rows })
}

fn record_section_create_shell_row(
    bob_dir: &Path,
    raw_text: &str,
    route: &str,
    marker_start: usize,
) -> Option<ShellRow> {
    let dir = route.trim_end_matches('/');
    let resolution =
        crate::native::records::context::RecordContext::resolve_route_head(
            bob_dir, dir, None,
        )
        .ok()?;
    let crate::native::records::context::RecordKindResolution::Valid {
        kind,
        ..
    } = resolution
    else {
        return None;
    };
    let record_type = kind.record_type();
    if marker_start > 0
        && raw_text.as_bytes().get(marker_start - 1) == Some(&b'@')
    {
        return None;
    }
    let full = format!("@{}/", record_type.dir);
    Some(ShellRow {
        nospace: ends_in_continuation(&full),
        full,
        description: format!("new {}", record_type.label.to_lowercase()),
        group: "records".to_string(),
    })
}

fn task_type_description(item: &super::model::TaskTypeCandidate) -> String {
    let mut description = if item.origin == "typed" {
        "new · not one of your types".to_string()
    } else {
        match (item.last_used.as_deref(), item.days_used) {
            (Some(date), Some(days)) => {
                let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map(|date| date.format("%b %-d").to_string())
                    .unwrap_or_else(|_| date.to_string());
                format!("your type · {date} · {days} days")
            }
            _ => "your type · never used".to_string(),
        }
    };
    if item.current {
        description.push_str(" · current · this task");
    } else if item.inherited
        && let Some(label) = item.suggestion_label.as_deref()
    {
        description.push_str(&format!(" · {label}"));
    }
    description
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

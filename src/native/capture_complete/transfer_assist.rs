//! Transfer-assist candidate scoping for `>` lines.
//!
//! The completion grammar routes transfer components through the existing
//! `Route`, `Task`, `Drawer`, and `Section` contexts; this module applies
//! the transfer-specific scoping the plan requires on top of those rows:
//! sources narrow to movable-or-mergeable tasks, merge targets drop the
//! typed sources and carry a Bob-authored merge consequence, a complete
//! bare destination gains the `+ merge into a task` hint, and sections
//! gain a creation row plus the §3.4 near-duplicate refusal. Everything
//! else reuses the ordinary rows unchanged.

use std::collections::HashSet;
use std::path::Path;

use super::model::{Candidates, CompleteError};
use crate::native::{
    capture, capture_drawers, capture_language, capture_project_note,
    note_tasks, task_complete, task_move,
};

/// Narrow `candidates` (and extend `warnings`) for `role` on a `>`
/// transfer line. Positions the plan leaves alone keep their rows.
pub(super) fn apply(
    bob_dir: &Path,
    field: &capture_language::CompletionField,
    role: capture_language::TransferCompletionRole,
    candidates: &mut Candidates,
    warnings: &mut Vec<String>,
) -> Result<(), CompleteError> {
    use capture_language::CompletionContext;
    use capture_language::TransferCompletionRole as Role;
    match role {
        Role::Source { route } => {
            if matches!(field.context, CompletionContext::Task) {
                filter_source_tasks(bob_dir, &route, candidates)?;
            }
        }
        Role::DestinationRoute => {}
        Role::DestinationNote { route } => {
            push_merge_hint(&route, warnings);
        }
        Role::MergeTarget { route, exclude } => {
            if matches!(field.context, CompletionContext::Task) {
                scope_merge_targets(&route, &exclude, candidates);
            }
        }
        Role::Drawer { .. } => {}
        Role::Section { route } => {
            if matches!(field.context, CompletionContext::Section) {
                scope_transfer_sections(
                    bob_dir,
                    &route,
                    &field.query,
                    candidates,
                    warnings,
                )?;
            }
        }
    }
    Ok(())
}

/// Keep only tasks the transfer can actually name: tasks that are movable
/// or mergeable, addressed by block ID. ID-less rows cannot form
/// `@route+id` sources, so they never survive here.
fn filter_source_tasks(
    bob_dir: &Path,
    route: &str,
    candidates: &mut Candidates,
) -> Result<(), CompleteError> {
    let Candidates::Task(items) = candidates else {
        return Ok(());
    };
    let relative = capture::route_label(route);
    let path = bob_dir.join(&relative);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => {
            items.clear();
            return Ok(());
        }
    };
    // `movable_roots` lists every column-0 open task and marks the
    // ineligible ones (`^prj`, duplicates, bad fragments) with a disabled
    // reason; only the unmarked rows can move.
    let movable: HashSet<String> =
        task_move::movable_roots(Path::new(&relative), &contents)
            .into_iter()
            .filter(|root| root.disabled_reason.is_none())
            .filter_map(|root| root.block_id)
            .collect();
    let mergeable = mergeable_source_ids(&contents, bob_dir);
    items.retain(|item| {
        if item.requires_block_id {
            return false;
        }
        let Some(id) = item.block_id.as_deref() else {
            return false;
        };
        movable.contains(id) || mergeable.contains(id)
    });
    Ok(())
}

/// Block IDs in `contents` a merge could retire: open, non-project,
/// non-recurring tasks with no open nested tasks. Mirrors the merge
/// planner's source refusals without moving anything.
fn mergeable_source_ids(contents: &str, bob_dir: &Path) -> HashSet<String> {
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(contents, &settings);
    let open: Vec<&note_tasks::NoteTask> = scan.open_tasks().collect();
    let spans = capture::line_spans(contents);
    let lines: Vec<&str> = contents.lines().collect();
    let mut ids = HashSet::new();
    for task in &open {
        let Some(id) = task.block_id.clone() else {
            continue;
        };
        if id == "prj" {
            continue;
        }
        if let Some(line) = lines.get(task.line_index)
            && task_complete::is_recurring_task_line(line)
        {
            continue;
        }
        // An open subtask is any open task inside this task's block range:
        // the same rule the merge planner refuses on.
        let nested_open = open.iter().any(|other| {
            other.line_index > task.line_index
                && spans
                    .get(other.line_index)
                    .is_some_and(|span| span.end <= task.block_end)
        });
        if nested_open {
            continue;
        }
        ids.insert(id);
    }
    ids
}

/// Merge targets are the destination note's open tasks minus the typed
/// sources, each labelled with what accepting does.
fn scope_merge_targets(
    route: &str,
    exclude: &[(String, String)],
    candidates: &mut Candidates,
) {
    let Candidates::Task(items) = candidates else {
        return;
    };
    let excluded: HashSet<&str> = exclude
        .iter()
        .filter(|(source_route, _)| source_route.eq_ignore_ascii_case(route))
        .map(|(_, id)| id.as_str())
        .collect();
    items.retain(|item| {
        if item.requires_block_id {
            return false;
        }
        let Some(id) = item.block_id.as_deref() else {
            return false;
        };
        !excluded.contains(id)
    });
    for item in items.iter_mut() {
        let id = item.block_id.as_deref().unwrap_or_default();
        item.consequence = Some(format!(
            "⤵ merge into [[{route}#^{id}]] — cuts the source notes under this task and retires the source",
        ));
    }
}

/// The `+ merge into a task` hint on a complete bare destination.
fn push_merge_hint(route: &str, warnings: &mut Vec<String>) {
    let hint = format!(
        "type + to merge into a task in {route} — @…+id merges, @…#section merges into a section",
    );
    if !warnings.contains(&hint) {
        warnings.push(hint);
    }
}

/// Section rows for a transfer destination: the note's eligible headings
/// (already ranked), then a creation row for an unmatched name, or the
/// §3.4 near-duplicate refusal when the name only almost matches. Section
/// completion has no vault-wide catalog, so there are no recents.
fn scope_transfer_sections(
    bob_dir: &Path,
    route: &str,
    query: &str,
    candidates: &mut Candidates,
    warnings: &mut Vec<String>,
) -> Result<(), CompleteError> {
    let Candidates::Section(items) = candidates else {
        return Ok(());
    };
    if query.is_empty() {
        return Ok(());
    }
    let slug = capture_language::selector_slug(query);
    let exact = items.iter().any(|item| {
        item.title.eq_ignore_ascii_case(query)
            || item.replacement == *query
            || item.replacement == slug
    });
    if exact {
        return Ok(());
    }
    let contents = match std::fs::read_to_string(
        bob_dir.join(capture::route_label(route)),
    ) {
        Ok(contents) => contents,
        Err(_) => return Ok(()),
    };
    let drawers = capture_drawers::project_h2_drawers(&contents);
    if let Some(near) =
        capture_drawers::find_near_duplicate_drawer(&drawers, query)
    {
        let refusal = format!("did you mean `## {}`?", near.title);
        if !warnings.contains(&refusal) {
            warnings.push(refusal);
        }
        return Ok(());
    }
    let normalized =
        capture_language::normalize_task_text(&query.replace('-', " "));
    let title = capture_project_note::format_project_section_title(&normalized);
    if title.is_empty() {
        return Ok(());
    }
    let key = capture_drawers::drawer_key(&title);
    if capture_drawers::is_reserved_drawer_key(&key)
        || capture_drawers::is_structural_h2_key(&key)
    {
        let refusal = format!("`{title}` is a reserved section heading");
        if !warnings.contains(&refusal) {
            warnings.push(refusal);
        }
        return Ok(());
    }
    items.push(super::model::SectionCandidate {
        replacement: section_creation_selector(&title),
        title,
        level: 2,
    });
    Ok(())
}

/// The selector a new `##` title is typed with: the raw title when it is
/// one unquoted token, else the slug, else a quoted literal.
fn section_creation_selector(title: &str) -> String {
    if !title.is_empty() && !title.contains([' ', '\t', '"', '“', '#', '\\'])
    {
        return title.to_string();
    }
    let slug = capture_language::selector_slug(title);
    if !slug.is_empty() {
        return slug;
    }
    format!("\"{}\"", title.replace('\\', "\\\\").replace('"', "\\\""))
}

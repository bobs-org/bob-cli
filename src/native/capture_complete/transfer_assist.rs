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

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::model::{Candidates, CompleteError};
use crate::native::{
    capture, capture_drawers, capture_language, capture_project_note,
    note_tasks, task_children, task_merge, task_move,
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
        Role::VaultMergeTarget { exclude } => {
            if matches!(field.context, CompletionContext::TaskParent) {
                scope_vault_merge_targets(&exclude, candidates);
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

/// Scope a `<` source picker to mergeable tasks, retaining ID-less rows for
/// the explicit Add block ID flow and attaching Bob's moved-note detail.
pub(super) fn apply_merge_in(
    bob_dir: &Path,
    field: &capture_language::CompletionField,
    role: capture_language::MergeInCompletionRole,
    candidates: &mut Candidates,
) -> Result<(), CompleteError> {
    use capture_language::CompletionContext;
    match field.context {
        CompletionContext::Task => {
            if let Some(route) = field.route.as_deref() {
                scope_note_merge_in_sources(
                    bob_dir,
                    route,
                    &role.exclude,
                    candidates,
                )?;
            }
        }
        CompletionContext::TaskParent => {
            scope_vault_merge_in_sources(bob_dir, &role.exclude, candidates);
        }
        _ => {}
    }
    Ok(())
}

fn scope_note_merge_in_sources(
    bob_dir: &Path,
    route: &str,
    exclude: &[(String, String)],
    candidates: &mut Candidates,
) -> Result<(), CompleteError> {
    let Candidates::Task(items) = candidates else {
        return Ok(());
    };
    let relative = capture::route_label(route);
    let contents = super::candidates::read_target(bob_dir, route)?;
    let details = mergeable_source_details(&contents, bob_dir);
    let route_paths = route_alias_paths(bob_dir);
    let candidate_path = route_paths
        .get(&route.to_ascii_lowercase())
        .cloned()
        .unwrap_or(relative);
    items.retain_mut(|item| {
        let Some(moved_notes) = details.get(&item.line).copied() else {
            return false;
        };
        let excluded = item.block_id.as_deref().is_some_and(|id| {
            exclude.iter().any(|(source_route, source_id)| {
                id == source_id
                    && route_path(source_route, &route_paths)
                        .eq_ignore_ascii_case(&candidate_path)
            })
        });
        if excluded {
            return false;
        }
        item.detail = Some(merge_in_detail(moved_notes));
        true
    });
    Ok(())
}

fn scope_vault_merge_in_sources(
    bob_dir: &Path,
    exclude: &[(String, String)],
    candidates: &mut Candidates,
) {
    let Candidates::TaskParent(items) = candidates else {
        return;
    };
    let route_paths = route_alias_paths(bob_dir);
    let mut note_details = HashMap::<String, HashMap<usize, usize>>::new();
    let mut seen = HashSet::<(String, usize)>::new();
    items.retain_mut(|item| {
        let normalized_path = item.note_path.to_ascii_lowercase();
        let details = note_details
            .entry(item.note_path.clone())
            .or_insert_with(|| {
                let path = bob_dir.join(&item.note_path);
                std::fs::read_to_string(path)
                    .map(|contents| {
                        mergeable_source_details(&contents, bob_dir)
                    })
                    .unwrap_or_default()
            });
        let Some(moved_notes) = details.get(&item.line).copied() else {
            return false;
        };
        if !seen.insert((normalized_path.clone(), item.line)) {
            return false;
        }
        let excluded = item.block_id.as_deref().is_some_and(|id| {
            exclude.iter().any(|(source_route, source_id)| {
                id == source_id
                    && route_path(source_route, &route_paths)
                        .eq_ignore_ascii_case(&normalized_path)
            })
        });
        if excluded {
            return false;
        }
        item.detail = Some(merge_in_detail(moved_notes));
        true
    });
}

fn route_alias_paths(bob_dir: &Path) -> HashMap<String, String> {
    let report = crate::native::capture_targets::scan_capture_targets(bob_dir);
    let mut paths = HashMap::new();
    for target in report.targets {
        if matches!(
            target.kind,
            crate::native::capture_targets::CaptureTargetKind::Record
                | crate::native::capture_targets::CaptureTargetKind::Notebook
        ) {
            continue;
        }
        paths.insert(
            target.route.to_ascii_lowercase(),
            target.relative_path.clone(),
        );
        for alias in target.project_name_aliases {
            paths.insert(
                alias.to_ascii_lowercase(),
                target.relative_path.clone(),
            );
        }
    }
    paths
}

fn route_path(route: &str, aliases: &HashMap<String, String>) -> String {
    aliases
        .get(&route.to_ascii_lowercase())
        .cloned()
        .unwrap_or_else(|| capture::route_label(route))
}

fn merge_in_detail(moved_notes: usize) -> String {
    if moved_notes == 0 {
        "⤵ merge in · title only".to_string()
    } else {
        let unit = if moved_notes == 1 { "note" } else { "notes" };
        format!("⤵ merge in · {moved_notes} {unit}")
    }
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
    // Movable roots already expose ID-less rows; their line numbers let
    // completion keep those rows for the explicit Add block ID flow.
    let movable: HashSet<usize> =
        task_move::movable_roots(Path::new(&relative), &contents)
            .into_iter()
            .filter(|root| root.disabled_reason.is_none())
            .map(|root| root.line)
            .collect();
    let mergeable = mergeable_source_details(&contents, bob_dir);
    items.retain(|item| {
        movable.contains(&item.line) || mergeable.contains_key(&item.line)
    });
    Ok(())
}

/// Mergeable task lines and the number of notes each merge would move.
/// Open rows pass the task-merge planner's own recurring/project/subtask
/// source refusals; the payload count stages no writes.
fn mergeable_source_details(
    contents: &str,
    bob_dir: &Path,
) -> HashMap<usize, usize> {
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(contents, &settings);
    let open: Vec<&note_tasks::NoteTask> = scan.open_tasks().collect();
    let spans = capture::line_spans(contents);
    let mut id_counts = HashMap::<&str, usize>::new();
    for task in &open {
        if let Some(id) = task.block_id.as_deref() {
            *id_counts.entry(id).or_default() += 1;
        }
    }
    let mut details = HashMap::new();
    for task in &open {
        if task.block_id.as_deref().is_some_and(|id| {
            id_counts.get(id).copied().unwrap_or_default() > 1
        }) {
            continue;
        }
        let source_line = spans
            .get(task.line_index)
            .map(|span| span.text)
            .unwrap_or_default();
        let block_id = task.block_id.as_deref().unwrap_or_default();
        if task_merge::source_identity_refusal(source_line, task, block_id)
            .is_some()
            || task_merge::open_subtask_count(scan.tasks(), &spans, task) > 0
        {
            continue;
        }
        let moved_notes = moved_note_count(contents, task, &settings);
        details.insert(task.line_index + 1, moved_notes);
    }
    details
}

/// Count the authored groups the merge payload planner would append after
/// the source title. Managed links/logs are pruned; body text left outside
/// those groups moves as one note fragment.
fn moved_note_count(
    contents: &str,
    task: &note_tasks::NoteTask,
    settings: &note_tasks::NoteTaskSettings,
) -> usize {
    let children =
        task_children::classify_direct_children(contents, task, settings);
    let authored = children
        .iter()
        .filter(|child| !child.kind.is_managed())
        .count();
    if authored > 0 {
        return authored;
    }

    let lines = capture::line_spans(contents);
    let end_line = lines
        .iter()
        .position(|line| line.end > task.block_end)
        .unwrap_or(lines.len());
    let mut cursor = task.line_index + 1;
    for (offset, child) in children.iter().enumerate() {
        if !child.kind.is_managed() {
            continue;
        }
        let end = children
            .get(offset + 1)
            .map(|next| next.line_index)
            .unwrap_or(end_line);
        if cursor < child.line_index
            && lines[cursor..child.line_index]
                .iter()
                .any(|line| !line.text.trim().is_empty())
        {
            return 1;
        }
        cursor = cursor.max(end);
    }
    if cursor < end_line
        && lines[cursor..end_line]
            .iter()
            .any(|line| !line.text.trim().is_empty())
    {
        1
    } else {
        0
    }
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
        item.consequence = Some(merge_consequence(route, id));
    }
}

/// Vault-wide merge targets for the `> +` picker: identified rows only,
/// excluding any typed source by note identity and block ID.
fn scope_vault_merge_targets(
    exclude: &[(String, String)],
    candidates: &mut Candidates,
) {
    let Candidates::TaskParent(items) = candidates else {
        return;
    };
    items.retain(|item| {
        if item.requires_block_id {
            return false;
        }
        let Some(id) = item.block_id.as_deref() else {
            return false;
        };
        !exclude.iter().any(|(source_route, source_id)| {
            source_id == id && same_route(source_route, &item.route)
        })
    });
    for item in items.iter_mut() {
        if let Some(id) = item.block_id.as_deref() {
            item.consequence = Some(merge_consequence(&item.route, id));
        }
    }
}

fn same_route(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
        || capture::route_label(left)
            .eq_ignore_ascii_case(&capture::route_label(right))
}

fn merge_consequence(route: &str, id: &str) -> String {
    format!(
        "⤵ merge into [[{route}#^{id}]] — copies the source notes under this task and retires the source",
    )
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
        item.as_section().is_some_and(|item| {
            item.title.eq_ignore_ascii_case(query)
                || item.replacement == *query
                || item.replacement == slug
        })
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
    items.push(super::model::SectionMenuRow::Section(
        super::model::SectionCandidate {
            replacement: section_creation_selector(&title),
            title,
            level: 2,
            refused: None,
        },
    ));
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

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use super::{
    model::{
        ActiveTaskCandidate, ActiveTaskPomodoroCandidate, BlockedTaskCandidate,
        Candidates, CompleteError, DependencyCandidate, PickerRelationEntry,
        PickerRelationship, PickerRelationshipEntry, RelationCandidate,
        RelationLabelSuggestion, RouteCandidate, SectionCandidate,
        TaskCandidate, TaskCompleteCandidate, TaskGotoCandidate,
        TaskLinkCandidate, TaskParentCandidate, TaskSectionCandidate,
    },
    support::rank,
};
use crate::native::{
    capture, capture_active_tasks, capture_block_ids,
    capture_completable_tasks, capture_dependency_tasks,
    capture_language::{
        DependencyEntry, DependencyTarget, DependencyTargetKind,
        EditorItemParse, RelationDirection, RelationEntry,
    },
    capture_link_tasks, capture_targets, capture_task_sections, capture_tasks,
    config,
    note_tasks::{self, BlockIdLookup},
    pomodoro, project_dependencies, task_complete, task_relations,
    task_status_hooks,
};

pub(super) fn route_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<Candidates, CompleteError> {
    let report = capture_targets::scan_capture_targets(bob_dir);
    if !report.issues.is_empty() {
        return Err(CompleteError::io(report.issue_summary()));
    }

    let ranked = rank_route_targets(report.targets, query);
    Ok(Candidates::Route(
        ranked
            .into_iter()
            .map(|(target, match_kind)| RouteCandidate {
                replacement: target.route.clone(),
                route: target.route,
                label: target.label,
                kind: target.kind.into(),
                status: target.status,
                match_kind,
            })
            .collect(),
    ))
}

/// Route ranking with `project_name_aliases`: canonical prefix matches
/// first, alias matches (canonical replacement, `match_kind: "alias"`)
/// next, then the remaining substring matches, each group keeping the
/// scan's stable order. One candidate per route; an empty query keeps
/// every target with no `match_kind`.
fn rank_route_targets(
    targets: Vec<capture_targets::CaptureTarget>,
    query: &str,
) -> Vec<(capture_targets::CaptureTarget, Option<&'static str>)> {
    if query.is_empty() {
        return targets.into_iter().map(|target| (target, None)).collect();
    }
    let lowered = query.to_lowercase();
    let mut prefix = Vec::new();
    let mut alias = Vec::new();
    let mut substring = Vec::new();
    for target in targets {
        let route_lower = target.route.to_lowercase();
        if route_lower.starts_with(&lowered) {
            prefix.push((target, None));
            continue;
        }
        let alias_hit = target.project_name_aliases.iter().any(|name| {
            let name_lower = name.to_lowercase();
            name_lower.starts_with(&lowered) || name_lower.contains(&lowered)
        });
        if alias_hit {
            alias.push((target, Some("alias")));
            continue;
        }
        if route_lower.contains(&lowered) {
            substring.push((target, None));
        }
    }
    prefix.extend(alias);
    prefix.extend(substring);
    prefix
}

pub(super) fn section_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<Candidates, CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let sections = capture::non_tasks_section_headings(&contents);
    let ranked = rank(sections, query, |section| section.title.as_str());

    Ok(Candidates::Section(
        ranked
            .into_iter()
            .map(|section| SectionCandidate {
                replacement: section.title.clone(),
                title: section.title,
                level: section.level,
            })
            .collect(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskSearch {
    BlockIdOnly,
    MultiField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MatchKind {
    Prefix,
    Substring,
}

pub(super) fn task_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
    include_missing: bool,
    search: TaskSearch,
) -> Result<Candidates, CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let used_ids: Vec<String> =
        capture_block_ids::collect_used(&contents, &settings)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
    let ranked =
        rank_open_tasks(scan.open_tasks(), query, include_missing, search);

    Ok(Candidates::Task(
        ranked
            .into_iter()
            .map(|task| {
                let requires_block_id = task.block_id.is_none();
                TaskCandidate {
                    replacement: task.block_id.clone().unwrap_or_default(),
                    task_ref: task.task_ref(),
                    block_id: task.block_id.clone(),
                    route: route.to_string(),
                    requires_block_id,
                    block_id_suggestions: if requires_block_id {
                        let suggestions = capture_block_ids::suggest_ids(
                            &task.description,
                            '^',
                            &used_ids,
                        );
                        (!suggestions.is_empty()).then_some(suggestions)
                    } else {
                        None
                    },
                    status_symbol: task.status_symbol,
                    status_name: task.status_name.clone(),
                    status_type: capture_tasks::status_type_label(
                        task.status_type,
                    ),
                    text: task.description.clone(),
                    disabled_reason: None,
                    section: task.section.clone(),
                    depth: capture_tasks::indentation_depth(&task.indentation),
                    child_count: task.child_count,
                    line: task.line_index + 1,
                    pomodoro: None,
                    task_kind: task.task_kind,
                }
            })
            .collect(),
    ))
}

/// Link-only candidates for `pomodoro_block_id` with `link` intent:
/// identified open tasks the link path accepts (Ready, Blocked, Next, In
/// Progress via the same predicate the link resolver uses), annotated with
/// the queued Pomodoro from today's ledger.
pub(super) fn link_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let linkable = scan.open_tasks().filter(|task| {
        task.block_id.is_some()
            && capture_link_tasks::is_linkable_status(task.status_symbol)
    });
    let ranked =
        rank_task_group(linkable.collect(), query, TaskSearch::BlockIdOnly);
    let day_file = pomodoro::day_file_for(bob_dir);
    let mut warnings = Vec::new();
    let ledger = capture_active_tasks::read_ledger(&day_file, &mut warnings);
    let candidates = ranked
        .into_iter()
        .map(|task| {
            let block_id =
                task.block_id.clone().expect("filtered to identified tasks");
            let pomodoro = ledger
                .owners
                .get(&(route.to_string(), block_id.clone()))
                .map(|entry| ActiveTaskPomodoroCandidate {
                    line: entry.line,
                    name: entry.name.clone(),
                    time_range: entry.time_range.clone(),
                    is_current: entry.is_current,
                });
            TaskCandidate {
                replacement: block_id.clone(),
                task_ref: task.task_ref(),
                block_id: Some(block_id),
                route: route.to_string(),
                requires_block_id: false,
                block_id_suggestions: None,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.description.clone(),
                disabled_reason: None,
                section: task.section.clone(),
                depth: capture_tasks::indentation_depth(&task.indentation),
                child_count: task.child_count,
                line: task.line_index + 1,
                pomodoro,
                task_kind: task.task_kind,
            }
        })
        .collect();
    Ok((Candidates::Task(candidates), warnings))
}

pub(super) fn rank_open_tasks<'a>(
    tasks: impl Iterator<Item = &'a note_tasks::NoteTask>,
    query: &str,
    include_missing: bool,
    search: TaskSearch,
) -> Vec<&'a note_tasks::NoteTask> {
    let mut identified = Vec::new();
    let mut unidentified = Vec::new();
    for task in tasks {
        if task.block_id.is_some() {
            identified.push(task);
        } else if include_missing {
            unidentified.push(task);
        }
    }

    let mut ranked = rank_task_group(identified, query, search);
    ranked.extend(rank_task_group(unidentified, query, search));
    ranked
}

pub(super) fn rank_task_group<'a>(
    tasks: Vec<&'a note_tasks::NoteTask>,
    query: &str,
    search: TaskSearch,
) -> Vec<&'a note_tasks::NoteTask> {
    if query.is_empty() {
        return tasks;
    }

    let query = query.to_lowercase();
    let mut prefix_matches = Vec::new();
    let mut substring_matches = Vec::new();
    for task in tasks {
        match task_match_kind(task, &query, search) {
            Some(MatchKind::Prefix) => prefix_matches.push(task),
            Some(MatchKind::Substring) => substring_matches.push(task),
            None => {}
        }
    }
    prefix_matches.extend(substring_matches);
    prefix_matches
}

pub(super) fn task_match_kind(
    task: &note_tasks::NoteTask,
    query: &str,
    search: TaskSearch,
) -> Option<MatchKind> {
    let mut prefix = false;
    let mut substring = false;
    for field in task_search_fields(task, search) {
        let value = field.to_lowercase();
        if value.starts_with(query) {
            prefix = true;
        } else if value.contains(query) {
            substring = true;
        }
    }
    if prefix {
        Some(MatchKind::Prefix)
    } else if substring {
        Some(MatchKind::Substring)
    } else {
        None
    }
}

pub(super) fn task_section_candidates(
    bob_dir: &Path,
    route: &str,
    block_id: Option<&str>,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let Some(block_id) = block_id.filter(|id| !id.is_empty()) else {
        return Ok((Candidates::TaskSection(Vec::new()), Vec::new()));
    };

    let target = bob_dir.join(capture::route_label(route));
    let contents = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::MissingNote,
                )],
            ));
        }
        Err(error) => {
            return Err(CompleteError::io(format!(
                "read target {}: {error}",
                target.display()
            )));
        }
    };

    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = match scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        BlockIdLookup::Missing => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::Missing {
                        suggestion: scan
                            .suggest_block_id(block_id)
                            .map(str::to_string),
                    },
                )],
            ));
        }
        BlockIdLookup::Duplicate(count) => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::Duplicate(count),
                )],
            ));
        }
        BlockIdLookup::NotATask { .. } => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::NotATask,
                )],
            ));
        }
    };

    let parent_text = parent.description.clone();
    let parent_block_id = parent.block_id.clone();
    let ranked = rank(
        capture_task_sections::task_sections(&contents, parent),
        query,
        |section| section.slug.as_str(),
    );

    Ok((
        Candidates::TaskSection(
            ranked
                .into_iter()
                .map(|section| TaskSectionCandidate {
                    replacement: section.slug.clone(),
                    title: section.title,
                    slug: section.slug,
                    route: route.to_string(),
                    block_id: parent_block_id.clone(),
                    text: parent_text.clone(),
                    line: section.line,
                    child_count: section.child_count,
                })
                .collect(),
        ),
        Vec::new(),
    ))
}

/// Active-task candidates for a solo leading `^` token: In Progress and
/// Next tasks with block IDs, ordered by today's open-Pomodoro Task Links
/// and ranked by the query. The `replacement` is the `route:block-id` an
/// accept inserts; `pomodoro` is the queued entry, or `null` when the task
/// is not queued.
pub(super) fn active_task_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_active_tasks::discover(bob_dir);
    let candidates = capture_active_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| ActiveTaskCandidate {
            replacement: task.replacement(),
            task_ref: task.task_ref.clone(),
            route: task.route.clone(),
            block_id: task.block_id.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                ActiveTaskPomodoroCandidate {
                    line: pomodoro.line,
                    name: pomodoro.name.clone(),
                    time_range: pomodoro.time_range.clone(),
                    is_current: pomodoro.is_current,
                }
            }),
            task_kind: task.task_kind,
        })
        .collect();
    (Candidates::ActiveTask(candidates), discovered.warnings)
}

/// `task_link` candidates for a solo leading `:` token: every linkable
/// open task (Ready, Blocked, Next, In Progress) in the routable inbox,
/// area, and non-terminal project notes, in canonical order and ranked by
/// the query. ID-less tasks are always included; `--all-tasks` does not
/// affect this context. The `replacement` covers the whole `:` token,
/// sigil included, because accepting rewrites the query into the canonical
/// `@route:block-id` marker.
pub(super) fn task_link_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_link_tasks::discover(bob_dir);
    let candidates = capture_link_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| TaskLinkCandidate {
            replacement: task.replacement(),
            task_ref: task.task_ref.clone(),
            route: task.route.clone(),
            note_kind: task.note_kind,
            block_id: task.block_id.clone(),
            requires_block_id: task.block_id.is_none(),
            block_id_suggestions: task.block_id_suggestions.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            depth: task.depth,
            line: task.line,
            group: task.group,
            scheduled: task.scheduled.clone(),
            pulls_forward: task.pulls_forward,
            pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                ActiveTaskPomodoroCandidate {
                    line: pomodoro.line,
                    name: pomodoro.name.clone(),
                    time_range: pomodoro.time_range.clone(),
                    is_current: pomodoro.is_current,
                }
            }),
            task_kind: task.task_kind,
        })
        .collect();
    (Candidates::TaskLink(candidates), discovered.warnings)
}

/// `task_goto` candidates for a whole-draft `/` token: every linkable
/// open task (Ready, Blocked, Next, In Progress) in the routable inbox,
/// area, and non-terminal project notes, in canonical order and ranked by
/// the query -- exactly the `task_link` rows and order for the same query,
/// including ID-less tasks. `--all-tasks` does not affect this context.
/// The `replacement` is always empty (clients never insert it) and carries
/// no Add block ID metadata; `note_path` is the exact vault-relative path
/// and `target` the absolute path for opening the task.
pub(super) fn task_goto_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_link_tasks::discover(bob_dir);
    let candidates = capture_link_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| TaskGotoCandidate {
            replacement: String::new(),
            task_ref: task.task_ref.clone(),
            route: task.route.clone(),
            note_kind: task.note_kind,
            note_path: task.note_path.clone(),
            target: bob_dir.join(&task.note_path).display().to_string(),
            block_id: task.block_id.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            depth: task.depth,
            line: task.line,
            group: task.group,
            scheduled: task.scheduled.clone(),
            pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                ActiveTaskPomodoroCandidate {
                    line: pomodoro.line,
                    name: pomodoro.name.clone(),
                    time_range: pomodoro.time_range.clone(),
                    is_current: pomodoro.is_current,
                }
            }),
            task_kind: task.task_kind,
        })
        .collect();
    (Candidates::TaskGoto(candidates), discovered.warnings)
}

/// `task_parent` candidates for a bare-plus selector. Reuses the colon
/// picker discovery and stable ranking, but emits the parent-task `+`
/// spelling and omits link-only pull-forward metadata.
pub(super) fn task_parent_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_link_tasks::discover(bob_dir);
    let candidates = capture_link_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| {
            let replacement = task
                .block_id
                .as_ref()
                .map(|block_id| format!("@{}+{block_id}", task.route))
                .unwrap_or_default();
            TaskParentCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                route: task.route.clone(),
                note_kind: task.note_kind,
                block_id: task.block_id.clone(),
                requires_block_id: task.block_id.is_none(),
                block_id_suggestions: task.block_id_suggestions.clone(),
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: task.status_type,
                text: task.text.clone(),
                disabled_reason: None,
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                group: task.group,
                pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                    ActiveTaskPomodoroCandidate {
                        line: pomodoro.line,
                        name: pomodoro.name.clone(),
                        time_range: pomodoro.time_range.clone(),
                        is_current: pomodoro.is_current,
                    }
                }),
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::TaskParent(candidates), discovered.warnings)
}

/// Adds relationship-aware guards to either parent-task picker and returns
/// the matching picker descriptor data. The same vault-wide task catalog and
/// dependency graph power both the `+` and note-scoped `@route+` pickers.
pub(super) fn apply_relationship_picker(
    bob_dir: &Path,
    candidates: &mut Candidates,
    item: &EditorItemParse,
) -> Option<PickerRelationship> {
    if item.dependencies.is_empty()
        && item.blocked_tasks.is_empty()
        && item.relations.is_empty()
    {
        return None;
    }

    let discovered = capture_dependency_tasks::discover(bob_dir);
    let graph =
        capture_dependency_tasks::dependency_graph(bob_dir, &discovered);
    let resolved_dependencies: Vec<_> = item
        .dependencies
        .iter()
        .map(|entry| resolve_relationship_entry(bob_dir, &discovered, entry))
        .collect();
    let resolved_blocked: Vec<_> = item
        .blocked_tasks
        .iter()
        .map(|entry| resolve_relationship_entry(bob_dir, &discovered, entry))
        .collect();
    let relations = item
        .relations
        .iter()
        .map(|entry| {
            let text = capture_dependency_tasks::resolve_dependency_note(
                bob_dir,
                &discovered,
                &entry.note,
            )
            .ok()
            .map(|path| capture_dependency_tasks::display_path(&path))
            .and_then(|note_path| {
                unique_task(&discovered, &note_path, &entry.block_id)
                    .map(|task| task.text.clone())
            })
            .unwrap_or_else(|| entry.raw.clone());
            PickerRelationEntry {
                direction: entry.direction,
                text,
            }
        })
        .collect();
    let recognized_projects = recognized_project_paths(bob_dir, &discovered);
    let settings = note_tasks::read_settings(bob_dir);
    let mut note_cache = HashMap::new();

    let mut annotate =
        |route: &str,
         block_id: Option<&str>,
         replacement: &mut String,
         disabled_reason: &mut Option<String>| {
            let Some(block_id) = block_id else {
                return;
            };
            let Some(identity) =
                task_identity_for_route(bob_dir, &discovered, route, block_id)
            else {
                return;
            };
            let reason = relationship_disabled_reason(
                bob_dir,
                &discovered,
                &graph,
                &recognized_projects,
                &settings,
                &mut note_cache,
                &identity,
                &resolved_dependencies,
                &resolved_blocked,
            );
            if let Some(reason) = reason {
                *replacement = String::new();
                *disabled_reason = Some(reason);
            }
        };

    match candidates {
        Candidates::TaskParent(rows) => {
            for row in rows {
                annotate(
                    &row.route,
                    row.block_id.as_deref(),
                    &mut row.replacement,
                    &mut row.disabled_reason,
                );
            }
        }
        Candidates::Task(rows) => {
            for row in rows {
                annotate(
                    &row.route,
                    row.block_id.as_deref(),
                    &mut row.replacement,
                    &mut row.disabled_reason,
                );
            }
        }
        _ => return None,
    }

    Some(PickerRelationship {
        dependencies: resolved_dependencies
            .into_iter()
            .map(|entry| entry.wire)
            .collect(),
        blocked_tasks: resolved_blocked
            .into_iter()
            .map(|entry| entry.wire)
            .collect(),
        relations,
    })
}

struct ResolvedRelationshipEntry {
    wire: PickerRelationshipEntry,
    identity: Option<capture_dependency_tasks::TaskIdentity>,
}

fn resolve_relationship_entry(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    entry: &DependencyEntry,
) -> ResolvedRelationshipEntry {
    let resolved = capture_dependency_tasks::resolve_dependency_note(
        bob_dir,
        discovered,
        &entry.note,
    )
    .ok()
    .map(|path| capture_dependency_tasks::display_path(&path));
    let task = resolved.as_ref().and_then(|note_path| {
        unique_task(discovered, note_path, &entry.block_id)
    });
    let note_path = task
        .map(|task| task.note_path.clone())
        .or_else(|| resolved.clone())
        .unwrap_or_else(|| entry.note.clone());
    ResolvedRelationshipEntry {
        wire: PickerRelationshipEntry {
            raw: entry.raw.clone(),
            note_path,
            block_id: entry.block_id.clone(),
            text: task.map(|task| task.text.clone()),
        },
        identity: task
            .map(|task| (task.note_path.clone(), entry.block_id.clone())),
    }
}

fn unique_task<'a>(
    discovered: &'a capture_dependency_tasks::DependencyTaskResult,
    note_path: &str,
    block_id: &str,
) -> Option<&'a capture_dependency_tasks::DependencyTask> {
    let mut matches = discovered.tasks.iter().filter(|task| {
        task.note_path == note_path
            && task.block_id.as_deref() == Some(block_id)
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn task_identity_for_route(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    route: &str,
    block_id: &str,
) -> Option<capture_dependency_tasks::TaskIdentity> {
    let typed = capture::route_label(route);
    let path = capture_dependency_tasks::resolve_dependency_note(
        bob_dir, discovered, &typed,
    )
    .ok()?;
    let note_path = capture_dependency_tasks::display_path(&path);
    unique_task(discovered, &note_path, block_id)?;
    Some((note_path, block_id.to_string()))
}

fn relationship_disabled_reason(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    graph: &HashMap<
        capture_dependency_tasks::TaskIdentity,
        HashSet<capture_dependency_tasks::TaskIdentity>,
    >,
    recognized_projects: &HashSet<String>,
    settings: &note_tasks::NoteTaskSettings,
    note_cache: &mut HashMap<PathBuf, String>,
    candidate: &capture_dependency_tasks::TaskIdentity,
    dependencies: &[ResolvedRelationshipEntry],
    blocked_tasks: &[ResolvedRelationshipEntry],
) -> Option<String> {
    if dependencies
        .iter()
        .any(|entry| entry.identity.as_ref() == Some(candidate))
    {
        return Some("the prerequisite itself".to_string());
    }
    if blocked_tasks
        .iter()
        .any(|entry| entry.identity.as_ref() == Some(candidate))
    {
        return Some("the blocked task itself".to_string());
    }

    let dependency_cycle = dependencies.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            dependency_reachable(graph, std::slice::from_ref(identity))
                .contains(candidate)
        })
    });
    let blocked_cycle = blocked_tasks.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            dependency_reachable(graph, std::slice::from_ref(candidate))
                .contains(identity)
        })
    });
    if dependency_cycle || blocked_cycle {
        return Some("would create a dependency cycle".to_string());
    }

    let candidate_note = candidate.0.as_str();
    // `&` candidate is the dependent: refuse only when it is `^prj`
    // of a recognised project and a typed prerequisite lives in that
    // note or a descendant. `*` typed blocked task is the dependent:
    // refuse only when that task is such a `^prj` and the candidate
    // lives in its note or a descendant. Matches the writer
    // (`refuse_internal_prj_targets`).
    let project_resident = dependencies.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            project_cannot_wait_on_resident(
                bob_dir,
                discovered,
                recognized_projects,
                settings,
                note_cache,
                candidate,
                &identity.0,
            )
        })
    }) || blocked_tasks.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            project_cannot_wait_on_resident(
                bob_dir,
                discovered,
                recognized_projects,
                settings,
                note_cache,
                identity,
                candidate_note,
            )
        })
    });
    project_resident
        .then(|| "a project cannot depend on its own resident".to_string())
}

fn recognized_project_paths(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
) -> HashSet<String> {
    let settings = note_tasks::read_settings(bob_dir);
    let mut note_cache = HashMap::new();
    discovered
        .tasks
        .iter()
        .filter(|task| task.block_id.as_deref() == Some("prj") && task.open)
        .filter_map(|task| {
            let path = PathBuf::from(&task.note_path);
            read_note_cached(bob_dir, &path, &mut note_cache)
                .filter(|contents| {
                    is_recognized_project(&path, contents, bob_dir, &settings)
                })
                .map(|_| task.note_path.clone())
        })
        .collect()
}

fn project_cannot_wait_on_resident(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    recognized_projects: &HashSet<String>,
    settings: &note_tasks::NoteTaskSettings,
    note_cache: &mut HashMap<PathBuf, String>,
    dependent: &capture_dependency_tasks::TaskIdentity,
    other_note: &str,
) -> bool {
    if dependent.1 != "prj" {
        return false;
    }
    project_resident_relation(
        bob_dir,
        discovered,
        recognized_projects,
        settings,
        note_cache,
        &dependent.0,
        other_note,
    )
}

fn project_resident_relation(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    recognized_projects: &HashSet<String>,
    settings: &note_tasks::NoteTaskSettings,
    note_cache: &mut HashMap<PathBuf, String>,
    project_note: &str,
    resident_note: &str,
) -> bool {
    if !recognized_projects.contains(project_note) {
        return false;
    }
    if project_note == resident_note {
        return true;
    }
    let resident_path = PathBuf::from(resident_note);
    let Some(contents) = read_note_cached(bob_dir, &resident_path, note_cache)
    else {
        return false;
    };
    project_dependencies::project_ancestors(
        &resident_path,
        &contents,
        &discovered.index,
        |path| read_note_cached(bob_dir, path, note_cache),
        |path, contents| {
            is_recognized_project(path, contents, bob_dir, settings)
        },
    )
    .into_iter()
    .any(|path| capture_dependency_tasks::display_path(&path) == project_note)
}

/// `task_dependency` candidates for an `&` token: every task in the
/// vault-wide catalog in picker order (same-note open tasks, In
/// Progress, Next, other open tasks grouped by note, then
/// completed/cancelled history; ranked matches first for a query),
/// annotated with the exact note identity, display locator, stable
/// group, and the guards the picker badges.
///
/// Known self/already-selected targets and unusable identities stay
/// visible with an explanatory reason and no insertable replacement;
/// ID-less rows carry `requires_block_id` and suggestions for the
/// explicit Add block ID flow instead. Read-only: selecting or
/// highlighting a row writes nothing. No ledger file is required.
pub(super) fn dependency_candidates(
    bob_dir: &Path,
    query: &str,
    owner: Option<&DependencyTarget>,
    typed: &[(String, String)],
) -> (Candidates, Vec<String>) {
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let owner_note = owner
        .as_ref()
        .and_then(|target| target.route.as_deref())
        .map(capture::route_label);
    let owner_block_id = owner.as_ref().and_then(|target| {
        if target.kind == DependencyTargetKind::ExistingTask {
            target.block_id.as_deref()
        } else {
            None
        }
    });
    let dependent_note = owner_note.as_deref().map(Path::new);
    let present = capture_dependency_tasks::already_present_prerequisites(
        bob_dir,
        &discovered,
        dependent_note,
        owner_block_id,
        typed,
    );
    let ordered = capture_dependency_tasks::order_for_picker(
        &discovered.tasks,
        query,
        owner_note.as_deref(),
    );
    let candidates = ordered
        .into_iter()
        .map(|task| {
            let is_self = owner_block_id.is_some_and(|id| {
                Some(task.note_path.as_str()) == owner_note.as_deref()
                    && task.block_id.as_deref() == Some(id)
            });
            let already = present.contains(&(
                task.note_path.clone(),
                task.block_id.clone().unwrap_or_default(),
            )) && task.block_id.is_some();
            let disabled_reason = if is_self {
                Some("the dependent task itself".to_string())
            } else if task.duplicate_id {
                Some(format!(
                    "duplicate block ID ^{} in {}",
                    task.block_id.as_deref().unwrap_or_default(),
                    task.note_path
                ))
            } else {
                None
            };
            let requires_block_id = task.block_id.is_none();
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for(
                    &discovered.index,
                    Path::new(&task.note_path),
                    task.block_id.as_deref().expect("identified task"),
                )
            };
            DependencyCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: capture_dependency_tasks::group_for(
                    task,
                    owner_note.as_deref(),
                )
                .to_string(),
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                already_dependency: already,
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::Dependency(candidates), discovered.warnings)
}

/// `blocked_task` candidates for a `*` modifier. Only open, editable tasks
/// are offered. Today links and the task lanes establish the stable order;
/// the dependency graph adds early self, cycle, project-resident, duplicate,
/// and already-blocked metadata while the capture writer remains final.
pub(super) fn blocked_task_candidates(
    bob_dir: &Path,
    query: &str,
    owner: Option<&DependencyTarget>,
    owner_note: &str,
    typed_dependencies: &[(String, String)],
    typed_blocked: &[(String, String)],
) -> (Candidates, Vec<String>) {
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let graph =
        capture_dependency_tasks::dependency_graph(bob_dir, &discovered);
    let day_file = pomodoro::day_file_for(bob_dir);
    let mut ledger_warnings = Vec::new();
    let ledger =
        capture_active_tasks::read_ledger(&day_file, &mut ledger_warnings);
    let _ = ledger_warnings;
    let warnings = discovered.warnings.clone();

    let owner_path = PathBuf::from(capture::route_label(owner_note));
    let owner_path_display = owner_path.to_string_lossy().replace('\\', "/");
    let owner_identity = owner.and_then(|target| {
        if target.kind != DependencyTargetKind::ExistingTask {
            return None;
        }
        let block_id = target.block_id.as_ref()?;
        let path = target
            .route
            .as_deref()
            .map(capture::route_label)
            .unwrap_or_else(|| owner_path_display.clone());
        Some((path, block_id.clone()))
    });

    let mut typed_blocked_identities = HashSet::new();
    for (note, block_id) in typed_blocked {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            note,
        ) {
            typed_blocked_identities.insert((
                capture_dependency_tasks::display_path(&path),
                block_id.clone(),
            ));
        }
    }

    let mut cycle_roots = Vec::new();
    if let Some(identity) = &owner_identity {
        cycle_roots.push(identity.clone());
    }
    for (note, block_id) in typed_dependencies {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            note,
        ) {
            cycle_roots.push((
                capture_dependency_tasks::display_path(&path),
                block_id.clone(),
            ));
        }
    }
    cycle_roots.sort();
    cycle_roots.dedup();
    let reachable = dependency_reachable(&graph, &cycle_roots);

    let settings = note_tasks::read_settings(bob_dir);
    let mut note_cache: HashMap<PathBuf, String> = HashMap::new();
    let recognized_projects = recognized_project_paths(bob_dir, &discovered);

    let mut ordered: Vec<(&capture_dependency_tasks::DependencyTask, u32)> =
        discovered
            .tasks
            .iter()
            .filter(|task| is_editable_open_task(task))
            .filter_map(|task| {
                capture_link_tasks::match_score(
                    &capture_dependency_tasks::dependency_search_fields(task),
                    query,
                )
                .map(|score| (task, score))
            })
            .collect();
    ordered.sort_by(|(left, left_score), (right, right_score)| {
        right_score.cmp(left_score).then_with(|| {
            blocked_task_order_key(left, &ledger, &owner_path_display).cmp(
                &blocked_task_order_key(right, &ledger, &owner_path_display),
            )
        })
    });

    let candidates = ordered
        .into_iter()
        .map(|(task, _)| {
            let key = task
                .block_id
                .as_ref()
                .map(|id| (task.note_path.clone(), id.clone()));
            let is_self = owner_identity
                .as_ref()
                .is_some_and(|owner| key.as_ref() == Some(owner));
            let already = key.as_ref().is_some_and(|key| {
                typed_blocked_identities.contains(key)
                    || owner_identity.as_ref().is_some_and(|owner| {
                        graph.get(key).is_some_and(|prerequisites| {
                            prerequisites.contains(owner)
                        })
                    })
            });
            let internal_project = key.as_ref().is_some_and(|identity| {
                project_cannot_wait_on_resident(
                    bob_dir,
                    &discovered,
                    &recognized_projects,
                    &settings,
                    &mut note_cache,
                    identity,
                    &owner_path_display,
                )
            });
            let disabled_reason = if is_self {
                Some("the owner task itself".to_string())
            } else if task.duplicate_id {
                Some(format!(
                    "duplicate block ID ^{} in {}",
                    task.block_id.as_deref().unwrap_or_default(),
                    task.note_path
                ))
            } else if !already
                && key.as_ref().is_some_and(|key| reachable.contains(key))
            {
                Some("would create a dependency cycle".to_string())
            } else if internal_project {
                Some("a project cannot depend on its own resident".to_string())
            } else {
                None
            };
            let requires_block_id = task.block_id.is_none();
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for_sigil(
                    &discovered.index,
                    Path::new(&task.note_path),
                    task.block_id.as_deref().expect("identified task"),
                    b'*',
                )
            };
            BlockedTaskCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: blocked_task_group(task, &ledger, &owner_path_display)
                    .to_string(),
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                already_blocked: already,
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::BlockedTask(candidates), warnings)
}

/// Relation targets use the same complete task catalog as dependencies.
/// `to` reads the owner's LINKS line, while `from` reads each offered
/// source's LINKS line because that is the task the capture would edit.
pub(super) fn relation_candidates(
    bob_dir: &Path,
    query: &str,
    direction: RelationDirection,
    owner: Option<&DependencyTarget>,
    owner_note: &str,
    typed_relations: &[RelationEntry],
    typed_dependencies: &[DependencyEntry],
) -> (Candidates, Vec<String>) {
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let (links, mut warnings, _) = task_links_catalog(bob_dir, &discovered);
    let graph =
        capture_dependency_tasks::dependency_graph(bob_dir, &discovered);
    let mut ledger_warnings = Vec::new();
    let ledger = capture_active_tasks::read_ledger(
        &pomodoro::day_file_for(bob_dir),
        &mut ledger_warnings,
    );
    warnings.extend(ledger_warnings);
    let owner_identity = owner.and_then(|target| {
        if target.kind != DependencyTargetKind::ExistingTask {
            return None;
        }
        let route = target.route.as_deref()?;
        let block_id = target.block_id.as_deref()?;
        task_identity_for_route(bob_dir, &discovered, route, block_id)
    });

    let mut prerequisites = HashSet::new();
    if let Some(identity) = &owner_identity
        && let Some(targets) = graph.get(identity)
    {
        prerequisites.extend(targets.iter().cloned());
    }
    for entry in typed_dependencies {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            &entry.note,
        ) {
            prerequisites.insert((
                capture_dependency_tasks::display_path(&path),
                entry.block_id.clone(),
            ));
        }
    }

    let mut typed = HashMap::new();
    for entry in typed_relations
        .iter()
        .filter(|entry| entry.direction == direction)
    {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            &entry.note,
        ) {
            typed
                .entry((
                    capture_dependency_tasks::display_path(&path),
                    entry.block_id.clone(),
                ))
                .or_insert_with(|| entry.label.clone());
        }
    }

    let owner_links = owner_identity
        .as_ref()
        .and_then(|identity| links.get(identity));
    let ordered = relation_picker_order(
        bob_dir,
        &discovered.tasks,
        query,
        Some(owner_note),
        direction,
        &ledger,
    );
    let candidates = ordered
        .into_iter()
        .map(|task| {
            let key = task
                .block_id
                .as_ref()
                .map(|id| (task.note_path.clone(), id.clone()));
            let is_self = key.as_ref().is_some_and(|identity| {
                owner_identity.as_ref() == Some(identity)
            });
            let candidate_links =
                key.as_ref().and_then(|identity| links.get(identity));
            let stored_link = match direction {
                RelationDirection::To => key.as_ref().and_then(|identity| {
                    owner_links.and_then(|source| source.outgoing.get(identity))
                }),
                RelationDirection::From => {
                    owner_identity.as_ref().and_then(|owner| {
                        candidate_links
                            .and_then(|source| source.outgoing.get(owner))
                    })
                }
            };
            let stored_label = stored_link.cloned().flatten();
            let typed_label = key
                .as_ref()
                .and_then(|identity| typed.get(identity).cloned())
                .flatten();
            let already_linked = stored_link.is_some()
                || key
                    .as_ref()
                    .is_some_and(|identity| typed.contains_key(identity));
            let archived = is_done_path(Path::new(&task.note_path));
            let read_only = direction == RelationDirection::From
                && is_relation_source_read_only(bob_dir, task);
            let malformed = direction == RelationDirection::From
                && candidate_links.is_some_and(|source| source.malformed);
            let disabled_reason = if is_self {
                Some("the owner task itself".to_string())
            } else if task.duplicate_id {
                Some(format!(
                    "duplicate block ID ^{} in {}",
                    task.block_id.as_deref().unwrap_or_default(),
                    task.note_path
                ))
            } else if read_only {
                Some("read-only note".to_string())
            } else if malformed {
                Some("its LINKS line is malformed".to_string())
            } else if archived && task.block_id.is_none() {
                Some("archived tasks can't get an id".to_string())
            } else {
                None
            };
            let requires_block_id = task.block_id.is_none();
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for_prefix(
                    &discovered.index,
                    Path::new(&task.note_path),
                    task.block_id.as_deref().expect("identified relation task"),
                    match direction {
                        RelationDirection::To => "~",
                        RelationDirection::From => "~~",
                    },
                )
            };
            RelationCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: capture_dependency_tasks::group_for(
                    task,
                    Some(owner_note),
                )
                .to_string(),
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                already_linked,
                existing_label: typed_label.or(stored_label),
                also_prerequisite: key
                    .as_ref()
                    .is_some_and(|identity| prerequisites.contains(identity)),
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                task_kind: task.task_kind,
            }
        })
        .collect::<Vec<_>>();

    (Candidates::Relation(candidates), warnings)
}

/// Build one source LINKS view per identified task and count normalized
/// labels across every accepted LINKS line, including `done/` history.
fn task_links_catalog(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
) -> (
    HashMap<capture_dependency_tasks::TaskIdentity, TaskLinks>,
    Vec<String>,
    Vec<(String, usize)>,
) {
    let mut tasks_by_note: HashMap<
        &str,
        Vec<&capture_dependency_tasks::DependencyTask>,
    > = HashMap::new();
    for task in &discovered.tasks {
        tasks_by_note
            .entry(task.note_path.as_str())
            .or_default()
            .push(task);
    }
    let mut links = HashMap::new();
    let mut warnings = Vec::new();
    let mut labels: HashMap<String, (String, usize)> = HashMap::new();
    for (note_path, tasks) in tasks_by_note {
        let Ok(contents) = fs::read_to_string(bob_dir.join(note_path)) else {
            continue;
        };
        let lines = contents.lines().collect::<Vec<_>>();
        let fenced =
            crate::native::markdown::fenced_lines(&lines, 0..lines.len());
        for task in tasks {
            let block_id = task.block_id.as_deref();
            let task_line = task.line.saturating_sub(1);
            let children = task_relations::parse::links_children_of(
                &lines, &fenced, task_line,
            );
            let mut source = TaskLinks::default();
            for child in children {
                match child.parsed {
                    task_relations::LinksLine::Accepted { entries, .. } => {
                        for entry in entries {
                            if let Some(label) = entry.label.as_deref() {
                                let folded = label.to_lowercase();
                                let value = labels
                                    .entry(folded)
                                    .or_insert_with(|| (label.to_string(), 0));
                                value.1 += 1;
                            }
                            let target_path = if entry.target.is_empty() {
                                Some(PathBuf::from(&task.note_path))
                            } else {
                                discovered.index.resolve(
                                    Some(Path::new(&task.note_path)),
                                    &entry.target,
                                )
                            };
                            if let Some(target_path) = target_path {
                                let identity = (
                                    capture_dependency_tasks::display_path(
                                        &target_path,
                                    ),
                                    entry.block_id,
                                );
                                source
                                    .outgoing
                                    .entry(identity)
                                    .or_insert(entry.label);
                            }
                        }
                    }
                    task_relations::LinksLine::Malformed => {
                        source.malformed = true;
                    }
                    task_relations::LinksLine::Empty
                    | task_relations::LinksLine::NotALine => {}
                }
            }
            if let Some(block_id) = block_id {
                links.insert(
                    (task.note_path.clone(), block_id.to_string()),
                    source,
                );
            }
        }
    }
    let mut labels = labels.into_values().collect::<Vec<_>>();
    labels.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.to_lowercase().cmp(&right.0.to_lowercase()))
            .then_with(|| left.0.cmp(&right.0))
    });
    warnings.extend(discovered.warnings.iter().cloned());
    (links, warnings, labels)
}

fn relation_picker_order<'a>(
    bob_dir: &Path,
    tasks: &'a [capture_dependency_tasks::DependencyTask],
    query: &str,
    owner_note: Option<&str>,
    direction: RelationDirection,
    ledger: &capture_active_tasks::Ledger,
) -> Vec<&'a capture_dependency_tasks::DependencyTask> {
    let mut scored = tasks
        .iter()
        .filter(|task| {
            direction == RelationDirection::To
                || is_editable_open_task(task)
                || is_relation_source_read_only(bob_dir, task)
        })
        .filter_map(|task| {
            capture_link_tasks::match_score(
                &capture_dependency_tasks::dependency_search_fields(task),
                query,
            )
            .map(|score| (task, score))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score.cmp(left_score).then_with(|| {
            relation_order_key(left, owner_note, direction, ledger)
                .cmp(&relation_order_key(right, owner_note, direction, ledger))
        })
    });
    scored.into_iter().map(|(task, _)| task).collect()
}

fn relation_order_key(
    task: &capture_dependency_tasks::DependencyTask,
    owner_note: Option<&str>,
    direction: RelationDirection,
    ledger: &capture_active_tasks::Ledger,
) -> (u8, u8, usize, usize, u8, String, usize) {
    let hidden = u8::from(task.hidden);
    if direction == RelationDirection::To {
        let group = if owner_note == Some(task.note_path.as_str()) {
            0
        } else if is_done_path(Path::new(&task.note_path)) {
            5
        } else if task.open && task.status_symbol == '/' {
            1
        } else if task.open && task.status_symbol == '*' {
            2
        } else if task.open {
            3
        } else {
            4
        };
        return (group, hidden, 0, 0, 0, task.note_path.clone(), task.line);
    }

    if let Some((entry, link)) = task_ledger_position(task, ledger) {
        let running = task
            .block_id
            .as_ref()
            .and_then(|id| {
                ledger
                    .owners
                    .get(&(task_route(&task.note_path), id.clone()))
            })
            .is_some_and(|pomodoro| pomodoro.is_current);
        return (
            0,
            u8::from(!running),
            entry,
            link,
            hidden,
            task.note_path.clone(),
            task.line,
        );
    }
    let group = if task.status_symbol == '/' {
        1
    } else if task.status_symbol == '*' {
        2
    } else if owner_note == Some(task.note_path.as_str()) {
        3
    } else {
        4
    };
    (group, 0, 0, 0, hidden, task.note_path.clone(), task.line)
}

#[derive(Default)]
struct TaskLinks {
    outgoing: HashMap<capture_dependency_tasks::TaskIdentity, Option<String>>,
    malformed: bool,
}

/// Relation label rows for one `relation_label` probe. `one_off` is true
/// when the probe sits in a `||` label: `replacement` then keeps the
/// `||` spelling while `hide_replacement` is always the `||` form.
/// Configured closing labels mark their rows with their effect flag, and
/// zero-use configured labels appear as a trailing `configured` overlay
/// that respects hiding.
pub(super) fn relation_label_suggestions(
    bob_dir: &Path,
    query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) -> (Candidates, Vec<String>, usize) {
    let catalog = match crate::native::relation_labels::Catalog::load(bob_dir) {
        Ok(catalog) => catalog,
        Err(error) => {
            return (
                Candidates::RelationLabel(Vec::new()),
                vec![format!("relation label catalog: {error}")],
                0,
            );
        }
    };
    let validation =
        crate::native::task_relations::label::normalize_relation_label(query);
    let normalized_query = validation.as_ref().cloned().unwrap_or_else(|_| {
        crate::native::task_relations::label::collapse_label_text(query)
    });
    let mut suggestions = catalog
        .rank_for_query(
            &normalized_query,
            crate::native::relation_labels::HiddenFilter::Exclude,
        )
        .into_iter()
        .map(|record| RelationLabelSuggestion {
            replacement: if one_off {
                record.hide_replacement()
            } else {
                record.replacement()
            },
            hide_replacement: record.hide_replacement(),
            label: record.name.clone(),
            key: record.key.clone(),
            inverse: record.inverse.clone(),
            count: Some(record.count as usize),
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: policy.is_cancel_label(&record.key),
            completes_source: policy.is_complete_label(&record.key),
            configured: false,
        })
        .collect::<Vec<_>>();
    append_cancel_overlay(
        &mut suggestions,
        &catalog,
        &normalized_query,
        one_off,
        policy,
    );
    append_complete_overlay(
        &mut suggestions,
        &catalog,
        &normalized_query,
        one_off,
        policy,
    );
    if !normalized_query.is_empty() {
        let (label, disabled_reason) = match validation {
            Ok(label) => (label, None),
            Err(error) => (
                normalized_query.clone(),
                Some(match error {
                    crate::native::task_relations::label::LabelError::ReservedMention => {
                        "💬 is reserved for text mentions"
                    }
                    crate::native::task_relations::label::LabelError::Empty => {
                        "label can't be empty"
                    }
                    crate::native::task_relations::label::LabelError::TooLong => {
                        "label can't exceed 40 characters"
                    }
                    crate::native::task_relations::label::LabelError::ForbiddenCharacter => {
                        "label contains a forbidden character"
                    }
                    crate::native::task_relations::label::LabelError::InvalidStart => {
                        "label can't start with a middle dot or comma"
                    }
                }
                .to_string()),
            ),
        };
        let key = crate::native::relation_labels::relation_label_key(&label);
        let existing = catalog.get(&key);
        let canonical_label =
            existing.map_or(label, |record| record.name.clone());
        let hidden =
            existing.is_some_and(|record| catalog.is_hidden(&record.key));
        let replacement = if disabled_reason.is_some() {
            String::new()
        } else if one_off {
            crate::native::task_relations::label::relation_label_hide_form(
                &canonical_label,
            )
        } else {
            crate::native::task_relations::label::relation_label_quoted_form(
                &canonical_label,
            )
        };
        let hide_replacement =
            crate::native::task_relations::label::relation_label_hide_form(
                &canonical_label,
            );
        let inverse = crate::native::task_relations::label::inverse_phrase(
            &canonical_label,
        );
        suggestions.retain(|item| item.key != key);
        suggestions.insert(
            0,
            RelationLabelSuggestion {
                replacement,
                hide_replacement,
                label: canonical_label,
                key: key.clone(),
                inverse,
                count: existing.map(|record| record.count as usize),
                typed: true,
                disabled_reason,
                existing: Some(existing.is_some()),
                hidden: Some(hidden),
                cancels_source: policy.is_cancel_label(&key),
                completes_source: policy.is_complete_label(&key),
                configured: false,
            },
        );
    }
    let mut warnings = catalog.warnings().to_vec();
    warnings.extend(policy.warnings.iter().cloned());
    (
        Candidates::RelationLabel(suggestions),
        warnings,
        catalog.hidden_count(),
    )
}

/// Zero-use configured cancel labels, in config order, ranked last.
/// Hidden labels stay out; the typed query still matches them exactly
/// through the typed row above.
fn append_cancel_overlay(
    suggestions: &mut Vec<RelationLabelSuggestion>,
    catalog: &crate::native::relation_labels::Catalog,
    normalized_query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) {
    if policy.cancel_labels.is_empty() {
        return;
    }
    let query_key = crate::native::task_relations::label::collapse_label_text(
        normalized_query,
    )
    .to_lowercase();
    for cancel in &policy.cancel_labels {
        if suggestions.iter().any(|item| item.key == cancel.key) {
            continue;
        }
        if catalog.get(&cancel.key).is_some() {
            continue;
        }
        if catalog.is_hidden(&cancel.key) {
            continue;
        }
        if !query_key.is_empty()
            && !crate::native::task_relations::label::collapse_label_text(
                &cancel.name,
            )
            .to_lowercase()
            .contains(&query_key)
        {
            continue;
        }
        suggestions.push(RelationLabelSuggestion {
            replacement: if one_off {
                crate::native::task_relations::label::relation_label_hide_form(
                    &cancel.name,
                )
            } else {
                crate::native::task_relations::label::relation_label_quoted_form(
                    &cancel.name,
                )
            },
            hide_replacement:
                crate::native::task_relations::label::relation_label_hide_form(
                    &cancel.name,
                ),
            label: cancel.name.clone(),
            key: cancel.key.clone(),
            inverse:
                crate::native::task_relations::label::inverse_phrase(
                    &cancel.name,
                ),
            count: None,
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: true,
            completes_source: false,
            configured: true,
        });
    }
}

/// Zero-use configured complete labels, in config order, ranked last.
/// Hidden labels stay out; a typed query still matches them through the
/// typed row above.
fn append_complete_overlay(
    suggestions: &mut Vec<RelationLabelSuggestion>,
    catalog: &crate::native::relation_labels::Catalog,
    normalized_query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) {
    if policy.complete_labels.is_empty() {
        return;
    }
    let query_key = crate::native::task_relations::label::collapse_label_text(
        normalized_query,
    )
    .to_lowercase();
    for complete in &policy.complete_labels {
        if suggestions.iter().any(|item| item.key == complete.key)
            || catalog.get(&complete.key).is_some()
            || catalog.is_hidden(&complete.key)
        {
            continue;
        }
        if !query_key.is_empty()
            && !crate::native::task_relations::label::collapse_label_text(
                &complete.name,
            )
            .to_lowercase()
            .contains(&query_key)
        {
            continue;
        }
        suggestions.push(RelationLabelSuggestion {
            replacement: if one_off {
                crate::native::task_relations::label::relation_label_hide_form(
                    &complete.name,
                )
            } else {
                crate::native::task_relations::label::relation_label_quoted_form(
                    &complete.name,
                )
            },
            hide_replacement:
                crate::native::task_relations::label::relation_label_hide_form(
                    &complete.name,
                ),
            label: complete.name.clone(),
            key: complete.key.clone(),
            inverse:
                crate::native::task_relations::label::inverse_phrase(
                    &complete.name,
                ),
            count: None,
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: false,
            completes_source: true,
            configured: true,
        });
    }
}

fn is_done_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "done")
}

fn is_relation_source_read_only(
    bob_dir: &Path,
    task: &capture_dependency_tasks::DependencyTask,
) -> bool {
    let path = Path::new(&task.note_path);
    is_done_path(path)
        || task_status_hooks::canonical_daily_date(path).is_some()
        || fs::metadata(bob_dir.join(path))
            .is_ok_and(|metadata| metadata.permissions().readonly())
}

fn is_editable_open_task(
    task: &capture_dependency_tasks::DependencyTask,
) -> bool {
    if !task.open || !task_complete::is_completable_status(task.status_symbol) {
        return false;
    }
    let path = Path::new(&task.note_path);
    if task_status_hooks::canonical_daily_date(path).is_some() {
        return false;
    }
    !path
        .components()
        .any(|component| component.as_os_str() == "done")
}

fn blocked_task_group(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
    owner_note: &str,
) -> &'static str {
    if task_has_today_link(task, ledger) {
        "today"
    } else if task.status_symbol == '/' {
        "in_progress"
    } else if task.status_symbol == '*' {
        "next"
    } else if task.note_path == owner_note {
        "in_this_note"
    } else {
        "open"
    }
}

fn blocked_task_order_key(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
    owner_note: &str,
) -> (u8, usize, usize) {
    if let Some((entry, link)) = task_ledger_position(task, ledger) {
        let running = task
            .block_id
            .as_ref()
            .and_then(|id| {
                ledger
                    .owners
                    .get(&(task_route(&task.note_path), id.clone()))
            })
            .is_some_and(|pomodoro| pomodoro.is_current);
        (if running { 0 } else { 1 }, entry, link)
    } else if task.status_symbol == '/' {
        (2, 0, 0)
    } else if task.status_symbol == '*' {
        (3, 0, 0)
    } else if task.note_path == owner_note {
        (4, 0, 0)
    } else {
        (5, 0, 0)
    }
}

fn task_has_today_link(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
) -> bool {
    task_ledger_position(task, ledger).is_some()
}

fn task_ledger_position(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
) -> Option<(usize, usize)> {
    let id = task.block_id.as_ref()?;
    ledger.position(&(task_route(&task.note_path), id.clone()))
}

fn task_route(note_path: &str) -> String {
    let mut path = PathBuf::from(note_path);
    path.set_extension("");
    path.to_string_lossy().replace('\\', "/")
}

fn dependency_reachable(
    graph: &HashMap<
        capture_dependency_tasks::TaskIdentity,
        HashSet<capture_dependency_tasks::TaskIdentity>,
    >,
    roots: &[capture_dependency_tasks::TaskIdentity],
) -> HashSet<capture_dependency_tasks::TaskIdentity> {
    let mut reachable = HashSet::new();
    let mut pending = roots.to_vec();
    while let Some(task) = pending.pop() {
        if !reachable.insert(task.clone()) {
            continue;
        }
        if let Some(prerequisites) = graph.get(&task) {
            pending.extend(prerequisites.iter().cloned());
        }
    }
    reachable
}

fn read_note_cached(
    bob_dir: &Path,
    path: &Path,
    cache: &mut HashMap<PathBuf, String>,
) -> Option<String> {
    if !cache.contains_key(path) {
        let contents = fs::read_to_string(bob_dir.join(path)).ok()?;
        cache.insert(path.to_path_buf(), contents);
    }
    cache.get(path).cloned()
}

fn is_recognized_project(
    relative: &Path,
    contents: &str,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
) -> bool {
    let tasks = task_status_hooks::parse_tasks(contents, settings);
    project_dependencies::ProjectContext::for_note(
        &task_status_hooks::FileScan {
            path: bob_dir.join(relative),
            relative_path: relative.to_path_buf(),
            contents: contents.to_string(),
            tasks,
        },
    )
    .recognised
}

/// `task_complete` candidates for a whole-item `!` token: every open
/// task (Ready, Blocked, Next, In Progress) in the vault-wide
/// completable catalog, in today-first picker order (today rows by
/// Pomodoro role, then In Progress, Next, and other open tasks) and
/// ranked by the query with today matches on top. ID-less and guarded
/// (recurring, already-selected) rows carry an empty replacement the
/// client must never insert: ID-less rows resolve through the explicit
/// Add block ID flow via `capture-task-id`'s `complete_replacement`.
/// Read-only: selecting or highlighting a row writes nothing. A
/// missing day file means no today rows and no warning.
pub(super) fn task_complete_candidates(
    bob_dir: &Path,
    query: &str,
    raw_text: &str,
    cursor: usize,
) -> (Candidates, Vec<String>) {
    use std::path::Path as StdPath;
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let day_file = pomodoro::day_file_for(bob_dir);
    let catalog =
        capture_completable_tasks::discover(bob_dir, &discovered, &day_file);
    let selected = capture_completable_tasks::draft_selected(
        &discovered,
        bob_dir,
        raw_text,
        cursor,
    );
    let ordered =
        capture_completable_tasks::order_for_picker(&catalog.tasks, query);
    let candidates = ordered
        .into_iter()
        .map(|task| {
            let requires_block_id = task.block_id.is_none();
            let already = task.block_id.as_ref().is_some_and(|_| {
                selected.contains(&(
                    task.note_path.clone(),
                    task.block_id.clone().unwrap_or_default(),
                ))
            });
            let disabled_reason = if task.recurring {
                Some(
                    capture_completable_tasks::RECURRING_DISABLED_REASON
                        .to_string(),
                )
            } else if already {
                Some(
                    capture_completable_tasks::ALREADY_SELECTED_DISABLED_REASON
                        .to_string(),
                )
            } else {
                None
            };
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for_sigil(
                    &catalog.index,
                    StdPath::new(&task.note_path),
                    task.block_id.as_deref().expect("identified task"),
                    b'!',
                )
            };
            TaskCompleteCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: task.group,
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                recurring: task.recurring,
                already_selected: already,
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                scheduled: task.scheduled.clone(),
                today: task.today.clone(),
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::TaskComplete(candidates), catalog.warnings)
}

pub(super) enum TaskSectionLookupFailure {
    MissingNote,
    Missing { suggestion: Option<String> },
    Duplicate(usize),
    NotATask,
}

/// One warning, no draft text, and no task description.
pub(super) fn unresolvable_parent_warning(
    route: &str,
    block_id: &str,
    failure: TaskSectionLookupFailure,
) -> String {
    match failure {
        TaskSectionLookupFailure::MissingNote => {
            format!("note does not exist: {route}.md")
        }
        TaskSectionLookupFailure::Missing { suggestion } => {
            match suggestion {
                Some(suggestion) => format!(
                    "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}?"
                ),
                None => {
                    format!("no task with block ID ^{block_id} in {route}.md")
                }
            }
        }
        TaskSectionLookupFailure::Duplicate(count) => {
            format!(
                "block ID ^{block_id} appears {count} times in {route}.md"
            )
        }
        TaskSectionLookupFailure::NotATask => {
            format!("^{block_id} in {route}.md is not a task")
        }
    }
}

pub(super) fn task_search_fields(
    task: &note_tasks::NoteTask,
    search: TaskSearch,
) -> Vec<String> {
    match search {
        TaskSearch::BlockIdOnly => task.block_id.iter().cloned().collect(),
        TaskSearch::MultiField => {
            let mut fields = Vec::new();
            if let Some(block_id) = &task.block_id {
                fields.push(block_id.clone());
            }
            fields.push(task.description.clone());
            if let Some(section) = &task.section {
                fields.push(section.clone());
            }
            fields.push(task.status_name.clone());
            fields.push(task.status_symbol.to_string());
            fields
        }
    }
}

/// Read one routed note's contents; a missing note is not an error, exactly
/// like `capture-sections` and `capture-tasks`.
pub(super) fn read_target(
    bob_dir: &Path,
    route: &str,
) -> Result<String, CompleteError> {
    let target = bob_dir.join(capture::route_label(route));
    match fs::read_to_string(&target) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(String::new())
        }
        Err(error) => Err(CompleteError::io(format!(
            "read target {}: {error}",
            target.display()
        ))),
    }
}

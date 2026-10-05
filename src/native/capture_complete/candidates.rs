use std::{fs, io, path::Path};

use super::{
    model::{
        ActiveTaskCandidate, ActiveTaskPomodoroCandidate, Candidates,
        CompleteError, DependencyCandidate, RouteCandidate, SectionCandidate,
        TaskCandidate, TaskCompleteCandidate, TaskLinkCandidate,
        TaskParentCandidate, TaskSectionCandidate,
    },
    support::rank,
};
use crate::native::{
    capture, capture_active_tasks, capture_block_ids,
    capture_completable_tasks, capture_dependency_tasks,
    capture_language::{DependencyTarget, DependencyTargetKind},
    capture_link_tasks, capture_targets, capture_task_sections, capture_tasks,
    note_tasks::{self, BlockIdLookup},
    pomodoro,
};

pub(super) fn route_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<Candidates, CompleteError> {
    let report = capture_targets::scan_capture_targets(bob_dir);
    if !report.issues.is_empty() {
        return Err(CompleteError::io(report.issue_summary()));
    }

    let ranked = rank(report.targets, query, |target| target.route.as_str());
    Ok(Candidates::Route(
        ranked
            .into_iter()
            .map(|target| RouteCandidate {
                replacement: target.route.clone(),
                route: target.route,
                label: target.label,
                kind: target.kind,
                status: target.status,
            })
            .collect(),
    ))
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
                    section: task.section.clone(),
                    depth: capture_tasks::indentation_depth(&task.indentation),
                    child_count: task.child_count,
                    line: task.line_index + 1,
                    pomodoro: None,
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
                section: task.section.clone(),
                depth: capture_tasks::indentation_depth(&task.indentation),
                child_count: task.child_count,
                line: task.line_index + 1,
                pomodoro,
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
        })
        .collect();
    (Candidates::TaskLink(candidates), discovered.warnings)
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
            }
        })
        .collect();
    (Candidates::TaskParent(candidates), discovered.warnings)
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
            }
        })
        .collect();
    (Candidates::Dependency(candidates), discovered.warnings)
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

//! Keep-moving net reporting across a draft (`docs/task-dependencies.md`
//! §12.15, KM21).
//!
//! The per-row SL20 checks in [`super::task_complete`] already drop an
//! offered row that a later item completes, re-blocks, links, or drops.
//! This module adds the *group* rule: a later item that links any open
//! resident of N, or closes (or re-blocks) N's `^prj`, drops N's whole
//! group. Group invalidation needs provenance, because the picks the
//! originating close itself linked are live residents of N in the final
//! day text and must never invalidate their own group. Provenance is the
//! item order: this tracer keeps the staged day text after every item once
//! a keep-moving offer exists, so a later item's link effects are the live
//! links it added over the previous step.
//!
//! Group invalidation changes reporting only. Task edits an earlier item
//! legitimately performed (picked links, stamps, minted IDs) stay written.
use super::*;
use crate::native::task_complete as engine;
use std::collections::BTreeSet;

type LiveKeys = BTreeSet<(PathBuf, String)>;

/// Staged day text and explicit link target after each item, recorded only
/// from the first item that reported a keep-moving or relation offer
/// onward, so drafts without either offer pay nothing. Relation-only
/// drafts must activate the tracer too, or a later item's relink of a
/// parked target would be invisible to the batches net pass (SA21).
#[derive(Debug, Default)]
pub(super) struct KeepMovingTrace {
    active: bool,
    steps: BTreeMap<usize, TraceStep>,
}

#[derive(Debug)]
struct TraceStep {
    day_after: Option<String>,
    explicit_link: Option<(PathBuf, String)>,
}

impl KeepMovingTrace {
    /// Record one planned item. `index` is the item's position in the
    /// draft; reference items never touch the day file and are skipped by
    /// the caller, which leaves a gap the diff walk treats as "no change".
    pub(super) fn record(
        &mut self,
        index: usize,
        item: &PlannedCaptureItem,
        planner: &CaptureBatchPlanner,
        day_file: &Path,
    ) {
        if !self.active {
            let has_offer = super::output::all_keep_moving(&item.result)
                .any(|row| row.offer.is_some())
                || super::output::all_relation_successors(&item.result)
                    .any(|row| row.offer.is_some());
            if !has_offer {
                return;
            }
            self.active = true;
        }
        self.steps.insert(
            index,
            TraceStep {
                day_after: planner.peek_text(day_file),
                explicit_link: explicit_link_key(&item.result),
            },
        );
    }
}

/// The `(note, block id)` an item explicitly linked into a Pomodoro, even
/// when the link was already current (so the live set did not change).
fn explicit_link_key(
    result: &super::output::CaptureItemResult,
) -> Option<(PathBuf, String)> {
    if result.kind != "pomodoro_link" {
        return None;
    }
    let block_id = result.block_id.as_ref()?;
    Some((PathBuf::from(&result.relative_target), block_id.clone()))
}

/// The project a keep-moving row belongs to, as its display path.
pub(super) fn row_project(
    row: &super::output::TaskCompleteUnblockedJson,
) -> String {
    row.offer
        .as_ref()
        .and_then(|offer| offer.project.clone())
        .unwrap_or_else(|| row.note_path.clone())
}

/// Drop every keep-moving group that a later item invalidated (KM21).
pub(super) fn drop_invalidated_groups(
    items: &mut [PlannedCaptureItem],
    trace: &KeepMovingTrace,
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &DependencyContext,
    bob_dir: &Path,
) {
    if !trace.active {
        return;
    }
    let any_rows = items.iter().any(|item| {
        super::output::all_keep_moving(&item.result)
            .any(|row| row.offer.is_some())
    });
    if !any_rows {
        return;
    }
    let day_file = pomodoro::day_file_for(bob_dir);
    let day_relative = capture_pomodoros::relative_day_file(&day_file, bob_dir);
    let index = super::task_complete::staged_note_index(
        bob_dir,
        planner,
        dependency_ctx,
    );
    let live: BTreeMap<usize, LiveKeys> = trace
        .steps
        .iter()
        .filter_map(|(item, step)| {
            let text = step.day_after.as_deref()?;
            Some((
                *item,
                engine::live_link_keys(text, Path::new(&day_relative), &index),
            ))
        })
        .collect();
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);

    let mut invalid: Vec<(usize, String)> = Vec::new();
    for (position, item) in items.iter().enumerate() {
        // Group the item's rows by project; the picks are the links the
        // originating close wrote, which never invalidate their own group.
        let mut groups: BTreeMap<String, BTreeSet<(PathBuf, String)>> =
            BTreeMap::new();
        for row in super::output::all_keep_moving(&item.result) {
            if row.offer.is_none() {
                continue;
            }
            let picks = groups.entry(row_project(row)).or_default();
            if row.link.is_some() {
                picks.insert((
                    PathBuf::from(&row.note_path),
                    row.block_id.clone(),
                ));
            }
        }
        if groups.is_empty() {
            continue;
        }
        let later = later_link_effects(position, items.len(), trace, &live);
        for (project, picks) in groups {
            if group_invalidated(
                &project,
                &picks,
                &later,
                planner,
                bob_dir,
                &tasks_settings,
            ) {
                invalid.push((position, project));
            }
        }
    }
    for (position, project) in invalid {
        for (rows, folds) in
            super::output::keep_moving_parts_mut(&mut items[position].result)
        {
            rows.retain(|row| row_project(row) != project);
            folds.retain(|fold| display_relative(&fold.project) != project);
        }
    }
}

/// Link effects of every item after `position`: the live links it added
/// over the previous recorded step, plus its explicit link target.
fn later_link_effects(
    position: usize,
    total: usize,
    trace: &KeepMovingTrace,
    live: &BTreeMap<usize, LiveKeys>,
) -> LiveKeys {
    let mut effects = LiveKeys::new();
    for later in (position + 1)..total {
        if let Some(current) = live.get(&later)
            && let Some((_, previous)) = live.range(..later).next_back()
        {
            effects.extend(current.difference(previous).cloned());
        }
        if let Some(step) = trace.steps.get(&later)
            && let Some(key) = step.explicit_link.as_ref()
        {
            effects.insert(key.clone());
        }
    }
    effects
}

/// Whether a later item linked an open resident of the project that is not
/// one of the originating close's own picks, or left its `^prj` closed or
/// Blocked.
fn group_invalidated(
    project: &str,
    picks: &BTreeSet<(PathBuf, String)>,
    later: &LiveKeys,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> bool {
    let project_path = PathBuf::from(project);
    let Ok(Some(contents)) = planner.current_contents(&bob_dir.join(project))
    else {
        return false;
    };
    let tasks = crate::native::task_status_hooks::parse_tasks(
        &contents,
        tasks_settings,
    );
    let is_open = |task: &crate::native::task_status_hooks::TaskLine| {
        task.status_recognized && task.status_type.is_open()
    };
    // KM21: a later item closed N's ^prj (or re-blocked it, which K5
    // refuses the same way).
    let root_open = tasks.iter().any(|task| {
        crate::native::project_dependencies::is_project_task_id(
            task.block_id.as_deref(),
        ) && is_open(task)
            && task.status != '?'
    });
    if !root_open {
        return true;
    }
    later.iter().any(|(path, block_id)| {
        *path == project_path
            && !picks.contains(&(path.clone(), block_id.clone()))
            && tasks.iter().any(|task| {
                task.block_id.as_deref() == Some(block_id.as_str())
                    && is_open(task)
            })
    })
}

/// Drop relation rows and parked groups a later item invalidated (SA21).
///
/// Two rules share the keep-moving provenance mechanism, so an
/// originating close's own picks never invalidate their own groups:
///
/// - Later live-linking a parked T drops that T's parked group. Rows
///   naming several triggers keep the surviving targets; a row whose
///   winning group is not relation keeps its independent reason with
///   the relation metadata cleared, while a relation-winning row with
///   no surviving trigger is dropped.
/// - Later linking S drops S's row outright, including picked rows a
///   final-live check alone would keep when the relink leaves the live
///   set unchanged (the trace records explicit link targets).
///
/// Group invalidation changes reporting only; staged picks stay linked.
pub(super) fn drop_invalidated_relation_groups(
    items: &mut [PlannedCaptureItem],
    trace: &KeepMovingTrace,
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &DependencyContext,
    bob_dir: &Path,
) {
    if !trace.active {
        return;
    }
    let any_rows = items.iter().any(|item| {
        super::output::all_relation_successors(&item.result)
            .any(|row| row.offer.is_some())
    });
    if !any_rows {
        return;
    }
    let day_file = pomodoro::day_file_for(bob_dir);
    let day_relative = capture_pomodoros::relative_day_file(&day_file, bob_dir);
    let index = super::task_complete::staged_note_index(
        bob_dir,
        planner,
        dependency_ctx,
    );
    let live: BTreeMap<usize, LiveKeys> = trace
        .steps
        .iter()
        .filter_map(|(item, step)| {
            let text = step.day_after.as_deref()?;
            Some((
                *item,
                engine::live_link_keys(text, Path::new(&day_relative), &index),
            ))
        })
        .collect();
    for position in 0..items.len() {
        let has_relation =
            super::output::all_relation_successors(&items[position].result)
                .any(|row| row.offer.is_some());
        if !has_relation {
            continue;
        }
        let later = later_link_effects(position, items.len(), trace, &live);
        if later.is_empty() {
            continue;
        }
        // Later linking S drops S's row even when the originating pick
        // already made S live; own picks are absent from `later`, so
        // they never invalidate their own rows. This runs for every
        // relation offer, parked or done.
        prune_relation_rows_for_source_links(
            &mut items[position].result,
            &later,
        );
        let parked: BTreeSet<(PathBuf, String)> =
            parked_targets(&items[position].result);
        let invalidated: BTreeSet<(PathBuf, String)> =
            parked.intersection(&later).cloned().collect();
        if invalidated.is_empty() {
            continue;
        }
        prune_relation_rows_for_targets(
            &mut items[position].result,
            &invalidated,
        );
        prune_relation_groups_for_targets(
            &mut items[position].result,
            &invalidated,
        );
    }
}

/// Parked event targets in an item's relation groups, as live-link keys.
fn parked_targets(
    result: &super::output::CaptureItemResult,
) -> BTreeSet<(PathBuf, String)> {
    let mut targets = BTreeSet::new();
    let groups: Vec<&crate::native::task_complete::RelationOfferGroup> =
        result
            .relation_groups
            .iter()
            .chain(
                result
                    .task_complete
                    .iter()
                    .flat_map(|summary| summary.relation_groups.iter()),
            )
            .chain(result.relation_completes.iter().flat_map(|complete| {
                complete.task_complete.relation_groups.iter()
            }))
            .chain(
                result
                    .pomodoro_close
                    .iter()
                    .flat_map(|summary| summary.relation_groups.iter()),
            )
            .collect();
    for group in groups {
        if group.event
            == crate::native::task_complete::relation_successors::RelationEventKind::Parked
        {
            targets.insert((
                group.target.note_path.clone(),
                group.target.block_id.clone(),
            ));
        }
    }
    targets
}

/// Drop relation rows whose source a later item linked. Own picks are
/// absent from `later`, so they never invalidate their own rows.
fn prune_relation_rows_for_source_links(
    result: &mut super::output::CaptureItemResult,
    later: &BTreeSet<(PathBuf, String)>,
) {
    for vec in super::output::relation_successors_vecs_mut(result) {
        vec.retain(|row| {
            !later.contains(&(
                PathBuf::from(&row.note_path),
                row.block_id.clone(),
            ))
        });
    }
}

/// Prune invalidated parked targets from relation rows: a parked T
/// prunes its entry from the row's trigger list, dropping
/// relation-winning rows with no surviving trigger and clearing
/// second-reason metadata on rows whose winning group is independent.
fn prune_relation_rows_for_targets(
    result: &mut super::output::CaptureItemResult,
    invalidated: &BTreeSet<(PathBuf, String)>,
) {
    for vec in super::output::relation_successors_vecs_mut(result) {
        let mut dropped: Vec<bool> = Vec::with_capacity(vec.len());
        for row in vec.iter_mut() {
            let Some(relation) = row.relation.as_mut() else {
                dropped.push(false);
                continue;
            };
            if relation.event != "parked" {
                dropped.push(false);
                continue;
            }
            let before = relation.after.len();
            relation.after.retain(|target| {
                !invalidated.contains(&(
                    PathBuf::from(&target.note_path),
                    target.block_id.clone(),
                ))
            });
            if relation.after.len() == before {
                dropped.push(false);
                continue;
            }
            if relation.after.is_empty() {
                let winning_relation = row
                    .offer
                    .as_ref()
                    .is_some_and(|offer| offer.group == "relation");
                if winning_relation {
                    dropped.push(true);
                } else {
                    row.relation = None;
                    dropped.push(false);
                }
                continue;
            }
            // Keep the surviving triggers; the offer's single `after`
            // follows the new first trigger for relation winners.
            if let Some(offer) = row.offer.as_mut()
                && offer.group == "relation"
            {
                let first = &relation.after[0];
                offer.after =
                    Some(format!("{}#^{}", first.note_path, first.block_id));
            }
            dropped.push(false);
        }
        let mut kept = Vec::with_capacity(vec.len());
        for (row, drop) in vec.drain(..).zip(dropped) {
            if !drop {
                kept.push(row);
            }
        }
        *vec = kept;
    }
}

/// Prune parked groups whose target a later item live-linked.
fn prune_relation_groups_for_targets(
    result: &mut super::output::CaptureItemResult,
    invalidated: &BTreeSet<(PathBuf, String)>,
) {
    for vec in super::output::relation_groups_mut(result) {
        vec.retain(|group| {
            if group.event
                != crate::native::task_complete::relation_successors::RelationEventKind::Parked
            {
                return true;
            }
            !invalidated.contains(&(
                group.target.note_path.clone(),
                group.target.block_id.clone(),
            ))
        });
    }
}

/// Drop folded counts whose project no longer has a surviving row: a group
/// with no selectable rows is silent, folds included.
pub(super) fn prune_empty_folds(items: &mut [PlannedCaptureItem]) {
    for item in items.iter_mut() {
        for (rows, folds) in
            super::output::keep_moving_parts_mut(&mut item.result)
        {
            folds.retain(|fold| {
                let project = display_relative(&fold.project);
                rows.iter().any(|row| row_project(row) == project)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str, id: &str) -> (PathBuf, String) {
        (PathBuf::from(path), id.to_string())
    }

    fn live(keys: &[(&str, &str)]) -> LiveKeys {
        keys.iter().map(|(path, id)| key(path, id)).collect()
    }

    fn step(explicit: Option<(&str, &str)>) -> TraceStep {
        TraceStep {
            day_after: Some(String::new()),
            explicit_link: explicit.map(|(path, id)| key(path, id)),
        }
    }

    // KM21: only links a LATER item added count as later effects; the
    // originating item's own picks (already live at its step) never do.
    #[test]
    fn later_effects_are_the_links_added_after_the_group_item() {
        let mut trace = KeepMovingTrace::default();
        trace.steps.insert(0, step(None));
        trace.steps.insert(1, step(None));
        trace.steps.insert(2, step(None));
        let lives = BTreeMap::from([
            (0, live(&[("n.md", "a"), ("n.md", "b")])),
            (1, live(&[("n.md", "a"), ("n.md", "b")])),
            (2, live(&[("n.md", "a"), ("n.md", "b"), ("n.md", "x")])),
        ]);
        assert_eq!(
            later_link_effects(0, 3, &trace, &lives),
            live(&[("n.md", "x")])
        );
        // From the last item there is nothing later.
        assert!(later_link_effects(2, 3, &trace, &lives).is_empty());
    }

    // A re-link that left the live set unchanged still counts through the
    // item's explicit link target.
    #[test]
    fn explicit_relinks_count_even_when_the_live_set_is_unchanged() {
        let mut trace = KeepMovingTrace::default();
        trace.steps.insert(0, step(None));
        trace.steps.insert(1, step(Some(("n.md", "a"))));
        let lives = BTreeMap::from([
            (0, live(&[("n.md", "a")])),
            (1, live(&[("n.md", "a")])),
        ]);
        assert_eq!(
            later_link_effects(0, 2, &trace, &lives),
            live(&[("n.md", "a")])
        );
    }

    // Reference items leave a gap in the trace; the diff walk compares an
    // item against the nearest earlier recorded step.
    #[test]
    fn gaps_compare_against_the_nearest_earlier_step() {
        let mut trace = KeepMovingTrace::default();
        trace.steps.insert(0, step(None));
        trace.steps.insert(2, step(None));
        let lives = BTreeMap::from([
            (0, live(&[("n.md", "a")])),
            (2, live(&[("n.md", "a"), ("n.md", "c")])),
        ]);
        assert_eq!(
            later_link_effects(0, 3, &trace, &lives),
            live(&[("n.md", "c")])
        );
    }

    #[test]
    fn row_project_falls_back_to_the_row_note() {
        let mut row = super::super::output::TaskCompleteUnblockedJson {
            note_path: "n.md".to_string(),
            block_id: "a".to_string(),
            line: 1,
            text: String::new(),
            previous_status_symbol: ' ',
            previous_status_name: String::new(),
            status_symbol: ' ',
            status_name: String::new(),
            inbox: false,
            unblocked_by: Vec::new(),
            link: None,
            not_linked: None,
            trigger: "last_planned",
            hide_removed: false,
            offer: None,
            relation: None,
        };
        assert_eq!(row_project(&row), "n.md");
        row.note_path = "elsewhere.md".to_string();
        assert_eq!(row_project(&row), "elsewhere.md");
    }
}

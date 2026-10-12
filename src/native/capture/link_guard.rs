//! Link-guard assessment for gestures that would create or keep a live
//! Task Link (`docs/task-dependencies.md` §13.5 gesture rule, PDG1–PDG6).
//!
//! A gesture that would link a task which is Blocked after the gesture
//! follows one rule:
//!
//! - only the task's own future `scheduled` blocks it → pull forward as
//!   today (retire the schedule, link it; PDG2);
//! - any dependency or project-dependency reason → do not link; promote
//!   the open prerequisites (own plus inherited) and report it (PDG1);
//! - a `project_schedule` reason without a dependency reason → do not
//!   link; report the project date (PDG3);
//! - a `^prj` target whose only reason is its own date → refuse, with
//!   "move the project date from the `^prj` Schedule action": never a
//!   pull-forward, which would silently unblock the project and strand
//!   residents' copies (PDG6).
//!
//! Reasons derive from the staged capture graph (line links plus
//! openness plus inline dates), never from the `[?]` cache (§13.3).
//! Reconcile stays the authority: a missed immediate write is repaired
//! within one reconcile interval.

use std::collections::BTreeMap;

use super::*;
use crate::native::project_dependencies::{BlockReason, ProjectContext};
use crate::native::projects::edits::normalize_prj_is_blocked_tag;
use crate::native::task_status_hooks;

/// One effective-blocking reason behind a guard verdict: its §13.9
/// kind for JSON plus a human label for terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockedReason {
    pub(super) kind: &'static str,
    pub(super) label: String,
}

/// What a link gesture does about its target's effective reasons.
pub(super) enum LinkGuardVerdict {
    /// No reasons: link as today.
    Link,
    /// Only the target's own future `scheduled` (and not a `^prj`):
    /// the existing `plan_task_link` pull-forward retires it.
    PullForward,
    /// Blocked: skip the link, promote the seeds, report. Boxed: the
    /// plan carries the whole staged graph.
    Consume(Box<ConsumePlan>),
}

/// A consumption decision: the reasons to report, the open
/// prerequisite identities (own plus inherited `^prj` links) to
/// promote, the lane to raise them to, and the staged graph to
/// promote along.
pub(super) struct ConsumePlan {
    pub(super) reasons: Vec<BlockedReason>,
    pub(super) seeds: Vec<(String, String)>,
    pub(super) lane: u8,
    pub(super) graph: DependencyGraph,
}

/// Whether this task has any local dependency input that needs openness
/// resolution. A non-empty field or a managed Depends-On link makes the
/// full graph necessary; no such input proves the task has no local
/// dependency blocker.
fn has_dependency_inputs(
    contents: &str,
    task: &task_status_hooks::TaskLine,
) -> bool {
    let lines: Vec<&str> = contents.lines().collect();
    let Some(line) = lines.get(task.line_index).copied() else {
        return true;
    };
    let metadata =
        task_status_hooks::task_metadata(line, task.block_id.as_deref());
    if !metadata.depends_on.is_empty() {
        return true;
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let Some(child) =
        crate::native::task_dependencies::parse::dependency_child_of(
            &lines,
            &fenced,
            task.line_index,
        )
    else {
        return false;
    };
    match &child.parsed {
        crate::native::task_dependencies::parse::DependencyLine::Accepted {
            links,
            ..
        } => !links.is_empty(),
        crate::native::task_dependencies::parse::DependencyLine::Empty => false,
        crate::native::task_dependencies::parse::DependencyLine::Malformed
        | crate::native::task_dependencies::parse::DependencyLine::NotALine => {
            true
        }
    }
}

/// Assess one link target's effective reasons against the staged
/// capture graph. Returns `Link` when nothing blocks the target,
/// `PullForward` when only its own future schedule does (and the
/// caller runs the retiring `plan_task_link`), `Consume` otherwise,
/// and a usage refusal for a date-only `^prj` (PDG6).
pub(super) fn assess_link_target(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target_rel: &Path,
    block_id: &str,
    today: NaiveDate,
) -> Result<LinkGuardVerdict, CaptureError> {
    let note_display = display_relative(target_rel);
    let identity = (note_display.clone(), block_id.to_string());

    let contents = current_note_text(planner, &bob_dir.join(target_rel))?;
    let file_tasks = task_status_hooks::parse_tasks(&contents, ctx.settings());
    let context = ProjectContext::for_note(&task_status_hooks::FileScan {
        path: bob_dir.join(target_rel),
        relative_path: target_rel.to_path_buf(),
        contents: contents.clone(),
        tasks: file_tasks.clone(),
    });
    let target_task = file_tasks
        .iter()
        .find(|task| task.block_id.as_deref() == Some(block_id));

    let is_prj = block_id == "prj" && context.recognised;
    let is_resident = target_task.is_some_and(|task| context.is_resident(task));

    // Most link gestures target an ordinary, unblocked task. Check its
    // own note first so those previews do not build a vault-wide graph.
    // A resident also inherits the local inputs on its note's ^prj line.
    let task_has_local_inputs = target_task.is_none_or(|task| {
        task.status == '?'
            || has_dependency_inputs(&contents, task)
            || task.scheduled.is_some_and(|date| date > today)
    });
    let prj_task = context
        .project_task_index()
        .and_then(|index| file_tasks.get(index));
    let project_has_local_inputs = is_resident
        && prj_task.is_none_or(|task| {
            task.status == '?'
                || has_dependency_inputs(&contents, task)
                || task.scheduled.is_some_and(|date| date > today)
        });
    if !task_has_local_inputs && !project_has_local_inputs {
        return Ok(LinkGuardVerdict::Link);
    }

    let index = super::dependencies::staged_note_index(ctx, planner, bob_dir);
    let graph = DependencyGraph::build(ctx, planner, bob_dir, &index)?;

    // Own open prerequisites: the target's staged line links. A
    // resident inherits `^prj`'s projected links as implicit edges in
    // the graph, so subtract those to keep own reasons own.
    let edges: Vec<(String, String)> = graph
        .edges
        .get(&identity)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let prj_identity = (note_display.clone(), "prj".to_string());
    let prj_targets: Vec<(String, String)> = graph
        .edges
        .get(&prj_identity)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let is_open =
        |id: &(String, String)| graph.open.get(id).copied().unwrap_or(false);
    let mut own_open: Vec<(String, String)> = edges
        .iter()
        .filter(|id| is_open(id) && !(is_resident && prj_targets.contains(id)))
        .cloned()
        .collect();
    own_open.sort();
    own_open.dedup();
    let mut inherited_open: Vec<(String, String)> = if is_resident {
        prj_targets
            .iter()
            .filter(|id| is_open(id))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    inherited_open.sort();
    inherited_open.dedup();

    let own_scheduled: Option<NaiveDate> = graph
        .scheduled
        .get(&identity)
        .copied()
        .flatten()
        .filter(|date| *date > today);
    let prj_scheduled: Option<NaiveDate> = if is_resident {
        graph
            .scheduled
            .get(&prj_identity)
            .copied()
            .flatten()
            .filter(|date| *date > today)
    } else {
        None
    };

    let mut reasons = Vec::new();
    if !own_open.is_empty() {
        reasons.push(BlockedReason {
            kind: BlockReason::LocalDependency {
                open_ids: Vec::new(),
            }
            .kind(),
            label: format!(
                "waits on {}",
                own_open
                    .iter()
                    .map(|id| format!("[[{}#^{}]]", id.0, id.1))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    if let Some(date) = own_scheduled {
        reasons.push(BlockedReason {
            kind: BlockReason::LocalSchedule { date }.kind(),
            label: format!("scheduled {date}"),
        });
    }
    if !inherited_open.is_empty() {
        reasons.push(BlockedReason {
            kind: BlockReason::ProjectDependency {
                project: target_rel.to_path_buf(),
                open_ids: Vec::new(),
            }
            .kind(),
            label: format!(
                "project waits on {}",
                inherited_open
                    .iter()
                    .map(|id| format!("[[{}#^{}]]", id.0, id.1))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    if let Some(date) = prj_scheduled {
        reasons.push(BlockedReason {
            kind: BlockReason::ProjectSchedule {
                project: target_rel.to_path_buf(),
                date,
            }
            .kind(),
            label: format!("project scheduled {date}"),
        });
    }

    if reasons.is_empty() {
        return Ok(LinkGuardVerdict::Link);
    }

    // PDG2: only the task's own future schedule blocks it, and it is
    // not a `^prj`: the pull-forward retires that single field. The
    // single-field shape gates it: with several `scheduled` fields
    // the toggle retires nothing, so that stays a consumption.
    let only_own_date = own_scheduled.is_some()
        && own_open.is_empty()
        && inherited_open.is_empty()
        && prj_scheduled.is_none();
    if only_own_date {
        if is_prj {
            // PDG6: refusing a date-only `^prj` is never a
            // pull-forward, which would silently unblock the project
            // and strand residents' copies.
            return Err(CaptureError::usage(format!(
                "cannot link ^{block_id} in {note_display}: its own [scheduled:: {}] blocks the project; move the project date from the `^prj` Schedule action",
                own_scheduled.expect("gated above"),
            )));
        }
        let lines: Vec<&str> = contents.lines().collect();
        let single = target_task
            .and_then(|task| lines.get(task.line_index).copied())
            .is_some_and(|line| {
                capture_task_toggle::task_update_has_single_future_scheduled(
                    line, today,
                )
            });
        if single {
            return Ok(LinkGuardVerdict::PullForward);
        }
    }

    let mut seeds = own_open;
    seeds.extend(inherited_open);
    seeds.sort();
    seeds.dedup();
    let status = graph.status.get(&identity).copied().unwrap_or(' ');
    let lane = lane_for_status(status).unwrap_or(0).max(1);
    Ok(LinkGuardVerdict::Consume(Box::new(ConsumePlan {
        reasons,
        seeds,
        lane,
        graph,
    })))
}

/// Assess a destination note for a task capture creates: `Some` plan
/// with the project's reasons and `^prj`'s open seeds when the note
/// is a recognised project that is blocked after the gesture, `None`
/// otherwise. The new task is not in the graph yet; the caller flips
/// its line to `[?]` plus the tag and promotes the returned seeds.
pub(super) fn assess_destination_project(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target_rel: &Path,
    today: NaiveDate,
) -> Result<Option<ConsumePlan>, CaptureError> {
    let absolute = bob_dir.join(target_rel);
    let contents = match planner.current_contents(&absolute)? {
        Some(staged) => staged,
        None => match std::fs::read_to_string(&absolute) {
            Ok(disk) => disk,
            Err(_) => return Ok(None),
        },
    };
    let file_tasks = task_status_hooks::parse_tasks(&contents, ctx.settings());
    let context = ProjectContext::for_note(&task_status_hooks::FileScan {
        path: absolute,
        relative_path: target_rel.to_path_buf(),
        contents: contents.clone(),
        tasks: file_tasks.clone(),
    });
    if !context.recognised {
        return Ok(None);
    }
    let Some(prj_task) = context
        .project_task_index()
        .and_then(|index| file_tasks.get(index))
    else {
        return Ok(None);
    };
    if prj_task.status != '?'
        && !has_dependency_inputs(&contents, prj_task)
        && prj_task.scheduled.is_none_or(|date| date <= today)
    {
        return Ok(None);
    }
    let index = super::dependencies::staged_note_index(ctx, planner, bob_dir);
    let graph = DependencyGraph::build(ctx, planner, bob_dir, &index)?;
    let note_display = display_relative(target_rel);
    let prj_identity = (note_display, "prj".to_string());
    let mut open: Vec<(String, String)> = graph
        .edges
        .get(&prj_identity)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|id| graph.open.get(id).copied().unwrap_or(false))
        .collect();
    open.sort();
    open.dedup();
    let scheduled: Option<NaiveDate> = graph
        .scheduled
        .get(&prj_identity)
        .copied()
        .flatten()
        .filter(|date| *date > today);
    if open.is_empty() && scheduled.is_none() {
        return Ok(None);
    }
    let mut reasons = Vec::new();
    if !open.is_empty() {
        reasons.push(BlockedReason {
            kind: BlockReason::ProjectDependency {
                project: target_rel.to_path_buf(),
                open_ids: Vec::new(),
            }
            .kind(),
            label: format!(
                "project waits on {}",
                open.iter()
                    .map(|id| format!("[[{}#^{}]]", id.0, id.1))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    if let Some(date) = scheduled {
        reasons.push(BlockedReason {
            kind: BlockReason::ProjectSchedule {
                project: target_rel.to_path_buf(),
                date,
            }
            .kind(),
            label: format!("project scheduled {date}"),
        });
    }
    Ok(Some(ConsumePlan {
        reasons,
        seeds: open,
        lane: 1,
        graph,
    }))
}

/// Promote a consumption plan's seeds along the staged graph and
/// report the touched identities as `route#^block` labels.
pub(super) fn promote_consume_plan(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    plan: ConsumePlan,
    dependent_desc: &str,
) -> Result<Vec<String>, CaptureError> {
    if plan.seeds.is_empty() {
        return Ok(Vec::new());
    }
    let touches = plan_promotions(
        ctx,
        planner,
        bob_dir,
        &plan.graph,
        &BTreeMap::new(),
        plan.lane,
        &plan.seeds,
        dependent_desc,
    )?;
    Ok(touches
        .iter()
        .map(|touch| {
            format!("{}#^{}", touch.relative.display(), touch.block_id)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn assess_simple_target(contents: &str, block_id: &str) -> usize {
        let vault = tempfile::tempdir().expect("temporary vault");
        fs::write(vault.path().join("cash.md"), contents)
            .expect("write target note");
        let today = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let context = DependencyContext::new(vault.path(), today);
        let mut planner = CaptureBatchPlanner::default();
        let verdict = assess_link_target(
            &context,
            &mut planner,
            vault.path(),
            Path::new("cash.md"),
            block_id,
            today,
        )
        .expect("assess link target");
        assert!(matches!(verdict, LinkGuardVerdict::Link));
        context.graph_build_count()
    }

    #[test]
    fn ordinary_unblocked_target_skips_the_vault_graph() {
        assert_eq!(
            assess_simple_target("- [ ] #task Work ^work\n", "work"),
            0,
            "the target note is sufficient for an unblocked task"
        );
    }

    #[test]
    fn unblocked_project_resident_skips_the_vault_graph() {
        let contents = concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n\n",
            "- [ ] #task Ship project #prj #hide ^prj\n",
            "- [ ] #task Work ^work\n",
        );
        assert_eq!(
            assess_simple_target(contents, "work"),
            0,
            "the local ^prj line proves this resident is not inherited-Blocked"
        );
    }

    #[test]
    fn capture_into_unblocked_project_skips_the_vault_graph() {
        let vault = tempfile::tempdir().expect("temporary vault");
        let contents = concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n\n",
            "- [ ] #task Ship project #prj #hide ^prj\n",
            "- [ ] #task Work ^work\n",
        );
        fs::write(vault.path().join("cash.md"), contents)
            .expect("write project note");
        let today = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let context = DependencyContext::new(vault.path(), today);
        let mut planner = CaptureBatchPlanner::default();
        let blocked = assess_destination_project(
            &context,
            &mut planner,
            vault.path(),
            Path::new("cash.md"),
            today,
        )
        .expect("assess project destination");
        assert!(blocked.is_none());
        assert_eq!(context.graph_build_count(), 0);
    }

    #[test]
    fn new_task_flip_blocks_open_lines_tags_blocked_and_skips_closed() {
        // PDG5: every open resident lands `[?]` plus exactly one tag.
        assert_eq!(
            apply_new_task_project_block(
                "- [ ] #task Work [created::2026-07-10] ^work"
            )
            .as_deref(),
            Some(
                "- [?] #task Work [created::2026-07-10] #prj_is_blocked ^work"
            )
        );
        assert_eq!(
            apply_new_task_project_block(
                "- [*] #task Work [created::2026-07-10] ^work"
            )
            .as_deref(),
            Some(
                "- [?] #task Work [created::2026-07-10] #prj_is_blocked ^work"
            )
        );
        // An existing `[?]` only gains the tag, once.
        assert_eq!(
            apply_new_task_project_block(
                "- [?] #task Work [created::2026-07-10] #prj_is_blocked ^work"
            )
            .as_deref(),
            Some(
                "- [?] #task Work [created::2026-07-10] #prj_is_blocked ^work"
            )
        );
        // Closed lines are never edited; non-task lines return `None`.
        assert_eq!(
            apply_new_task_project_block(
                "- [x] #task Done [created::2026-07-10] ^done"
            ),
            None
        );
        assert_eq!(apply_new_task_project_block("ordinary prose"), None);
    }
}

/// Flip a new task's first line to Blocked plus the machine-owned tag
/// (PDG5): open Ready/Next/In Progress lines become `[?]`, an existing
/// `[?]` only gains the tag, and closed or custom-status lines come
/// back untouched. Returns `None` when the line is not a task line.
pub(super) fn apply_new_task_project_block(line: &str) -> Option<String> {
    let indent_len = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let rest = line.get(indent_len..)?;
    let symbol = rest.strip_prefix("- [")?.chars().next()?;
    if !rest[3 + symbol.len_utf8()..].starts_with(']') {
        return None;
    }
    match symbol {
        ' ' | '*' | '/' => {
            let blocked = replace_status_char(line, '?')?;
            Some(normalize_prj_is_blocked_tag(&blocked, true))
        }
        '?' => Some(normalize_prj_is_blocked_tag(line, true)),
        _ => None,
    }
}

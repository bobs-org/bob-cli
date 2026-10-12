//! Project side effects for `&` dependency captures on `^prj`
//! (`docs/task-dependencies.md` §13.6, §13.7, §13.10).
//!
//! `&` with a `^prj` dependent keeps the ordinary Depends-On write and,
//! when any new target is open, blocks the whole project note in the same
//! staged write: `^prj` and every open resident flip to `[?]`, residents
//! gain exactly one `#prj_is_blocked`, and their live links leave today's
//! open Pomodoros (emptied entries go with them). Same-note targets refuse:
//! a project cannot wait on its own tasks.

use super::*;
use crate::native::{
    project_dependencies::ProjectContext,
    projects::edits::normalize_prj_is_blocked_tag,
    task_status_hooks::{self, FileScan, TaskLine},
};

/// The exact `^prj` block id the line model recognises. Matching stays
/// exact (like `ProjectContext`): resident side effects only run for
/// recognised projects, so a wider case-insensitive match would attach
/// project semantics to notes the model does not recognise.
pub(super) fn is_prj_dependent(block_id: &str) -> bool {
    block_id == "prj"
}

/// The recognised project context for an `&` dependent, with its
/// pre-write task parse for commitment transfer. `None` for ordinary
/// dependents and for `^prj` lines in notes the model does not
/// recognise (terminal, daily, or ambiguous notes).
pub(super) fn prj_context_for_dependent(
    ctx: &DependencyContext,
    dep_rel: &Path,
    dep_abs: &Path,
    contents: &str,
    dependent_block_id: &str,
) -> Option<(ProjectContext, Vec<TaskLine>)> {
    if !is_prj_dependent(dependent_block_id) {
        return None;
    }
    let tasks = task_status_hooks::parse_tasks(contents, ctx.settings());
    let context = ProjectContext::for_note(&FileScan {
        path: dep_abs.to_path_buf(),
        relative_path: dep_rel.to_path_buf(),
        contents: contents.to_string(),
        tasks,
    });
    if !context.recognised {
        return None;
    }
    let tasks = task_status_hooks::parse_tasks(contents, ctx.settings());
    Some((context, tasks))
}

/// Refuse same-note targets for a `^prj` dependent (§13.6): a project
/// cannot wait on its own tasks. Only new links refuse; pre-existing
/// hand-authored internal links stay on the line unprojected.
pub(super) fn refuse_internal_prj_targets(
    dep_rel: &Path,
    dep_desc: &str,
    prereqs: &[ResolvedPrerequisite],
) -> Result<(), CaptureError> {
    for prereq in prereqs {
        if prereq.relative.as_path() == dep_rel {
            return Err(CaptureError::usage(format!(
                "cannot add dependency for {dep_desc}: ^{block} lives in the same project note (inside this project); a project cannot wait on its own tasks",
                block = prereq.block_id,
            )));
        }
    }
    Ok(())
}

/// Drop the field projection of pre-existing same-note links on a `^prj`
/// line (§13.6): hand-authored internal targets stay on the line but are
/// never projected into `[dependsOn::]`. Returns the kept count for the
/// `project_internal_dependency` warning.
pub(super) fn deproject_internal_prj_links(
    dep_rel: &Path,
    merged: &mut [MergedMember],
) -> usize {
    let here = display_relative(dep_rel);
    let mut kept = 0usize;
    for member in merged.iter_mut() {
        if member.identity.0 == here && member.projected_id.is_some() {
            member.projected_id = None;
            kept += 1;
        }
    }
    kept
}

/// Strongest pre-capture lane among `^prj` and its open residents (§13.4
/// commitment transfer): each open prerequisite rises to at least this
/// lane. Closed tasks and non-residents never contribute.
pub(super) fn strongest_project_lane(
    context: &ProjectContext,
    tasks: &[TaskLine],
) -> Option<u8> {
    let mut best: Option<u8> = None;
    for task in tasks {
        let is_prj = task.block_id.as_deref() == Some("prj");
        if !is_prj && !context.is_resident(task) {
            continue;
        }
        if let Some(lane) = lane_for_status(task.status) {
            best = Some(best.map_or(lane, |known: u8| known.max(lane)));
        }
    }
    best
}

/// One staged resident edit: the resident id plus its rebuilt line.
pub(super) struct ResidentEdit {
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
}

/// Flip every open resident to `[?]` and ensure exactly one
/// `#prj_is_blocked` (PDT1, PDT8, PDT9). Closed lines are never touched
/// (PDT7); `^prj` is the caller's own write and is never tagged (PDT6).
/// Returns the staged line edits plus every open resident id for link
/// consumption, whether or not its line changed.
pub(super) fn plan_resident_block(
    context: &ProjectContext,
    tasks: &[TaskLine],
    staged_lines: &[String],
) -> (Vec<ResidentEdit>, Vec<String>) {
    let mut edits = Vec::new();
    let mut residents = Vec::new();
    for task in tasks {
        if !context.is_resident(task) {
            continue;
        }
        let Some(block_id) = task.block_id.clone() else {
            continue;
        };
        residents.push(block_id.clone());
        let Some(line) = staged_lines.get(task.line_index) else {
            continue;
        };
        let blocked =
            replace_status_char(line, '?').unwrap_or_else(|| line.clone());
        let tagged = normalize_prj_is_blocked_tag(&blocked, true);
        if tagged != *line {
            edits.push(ResidentEdit {
                block_id,
                line: task.line_index,
                text: tagged,
            });
        }
    }
    (edits, residents)
}

/// Remove every `[[route#^id]]` live link from today's open Pomodoros,
/// then drop emptied entries (§13.5). A missing day file is nothing to
/// consume. Returns the removed link count.
pub(super) fn consume_live_links(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    route: &str,
    block_ids: &[String],
) -> Result<usize, CaptureError> {
    if block_ids.is_empty() {
        return Ok(0);
    }
    let day_absolute = pomodoro::day_file_for(bob_dir);
    let before = match planner.current_contents(&day_absolute)? {
        Some(staged) => staged,
        None => match fs::read_to_string(&day_absolute) {
            Ok(disk) => disk,
            Err(_) => return Ok(0),
        },
    };
    let mut contents = before.clone();
    let mut removed = 0usize;
    for block_id in block_ids {
        let block_link = format!("[[{route}#^{block_id}]]");
        let plan =
            capture_task_toggle::plan_link_removal(&contents, &block_link);
        contents = plan.content;
        removed += plan.removed_links;
    }
    if contents == before {
        return Ok(removed);
    }
    let before_lines = task_status_hooks::logical_lines(&before);
    if let Some(section) = pomodoro::pomodoros_section_range(&before_lines) {
        let original =
            task_status_hooks::scan_pomodoros(&before_lines, section);
        let emptied = task_status_hooks::plan_empty_pomodoro_removals(
            &contents, &original,
        );
        contents =
            task_status_hooks::apply_empty_pomodoro_plan(&contents, &emptied);
    }
    planner.stage(&day_absolute, contents)?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn settings() -> note_tasks::NoteTaskSettings {
        note_tasks::NoteTaskSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', task_status_hooks::TaskStatusType::Todo),
                ('x', task_status_hooks::TaskStatusType::Done),
                ('X', task_status_hooks::TaskStatusType::Done),
                ('/', task_status_hooks::TaskStatusType::InProgress),
                ('*', task_status_hooks::TaskStatusType::OnHold),
                ('?', task_status_hooks::TaskStatusType::OnHold),
                ('-', task_status_hooks::TaskStatusType::Cancelled),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        }
    }

    fn project_context(body: &str) -> (ProjectContext, Vec<TaskLine>) {
        let contents =
            format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}");
        let tasks = task_status_hooks::parse_tasks(&contents, &settings());
        let context = ProjectContext::for_note(&FileScan {
            path: PathBuf::from("/vault/proj.md"),
            relative_path: PathBuf::from("proj.md"),
            contents: contents.clone(),
            tasks,
        });
        assert!(context.recognised, "fixture must be a project");
        let tasks = task_status_hooks::parse_tasks(&contents, &settings());
        (context, tasks)
    }

    #[test]
    fn resident_block_flips_tags_and_skips_prj_and_closed() {
        // PDT1/PDT6/PDT7/PDT8 (§13.18).
        let (context, tasks) = project_context(concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [ ] #task Work ^work\n",
            "- [?] #task Waiting #prj_is_blocked ^waiting\n",
            "- [x] #task Done #prj_is_blocked ^done\n",
        ));
        let contents = "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Work ^work\n- [?] #task Waiting #prj_is_blocked ^waiting\n- [x] #task Done #prj_is_blocked ^done\n";
        let staged: Vec<String> =
            contents.lines().map(str::to_string).collect();
        let (edits, residents) = plan_resident_block(&context, &tasks, &staged);
        assert_eq!(
            residents,
            vec!["work".to_string(), "waiting".to_string()],
            "open residents only: no ^prj, no closed lines"
        );
        assert_eq!(edits.len(), 1, "only the untagged resident changes");
        assert_eq!(edits[0].block_id, "work");
        assert!(
            edits[0].text.starts_with("- [?] "),
            "resident flips to Blocked: {}",
            edits[0].text
        );
        assert_eq!(
            edits[0].text.matches("#prj_is_blocked").count(),
            1,
            "exactly one tag: {}",
            edits[0].text
        );
        assert!(
            edits[0].text.ends_with("^work"),
            "the tag lands before the block id: {}",
            edits[0].text
        );
    }

    #[test]
    fn strongest_lane_ignores_closed_and_outsiders() {
        let (context, tasks) = project_context(concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [/] #task Grind ^grind\n",
            "- [x] #task Done ^done\n",
        ));
        assert_eq!(strongest_project_lane(&context, &tasks), Some(2));
        let (context, tasks) = project_context(concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [ ] #task Work ^work\n",
        ));
        assert_eq!(strongest_project_lane(&context, &tasks), Some(0));
    }

    #[test]
    fn prj_gate_matches_only_recognised_projects() {
        assert!(is_prj_dependent("prj"));
        assert!(!is_prj_dependent("work"));
        // Case variants are not line-model projects: the capture
        // picker's wider reservation does not drive side effects.
        assert!(!is_prj_dependent("PRJ"));
    }
}

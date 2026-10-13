//! Shared project-surface predicate (`docs/task-dependencies.md` §12.10
//! Terms).
//!
//! The same EMPTY rule `bob projects sync` uses to surface `^prj`, over one
//! staged note text: recognition, plannability, the effective status, the
//! sync schedule, and the recounted effective-unhidden count come from the
//! factored sync helpers, so the close-gesture pull-in and sync cannot
//! drift. Sub-projects come from R's sync-maintained marker line instead of
//! child notes' `parent:` frontmatter, which would need a vault-wide walk.

use std::path::Path;

use chrono::NaiveDate;

use super::super::task_status_hooks::canonical_daily_date;
use super::*;

/// Surface state of one project note N (§12.10 Terms).
#[derive(Debug, Clone)]
pub(crate) struct ProjectSurface {
    /// 1-based line number of R's `^prj` line in the staged text: the
    /// planner's R identity (it edits exactly this line).
    pub(crate) prj_line_number: usize,
    /// SCHED(N): sync's project schedule date.
    pub(crate) sched_date: Option<NaiveDate>,
    /// Raw SCHED(N) date (`YYYY-MM-DD`) for row reporting.
    pub(crate) sched_raw: Option<String>,
    /// REMAINING(N): sync's effective-unhidden count on the staged text.
    pub(crate) remaining: usize,
    /// SUBOPEN(N): the marker line lists an open child.
    pub(crate) subopen: bool,
    /// Open children on R's `🧩 **Sub-projects:**` marker, counted with
    /// the same predicate as `subopen` (a `closed_child` stem is ignored).
    pub(crate) open_subprojects: usize,
}

impl ProjectSurface {
    /// EMPTY(N): no remaining work and no open sub-project.
    pub(crate) fn is_empty(&self) -> bool {
        should_surface_project(self.remaining, self.subopen)
    }
}

/// Surface state of the project note at `relative` with staged `contents`,
/// or `None` unless N is recognised and plannable per the Terms: outside
/// `done/`, not a canonical daily note, project frontmatter with
/// non-terminal status, exactly one open `^prj` task R with non-terminal
/// effective status, and a clean scan (no sync issue for the file).
pub(crate) fn project_surface(
    relative: &Path,
    contents: &str,
    today: NaiveDate,
) -> Option<ProjectSurface> {
    project_surface_inner(relative, contents, today, None)
}

/// [`project_surface`] with one sub-project read as closed: the marker
/// entry naming `closed_child` counts as Done for SUBOPEN(N), whatever
/// the marker line still says (`docs/task-dependencies.md` §12.10
/// trigger (c)). The entry matches by lowercased file stem, which is
/// how sync keys marker displays, so `[[N]]`, `[[N|Alias]]`,
/// `[[dir/N]]` and `[[N#Heading]]` all match.
pub(crate) fn project_surface_with_closed_child(
    relative: &Path,
    contents: &str,
    today: NaiveDate,
    closed_child: &Path,
) -> Option<ProjectSurface> {
    project_surface_inner(relative, contents, today, Some(closed_child))
}

fn project_surface_inner(
    relative: &Path,
    contents: &str,
    today: NaiveDate,
    closed_child: Option<&Path>,
) -> Option<ProjectSurface> {
    if is_done_path(relative) || canonical_daily_date(relative).is_some() {
        return None;
    }
    let mut issues = Vec::new();
    let project = parse_project(relative, contents, &mut issues)?;
    if !issues.is_empty() {
        return None;
    }
    if project.status.is_terminal() {
        return None;
    }
    if project.prj_task.state != PrjTaskState::Open {
        return None;
    }
    let mut effective_status = project.status.clone();
    if let Some(target) = project.prj_task.target_status(&project.status)
        && !target.matches(&project.status)
    {
        effective_status = target.as_project_status();
    }
    if effective_status.is_terminal() {
        return None;
    }
    let sync_schedule = project_schedule_for_sync(&project, &effective_status);
    let remaining =
        effective_unhidden_count(&project, sync_schedule.as_ref(), today);
    let closed_stem = closed_child.and_then(|child| {
        child
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.to_ascii_lowercase())
    });
    let open_subprojects = project
        .prj_task
        .sub_block
        .first_marker_line()
        .map(|line| subproject_displays_in_marker_line(&line.trimmed_text))
        .map(|displays| {
            displays
                .iter()
                .filter(|(link_name, display)| {
                    display.state.is_open()
                        && closed_stem.as_deref() != Some(link_name.as_str())
                })
                .count()
        })
        .unwrap_or(0);
    let prj_line = project.task_lines.iter().find(|task| task.is_prj)?;
    Some(ProjectSurface {
        prj_line_number: prj_line.line_number,
        sched_date: sync_schedule.as_ref().map(|sched| sched.date),
        sched_raw: sync_schedule.as_ref().map(|sched| sched.raw.clone()),
        remaining,
        subopen: open_subprojects > 0,
        open_subprojects,
    })
}

/// Whether a vault-relative note path sits under an archived `done/`
/// directory at any depth, mirroring sync's directory exclusion.
fn is_done_path(relative: &Path) -> bool {
    relative
        .components()
        .any(|component| component.as_os_str() == "done")
}

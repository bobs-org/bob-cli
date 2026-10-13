//! Project recognition and effective-blocking reasons
//! (`docs/task-dependencies.md` §13.1, §13.3).
//!
//! Project prerequisites are ordinary Depends-On links on the open `^prj`
//! task. Their projected task ids and openness come from the existing task
//! dependency model; this module only adds the project-resident rule.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::projects::{
    frontmatter_is_project, frontmatter_value, parse_frontmatter, ProjectStatus,
};
use super::super::task_status_hooks::{
    canonical_daily_date, fenced_lines, logical_lines, FileScan, TaskLine,
};

pub(crate) fn is_project_task_id(block_id: Option<&str>) -> bool {
    // Project recognition is exact. Capture reserves case-insensitive
    // look-alikes only to avoid minting confusing ids; `^PRJ` is still
    // an ordinary task everywhere that derives project behavior.
    block_id == Some("prj")
}

/// A note's project shape and the location of its sole open `^prj` task.
#[derive(Debug, Clone)]
pub(crate) struct ProjectContext {
    /// Vault-relative note path.
    pub(crate) relative_path: PathBuf,
    /// Whether the note satisfies `project(T)` in §13.1.
    pub(crate) recognised: bool,
    /// Index into `FileScan::tasks` for the project's open `^prj` task.
    project_task_index: Option<usize>,
    /// Logical line index for the project's open `^prj` task.
    project_task_line_index: Option<usize>,
    /// The project's own inline `scheduled` value.
    pub(crate) scheduled: Option<NaiveDate>,
    /// Fenced logical-line indices; fenced task-looking lines are not
    /// project residents (§13.1).
    fenced: BTreeSet<usize>,
}

impl ProjectContext {
    /// Build the project context for one scanned note.
    pub(crate) fn for_note(file: &FileScan) -> Self {
        let lines = logical_lines(&file.contents);
        let mut context = Self {
            relative_path: file.relative_path.clone(),
            recognised: false,
            project_task_index: None,
            project_task_line_index: None,
            scheduled: None,
            fenced: fenced_lines(&lines, 0..lines.len()),
        };
        let Some(frontmatter) = parse_frontmatter(&file.contents) else {
            return context;
        };
        if !frontmatter_is_project(&frontmatter)
            || ProjectStatus::parse(frontmatter_value(&frontmatter, "status"))
                .is_terminal()
            || canonical_daily_date(&file.relative_path).is_some()
        {
            return context;
        }

        let open_prj = file
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| is_open_prj_task(task, &context.fenced))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [project_task_index] = open_prj.as_slice() else {
            return context;
        };

        context.recognised = true;
        context.project_task_index = Some(*project_task_index);
        context.project_task_line_index =
            Some(file.tasks[*project_task_index].line_index);
        context.scheduled = file.tasks[*project_task_index].scheduled;
        context
    }

    /// Index of the `^prj` task in the scanned note, when this is a project.
    pub(crate) fn project_task_index(&self) -> Option<usize> {
        self.recognised.then_some(self.project_task_index).flatten()
    }

    /// Whether `task` is this note's recognised open `^prj` line.
    pub(crate) fn is_project_task(&self, task: &TaskLine) -> bool {
        self.recognised && self.project_task_line_index == Some(task.line_index)
    }

    /// Whether a logical line sits inside a fenced block: fenced
    /// task-looking lines are never residents and reconcile never
    /// edits them.
    pub(crate) fn is_fenced(&self, line_index: usize) -> bool {
        self.fenced.contains(&line_index)
    }

    /// Whether `task` is an open resident: physically in the note except
    /// `^prj`, including nested and `#ref` tasks; fenced and unrecognised
    /// status lines are excluded (§13.1).
    pub(crate) fn is_resident(&self, task: &TaskLine) -> bool {
        self.recognised
            && task.status_recognized
            && task.status_type.is_open()
            && !is_project_task_id(task.block_id.as_deref())
            && !self.fenced.contains(&task.line_index)
    }
}

/// Whether `task` is an open `^prj` completion-criteria task.
fn is_open_prj_task(task: &TaskLine, fenced: &BTreeSet<usize>) -> bool {
    is_project_task_id(task.block_id.as_deref())
        && task.status_recognized
        && task.status_type.is_open()
        && !fenced.contains(&task.line_index)
}

/// A project's own blocking inputs (§13.3), derived from the ordinary
/// Depends-On line and inline schedule on its `^prj` task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectBlockReasons {
    /// Open prerequisite ids projected from the `^prj` Depends-On line.
    pub(crate) open_dependency_ids: Vec<String>,
    /// Project `scheduled` date when strictly later than today.
    pub(crate) future_scheduled: Option<NaiveDate>,
}

impl ProjectBlockReasons {
    /// Whether the project contributes no reason.
    pub(crate) fn is_empty(&self) -> bool {
        self.open_dependency_ids.is_empty() && self.future_scheduled.is_none()
    }
}

/// Open ids that may block `task`. Internal same-note ids on the sole
/// recognised project task never count (PDR7), even when a dirty
/// `[dependsOn::]` still lists them. An ordinary `^prj`-shaped task in
/// an unrecognised note follows normal dependency rules.
pub(crate) fn blocking_open_dependency_ids(
    file: &FileScan,
    task: &TaskLine,
    open_ids: &[String],
) -> Vec<String> {
    if !is_project_task_id(task.block_id.as_deref()) {
        return open_ids.to_vec();
    }
    let context = ProjectContext::for_note(file);
    if context
        .project_task_index()
        .and_then(|index| file.tasks.get(index))
        .is_some_and(|project_task| project_task.line_index == task.line_index)
    {
        filter_internal_project_ids(file, open_ids)
    } else {
        open_ids.to_vec()
    }
}

/// Drop ids that name a task in `file` itself. Internal `^prj` targets
/// never project (PDR7), so they must not become project reasons even
/// when a field-only adoption or legacy child left them in
/// `[dependsOn::]`.
pub(crate) fn filter_internal_project_ids(
    file: &FileScan,
    open_ids: &[String],
) -> Vec<String> {
    if open_ids.is_empty() {
        return Vec::new();
    }
    let internal = same_note_dependency_ids(file);
    open_ids
        .iter()
        .filter(|id| !internal.contains(id.as_str()))
        .cloned()
        .collect()
}

fn same_note_dependency_ids(file: &FileScan) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for task in &file.tasks {
        if let Some(id) = &task.task_id {
            ids.insert(id.clone());
        }
    }
    ids
}

/// Pure project reasons for one recognised project. Open dependency ids are
/// supplied by the existing task dependency projection for the `^prj` line.
pub(crate) fn project_block_reasons(
    context: &ProjectContext,
    project_open_dependency_ids: &[String],
    today: NaiveDate,
) -> ProjectBlockReasons {
    if !context.recognised {
        return ProjectBlockReasons::default();
    }
    let mut open_dependency_ids = project_open_dependency_ids.to_vec();
    open_dependency_ids.sort();
    open_dependency_ids.dedup();
    ProjectBlockReasons {
        open_dependency_ids,
        future_scheduled: context.scheduled.filter(|date| *date > today),
    }
}

/// Why a task is Blocked, with the §13.9 reason kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockReason {
    /// The task's own Depends-On line has an open prerequisite.
    LocalDependency { open_ids: Vec<String> },
    /// The task's own `scheduled` is after today.
    LocalSchedule { date: NaiveDate },
    /// The task's project has open prerequisites on `^prj`.
    ProjectDependency {
        project: PathBuf,
        open_ids: Vec<String>,
    },
    /// The task's project `^prj` schedule is after today.
    ProjectSchedule { project: PathBuf, date: NaiveDate },
}

impl BlockReason {
    /// The §13.9 reason kind for reconcile, capture, and plugin JSON.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::LocalDependency { .. } => "local_dependency",
            Self::LocalSchedule { .. } => "local_schedule",
            Self::ProjectDependency { .. } => "project_dependency",
            Self::ProjectSchedule { .. } => "project_schedule",
        }
    }
}

/// Pure effective-blocking reasons for one task (§13.3): its own open
/// prerequisites and future schedule, plus its project's own and inherited
/// reasons when it is a resident. A `^prj` reports its own reasons as local
/// reasons and only inherited reasons as project reasons.
pub(crate) fn effective_block_reasons(
    task: &TaskLine,
    local_open_dependency_ids: &[String],
    context: &ProjectContext,
    project_reasons: &ProjectBlockReasons,
    inherited_reasons: &[(PathBuf, ProjectBlockReasons)],
    today: NaiveDate,
) -> Vec<BlockReason> {
    let mut reasons = Vec::new();
    if !local_open_dependency_ids.is_empty() {
        let mut open_ids = local_open_dependency_ids.to_vec();
        open_ids.sort();
        open_ids.dedup();
        reasons.push(BlockReason::LocalDependency { open_ids });
    }
    if let Some(date) = task.scheduled
        && date > today
    {
        reasons.push(BlockReason::LocalSchedule { date });
    }
    if context.is_resident(task) {
        append_project_reasons(
            &mut reasons,
            &context.relative_path,
            project_reasons,
        );
        append_inherited_reasons(&mut reasons, inherited_reasons);
    } else if context.is_project_task(task) {
        append_inherited_reasons(&mut reasons, inherited_reasons);
    }
    reasons
}

fn append_inherited_reasons(
    reasons: &mut Vec<BlockReason>,
    inherited: &[(PathBuf, ProjectBlockReasons)],
) {
    for (project, project_reasons) in inherited {
        append_project_reasons(reasons, project, project_reasons);
    }
}

fn append_project_reasons(
    reasons: &mut Vec<BlockReason>,
    project: &Path,
    project_reasons: &ProjectBlockReasons,
) {
    if !project_reasons.open_dependency_ids.is_empty() {
        reasons.push(BlockReason::ProjectDependency {
            project: project.to_path_buf(),
            open_ids: project_reasons.open_dependency_ids.clone(),
        });
    }
    if let Some(date) = project_reasons.future_scheduled {
        reasons.push(BlockReason::ProjectSchedule {
            project: project.to_path_buf(),
            date,
        });
    }
}

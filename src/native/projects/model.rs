//! Scan and project models, status types, and frontmatter types.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScanReport {
    pub(super) projects: Vec<Project>,
    pub(super) issues: Vec<ScanIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScanIssue {
    pub(super) relative_path: PathBuf,
    pub(super) line_number: Option<usize>,
    pub(super) message: String,
}

impl ScanIssue {
    pub(super) fn path(
        relative_path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            relative_path: relative_path.into(),
            line_number: None,
            message: message.into(),
        }
    }

    pub(super) fn line(
        relative_path: impl Into<PathBuf>,
        line_number: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            relative_path: relative_path.into(),
            line_number: Some(line_number),
            message: message.into(),
        }
    }

    pub(super) fn display(&self) -> String {
        let path = display_path(&self.relative_path);
        match self.line_number {
            Some(line_number) => {
                format!("{path}:{line_number}: {}", self.message)
            }
            None => format!("{path}: {}", self.message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Project {
    pub(super) relative_path: PathBuf,
    pub(super) name: String,
    pub(super) link_name: String,
    pub(super) link_stem: String,
    pub(super) parent_target: Option<String>,
    pub(super) scheduled: Option<ProjectSchedule>,
    pub(super) status: ProjectStatus,
    pub(super) open_task_count: usize,
    pub(super) open_unhidden_count: usize,
    pub(super) dash_visible_count: usize,
    pub(super) task_lines: Vec<ProjectTaskLine>,
    pub(super) prj_task: PrjTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProjectSchedule {
    pub(super) raw: String,
    pub(super) date: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProjectTaskLine {
    pub(super) line_number: usize,
    pub(super) mark: char,
    pub(super) hide_tag_count: usize,
    pub(super) is_prj: bool,
    pub(super) scheduled_field_count: usize,
    pub(super) scheduled_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskSchedulePolicy {
    pub(super) scheduled: NaiveDate,
    pub(super) future: bool,
    pub(super) include_prj: bool,
}

impl TaskSchedulePolicy {
    pub(super) fn for_schedule(
        scheduled: NaiveDate,
        today: NaiveDate,
        task_count: usize,
    ) -> Self {
        let future = scheduled > today;
        Self {
            scheduled,
            future,
            include_prj: future || task_count == 1,
        }
    }

    pub(super) fn prj_hide_needs_change(self, task: ProjectTaskLine) -> bool {
        if !task.is_prj || !self.include_prj {
            return false;
        }
        if self.future {
            task.hide_tag_count != 1
        } else {
            task.hide_tag_count > 0
        }
    }

    pub(super) fn ordinary_schedule_needs_change(
        self,
        task: ProjectTaskLine,
    ) -> bool {
        !task.is_prj
            && is_propagated_schedule_mark(task.mark)
            && task.scheduled_field_count < 2
            && task.scheduled_date.is_none_or(|date| date < self.scheduled)
    }

    pub(super) fn ordinary_hide_needs_removal(
        self,
        task: ProjectTaskLine,
    ) -> bool {
        !task.is_prj
            && task.scheduled_field_count < 2
            && task.hide_tag_count > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectStatus {
    Wip,
    Waiting,
    Done,
    Canceled,
    Other(String),
}

impl ProjectStatus {
    pub(crate) fn parse(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::Wip;
        };
        let normalized = trim_yaml_scalar(value).to_ascii_lowercase();
        match normalized.as_str() {
            "" | "wip" => Self::Wip,
            "waiting" => Self::Waiting,
            "done" => Self::Done,
            "canceled" | "cancelled" => Self::Canceled,
            _ => Self::Other(normalized),
        }
    }

    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Wip => "wip",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Canceled => "canceled",
            Self::Other(value) => value.as_str(),
        }
    }

    pub(super) fn sort_rank(&self) -> usize {
        match self {
            Self::Wip | Self::Other(_) => 0,
            Self::Waiting => 1,
            Self::Done => 2,
            Self::Canceled => 3,
        }
    }

    pub(super) fn is_waiting(&self) -> bool {
        matches!(self, Self::Waiting)
    }

    pub(super) fn is_done(&self) -> bool {
        matches!(self, Self::Done)
    }

    pub(super) fn is_canceled(&self) -> bool {
        matches!(self, Self::Canceled)
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.is_done() || self.is_canceled()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TargetProjectStatus {
    Wip,
    Done,
    Canceled,
}

impl TargetProjectStatus {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Wip => "wip",
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }

    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Wip => "^prj task opened",
            Self::Done => "^prj task checked",
            Self::Canceled => "^prj task canceled",
        }
    }

    pub(super) fn as_project_status(self) -> ProjectStatus {
        match self {
            Self::Wip => ProjectStatus::Wip,
            Self::Done => ProjectStatus::Done,
            Self::Canceled => ProjectStatus::Canceled,
        }
    }

    pub(super) fn matches(self, status: &ProjectStatus) -> bool {
        matches!(
            (self, status),
            (Self::Wip, ProjectStatus::Wip)
                | (Self::Done, ProjectStatus::Done)
                | (Self::Canceled, ProjectStatus::Canceled)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrjTask {
    pub(super) state: PrjTaskState,
    pub(super) scheduled: Option<String>,
    pub(super) description: String,
    pub(super) hidden: bool,
    pub(super) placeholder: bool,
    pub(super) sub_block: PrjSubBlock,
}

impl PrjTask {
    pub(super) fn missing() -> Self {
        Self {
            state: PrjTaskState::Missing,
            scheduled: None,
            description: String::new(),
            hidden: false,
            placeholder: false,
            sub_block: PrjSubBlock::default(),
        }
    }

    pub(super) fn invalid(state: PrjTaskState) -> Self {
        Self {
            state,
            scheduled: None,
            description: String::new(),
            hidden: false,
            placeholder: false,
            sub_block: PrjSubBlock::default(),
        }
    }

    /// Resolves the lifecycle status this `^prj` task targets for `status`.
    ///
    /// Checked and canceled tasks always close the project. An open task only
    /// reopens to `wip` when the parsed frontmatter status is terminal, so
    /// `waiting`, missing, and other non-terminal statuses are left untouched.
    pub(super) fn target_status(
        &self,
        status: &ProjectStatus,
    ) -> Option<TargetProjectStatus> {
        match self.state {
            PrjTaskState::Done => Some(TargetProjectStatus::Done),
            PrjTaskState::Canceled => Some(TargetProjectStatus::Canceled),
            PrjTaskState::Open => {
                status.is_terminal().then_some(TargetProjectStatus::Wip)
            }
            PrjTaskState::Missing
            | PrjTaskState::Malformed
            | PrjTaskState::Multiple => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrjTaskState {
    Missing,
    Open,
    Done,
    Canceled,
    Malformed,
    Multiple,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct PrjSubBlock {
    pub(super) prj_indent: String,
    pub(super) lines: Vec<PrjSubBlockLine>,
}

impl PrjSubBlock {
    pub(super) fn first_marker_line(&self) -> Option<&PrjSubBlockLine> {
        self.lines.iter().find(|line| line.is_marker)
    }

    pub(super) fn marker_line_count(&self) -> usize {
        self.lines.iter().filter(|line| line.is_marker).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrjSubBlockLine {
    pub(super) line_number: usize,
    pub(super) indentation: String,
    pub(super) trimmed_text: String,
    pub(super) is_marker: bool,
    pub(super) links: Vec<WikilinkRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WikilinkRef {
    pub(super) link_name: String,
    pub(super) stem: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WikilinkSpan {
    pub(super) link: WikilinkRef,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SubprojectState {
    Open,
    Done,
    Canceled,
}

impl SubprojectState {
    pub(super) fn from_project(project: &Project) -> Option<Self> {
        if let Some(target) = project.prj_task.target_status(&project.status) {
            return Some(Self::from_target_status(target));
        }
        match project.status {
            ProjectStatus::Done => Some(Self::Done),
            ProjectStatus::Canceled => Some(Self::Canceled),
            ProjectStatus::Wip
            | ProjectStatus::Waiting
            | ProjectStatus::Other(_) => match project.prj_task.state {
                PrjTaskState::Open => Some(Self::Open),
                PrjTaskState::Missing
                | PrjTaskState::Done
                | PrjTaskState::Canceled
                | PrjTaskState::Malformed
                | PrjTaskState::Multiple => None,
            },
        }
    }

    pub(super) fn from_target_status(status: TargetProjectStatus) -> Self {
        match status {
            TargetProjectStatus::Wip => Self::Open,
            TargetProjectStatus::Done => Self::Done,
            TargetProjectStatus::Canceled => Self::Canceled,
        }
    }

    pub(super) fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    pub(super) fn is_terminal(self) -> bool {
        !self.is_open()
    }

    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Open => "open sub-project",
            Self::Done => "sub-project completed",
            Self::Canceled => "sub-project canceled",
        }
    }

    pub(super) fn closed_marker(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::Done => Some(SUBPROJECT_DONE_MARKER),
            Self::Canceled => Some(SUBPROJECT_CANCELED_MARKER),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SubprojectEntry {
    pub(super) link_name: String,
    pub(super) stem: String,
    pub(super) state: SubprojectState,
    pub(super) future_scheduled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SubprojectDisplay {
    pub(super) state: SubprojectState,
    pub(super) future_scheduled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskStatus {
    Open,
    Done,
    Canceled,
}

impl TaskStatus {
    pub(super) fn from_mark(mark: char) -> Self {
        match mark {
            'x' | 'X' => Self::Done,
            '-' => Self::Canceled,
            _ => Self::Open,
        }
    }

    pub(super) fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParsedTaskLine<'a> {
    pub(super) mark: char,
    pub(super) status: TaskStatus,
    pub(super) text: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrjCandidate<'a> {
    pub(super) line_number: usize,
    pub(super) line_index: usize,
    pub(super) line: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct Frontmatter<'a> {
    pub(super) lines: Vec<&'a str>,
    pub(super) body_start_line: usize,
}

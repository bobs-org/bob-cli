use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{
    env as bob_env, is_always_excluded_note_directory_name,
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob projects";
const PLACEHOLDER_CRITERIA: &str =
    "<short_project_completion_criteria_goes_here>";
const PROJECT_TASK_SHAPE: &str =
    "- [ ] #task #prj <completion criteria> #hide ^prj";
const HIDE_TAG: &str = "#hide";
const PROJECT_TASK_TAG: &str = "#prj";
const SUBPROJECTS_MARKER_PREFIX: &str = "🧩 **Sub-projects:**";
const SUBPROJECTS_SEPARATOR: &str = "•";
const SUBPROJECT_DONE_MARKER: &str = "✅";
const SUBPROJECT_CANCELED_MARKER: &str = "❌";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    match matches.subcommand() {
        Some(("list", sub_matches)) => run_list(sub_matches),
        Some(("sync", sub_matches)) => run_sync(sub_matches),
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        None => 2,
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Manage project notes via their ^prj tasks")
        .long_about(
            "Manage Bob project notes through the completion-criteria task \
anchored with ^prj.\n\n\
The list subcommand is read-only: it scans project notes, counts open #task \
items, counts open non-hidden tasks, and shows the current ^prj state. The \
sync subcommand updates project status and manages the ^prj task's #hide tag \
and single Sub-projects line from that same ^prj task.",
        )
        .after_help(
            "Examples:\n  bob projects list\n  bob projects sync --dry-run\n  bob projects sync -b ~/bob",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(list_command())
        .subcommand(sync_command())
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List project notes and their ^prj task state")
        .after_help(
            "Examples:\n  bob projects list\n  bob projects list --bob-dir ~/bob\n  bob projects list -b /tmp/bob-vault",
        )
        .arg(bob_dir_arg())
}

fn sync_command() -> ClapCommand {
    ClapCommand::new("sync")
        .about("Sync project status and ^prj #hide tag from ^prj tasks")
        .long_about(
            "Sync Bob project notes from the completion-criteria task anchored \
with ^prj.\n\n\
A checked ^prj task sets frontmatter status to done. A canceled ^prj task sets \
status to canceled. Active projects with no non-hidden open tasks and no \
open sub-projects have the #hide tag removed from their open ^prj task so \
it surfaces in dash.md's Tasks section; projects with non-hidden open tasks \
or open sub-projects get #hide added back. Sync also maintains a single \
Sub-projects line nested directly under open ^prj tasks.",
        )
        .after_help(
            "Examples:\n  bob projects sync --dry-run\n  bob projects sync -d -b ~/bob\n  bob projects sync --bob-dir /tmp/bob-vault",
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview changes without writing files")
}

fn run_list(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);

    let report = scan_projects(&bob_dir);
    let styler = Styler::detect();
    print_project_list(&report.projects, &styler);

    for issue in &report.issues {
        eprintln!("{COMMAND_NAME}: {}", issue.display());
    }

    if report.issues.is_empty() {
        0
    } else {
        1
    }
}

fn run_sync(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let dry_run = matches.get_flag("dry-run");

    let report = sync_projects(&bob_dir, dry_run);
    let styler = Styler::detect();
    print_sync_report(&report, dry_run, &styler);

    for issue in &report.issues {
        eprintln!("{COMMAND_NAME}: {}", issue.display());
    }

    if report.issues.is_empty() {
        0
    } else {
        1
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanReport {
    projects: Vec<Project>,
    issues: Vec<ScanIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanIssue {
    relative_path: PathBuf,
    line_number: Option<usize>,
    message: String,
}

impl ScanIssue {
    fn path(
        relative_path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            relative_path: relative_path.into(),
            line_number: None,
            message: message.into(),
        }
    }

    fn line(
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

    fn display(&self) -> String {
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
struct Project {
    relative_path: PathBuf,
    name: String,
    link_name: String,
    link_stem: String,
    parent_target: Option<String>,
    status: ProjectStatus,
    open_task_count: usize,
    open_unhidden_count: usize,
    prj_task: PrjTask,
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

    fn sort_rank(&self) -> usize {
        match self {
            Self::Wip | Self::Other(_) => 0,
            Self::Waiting => 1,
            Self::Done => 2,
            Self::Canceled => 3,
        }
    }

    fn is_waiting(&self) -> bool {
        matches!(self, Self::Waiting)
    }

    fn is_done(&self) -> bool {
        matches!(self, Self::Done)
    }

    fn is_canceled(&self) -> bool {
        matches!(self, Self::Canceled)
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.is_done() || self.is_canceled()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetProjectStatus {
    Wip,
    Done,
    Canceled,
}

impl TargetProjectStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Wip => "wip",
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Wip => "^prj task opened",
            Self::Done => "^prj task checked",
            Self::Canceled => "^prj task canceled",
        }
    }

    fn as_project_status(self) -> ProjectStatus {
        match self {
            Self::Wip => ProjectStatus::Wip,
            Self::Done => ProjectStatus::Done,
            Self::Canceled => ProjectStatus::Canceled,
        }
    }

    fn matches(self, status: &ProjectStatus) -> bool {
        matches!(
            (self, status),
            (Self::Wip, ProjectStatus::Wip)
                | (Self::Done, ProjectStatus::Done)
                | (Self::Canceled, ProjectStatus::Canceled)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrjTask {
    state: PrjTaskState,
    scheduled: Option<String>,
    description: String,
    hidden: bool,
    placeholder: bool,
    sub_block: PrjSubBlock,
}

impl PrjTask {
    fn missing() -> Self {
        Self {
            state: PrjTaskState::Missing,
            scheduled: None,
            description: String::new(),
            hidden: false,
            placeholder: false,
            sub_block: PrjSubBlock::default(),
        }
    }

    fn invalid(state: PrjTaskState) -> Self {
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
    fn target_status(
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
enum PrjTaskState {
    Missing,
    Open,
    Done,
    Canceled,
    Malformed,
    Multiple,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PrjSubBlock {
    prj_indent: String,
    lines: Vec<PrjSubBlockLine>,
}

impl PrjSubBlock {
    fn first_marker_line(&self) -> Option<&PrjSubBlockLine> {
        self.lines.iter().find(|line| line.is_marker)
    }

    fn marker_line_count(&self) -> usize {
        self.lines.iter().filter(|line| line.is_marker).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrjSubBlockLine {
    line_number: usize,
    indentation: String,
    trimmed_text: String,
    is_marker: bool,
    links: Vec<WikilinkRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WikilinkRef {
    link_name: String,
    stem: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WikilinkSpan {
    link: WikilinkRef,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubprojectState {
    Open,
    Done,
    Canceled,
}

impl SubprojectState {
    fn from_project(project: &Project) -> Option<Self> {
        if let Some(target) =
            project.prj_task.target_status(&project.status)
        {
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

    fn from_target_status(status: TargetProjectStatus) -> Self {
        match status {
            TargetProjectStatus::Wip => Self::Open,
            TargetProjectStatus::Done => Self::Done,
            TargetProjectStatus::Canceled => Self::Canceled,
        }
    }

    fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    fn is_terminal(self) -> bool {
        !self.is_open()
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Open => "open sub-project",
            Self::Done => "sub-project completed",
            Self::Canceled => "sub-project canceled",
        }
    }

    fn closed_marker(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::Done => Some(SUBPROJECT_DONE_MARKER),
            Self::Canceled => Some(SUBPROJECT_CANCELED_MARKER),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubprojectEntry {
    link_name: String,
    stem: String,
    state: SubprojectState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStatus {
    Open,
    Done,
    Canceled,
}

impl TaskStatus {
    fn from_mark(mark: char) -> Self {
        match mark {
            'x' | 'X' => Self::Done,
            '-' => Self::Canceled,
            _ => Self::Open,
        }
    }

    fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedTaskLine<'a> {
    status: TaskStatus,
    text: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrjCandidate<'a> {
    line_number: usize,
    line_index: usize,
    line: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct Frontmatter<'a> {
    lines: Vec<&'a str>,
    body_start_line: usize,
}

fn scan_projects(bob_dir: &Path) -> ScanReport {
    let mut projects = Vec::new();
    let mut issues = Vec::new();
    scan_directory(bob_dir, bob_dir, &mut projects, &mut issues);
    projects.sort_by(|left, right| {
        left.status
            .sort_rank()
            .cmp(&right.status.sort_rank())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    ScanReport { projects, issues }
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    projects: &mut Vec<Project>,
    issues: &mut Vec<ScanIssue>,
) {
    let entries = match read_sorted_directory(directory) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(ScanIssue::path(
                relative_or_original(root, directory),
                format!("failed to read directory: {error}"),
            ));
            return;
        }
    };

    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                issues.push(ScanIssue::path(
                    relative_or_original(root, &path),
                    format!("failed to inspect path: {error}"),
                ));
                continue;
            }
        };

        if file_type.is_dir() {
            if is_excluded_directory(&path) {
                continue;
            }
            scan_directory(root, &path, projects, issues);
            continue;
        }

        if file_type.is_file() && is_markdown_file(&path) {
            scan_markdown_file(root, &path, projects, issues);
        }
    }
}

fn read_sorted_directory(directory: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());
    Ok(entries)
}

fn scan_markdown_file(
    root: &Path,
    path: &Path,
    projects: &mut Vec<Project>,
    issues: &mut Vec<ScanIssue>,
) {
    let relative_path = relative_or_original(root, path);
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            issues.push(ScanIssue::path(
                relative_path,
                format!("failed to read file: {error}"),
            ));
            return;
        }
    };

    let Some(project) = parse_project(&relative_path, &contents, issues) else {
        return;
    };
    projects.push(project);
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct SyncReport {
    project_count: usize,
    events: Vec<SyncEvent>,
    issues: Vec<ScanIssue>,
}

impl SyncReport {
    fn status_update_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Status { .. }))
            .count()
    }

    fn prj_edit_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::PrjEdit { .. }))
            .count()
    }

    fn warning_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Warning { .. }))
            .count()
    }
}

#[derive(Debug, Clone)]
struct SyncFile {
    path: PathBuf,
    contents: String,
    project: Project,
    can_plan: bool,
}

impl SyncFile {
    fn clean_project(&self) -> Option<&Project> {
        self.can_plan.then_some(&self.project)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SyncEvent {
    Status {
        project_name: String,
        from: String,
        to: String,
        reason: String,
    },
    PrjEdit {
        project_name: String,
        action: PrjEditAction,
        field: String,
        reason: String,
    },
    Warning {
        project_name: String,
        message: String,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrjEditAction {
    Add,
    Remove,
    Update,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ProjectPlan {
    changes: Vec<ProjectChange>,
    warnings: Vec<SyncEvent>,
    desired_subprojects: Option<Vec<SubprojectEntry>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectChange {
    Status {
        from: String,
        to: TargetProjectStatus,
    },
    RemoveHideTag,
    AddHideTag {
        reason: AddHideReason,
    },
    RemoveScheduled {
        scheduled: String,
    },
    AddSubprojectLink {
        stem: String,
        state: SubprojectState,
    },
    RemoveSubprojectLink {
        stem: String,
    },
    MarkSubproject {
        stem: String,
        state: SubprojectState,
    },
    NormalizeSubprojects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddHideReason {
    NonHiddenOpenTasks,
    OpenSubprojects,
}

impl AddHideReason {
    fn label(self) -> &'static str {
        match self {
            Self::NonHiddenOpenTasks => "non-hidden open tasks exist",
            Self::OpenSubprojects => "project has open sub-projects",
        }
    }
}

impl ProjectChange {
    fn event(&self, project_name: &str) -> SyncEvent {
        match self {
            Self::Status { from, to } => SyncEvent::Status {
                project_name: project_name.to_string(),
                from: from.clone(),
                to: to.label().to_string(),
                reason: to.reason().to_string(),
            },
            Self::RemoveHideTag => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Remove,
                field: HIDE_TAG.to_string(),
                reason: "no non-hidden open tasks or open sub-projects"
                    .to_string(),
            },
            Self::AddHideTag { reason } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Add,
                field: HIDE_TAG.to_string(),
                reason: reason.label().to_string(),
            },
            Self::RemoveScheduled { scheduled } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Remove,
                field: format!("[scheduled::{scheduled}]"),
                reason: "scheduled is no longer used".to_string(),
            },
            Self::AddSubprojectLink { stem, state } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Add,
                field: format!("[[{stem}]]"),
                reason: state.reason().to_string(),
            },
            Self::RemoveSubprojectLink { stem, .. } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Remove,
                field: format!("[[{stem}]]"),
                reason: "no longer a sub-project".to_string(),
            },
            Self::MarkSubproject { stem, state } => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Update,
                field: format!("[[{stem}]]"),
                reason: state.reason().to_string(),
            },
            Self::NormalizeSubprojects => SyncEvent::PrjEdit {
                project_name: project_name.to_string(),
                action: PrjEditAction::Update,
                field: "sub-projects".to_string(),
                reason: "canonical format".to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InlineFieldSpan {
    start: usize,
    end: usize,
    value_start: usize,
    value_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextEdit {
    start: usize,
    end: usize,
    replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineSpan {
    line_number: usize,
    start: usize,
    end: usize,
    next_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrontmatterLayout {
    body_start_line: usize,
    type_line: Option<LineSpan>,
    status_line: Option<LineSpan>,
}

fn sync_projects(bob_dir: &Path, dry_run: bool) -> SyncReport {
    let mut report = SyncReport::default();
    let mut files = Vec::new();
    collect_sync_directory(bob_dir, bob_dir, &mut report, &mut files);
    let subproject_children = subproject_children_by_parent_link_name(
        files.iter().filter_map(SyncFile::clean_project),
    );
    apply_sync_plans(&files, &subproject_children, dry_run, &mut report);
    report
}

fn collect_sync_directory(
    root: &Path,
    directory: &Path,
    report: &mut SyncReport,
    files: &mut Vec<SyncFile>,
) {
    let entries = match read_sorted_directory(directory) {
        Ok(entries) => entries,
        Err(error) => {
            report.issues.push(ScanIssue::path(
                relative_or_original(root, directory),
                format!("failed to read directory: {error}"),
            ));
            return;
        }
    };

    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                report.issues.push(ScanIssue::path(
                    relative_or_original(root, &path),
                    format!("failed to inspect path: {error}"),
                ));
                continue;
            }
        };

        if file_type.is_dir() {
            if is_excluded_directory(&path) {
                continue;
            }
            collect_sync_directory(root, &path, report, files);
            continue;
        }

        if file_type.is_file() && is_markdown_file(&path) {
            collect_sync_markdown_file(root, &path, report, files);
        }
    }
}

fn collect_sync_markdown_file(
    root: &Path,
    path: &Path,
    report: &mut SyncReport,
    files: &mut Vec<SyncFile>,
) {
    let relative_path = relative_or_original(root, path);
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            report.issues.push(ScanIssue::path(
                relative_path,
                format!("failed to read file: {error}"),
            ));
            return;
        }
    };

    let issue_count = report.issues.len();
    let Some(project) =
        parse_project(&relative_path, &contents, &mut report.issues)
    else {
        return;
    };
    report.project_count += 1;
    let can_plan = report.issues.len() == issue_count;
    files.push(SyncFile {
        path: path.to_path_buf(),
        contents,
        project,
        can_plan,
    });
}

fn apply_sync_plans(
    files: &[SyncFile],
    subproject_children: &HashMap<String, Vec<SubprojectEntry>>,
    dry_run: bool,
    report: &mut SyncReport,
) {
    for file in files {
        if !file.can_plan {
            continue;
        }

        let children = subproject_children
            .get(&file.project.link_name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let plan = plan_project_sync(&file.project, children);
        report.events.extend(plan.warnings);

        if plan.changes.is_empty() {
            continue;
        }

        if !dry_run {
            let new_contents = match apply_project_changes(
                &file.contents,
                &plan.changes,
                plan.desired_subprojects.as_deref(),
            ) {
                Ok(new_contents) => new_contents,
                Err(message) => {
                    report.issues.push(ScanIssue::path(
                        file.project.relative_path.clone(),
                        message,
                    ));
                    continue;
                }
            };

            if let Err(error) = fs::write(&file.path, new_contents) {
                report.issues.push(ScanIssue::path(
                    file.project.relative_path.clone(),
                    format!("failed to write file: {error}"),
                ));
                continue;
            }
        }

        for change in &plan.changes {
            report.events.push(change.event(&file.project.name));
        }
    }
}

fn subproject_children_by_parent_link_name<'a>(
    projects: impl IntoIterator<Item = &'a Project>,
) -> HashMap<String, Vec<SubprojectEntry>> {
    let mut children_by_parent: HashMap<
        String,
        BTreeMap<String, SubprojectEntry>,
    > = HashMap::new();

    for project in projects {
        let Some(state) = SubprojectState::from_project(project) else {
            continue;
        };
        let Some(parent) = &project.parent_target else {
            continue;
        };
        if parent == &project.link_name {
            continue;
        }
        children_by_parent
            .entry(parent.clone())
            .or_default()
            .entry(project.link_name.clone())
            .or_insert_with(|| SubprojectEntry {
                link_name: project.link_name.clone(),
                stem: project.link_stem.clone(),
                state,
            });
    }

    children_by_parent
        .into_iter()
        .map(|(parent, children)| {
            (parent, children.into_values().collect::<Vec<_>>())
        })
        .collect()
}

fn plan_project_sync(
    project: &Project,
    subproject_children: &[SubprojectEntry],
) -> ProjectPlan {
    let mut plan = ProjectPlan::default();

    if project.prj_task.state == PrjTaskState::Missing
        && !project.status.is_terminal()
    {
        plan.warnings.push(SyncEvent::Warning {
            project_name: project.name.clone(),
            message: "active project has no ^prj task".to_string(),
            detail: format!("add `{PROJECT_TASK_SHAPE}`"),
        });
    }

    if project.prj_task.placeholder {
        plan.warnings.push(SyncEvent::Warning {
            project_name: project.name.clone(),
            message: "^prj task still uses the template placeholder"
                .to_string(),
            detail: "replace it with concrete completion criteria".to_string(),
        });
    }

    let mut effective_status = project.status.clone();
    if let Some(target) = project.prj_task.target_status(&project.status)
        && !target.matches(&project.status)
    {
        plan.changes.push(ProjectChange::Status {
            from: project.status.label().to_string(),
            to: target,
        });
        effective_status = target.as_project_status();
    }

    if !effective_status.is_terminal()
        && project.prj_task.state == PrjTaskState::Open
    {
        let has_open_subprojects = subproject_children
            .iter()
            .any(|child| child.state.is_open());
        let should_surface =
            project.open_unhidden_count == 0 && !has_open_subprojects;
        if should_surface {
            if project.prj_task.hidden {
                plan.changes.push(ProjectChange::RemoveHideTag);
            }
        } else if !project.prj_task.hidden {
            let reason = if project.open_unhidden_count > 0 {
                AddHideReason::NonHiddenOpenTasks
            } else {
                AddHideReason::OpenSubprojects
            };
            plan.changes.push(ProjectChange::AddHideTag { reason });
        }

        let marker_line = project.prj_task.sub_block.first_marker_line();
        let mut marker_targets = HashSet::new();
        let marker_links = marker_line
            .map(|line| {
                line.links
                    .iter()
                    .filter(|link| {
                        marker_targets.insert(link.link_name.clone())
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let marker_display_states = marker_line
            .map(|line| {
                subproject_display_states_in_marker_line(&line.trimmed_text)
            })
            .unwrap_or_default();
        let desired_subprojects =
            desired_subproject_entries(subproject_children, &marker_targets);
        let desired_targets = desired_subprojects
            .iter()
            .map(|entry| entry.link_name.clone())
            .collect::<HashSet<_>>();
        let subproject_change_start = plan.changes.len();

        for entry in &desired_subprojects {
            if !marker_targets.contains(&entry.link_name) {
                plan.changes.push(ProjectChange::AddSubprojectLink {
                    stem: entry.stem.clone(),
                    state: entry.state,
                });
            }
        }

        for link in &marker_links {
            if !desired_targets.contains(&link.link_name) {
                plan.changes.push(ProjectChange::RemoveSubprojectLink {
                    stem: link.stem.clone(),
                });
            }
        }

        for entry in &desired_subprojects {
            if !marker_targets.contains(&entry.link_name) {
                continue;
            }
            let display_state = marker_display_states
                .get(&entry.link_name)
                .copied()
                .unwrap_or(SubprojectState::Open);
            if display_state != entry.state {
                plan.changes.push(ProjectChange::MarkSubproject {
                    stem: entry.stem.clone(),
                    state: entry.state,
                });
            }
        }

        if plan.changes.len() == subproject_change_start
            && subprojects_need_normalization(
                &project.prj_task.sub_block,
                &desired_subprojects,
            )
        {
            plan.changes.push(ProjectChange::NormalizeSubprojects);
        }
        if plan.changes.len() > subproject_change_start {
            plan.desired_subprojects = Some(desired_subprojects);
        }

        if let Some(scheduled) = &project.prj_task.scheduled {
            plan.changes.push(ProjectChange::RemoveScheduled {
                scheduled: scheduled.clone(),
            });
        }
    }

    plan
}

fn desired_subproject_entries(
    children: &[SubprojectEntry],
    marker_targets: &HashSet<String>,
) -> Vec<SubprojectEntry> {
    let mut open_entries = children
        .iter()
        .filter(|child| child.state.is_open())
        .cloned()
        .collect::<Vec<_>>();
    let mut closed_entries = children
        .iter()
        .filter(|child| {
            child.state.is_terminal()
                && marker_targets.contains(&child.link_name)
        })
        .cloned()
        .collect::<Vec<_>>();

    open_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    closed_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    open_entries.extend(closed_entries);
    open_entries
}

fn subproject_display_states_in_marker_line(
    line: &str,
) -> HashMap<String, SubprojectState> {
    let mut states = HashMap::new();
    for span in wikilink_spans_in_line(line) {
        states.entry(span.link.link_name).or_insert_with(|| {
            display_state_for_wikilink(line, span.start, span.end)
        });
    }
    states
}

fn display_state_for_wikilink(
    line: &str,
    link_start: usize,
    link_end: usize,
) -> SubprojectState {
    let before_link = line[..link_start].trim_end();
    let after_link = line[link_end..].trim_start();
    if !before_link.ends_with("~~") {
        return SubprojectState::Open;
    }

    let Some(after_strike) = after_link.strip_prefix("~~") else {
        return SubprojectState::Open;
    };
    let marker = after_strike.trim_start();
    if marker.starts_with(SUBPROJECT_DONE_MARKER) {
        SubprojectState::Done
    } else if marker.starts_with(SUBPROJECT_CANCELED_MARKER) {
        SubprojectState::Canceled
    } else {
        SubprojectState::Open
    }
}

fn subprojects_need_normalization(
    sub_block: &PrjSubBlock,
    desired_entries: &[SubprojectEntry],
) -> bool {
    let marker_count = sub_block.marker_line_count();
    if desired_entries.is_empty() {
        return marker_count > 0;
    }

    let Some(marker_line) = sub_block.first_marker_line() else {
        return false;
    };
    let expected_indent = format!("{}\t", sub_block.prj_indent);
    let expected_text = render_subprojects_line_text(desired_entries);
    marker_count > 1
        || marker_line.indentation != expected_indent
        || marker_line.trimmed_text != expected_text
}

fn apply_project_changes(
    contents: &str,
    changes: &[ProjectChange],
    desired_subprojects: Option<&[SubprojectEntry]>,
) -> Result<String, String> {
    let mut edits = Vec::new();
    let mut sync_subprojects = false;
    for change in changes {
        match change {
            ProjectChange::Status { to, .. } => {
                edits.push(status_edit(contents, to.label())?);
            }
            ProjectChange::RemoveHideTag => {
                edits.push(remove_prj_hide_tag_edit(contents)?);
            }
            ProjectChange::AddHideTag { .. } => {
                edits.push(add_hide_tag_edit(contents)?);
            }
            ProjectChange::RemoveScheduled { .. } => {
                edits
                    .push(remove_prj_inline_field_edit(contents, "scheduled")?);
            }
            ProjectChange::AddSubprojectLink { .. }
            | ProjectChange::RemoveSubprojectLink { .. }
            | ProjectChange::MarkSubproject { .. }
            | ProjectChange::NormalizeSubprojects => {
                sync_subprojects = true;
            }
        }
    }
    if sync_subprojects {
        let desired_subprojects = desired_subprojects.ok_or_else(|| {
            "failed to resolve sub-project line state".to_string()
        })?;
        edits.extend(sync_subprojects_line_edits(
            contents,
            desired_subprojects,
        )?);
    }

    edits.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| right.end.cmp(&left.end))
    });

    let mut output = contents.to_string();
    for edit in edits {
        output.replace_range(edit.start..edit.end, &edit.replacement);
    }
    Ok(output)
}

fn status_edit(contents: &str, status: &str) -> Result<TextEdit, String> {
    let layout = frontmatter_layout(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;

    if let Some(status_line) = layout.status_line {
        return status_value_edit(contents, status_line, status)
            .ok_or_else(|| "failed to locate status value".to_string());
    }

    let type_line = layout
        .type_line
        .ok_or_else(|| "failed to locate project type line".to_string())?;
    Ok(TextEdit {
        start: type_line.next_start,
        end: type_line.next_start,
        replacement: format!(
            "status: {status}{}",
            line_ending(contents, type_line)
        ),
    })
}

fn status_value_edit(
    contents: &str,
    line: LineSpan,
    status: &str,
) -> Option<TextEdit> {
    let line_text = trim_cr(&contents[line.start..line.end]);
    let trimmed = line_text.trim_start();
    let leading_width = line_text.len() - trimmed.len();
    let rest = trimmed.strip_prefix("status")?.strip_prefix(':')?;
    let value_offset = leading_width + "status:".len();
    let leading_value_width = rest.len() - rest.trim_start().len();
    let value_start = value_offset + leading_value_width;
    let value_end = value_offset + rest.trim_end().len();
    let replacement = if leading_value_width == 0 {
        format!(" {status}")
    } else {
        status.to_string()
    };

    Some(TextEdit {
        start: line.start + value_start,
        end: line.start + value_end,
        replacement,
    })
}

fn add_hide_tag_edit(contents: &str) -> Result<TextEdit, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    for line in line_spans(contents) {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = &contents[line.start..line.end];
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        let anchor_start = line_text
            .rfind("^prj")
            .ok_or_else(|| "failed to locate ^prj anchor".to_string())?;
        return Ok(TextEdit {
            start: line.start + anchor_start,
            end: line.start + anchor_start,
            replacement: format!("{HIDE_TAG} "),
        });
    }

    Err("failed to locate ^prj task".to_string())
}

fn remove_prj_hide_tag_edit(contents: &str) -> Result<TextEdit, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    for line in line_spans(contents) {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = &contents[line.start..line.end];
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        let Some((tag_start, tag_end)) = hide_tag_span(line_text) else {
            continue;
        };
        let (start, end) =
            inline_field_removal_range(line_text, tag_start, tag_end);
        return Ok(TextEdit {
            start: line.start + start,
            end: line.start + end,
            replacement: String::new(),
        });
    }

    Err("failed to locate #hide tag on ^prj task".to_string())
}

fn remove_prj_inline_field_edit(
    contents: &str,
    key: &str,
) -> Result<TextEdit, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    for line in line_spans(contents) {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = &contents[line.start..line.end];
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        let Some(field) = inline_field_span(line_text, key) else {
            continue;
        };
        let (start, end) =
            inline_field_removal_range(line_text, field.start, field.end);
        return Ok(TextEdit {
            start: line.start + start,
            end: line.start + end,
            replacement: String::new(),
        });
    }

    Err(format!("failed to locate [{key}::...] field on ^prj task"))
}

fn sync_subprojects_line_edits(
    contents: &str,
    desired_entries: &[SubprojectEntry],
) -> Result<Vec<TextEdit>, String> {
    let layout = prj_sub_block_layout(contents)?;
    let first_marker = layout.sub_block.first_marker_line();

    let marker_line_numbers = layout
        .sub_block
        .lines
        .iter()
        .filter(|line| line.is_marker)
        .map(|line| line.line_number)
        .collect::<Vec<_>>();
    let mut edits = Vec::new();

    if desired_entries.is_empty() {
        for line_number in marker_line_numbers {
            let line = line_by_number(&layout.lines, line_number)?;
            edits.push(TextEdit {
                start: line.start,
                end: line.next_start,
                replacement: String::new(),
            });
        }
        return Ok(edits);
    }

    let rendered = render_subprojects_line(&layout.prj_indent, desired_entries);

    if let Some(marker_line) = first_marker {
        let line = line_by_number(&layout.lines, marker_line.line_number)?;
        edits.push(TextEdit {
            start: line.start,
            end: line_content_end(contents, line),
            replacement: rendered,
        });
        for line_number in marker_line_numbers
            .into_iter()
            .filter(|line_number| *line_number != marker_line.line_number)
        {
            let line = line_by_number(&layout.lines, line_number)?;
            edits.push(TextEdit {
                start: line.start,
                end: line.next_start,
                replacement: String::new(),
            });
        }
    } else {
        let ending = line_ending(contents, layout.prj_line);
        let prj_has_ending = layout.prj_line.next_start > layout.prj_line.end;
        let replacement = if prj_has_ending {
            format!("{rendered}{ending}")
        } else {
            format!("{ending}{rendered}")
        };
        edits.push(TextEdit {
            start: layout.prj_line.next_start,
            end: layout.prj_line.next_start,
            replacement,
        });
    }

    Ok(edits)
}

fn line_by_number(
    lines: &[LineSpan],
    line_number: usize,
) -> Result<LineSpan, String> {
    lines
        .iter()
        .find(|line| line.line_number == line_number)
        .copied()
        .ok_or_else(|| "failed to locate sub-project marker line".to_string())
}

fn line_content_end(contents: &str, line: LineSpan) -> usize {
    if line.end > line.start && contents.as_bytes()[line.end - 1] == b'\r' {
        line.end - 1
    } else {
        line.end
    }
}

fn render_subprojects_line(
    indent: &str,
    entries: &[SubprojectEntry],
) -> String {
    format!("{}\t{}", indent, render_subprojects_line_text(entries))
}

fn render_subprojects_line_text(entries: &[SubprojectEntry]) -> String {
    let links = sorted_subproject_entries(entries)
        .into_iter()
        .map(render_subproject_entry)
        .collect::<Vec<_>>()
        .join(&format!(" {SUBPROJECTS_SEPARATOR} "));
    format!("- {SUBPROJECTS_MARKER_PREFIX} {links}")
}

fn sorted_subproject_entries(
    entries: &[SubprojectEntry],
) -> Vec<&SubprojectEntry> {
    let mut open_entries = entries
        .iter()
        .filter(|entry| entry.state.is_open())
        .collect::<Vec<_>>();
    let mut closed_entries = entries
        .iter()
        .filter(|entry| entry.state.is_terminal())
        .collect::<Vec<_>>();
    open_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    closed_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    open_entries.extend(closed_entries);
    open_entries
}

fn render_subproject_entry(entry: &SubprojectEntry) -> String {
    match entry.state.closed_marker() {
        Some(marker) => format!("~~[[{}]]~~ {marker}", entry.stem),
        None => format!("[[{}]]", entry.stem),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrjSubBlockLayout {
    lines: Vec<LineSpan>,
    prj_line: LineSpan,
    prj_indent: String,
    sub_block: PrjSubBlock,
}

fn prj_sub_block_layout(contents: &str) -> Result<PrjSubBlockLayout, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    let lines = line_spans(contents);
    for (index, line) in lines.iter().enumerate() {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = trim_cr(&contents[line.start..line.end]);
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        return Ok(PrjSubBlockLayout {
            lines: lines.clone(),
            prj_line: *line,
            prj_indent: leading_whitespace(line_text).to_string(),
            sub_block: parse_prj_sub_block(contents, &lines, index),
        });
    }

    Err("failed to locate ^prj task".to_string())
}

fn inline_field_removal_range(
    line_text: &str,
    field_start: usize,
    field_end: usize,
) -> (usize, usize) {
    let bytes = line_text.as_bytes();

    let mut after = field_end;
    while after < bytes.len() && is_inline_field_space(bytes[after]) {
        after += 1;
    }
    if after > field_end {
        return (field_start, after);
    }

    let mut before = field_start;
    while before > 0 && is_inline_field_space(bytes[before - 1]) {
        before -= 1;
    }
    (before, field_end)
}

fn frontmatter_layout(contents: &str) -> Option<FrontmatterLayout> {
    let lines = line_spans(contents);
    let first = lines.first()?;
    if trim_cr(&contents[first.start..first.end]) != "---" {
        return None;
    }

    let mut type_line = None;
    let mut status_line = None;
    for line in lines.iter().skip(1) {
        let line_text = trim_cr(&contents[line.start..line.end]);
        if line_text == "---" {
            return Some(FrontmatterLayout {
                body_start_line: line.line_number,
                type_line,
                status_line,
            });
        }
        if frontmatter_line_has_key(line_text, "type") {
            type_line = Some(*line);
        }
        if frontmatter_line_has_key(line_text, "status") {
            status_line = Some(*line);
        }
    }

    None
}

fn frontmatter_line_has_key(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.starts_with(':'))
}

fn line_spans(contents: &str) -> Vec<LineSpan> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut line_number = 1;

    for (index, byte) in contents.bytes().enumerate() {
        if byte != b'\n' {
            continue;
        }
        lines.push(LineSpan {
            line_number,
            start,
            end: index,
            next_start: index + 1,
        });
        start = index + 1;
        line_number += 1;
    }

    if start < contents.len() {
        lines.push(LineSpan {
            line_number,
            start,
            end: contents.len(),
            next_start: contents.len(),
        });
    }

    lines
}

fn line_ending(contents: &str, line: LineSpan) -> &'static str {
    if line.next_start == line.end {
        return "\n";
    }
    if line.end > line.start && contents.as_bytes()[line.end - 1] == b'\r' {
        "\r\n"
    } else {
        "\n"
    }
}

fn parse_project(
    relative_path: &Path,
    contents: &str,
    issues: &mut Vec<ScanIssue>,
) -> Option<Project> {
    let frontmatter = parse_frontmatter(contents)?;
    if !frontmatter_is_project(&frontmatter) {
        return None;
    }

    let status =
        ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
    let parent_target =
        frontmatter_value(&frontmatter, "parent").and_then(wikilink_target);
    let mut open_task_count = 0;
    let mut open_unhidden_count = 0;
    let mut prj_candidates = Vec::new();
    let lines = line_spans(contents);

    for (line_index, line_span) in lines.iter().enumerate() {
        if line_span.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line = trim_cr(&contents[line_span.start..line_span.end]);
        let has_prj_anchor = has_trailing_prj_anchor(line);
        if has_prj_anchor {
            prj_candidates.push(PrjCandidate {
                line_number: line_span.line_number,
                line_index,
                line,
            });
        }

        let Some(task) = parse_task_line(line) else {
            continue;
        };
        if !contains_task_tag(task.text) || !task.status.is_open() {
            continue;
        }

        open_task_count += 1;
        if !has_prj_anchor && !contains_hide_tag(task.text) {
            open_unhidden_count += 1;
        }
    }

    let sub_block = if prj_candidates.len() == 1 {
        parse_prj_sub_block(contents, &lines, prj_candidates[0].line_index)
    } else {
        PrjSubBlock::default()
    };
    let prj_task =
        classify_prj_task(relative_path, &prj_candidates, sub_block, issues);

    Some(Project {
        relative_path: relative_path.to_path_buf(),
        name: project_name(relative_path),
        link_name: project_link_name(relative_path),
        link_stem: project_link_stem(relative_path),
        parent_target,
        status,
        open_task_count,
        open_unhidden_count,
        prj_task,
    })
}

pub(crate) fn parse_frontmatter(contents: &str) -> Option<Frontmatter<'_>> {
    let mut lines = contents.lines();
    let first = lines.next()?;
    if trim_cr(first) != "---" {
        return None;
    }

    let mut frontmatter_lines = Vec::new();
    for (line_count, line) in (2..).zip(lines) {
        let line = trim_cr(line);
        if line == "---" {
            return Some(Frontmatter {
                lines: frontmatter_lines,
                body_start_line: line_count,
            });
        }
        frontmatter_lines.push(line);
    }

    None
}

pub(crate) fn frontmatter_is_project(frontmatter: &Frontmatter<'_>) -> bool {
    frontmatter_has_type(frontmatter, "[[project]]")
}

pub(crate) fn frontmatter_is_area(frontmatter: &Frontmatter<'_>) -> bool {
    frontmatter_has_type(frontmatter, "[[area]]")
}

fn frontmatter_has_type(frontmatter: &Frontmatter<'_>, expected: &str) -> bool {
    frontmatter_value(frontmatter, "type")
        .map(trim_yaml_scalar)
        .is_some_and(|value| value == expected)
}

pub(crate) fn frontmatter_value<'a>(
    frontmatter: &'a Frontmatter<'a>,
    key: &str,
) -> Option<&'a str> {
    for line in &frontmatter.lines {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(key) else {
            continue;
        };
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        return Some(value.trim());
    }
    None
}

pub(crate) fn trim_yaml_scalar(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn wikilink_target(value: &str) -> Option<String> {
    wikilink_ref(value).map(|target| target.link_name)
}

fn wikilink_ref(value: &str) -> Option<WikilinkRef> {
    let value = trim_yaml_scalar(value);
    let inner = value.strip_prefix("[[")?.strip_suffix("]]")?.trim();
    wikilink_ref_from_inner(inner)
}

fn wikilink_ref_from_inner(inner: &str) -> Option<WikilinkRef> {
    let before_alias =
        inner.split_once('|').map_or(inner, |(target, _)| target);
    let before_heading = before_alias
        .split_once('#')
        .map_or(before_alias, |(target, _)| target);
    let stem = before_heading.rsplit('/').next()?.trim();
    if stem.is_empty() {
        return None;
    }
    Some(WikilinkRef {
        link_name: stem.to_ascii_lowercase(),
        stem: stem.to_string(),
    })
}

fn wikilink_refs_in_line(line: &str) -> Vec<WikilinkRef> {
    wikilink_spans_in_line(line)
        .into_iter()
        .map(|span| span.link)
        .collect()
}

fn wikilink_spans_in_line(line: &str) -> Vec<WikilinkSpan> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(open_relative) = line[offset..].find("[[") {
        let open = offset + open_relative;
        let Some(close_relative) = line[open + 2..].find("]]") else {
            break;
        };
        let close = open + 2 + close_relative;
        if let Some(target) = wikilink_ref_from_inner(&line[open + 2..close]) {
            spans.push(WikilinkSpan {
                link: target,
                start: open,
                end: close + 2,
            });
        }
        offset = close + 2;
    }
    spans
}

fn parse_prj_sub_block(
    contents: &str,
    lines: &[LineSpan],
    prj_line_index: usize,
) -> PrjSubBlock {
    let prj_line = lines[prj_line_index];
    let prj_text = trim_cr(&contents[prj_line.start..prj_line.end]);
    let prj_indent = leading_whitespace(prj_text);
    let mut block = PrjSubBlock {
        prj_indent: prj_indent.to_string(),
        lines: Vec::new(),
    };

    for line in lines.iter().skip(prj_line_index + 1) {
        let line_text = trim_cr(&contents[line.start..line.end]);
        if line_text.trim().is_empty() {
            break;
        }
        let indentation = leading_whitespace(line_text);
        if indentation.len() <= prj_indent.len()
            || !indentation.starts_with(prj_indent)
        {
            break;
        }
        let trimmed_text = line_text.trim_start().to_string();
        let is_marker = list_item_content(line_text).is_some_and(|content| {
            content.starts_with(SUBPROJECTS_MARKER_PREFIX)
        });
        block.lines.push(PrjSubBlockLine {
            line_number: line.line_number,
            indentation: indentation.to_string(),
            trimmed_text,
            is_marker,
            links: wikilink_refs_in_line(line_text),
        });
    }

    block
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .find_map(|(index, character)| {
            (!character.is_whitespace()).then_some(index)
        })
        .unwrap_or(line.len());
    &line[..end]
}

fn list_item_content(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let bullet = trimmed.chars().next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let after_bullet = &trimmed[bullet.len_utf8()..];
    if !after_bullet.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some(after_bullet.trim_start())
}

fn classify_prj_task(
    relative_path: &Path,
    candidates: &[PrjCandidate<'_>],
    sub_block: PrjSubBlock,
    issues: &mut Vec<ScanIssue>,
) -> PrjTask {
    if candidates.is_empty() {
        return PrjTask::missing();
    }

    if candidates.len() > 1 {
        issues.push(ScanIssue::line(
            relative_path,
            candidates[1].line_number,
            "multiple ^prj tasks found; keep exactly one project completion task",
        ));
        return PrjTask::invalid(PrjTaskState::Multiple);
    }

    let candidate = &candidates[0];
    let Some(task) = parse_task_line(candidate.line) else {
        issues.push(malformed_prj_issue(relative_path, candidate.line_number));
        return PrjTask::invalid(PrjTaskState::Malformed);
    };
    if !contains_task_tag(task.text) {
        issues.push(malformed_prj_issue(relative_path, candidate.line_number));
        return PrjTask::invalid(PrjTaskState::Malformed);
    }

    let description = task_description(task.text);
    let placeholder = description == PLACEHOLDER_CRITERIA;
    PrjTask {
        state: match task.status {
            TaskStatus::Open => PrjTaskState::Open,
            TaskStatus::Done => PrjTaskState::Done,
            TaskStatus::Canceled => PrjTaskState::Canceled,
        },
        scheduled: inline_field_value(task.text, "scheduled"),
        hidden: contains_hide_tag(task.text),
        description,
        placeholder,
        sub_block,
    }
}

fn malformed_prj_issue(relative_path: &Path, line_number: usize) -> ScanIssue {
    ScanIssue::line(
        relative_path,
        line_number,
        format!("malformed ^prj task; expected `{PROJECT_TASK_SHAPE}`"),
    )
}

fn parse_task_line(line: &str) -> Option<ParsedTaskLine<'_>> {
    let trimmed = line.trim_start();
    let bullet = trimmed.chars().next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }

    let after_bullet = &trimmed[bullet.len_utf8()..];
    if !after_bullet.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }

    let after_bullet = after_bullet.trim_start();
    let after_open_bracket = after_bullet.strip_prefix('[')?;
    let mark = after_open_bracket.chars().next()?;
    let after_mark = &after_open_bracket[mark.len_utf8()..];
    let after_close_bracket = after_mark.strip_prefix(']')?;
    if !after_close_bracket.is_empty()
        && !after_close_bracket
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        return None;
    }

    Some(ParsedTaskLine {
        status: TaskStatus::from_mark(mark),
        text: after_close_bracket.trim_start(),
    })
}

fn contains_task_tag(text: &str) -> bool {
    contains_tag(text, "#task")
}

fn contains_hide_tag(text: &str) -> bool {
    contains_tag(text, HIDE_TAG)
}

fn contains_tag(text: &str, tag: &str) -> bool {
    tag_span(text, tag).is_some()
}

/// Locates `tag` as a whole token, honoring the same boundaries as the Tasks
/// plugin so substrings like `#taskish` or `#hidden` are not matched.
fn tag_span(text: &str, tag: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    while let Some(relative_index) = text[offset..].find(tag) {
        let index = offset + relative_index;
        let end = index + tag.len();
        let before = text[..index].chars().next_back();
        let after = text[end..].chars().next();
        if before.is_none_or(is_task_tag_left_boundary)
            && after.is_none_or(is_task_tag_right_boundary)
        {
            return Some((index, end));
        }
        offset = index + 1;
    }
    None
}

fn hide_tag_span(line_text: &str) -> Option<(usize, usize)> {
    tag_span(line_text, HIDE_TAG)
}

fn is_task_tag_left_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '(' | '[' | '{')
}

fn is_task_tag_right_boundary(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ']' | ')' | '}' | ':' | '.' | ',' | ';' | '!' | '?'
        )
}

fn has_trailing_prj_anchor(line: &str) -> bool {
    let trimmed = line.trim_end();
    let Some(before_anchor) = trimmed.strip_suffix("^prj") else {
        return false;
    };
    before_anchor.is_empty()
        || before_anchor
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
}

fn inline_field_value(text: &str, key: &str) -> Option<String> {
    inline_field_span(text, key).map(|field| {
        text[field.value_start..field.value_end].trim().to_string()
    })
}

fn inline_field_span(text: &str, key: &str) -> Option<InlineFieldSpan> {
    let mut offset = 0;
    while let Some(open_relative) = text[offset..].find('[') {
        let open = offset + open_relative;
        let Some(close_relative) = text[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_relative;
        let inner = &text[open + 1..close];
        if let Some((field_key, value)) = inner.split_once("::")
            && field_key.trim() == key
        {
            let value_start = open + 1 + field_key.len() + "::".len();
            return Some(InlineFieldSpan {
                start: open,
                end: close + 1,
                value_start,
                value_end: value_start + value.len(),
            });
        }
        offset = close + 1;
    }
    None
}

fn task_description(text: &str) -> String {
    let without_anchor = strip_trailing_block_id(text);
    let without_fields = remove_inline_fields(without_anchor);
    without_fields
        .split_whitespace()
        .filter(|token| {
            *token != "#task"
                && *token != PROJECT_TASK_TAG
                && *token != HIDE_TAG
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_trailing_block_id(text: &str) -> &str {
    let trimmed = text.trim_end();
    let Some(anchor_start) = trimmed.rfind('^') else {
        return trimmed;
    };
    if anchor_start > 0
        && !trimmed[..anchor_start]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        return trimmed;
    }

    let anchor = &trimmed[anchor_start + 1..];
    if anchor.is_empty() || anchor.chars().any(char::is_whitespace) {
        return trimmed;
    }

    trimmed[..anchor_start].trim_end()
}

fn remove_inline_fields(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut offset = 0;
    while let Some(open_relative) = text[offset..].find('[') {
        let open = offset + open_relative;
        let Some(close_relative) = text[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_relative;
        let inner = &text[open + 1..close];
        if inner.contains("::") {
            output.push_str(&text[offset..open]);
            offset = close + 1;
            continue;
        }
        output.push_str(&text[offset..=close]);
        offset = close + 1;
    }
    output.push_str(&text[offset..]);
    output
}

fn print_project_list(projects: &[Project], styler: &Styler) {
    let summary = Summary::from_projects(projects);
    let separator = styler.separator();
    println!(
        "Projects {separator} {} active {separator} {} waiting {separator} {} done {separator} {} canceled",
        summary.active, summary.waiting, summary.done, summary.canceled
    );

    println!();
    let project_width = projects
        .iter()
        .map(|project| display_width(&project.name))
        .max()
        .unwrap_or("PROJECT".len())
        .max("PROJECT".len());

    println!(
        "  {:project_width$}  {:<8}  {:>4}  {:>5}  ^PRJ",
        "PROJECT", "STATUS", "OPEN", "SHOWN"
    );

    for project in projects {
        let project_name =
            styler.cyan(&pad_right(&project.name, project_width));
        let status = styler
            .status(&pad_right(project.status.label(), 8), &project.status);
        println!(
            "  {}  {}  {:>4}  {:>5}  {}",
            project_name,
            status,
            project.open_task_count,
            project.open_unhidden_count,
            project.prj_task.column(styler)
        );
    }
}

fn print_sync_report(report: &SyncReport, dry_run: bool, styler: &Styler) {
    let project_width = report
        .events
        .iter()
        .map(SyncEvent::project_name)
        .map(display_width)
        .max()
        .unwrap_or(0);

    for event in &report.events {
        println!("{}", event.render(project_width, dry_run, styler));
    }

    let separator = styler.separator();
    let mut summary = format!(
        "{} projects {separator} {} status updated {separator} {} ^prj edited {separator} {} warnings",
        report.project_count,
        report.status_update_count(),
        report.prj_edit_count(),
        report.warning_count()
    );
    if !report.issues.is_empty() {
        summary
            .push_str(&format!(" {separator} {} errors", report.issues.len()));
    }
    println!("{summary}");
}

impl SyncEvent {
    fn project_name(&self) -> &str {
        match self {
            Self::Status { project_name, .. }
            | Self::PrjEdit { project_name, .. }
            | Self::Warning { project_name, .. } => project_name,
        }
    }

    fn render(
        &self,
        project_width: usize,
        dry_run: bool,
        styler: &Styler,
    ) -> String {
        let project_name =
            styler.cyan(&pad_right(self.project_name(), project_width));
        match self {
            Self::Status {
                from, to, reason, ..
            } => {
                let prefix = styler.success_prefix(dry_run);
                let verb = if dry_run {
                    "would set status"
                } else {
                    "status"
                };
                format!(
                    "  {prefix} {project_name}  {verb}: {from} -> {to}  {reason}"
                )
            }
            Self::PrjEdit {
                action,
                field,
                reason,
                ..
            } => {
                let prefix = styler.success_prefix(dry_run);
                let (verb, preposition) = match (dry_run, action) {
                    (true, PrjEditAction::Add) => ("would add", "to"),
                    (true, PrjEditAction::Remove) => ("would remove", "from"),
                    (true, PrjEditAction::Update) => ("would update", "on"),
                    (false, PrjEditAction::Add) => ("added", "to"),
                    (false, PrjEditAction::Remove) => ("removed", "from"),
                    (false, PrjEditAction::Update) => ("updated", "on"),
                };
                format!(
                    "  {prefix} {project_name}  {verb} {field} {preposition} ^prj  {reason}"
                )
            }
            Self::Warning {
                message, detail, ..
            } => {
                let prefix = styler.warning_prefix();
                format!("  {prefix} {project_name}  {message}  {detail}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    active: usize,
    waiting: usize,
    done: usize,
    canceled: usize,
}

impl Summary {
    fn from_projects(projects: &[Project]) -> Self {
        let waiting = projects
            .iter()
            .filter(|project| project.status.is_waiting())
            .count();
        let done = projects
            .iter()
            .filter(|project| project.status.is_done())
            .count();
        let canceled = projects
            .iter()
            .filter(|project| project.status.is_canceled())
            .count();
        let active = projects.len() - waiting - done - canceled;
        Self {
            active,
            waiting,
            done,
            canceled,
        }
    }
}

impl PrjTask {
    fn column(&self, styler: &Styler) -> String {
        match self.state {
            PrjTaskState::Missing => styler.warning_label("missing"),
            PrjTaskState::Malformed => styler.error_label("malformed"),
            PrjTaskState::Multiple => styler.error_label("multiple"),
            PrjTaskState::Done => styler.success_label("done"),
            PrjTaskState::Canceled => styler.canceled_label("canceled"),
            PrjTaskState::Open if self.placeholder => {
                styler.warning_label("placeholder")
            }
            PrjTaskState::Open => {
                if self.hidden {
                    styler.open_label()
                } else {
                    styler.on_dash_label()
                }
            }
        }
    }
}

trait ProjectStyleExt {
    fn status(&self, padded_label: &str, status: &ProjectStatus) -> String;
    fn open_label(&self) -> String;
    fn success_label(&self, label: &str) -> String;
    fn canceled_label(&self, label: &str) -> String;
    fn warning_label(&self, label: &str) -> String;
    fn error_label(&self, label: &str) -> String;
    fn on_dash_label(&self) -> String;
}

impl ProjectStyleExt for Styler {
    fn status(&self, padded_label: &str, status: &ProjectStatus) -> String {
        match status {
            ProjectStatus::Wip | ProjectStatus::Other(_) => {
                self.yellow(padded_label)
            }
            ProjectStatus::Waiting => self.blue(padded_label),
            ProjectStatus::Done => self.green(padded_label),
            ProjectStatus::Canceled => self.dim(padded_label),
        }
    }

    fn open_label(&self) -> String {
        if self.is_color() {
            self.yellow("\u{25cb} open")
        } else {
            "open".to_string()
        }
    }

    fn success_label(&self, label: &str) -> String {
        if self.is_color() {
            self.green(&format!("\u{2713} {label}"))
        } else {
            label.to_string()
        }
    }

    fn canceled_label(&self, label: &str) -> String {
        if self.is_color() {
            self.dim(&format!("\u{2715} {label}"))
        } else {
            label.to_string()
        }
    }

    fn warning_label(&self, label: &str) -> String {
        if self.is_color() {
            self.yellow(&format!("\u{26a0} {label}"))
        } else {
            label.to_string()
        }
    }

    fn error_label(&self, label: &str) -> String {
        if self.is_color() {
            self.red(&format!("\u{2717} {label}"))
        } else {
            label.to_string()
        }
    }

    fn on_dash_label(&self) -> String {
        if self.is_color() {
            self.blue("on dash")
        } else {
            "on dash".to_string()
        }
    }
}

fn project_name(relative_path: &Path) -> String {
    let mut path = relative_path.to_path_buf();
    path.set_extension("");
    display_path(&path)
}

fn project_link_name(relative_path: &Path) -> String {
    project_link_stem(relative_path).to_ascii_lowercase()
}

fn project_link_stem(relative_path: &Path) -> String {
    relative_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn relative_or_original(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn is_markdown_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "md")
}

fn is_excluded_directory(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        is_always_excluded_note_directory_name(name)
            || name.to_str() == Some("done")
    })
}

fn trim_cr(value: &str) -> &str {
    value.strip_suffix('\r').unwrap_or(value)
}

fn is_inline_field_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_clean_project(path: &str, contents: &str) -> Project {
        let mut issues = Vec::new();
        let project = parse_project(Path::new(path), contents, &mut issues)
            .expect("project note");
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
        project
    }

    fn subproject(stem: &str, state: SubprojectState) -> SubprojectEntry {
        SubprojectEntry {
            link_name: stem.to_ascii_lowercase(),
            stem: stem.to_string(),
            state,
        }
    }

    fn open_subproject(stem: &str) -> SubprojectEntry {
        subproject(stem, SubprojectState::Open)
    }

    fn done_subproject(stem: &str) -> SubprojectEntry {
        subproject(stem, SubprojectState::Done)
    }

    fn canceled_subproject(stem: &str) -> SubprojectEntry {
        subproject(stem, SubprojectState::Canceled)
    }

    fn apply_changes(contents: &str, changes: &[ProjectChange]) -> String {
        apply_project_changes(contents, changes, None).expect("apply edits")
    }

    fn apply_subproject_changes(
        contents: &str,
        changes: &[ProjectChange],
        desired_subprojects: &[SubprojectEntry],
    ) -> String {
        apply_project_changes(contents, changes, Some(desired_subprojects))
            .expect("apply edits")
    }

    #[test]
    fn task_tag_matches_tasks_plugin_boundaries() {
        assert!(contains_task_tag("#task"));
        assert!(contains_task_tag("prefix (#task)"));
        assert!(contains_task_tag("[#task]"));
        assert!(contains_task_tag("#task:"));
        assert!(contains_task_tag("#task,"));

        assert!(!contains_task_tag("prefix#task"));
        assert!(!contains_task_tag("#taskish"));
        assert!(!contains_task_tag("task #task/sub"));
    }

    #[test]
    fn wikilink_target_extracts_normalized_note_names() {
        assert_eq!(wikilink_target("[[Bob]]").as_deref(), Some("bob"));
        assert_eq!(wikilink_target("\"[[Bob]]\"").as_deref(), Some("bob"));
        assert_eq!(wikilink_target("'[[Bob]]'").as_deref(), Some("bob"));
        assert_eq!(
            wikilink_target("[[projects/Bob|Alias]]").as_deref(),
            Some("bob")
        );
        assert_eq!(
            wikilink_target("[[projects/Bob#Heading]]").as_deref(),
            Some("bob")
        );
        assert_eq!(
            wikilink_target("[[projects/Bob#^block]]").as_deref(),
            Some("bob")
        );
        assert_eq!(
            wikilink_target("[[Projects/Bob#Heading|Alias]]").as_deref(),
            Some("bob")
        );

        assert_eq!(wikilink_target("Bob"), None);
        assert_eq!(wikilink_target("[[]]"), None);
        assert_eq!(wikilink_target("[[folder/]]"), None);
    }

    #[test]
    fn project_parser_accepts_project_type_variants_and_counts_tasks() {
        let contents = r#"---
type: "[[project]]"
status: wip
---
- [ ] #task #prj Finish the project #hide ^prj
- [ ] #task shown one
- [/] #task shown in progress
- [*] #task shown next
- [ ] #task legacy priority is shown [p::1]
- [ ] #task hidden helper #hide
- [x] #task finished
- [-] #task canceled
- [ ] #taskish not a task
"#;
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Alpha.md"), contents, &mut issues)
                .expect("project note");

        assert!(issues.is_empty());
        assert_eq!(project.status, ProjectStatus::Wip);
        assert_eq!(project.open_task_count, 6);
        assert_eq!(project.open_unhidden_count, 4);
        assert_eq!(project.prj_task.state, PrjTaskState::Open);
        assert!(project.prj_task.hidden);
        assert_eq!(project.prj_task.description, "Finish the project");
        assert_eq!(project.link_name, "alpha");
        assert_eq!(project.link_stem, "Alpha");
    }

    #[test]
    fn project_parser_accepts_prj_tag_and_strips_it_from_description() {
        let mut issues = Vec::new();
        let tagged = parse_project(
            Path::new("Tagged.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task #prj Ship the outcome #hide ^prj\n",
            &mut issues,
        )
        .expect("project note");
        assert!(issues.is_empty());
        assert_eq!(tagged.prj_task.state, PrjTaskState::Open);
        assert!(tagged.prj_task.hidden);
        assert_eq!(tagged.prj_task.description, "Ship the outcome");

        // Legacy lines without #prj remain valid and parse identically.
        let legacy = parse_project(
            Path::new("Legacy.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship the outcome #hide ^prj\n",
            &mut issues,
        )
        .expect("project note");
        assert!(issues.is_empty());
        assert_eq!(legacy.prj_task.state, PrjTaskState::Open);
        assert_eq!(legacy.prj_task.description, "Ship the outcome");
    }

    #[test]
    fn project_parser_reads_parent_wikilink_target() {
        let project = parse_clean_project(
            "Projects/Child.md",
            "---\ntype: [[project]]\nparent: \"[[Areas/Parent#Now|Parent alias]]\"\n---\n- [ ] #task Ship #hide ^prj\n",
        );

        assert_eq!(project.name, "Projects/Child");
        assert_eq!(project.link_name, "child");
        assert_eq!(project.parent_target.as_deref(), Some("parent"));
    }

    #[test]
    fn project_parser_accepts_bare_project_type_and_prj_states() {
        let mut issues = Vec::new();
        let done = parse_project(
            Path::new("Done.md"),
            "---\ntype: [[project]]\nstatus: done\n---\n- [X] #task Ship #hide ^prj\n",
            &mut issues,
        )
        .expect("bare project note");
        assert_eq!(done.status, ProjectStatus::Done);
        assert_eq!(done.prj_task.state, PrjTaskState::Done);

        let canceled = parse_project(
            Path::new("Canceled.md"),
            "---\ntype: [[project]]\nstatus: canceled\n---\n- [-] #task Stop #hide ^prj\n",
            &mut issues,
        )
        .expect("canceled project note");
        assert_eq!(canceled.prj_task.state, PrjTaskState::Canceled);
        assert!(issues.is_empty());
    }

    #[test]
    fn project_parser_records_scheduled_and_placeholder_prj() {
        let contents = format!(
            "---\ntype: [[project]]\n---\n- [ ] #task {PLACEHOLDER_CRITERIA} #hide [scheduled::2026-06-11] ^prj\n"
        );
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Placeholder.md"), &contents, &mut issues)
                .expect("project note");

        assert!(issues.is_empty());
        assert_eq!(project.prj_task.state, PrjTaskState::Open);
        assert_eq!(project.prj_task.scheduled.as_deref(), Some("2026-06-11"));
        assert!(project.prj_task.hidden);
        assert!(project.prj_task.placeholder);
        assert_eq!(project.prj_task.column(&Styler::plain()), "placeholder");
    }

    #[test]
    fn project_parser_marks_unprioritized_prj_as_on_dash() {
        let mut issues = Vec::new();
        let project = parse_project(
            Path::new("OnDash.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n",
            &mut issues,
        )
        .expect("project note");

        assert!(issues.is_empty());
        assert!(!project.prj_task.hidden);
        assert_eq!(project.prj_task.column(&Styler::plain()), "on dash");
    }

    #[test]
    fn project_parser_reports_malformed_and_multiple_prj_lines() {
        let mut issues = Vec::new();
        let malformed = parse_project(
            Path::new("Malformed.md"),
            "---\ntype: [[project]]\n---\nComplete this ^prj\n",
            &mut issues,
        )
        .expect("project note");
        assert_eq!(malformed.prj_task.state, PrjTaskState::Malformed);
        assert!(issues[0].message.contains("malformed ^prj task"));

        issues.clear();
        let multiple = parse_project(
            Path::new("Multiple.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
            &mut issues,
        )
        .expect("project note");
        assert_eq!(multiple.prj_task.state, PrjTaskState::Multiple);
        assert!(issues[0].message.contains("multiple ^prj tasks"));
    }

    #[test]
    fn non_project_notes_are_ignored() {
        let mut issues = Vec::new();
        let note = parse_project(
            Path::new("Note.md"),
            "---\ntype: [[ref]]\n---\n- [ ] #task ignored\n",
            &mut issues,
        );
        assert!(note.is_none());
        assert!(issues.is_empty());
    }

    #[test]
    fn project_parser_records_prj_sub_block_marker_lines() {
        let project = parse_clean_project(
            "Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[BareChild]] • [[Projects/AliasChild|Child alias]]\n\t- [[ManualChild]]\n\t- [[MentionedChild]] kickoff notes\n\t\t- 🧩 **Sub-projects:** [[Projects/DeepChild#Next]]\n\t- prose with [[InlineOnly]] link\n",
        );

        let lines = &project.prj_task.sub_block.lines;
        assert_eq!(lines.len(), 5);
        assert_eq!(project.open_task_count, 1);
        assert_eq!(project.open_unhidden_count, 0);
        assert_eq!(project.prj_task.sub_block.prj_indent, "");
        assert_eq!(lines[0].indentation, "\t");
        assert!(lines[0].is_marker);
        assert_eq!(
            lines[0].trimmed_text,
            "- 🧩 **Sub-projects:** [[BareChild]] • [[Projects/AliasChild|Child alias]]"
        );
        assert_eq!(
            lines[0]
                .links
                .iter()
                .map(|link| (link.link_name.as_str(), link.stem.as_str()))
                .collect::<Vec<_>>(),
            vec![("barechild", "BareChild"), ("aliaschild", "AliasChild"),]
        );
        assert_eq!(
            lines[1]
                .links
                .iter()
                .map(|link| link.link_name.as_str())
                .collect::<Vec<_>>(),
            vec!["manualchild"]
        );
        assert!(!lines[1].is_marker);
        assert!(!lines[2].is_marker);
        assert!(lines[3].is_marker);
        assert_eq!(lines[3].indentation, "\t\t");
        assert_eq!(lines[3].links[0].stem, "DeepChild");
        assert_eq!(lines[4].links[0].link_name, "inlineonly");
        assert!(!lines[4].is_marker);
    }

    #[test]
    fn project_parser_stops_prj_sub_block_at_blank_line() {
        let project = parse_clean_project(
            "Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\n\t- [[NotInBlock]]\n",
        );

        assert!(project.prj_task.sub_block.lines.is_empty());
    }

    #[test]
    fn project_sync_plan_flips_status_without_prj_edits_after_effective_status()
    {
        let contents = "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Ship #hide ^prj\n";
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Alpha.md"), contents, &mut issues)
                .expect("project");
        let plan = plan_project_sync(&project, &[]);

        assert!(issues.is_empty());
        assert_eq!(
            plan.changes,
            vec![ProjectChange::Status {
                from: "wip".to_string(),
                to: TargetProjectStatus::Done,
            }]
        );
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn project_sync_plan_manages_prj_hide_tag_from_unhidden_count() {
        let mut issues = Vec::new();
        let stalled = parse_project(
            Path::new("Stalled.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Planned #hide\n",
            &mut issues,
        )
        .expect("project");
        assert_eq!(
            plan_project_sync(&stalled, &[]).changes,
            vec![ProjectChange::RemoveHideTag]
        );

        issues.clear();
        let has_unhidden = parse_project(
            Path::new("HasUnhidden.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n- [ ] #task Needs surfacing\n",
            &mut issues,
        )
        .expect("project");
        assert_eq!(
            plan_project_sync(&has_unhidden, &[]).changes,
            vec![ProjectChange::AddHideTag {
                reason: AddHideReason::NonHiddenOpenTasks,
            }]
        );

        issues.clear();
        let all_hidden_helpers = parse_project(
            Path::new("AllHidden.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Hidden helper #hide\n",
            &mut issues,
        )
        .expect("project");
        assert_eq!(all_hidden_helpers.open_unhidden_count, 0);
        assert_eq!(
            plan_project_sync(&all_hidden_helpers, &[]).changes,
            vec![ProjectChange::RemoveHideTag]
        );

        assert!(issues.is_empty());
    }

    #[test]
    fn project_sync_plan_manages_prj_hide_tag_from_open_subprojects() {
        let children = [open_subproject("Child")];
        let hidden_parent = parse_clean_project(
            "HiddenParent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
        );
        assert!(
            plan_project_sync(&hidden_parent, &children)
                .changes
                .is_empty(),
            "existing #hide tag should be kept while open sub-projects exist"
        );

        let missing_hide_tag = parse_clean_project(
            "MissingHideTagParent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
        );
        assert_eq!(
            plan_project_sync(&missing_hide_tag, &children).changes,
            vec![ProjectChange::AddHideTag {
                reason: AddHideReason::OpenSubprojects,
            }]
        );

        let surfacing_parent = parse_clean_project(
            "SurfacingParent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n",
        );
        assert_eq!(
            plan_project_sync(&surfacing_parent, &[]).changes,
            vec![ProjectChange::RemoveHideTag]
        );
    }

    #[test]
    fn project_sync_plan_reconciles_subprojects_marker_line() {
        let project = parse_clean_project(
            "Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[ExistingChild]] • [[stale_child]]\n\t- [[MentionedChild]] kickoff notes\n\t- prose with [[ManualOnly]] link\n",
        );
        let children = [
            open_subproject("AnotherChild"),
            open_subproject("ExistingChild"),
            open_subproject("MentionedChild"),
        ];

        assert_eq!(
            plan_project_sync(&project, &children).changes,
            vec![
                ProjectChange::AddSubprojectLink {
                    stem: "AnotherChild".to_string(),
                    state: SubprojectState::Open,
                },
                ProjectChange::AddSubprojectLink {
                    stem: "MentionedChild".to_string(),
                    state: SubprojectState::Open,
                },
                ProjectChange::RemoveSubprojectLink {
                    stem: "stale_child".to_string(),
                },
            ]
        );
    }

    #[test]
    fn project_sync_plan_matches_subproject_links_case_insensitively() {
        let project = parse_clean_project(
            "Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
        );
        let children = [open_subproject("child")];

        let changes = plan_project_sync(&project, &children).changes;
        assert!(!changes.iter().any(|change| matches!(
            change,
            ProjectChange::AddSubprojectLink { .. }
                | ProjectChange::RemoveSubprojectLink { .. }
        )));
        assert_eq!(changes, vec![ProjectChange::NormalizeSubprojects]);
    }

    #[test]
    fn project_sync_plan_normalizes_subprojects_marker_drift() {
        let children = [open_subproject("Alpha"), open_subproject("Beta")];
        for contents in [
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n  - 🧩 **Sub-projects:** [[Beta]], [[Alpha]]\n",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n",
        ] {
            let project = parse_clean_project("Parent.md", contents);
            assert_eq!(
                plan_project_sync(&project, &children).changes,
                vec![ProjectChange::NormalizeSubprojects]
            );
        }
    }

    #[test]
    fn project_sync_plan_marks_tracked_closed_subprojects() {
        let project = parse_clean_project(
            "Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[DoneChild]] • ~~[[CanceledChild]]~~ ❌ • [[OpenChild]]\n",
        );
        let children = [
            done_subproject("DoneChild"),
            canceled_subproject("CanceledChild"),
            open_subproject("OpenChild"),
            done_subproject("PrunedDoneChild"),
        ];

        let plan = plan_project_sync(&project, &children);
        assert_eq!(
            plan.changes,
            vec![ProjectChange::MarkSubproject {
                stem: "DoneChild".to_string(),
                state: SubprojectState::Done,
            }]
        );
        assert_eq!(
            plan.desired_subprojects,
            Some(vec![
                open_subproject("OpenChild"),
                canceled_subproject("CanceledChild"),
                done_subproject("DoneChild"),
            ])
        );
    }

    #[test]
    fn project_sync_plan_keeps_canonical_closed_subprojects_idempotent() {
        let project = parse_clean_project(
            "Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[CanceledChild]]~~ ❌ • ~~[[DoneChild]]~~ ✅\n",
        );
        let children = [
            done_subproject("DoneChild"),
            canceled_subproject("CanceledChild"),
        ];

        assert!(plan_project_sync(&project, &children).changes.is_empty());
    }

    #[test]
    fn render_subprojects_line_formats_closed_children_after_open_children() {
        let entries = [
            done_subproject("Gamma"),
            open_subproject("Beta"),
            canceled_subproject("Delta"),
            open_subproject("Alpha"),
        ];

        assert_eq!(
            render_subprojects_line_text(&entries),
            "- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]] • ~~[[Delta]]~~ ❌ • ~~[[Gamma]]~~ ✅"
        );
    }

    #[test]
    fn project_sync_plan_treats_user_sub_bullets_as_user_owned() {
        let project = parse_clean_project(
            "Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n\t- [[Child]]\n\t- [[ManualOnly]] kickoff notes\n- [ ] #task Needs priority\n",
        );

        assert_eq!(
            plan_project_sync(&project, &[open_subproject("Child")]).changes,
            vec![ProjectChange::AddSubprojectLink {
                stem: "Child".to_string(),
                state: SubprojectState::Open,
            }]
        );
    }

    #[test]
    fn project_sync_plan_skips_subproject_links_without_open_prj_edits() {
        let missing = parse_clean_project(
            "Missing.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Needs completion task\n",
        );
        let checked = parse_clean_project(
            "Checked.md",
            "---\ntype: [[project]]\n---\n- [x] #task Ship checked #hide ^prj\n\t- [[OldChild]]\n",
        );
        let terminal = parse_clean_project(
            "Terminal.md",
            "---\ntype: [[project]]\nstatus: canceled\n---\n- [-] #task Ship terminal #hide ^prj\n\t- [[OldChild]]\n",
        );
        let children = [open_subproject("Child")];

        for project in [&missing, &checked, &terminal] {
            assert!(!plan_project_sync(project, &children).changes.iter().any(
                |change| matches!(
                    change,
                    ProjectChange::AddSubprojectLink { .. }
                        | ProjectChange::RemoveSubprojectLink { .. }
                        | ProjectChange::MarkSubproject { .. }
                        | ProjectChange::NormalizeSubprojects
                )
            ));
        }
    }

    #[test]
    fn subproject_parent_links_classify_open_and_terminal_prj_children() {
        let parent = parse_clean_project(
            "Projects/Parent.md",
            "---\ntype: [[project]]\n---\n- [ ] #task Ship parent #hide ^prj\n",
        );
        let open_child = parse_clean_project(
            "Projects/OpenChild.md",
            "---\ntype: [[project]]\nparent: [[Projects/Parent]]\n---\n- [ ] #task Ship child #hide ^prj\n",
        );
        let path_case_child = parse_clean_project(
            "Projects/PathCaseChild.md",
            "---\ntype: [[project]]\nparent: [[areas/PARENT#Now|Parent]]\n---\n- [ ] #task Ship child #hide ^prj\n",
        );
        let terminal_status_open_child = parse_clean_project(
            "Projects/TerminalStatusOpenChild.md",
            "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [ ] #task Ship child #hide ^prj\n",
        );
        let checked_child = parse_clean_project(
            "Projects/CheckedChild.md",
            "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [x] #task Ship child #hide ^prj\n",
        );
        let canceled_child = parse_clean_project(
            "Projects/CanceledChild.md",
            "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [-] #task Ship child #hide ^prj\n",
        );
        let missing_prj_child = parse_clean_project(
            "Projects/MissingPrjChild.md",
            "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [ ] #task Needs completion task\n",
        );
        let self_link = parse_clean_project(
            "Projects/Self.md",
            "---\ntype: [[project]]\nparent: [[Self]]\n---\n- [ ] #task Ship self #hide ^prj\n",
        );
        let area_child = parse_clean_project(
            "Projects/AreaChild.md",
            "---\ntype: [[project]]\nparent: [[Area]]\n---\n- [ ] #task Ship child #hide ^prj\n",
        );

        let mut issues = Vec::new();
        let malformed_child = parse_project(
            Path::new("Projects/MalformedChild.md"),
            "---\ntype: [[project]]\nparent: [[Parent]]\n---\nShip malformed child ^prj\n",
            &mut issues,
        )
        .expect("malformed project");
        let multiple_child = parse_project(
            Path::new("Projects/MultipleChild.md"),
            "---\ntype: [[project]]\nparent: [[Parent]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
            &mut issues,
        )
        .expect("multiple project");
        assert_eq!(issues.len(), 2);

        let children_by_parent = subproject_children_by_parent_link_name([
            &open_child,
            &path_case_child,
            &terminal_status_open_child,
            &checked_child,
            &canceled_child,
            &missing_prj_child,
            &malformed_child,
            &multiple_child,
            &self_link,
            &area_child,
        ]);

        assert_eq!(
            children_by_parent.get(&parent.link_name),
            Some(&vec![
                canceled_subproject("CanceledChild"),
                done_subproject("CheckedChild"),
                open_subproject("OpenChild"),
                open_subproject("PathCaseChild"),
                open_subproject("TerminalStatusOpenChild"),
            ])
        );
        assert_eq!(
            children_by_parent.get("area"),
            Some(&vec![open_subproject("AreaChild")])
        );
        assert!(!children_by_parent.contains_key(&self_link.link_name));

        let non_parent_links =
            subproject_children_by_parent_link_name([&area_child]);
        assert!(!non_parent_links.contains_key(&parent.link_name));
    }

    #[test]
    fn subproject_state_treats_terminal_open_prj_child_as_open() {
        // A child whose frontmatter is terminal but whose ^prj task is open
        // again should count as open for parent ledgers in the same sync run,
        // mirroring the same-run handling for checked/canceled tasks.
        for terminal in ["done", "canceled"] {
            let reopened = parse_clean_project(
                "Child.md",
                &format!(
                    "---\ntype: [[project]]\nstatus: {terminal}\n---\n- [ ] #task Ship child #hide ^prj\n"
                ),
            );
            assert_eq!(
                SubprojectState::from_project(&reopened),
                Some(SubprojectState::Open),
                "terminal status {terminal} with an open ^prj should be open"
            );
        }

        // A terminal child whose ^prj task is missing stays terminal.
        let closed = parse_clean_project(
            "Closed.md",
            "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Needs completion task\n",
        );
        assert_eq!(
            SubprojectState::from_project(&closed),
            Some(SubprojectState::Done)
        );
    }

    #[test]
    fn project_sync_plan_is_idempotent_when_prj_hide_tag_matches_dash_state() {
        let mut issues = Vec::new();
        let already_on_dash = parse_project(
            Path::new("AlreadyOnDash.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n- [ ] #task Planned #hide\n",
            &mut issues,
        )
        .expect("project");
        assert!(plan_project_sync(&already_on_dash, &[]).changes.is_empty());

        issues.clear();
        let already_hidden = parse_project(
            Path::new("AlreadyHidden.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Needs surfacing\n",
            &mut issues,
        )
        .expect("project");
        assert!(plan_project_sync(&already_hidden, &[]).changes.is_empty());
        assert!(issues.is_empty());
    }

    #[test]
    fn project_sync_plan_removes_stale_scheduled_field() {
        let mut issues = Vec::new();
        let project = parse_project(
            Path::new("Scheduled.md"),
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide [scheduled::2026-06-01] ^prj\n",
            &mut issues,
        )
        .expect("project");

        assert!(issues.is_empty());
        assert_eq!(
            plan_project_sync(&project, &[]).changes,
            vec![
                ProjectChange::RemoveHideTag,
                ProjectChange::RemoveScheduled {
                    scheduled: "2026-06-01".to_string(),
                },
            ]
        );
    }

    #[test]
    fn project_sync_plan_reopens_terminal_project_from_open_prj() {
        for terminal in ["done", "canceled"] {
            let contents = format!(
                "---\ntype: [[project]]\nstatus: {terminal}\n---\n- [ ] #task Ship #hide ^prj\n"
            );
            let project =
                parse_clean_project("Reopened.md", &contents);
            let plan = plan_project_sync(&project, &[]);

            // The open ^prj reopens the terminal project to wip, and because
            // the effective status is now active the surfacing pass runs in the
            // same sync, removing #hide from the otherwise-empty project.
            assert_eq!(
                plan.changes,
                vec![
                    ProjectChange::Status {
                        from: terminal.to_string(),
                        to: TargetProjectStatus::Wip,
                    },
                    ProjectChange::RemoveHideTag,
                ],
                "status {terminal} should reopen to wip and surface"
            );
            assert!(plan.warnings.is_empty());
        }
    }

    #[test]
    fn project_sync_plan_leaves_non_terminal_open_prj_status_untouched() {
        for status in ["wip", "waiting"] {
            let contents = format!(
                "---\ntype: [[project]]\nstatus: {status}\n---\n- [ ] #task Ship #hide ^prj\n"
            );
            let project = parse_clean_project("Active.md", &contents);
            let plan = plan_project_sync(&project, &[]);

            // Only terminal statuses are reopenable; an open ^prj never forces
            // waiting (or an already-active status) to wip.
            assert!(
                !plan.changes.iter().any(|change| matches!(
                    change,
                    ProjectChange::Status { .. }
                )),
                "status {status} should not be rewritten by an open ^prj"
            );
        }
    }

    #[test]
    fn project_sync_plan_warns_on_placeholder_while_reopening() {
        let contents = format!(
            "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task {PLACEHOLDER_CRITERIA} #hide ^prj\n"
        );
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Alpha.md"), &contents, &mut issues)
                .expect("project");
        let plan = plan_project_sync(&project, &[]);

        assert!(issues.is_empty());
        // The drift between terminal frontmatter and an open ^prj is now an
        // explicit reopen rather than a warning, but the placeholder warning is
        // still surfaced.
        assert!(plan.changes.iter().any(|change| matches!(
            change,
            ProjectChange::Status {
                to: TargetProjectStatus::Wip,
                ..
            }
        )));
        assert!(
            !plan.warnings
                .iter()
                .any(|event| matches!(event, SyncEvent::Warning { message, .. } if message.contains("still open")))
        );
        assert_eq!(plan.warnings.len(), 1);
        assert!(
            plan.warnings
                .iter()
                .any(|event| matches!(event, SyncEvent::Warning { message, .. } if message.contains("placeholder")))
        );
    }

    #[test]
    fn project_changes_replace_status_append_missing_status_and_add_hide_tag() {
        let contents = "---\ntype: [[project]]\nstatus: waiting\n---\n- [ ] #task Ship ^prj\n";
        let output = apply_changes(
            contents,
            &[
                ProjectChange::Status {
                    from: "waiting".to_string(),
                    to: TargetProjectStatus::Canceled,
                },
                ProjectChange::AddHideTag {
                    reason: AddHideReason::NonHiddenOpenTasks,
                },
            ],
        );
        assert_eq!(
            output,
            "---\ntype: [[project]]\nstatus: canceled\n---\n- [ ] #task Ship #hide ^prj\n"
        );

        let output = apply_changes(
            "---\ntype: [[project]]\n---\n- [x] #task Ship #hide ^prj\n",
            &[ProjectChange::Status {
                from: "wip".to_string(),
                to: TargetProjectStatus::Done,
            }],
        );
        assert_eq!(
            output,
            "---\ntype: [[project]]\nstatus: done\n---\n- [x] #task Ship #hide ^prj\n"
        );
    }

    #[test]
    fn project_changes_remove_prj_fields_with_adjacent_whitespace() {
        let output = apply_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide [scheduled::2026-06-01] ^prj\n",
            &[
                ProjectChange::RemoveHideTag,
                ProjectChange::RemoveScheduled {
                    scheduled: "2026-06-01".to_string(),
                },
            ],
        );
        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship ^prj\n"
        );
    }

    #[test]
    fn project_changes_remove_prj_hide_tag_with_crlf() {
        let output = apply_changes(
            "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship #hide ^prj\r\n",
            &[ProjectChange::RemoveHideTag],
        );
        assert_eq!(
            output,
            "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship ^prj\r\n"
        );
    }

    #[test]
    fn project_changes_preserve_crlf_when_appending_status() {
        let output = apply_changes(
            "---\r\ntype: [[project]]\r\n---\r\n- [x] #task Ship #hide ^prj\r\n",
            &[ProjectChange::Status {
                from: "wip".to_string(),
                to: TargetProjectStatus::Done,
            }],
        );
        assert_eq!(
            output,
            "---\r\ntype: [[project]]\r\nstatus: done\r\n---\r\n- [x] #task Ship #hide ^prj\r\n"
        );
    }

    #[test]
    fn project_changes_insert_subproject_links_after_prj_with_tab_indent() {
        let desired = [open_subproject("Child")];
        let output = apply_subproject_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n## Tasks\n",
            &[ProjectChange::AddSubprojectLink {
                stem: "Child".to_string(),
                state: SubprojectState::Open,
            }],
            &desired,
        );

        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n## Tasks\n"
        );
    }

    #[test]
    fn project_changes_insert_subproject_line_above_user_bullets() {
        let desired = [open_subproject("Beta")];
        let output = apply_subproject_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n  - [[Alpha]]\n  - prose notes\n",
            &[ProjectChange::AddSubprojectLink {
                stem: "Beta".to_string(),
                state: SubprojectState::Open,
            }],
            &desired,
        );

        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Beta]]\n  - [[Alpha]]\n  - prose notes\n"
        );
    }

    #[test]
    fn project_changes_rewrite_subproject_line_in_place() {
        let desired = [open_subproject("Alpha"), open_subproject("Beta")];
        let output = apply_subproject_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- user notes\n  - 🧩 **Sub-projects:** [[Alpha]]\n",
            &[ProjectChange::AddSubprojectLink {
                stem: "Beta".to_string(),
                state: SubprojectState::Open,
            }],
            &desired,
        );

        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- user notes\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n"
        );
    }

    #[test]
    fn project_changes_mark_last_child_closed_and_keep_subproject_line() {
        let desired = [done_subproject("OldChild")];
        let output = apply_subproject_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[OldChild]]\n\t- user notes\n",
            &[ProjectChange::MarkSubproject {
                stem: "OldChild".to_string(),
                state: SubprojectState::Done,
            }],
            &desired,
        );

        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** ~~[[OldChild]]~~ ✅\n\t- user notes\n"
        );
    }

    #[test]
    fn project_changes_delete_subproject_line_for_stale_child() {
        let output = apply_subproject_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[OldChild]]\n\t- user notes\n",
            &[ProjectChange::RemoveSubprojectLink {
                stem: "OldChild".to_string(),
            }],
            &[],
        );

        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- user notes\n"
        );
    }

    #[test]
    fn project_changes_clean_duplicate_subproject_marker_lines() {
        let desired = [open_subproject("Alpha"), open_subproject("Beta")];
        let output = apply_subproject_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Beta]] • [[Alpha]] extra\n\t- keep me\n\t- 🧩 **Sub-projects:** [[Alpha]]\n",
            &[ProjectChange::NormalizeSubprojects],
            &desired,
        );

        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]\n\t- keep me\n"
        );
    }

    #[test]
    fn project_changes_preserve_crlf_for_subproject_link_insertions() {
        let desired = [open_subproject("Child")];
        let output = apply_subproject_changes(
            "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship #hide ^prj\r\n## Tasks\r\n",
            &[ProjectChange::AddSubprojectLink {
                stem: "Child".to_string(),
                state: SubprojectState::Open,
            }],
            &desired,
        );

        assert_eq!(
            output,
            "---\r\ntype: [[project]]\r\n---\r\n- [ ] #task Ship #hide ^prj\r\n\t- 🧩 **Sub-projects:** [[Child]]\r\n## Tasks\r\n"
        );
    }

    #[test]
    fn project_changes_insert_subproject_links_after_final_prj_line() {
        let desired = [open_subproject("Child")];
        let output = apply_subproject_changes(
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj",
            &[ProjectChange::AddSubprojectLink {
                stem: "Child".to_string(),
                state: SubprojectState::Open,
            }],
            &desired,
        );

        assert_eq!(
            output,
            "---\ntype: [[project]]\n---\n- [ ] #task Ship #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]"
        );
    }
}

use std::{
    env,
    ffi::OsString,
    fs, io,
    io::IsTerminal,
    iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::env as bob_env;

const COMMAND_NAME: &str = "bob projects";
const PLACEHOLDER_CRITERIA: &str =
    "<short_project_completion_criteria_goes_here>";
const PROJECT_TASK_SHAPE: &str =
    "- [ ] #task <completion criteria> [p::2] ^prj";

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
items, counts open P0 tasks, and shows the current ^prj state. The sync \
subcommand updates project status and schedules stalled project completion \
tasks from that same ^prj line.",
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
        .about("Sync project status and scheduling from ^prj tasks")
        .long_about(
            "Sync Bob project notes from the completion-criteria task anchored \
with ^prj.\n\n\
A checked ^prj task sets frontmatter status to done. A canceled ^prj task sets \
status to canceled. Active projects with no open P0 tasks have today's \
[scheduled::YYYY-mm-dd] field inserted on their open ^prj task.",
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
    let today = bob_env::current_datetime()
        .date()
        .format("%Y-%m-%d")
        .to_string();

    let report = sync_projects(&bob_dir, &today, dry_run);
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
    status: ProjectStatus,
    open_task_count: usize,
    open_p0_count: usize,
    prj_task: PrjTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectStatus {
    Wip,
    Waiting,
    Done,
    Canceled,
    Other(String),
}

impl ProjectStatus {
    fn parse(value: Option<&str>) -> Self {
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

    fn label(&self) -> &str {
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

    fn is_terminal(&self) -> bool {
        self.is_done() || self.is_canceled()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetProjectStatus {
    Done,
    Canceled,
}

impl TargetProjectStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Done => "^prj task checked",
            Self::Canceled => "^prj task canceled",
        }
    }

    fn as_project_status(self) -> ProjectStatus {
        match self {
            Self::Done => ProjectStatus::Done,
            Self::Canceled => ProjectStatus::Canceled,
        }
    }

    fn matches(self, status: &ProjectStatus) -> bool {
        matches!(
            (self, status),
            (Self::Done, ProjectStatus::Done)
                | (Self::Canceled, ProjectStatus::Canceled)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrjTask {
    state: PrjTaskState,
    scheduled: Option<String>,
    description: String,
    priority: Option<usize>,
    placeholder: bool,
}

impl PrjTask {
    fn missing() -> Self {
        Self {
            state: PrjTaskState::Missing,
            scheduled: None,
            description: String::new(),
            priority: None,
            placeholder: false,
        }
    }

    fn invalid(state: PrjTaskState) -> Self {
        Self {
            state,
            scheduled: None,
            description: String::new(),
            priority: None,
            placeholder: false,
        }
    }

    fn target_status(&self) -> Option<TargetProjectStatus> {
        match self.state {
            PrjTaskState::Done => Some(TargetProjectStatus::Done),
            PrjTaskState::Canceled => Some(TargetProjectStatus::Canceled),
            PrjTaskState::Missing
            | PrjTaskState::Open
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
    line: &'a str,
}

#[derive(Debug, Clone)]
struct Frontmatter<'a> {
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

    fn schedule_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Schedule { .. }))
            .count()
    }

    fn warning_count(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, SyncEvent::Warning { .. }))
            .count()
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
    Schedule {
        project_name: String,
        date: String,
        reason: String,
    },
    Warning {
        project_name: String,
        message: String,
        detail: String,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ProjectPlan {
    changes: Vec<ProjectChange>,
    warnings: Vec<SyncEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectChange {
    Status {
        from: String,
        to: TargetProjectStatus,
    },
    Schedule {
        date: String,
    },
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
            Self::Schedule { date } => SyncEvent::Schedule {
                project_name: project_name.to_string(),
                date: date.clone(),
                reason: "no open P0 tasks".to_string(),
            },
        }
    }
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

fn sync_projects(bob_dir: &Path, today: &str, dry_run: bool) -> SyncReport {
    let mut report = SyncReport::default();
    sync_directory(bob_dir, bob_dir, today, dry_run, &mut report);
    report
}

fn sync_directory(
    root: &Path,
    directory: &Path,
    today: &str,
    dry_run: bool,
    report: &mut SyncReport,
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
            sync_directory(root, &path, today, dry_run, report);
            continue;
        }

        if file_type.is_file() && is_markdown_file(&path) {
            sync_markdown_file(root, &path, today, dry_run, report);
        }
    }
}

fn sync_markdown_file(
    root: &Path,
    path: &Path,
    today: &str,
    dry_run: bool,
    report: &mut SyncReport,
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

    if report.issues.len() != issue_count {
        return;
    }

    let plan = plan_project_sync(&project, today);
    report.events.extend(plan.warnings);

    if plan.changes.is_empty() {
        return;
    }

    if !dry_run {
        let new_contents = match apply_project_changes(&contents, &plan.changes)
        {
            Ok(new_contents) => new_contents,
            Err(message) => {
                report.issues.push(ScanIssue::path(relative_path, message));
                return;
            }
        };

        if let Err(error) = fs::write(path, new_contents) {
            report.issues.push(ScanIssue::path(
                relative_path,
                format!("failed to write file: {error}"),
            ));
            return;
        }
    }

    for change in &plan.changes {
        report.events.push(change.event(&project.name));
    }
}

fn plan_project_sync(project: &Project, today: &str) -> ProjectPlan {
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

    if project.prj_task.state == PrjTaskState::Open
        && project.status.is_terminal()
    {
        plan.warnings.push(SyncEvent::Warning {
            project_name: project.name.clone(),
            message: "^prj task is still open".to_string(),
            detail: format!("frontmatter status is {}", project.status.label()),
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
    if let Some(target) = project.prj_task.target_status()
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
        && project.prj_task.scheduled.is_none()
        && project.open_p0_count == 0
    {
        plan.changes.push(ProjectChange::Schedule {
            date: today.to_string(),
        });
    }

    plan
}

fn apply_project_changes(
    contents: &str,
    changes: &[ProjectChange],
) -> Result<String, String> {
    let mut edits = Vec::new();
    for change in changes {
        match change {
            ProjectChange::Status { to, .. } => {
                edits.push(status_edit(contents, to.label())?);
            }
            ProjectChange::Schedule { date } => {
                edits.push(schedule_edit(contents, date)?);
            }
        }
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

fn schedule_edit(contents: &str, date: &str) -> Result<TextEdit, String> {
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
            replacement: format!("[scheduled::{date}] "),
        });
    }

    Err("failed to locate ^prj task".to_string())
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
    let mut open_task_count = 0;
    let mut open_p0_count = 0;
    let mut prj_candidates = Vec::new();

    for (index, line) in contents
        .lines()
        .skip(frontmatter.body_start_line)
        .enumerate()
    {
        let line_number = frontmatter.body_start_line + index + 1;
        let has_prj_anchor = has_trailing_prj_anchor(line);
        if has_prj_anchor {
            prj_candidates.push(PrjCandidate { line_number, line });
        }

        let Some(task) = parse_task_line(line) else {
            continue;
        };
        if !contains_task_tag(task.text) || !task.status.is_open() {
            continue;
        }

        open_task_count += 1;
        if !has_prj_anchor && task_priority(task.text).unwrap_or(0) == 0 {
            open_p0_count += 1;
        }
    }

    let prj_task = classify_prj_task(relative_path, &prj_candidates, issues);

    Some(Project {
        relative_path: relative_path.to_path_buf(),
        name: project_name(relative_path),
        status,
        open_task_count,
        open_p0_count,
        prj_task,
    })
}

fn parse_frontmatter(contents: &str) -> Option<Frontmatter<'_>> {
    let mut lines = contents.lines();
    let first = lines.next()?;
    if trim_cr(first) != "---" {
        return None;
    }

    let mut frontmatter_lines = Vec::new();
    let mut line_count = 1;
    for line in lines {
        line_count += 1;
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

fn frontmatter_is_project(frontmatter: &Frontmatter<'_>) -> bool {
    frontmatter_value(frontmatter, "type")
        .map(trim_yaml_scalar)
        .is_some_and(|value| value == "[[project]]")
}

fn frontmatter_value<'a>(
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

fn trim_yaml_scalar(value: &str) -> &str {
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

fn classify_prj_task(
    relative_path: &Path,
    candidates: &[PrjCandidate<'_>],
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
        priority: task_priority(task.text),
        description,
        placeholder,
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
    let mut offset = 0;
    while let Some(relative_index) = text[offset..].find("#task") {
        let index = offset + relative_index;
        let before = text[..index].chars().next_back();
        let after = text[index + "#task".len()..].chars().next();
        if before.is_none_or(is_task_tag_left_boundary)
            && after.is_none_or(is_task_tag_right_boundary)
        {
            return true;
        }
        offset = index + 1;
    }
    false
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

fn task_priority(text: &str) -> Option<usize> {
    inline_field_value(text, "p").and_then(|value| value.parse().ok())
}

fn inline_field_value(text: &str, key: &str) -> Option<String> {
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
            return Some(value.trim().to_string());
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
        .filter(|token| *token != "#task")
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
        "  {:project_width$}  {:<8}  {:>4}  {:>2}  {}",
        "PROJECT", "STATUS", "OPEN", "P0", "^PRJ"
    );

    for project in projects {
        let project_name =
            styler.cyan(&pad_right(&project.name, project_width));
        let status = styler
            .status(&pad_right(project.status.label(), 8), &project.status);
        println!(
            "  {}  {}  {:>4}  {:>2}  {}",
            project_name,
            status,
            project.open_task_count,
            project.open_p0_count,
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
        "{} projects {separator} {} status updated {separator} {} scheduled {separator} {} warnings",
        report.project_count,
        report.status_update_count(),
        report.schedule_count(),
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
            | Self::Schedule { project_name, .. }
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
            Self::Schedule { date, reason, .. } => {
                let prefix = styler.schedule_prefix(dry_run);
                let verb = if dry_run {
                    "would schedule ^prj for"
                } else {
                    "scheduled ^prj for"
                };
                format!("  {prefix} {project_name}  {verb} {date}  {reason}")
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
                if let Some(scheduled) = &self.scheduled {
                    styler.scheduled_label(scheduled)
                } else {
                    styler.open_label()
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Styler {
    color: bool,
}

impl Styler {
    fn detect() -> Self {
        Self {
            color: io::stdout().is_terminal()
                && env::var_os("NO_COLOR").is_none(),
        }
    }

    #[cfg(test)]
    fn plain() -> Self {
        Self { color: false }
    }

    fn separator(&self) -> &'static str {
        if self.color {
            "\u{b7}"
        } else {
            "-"
        }
    }

    fn cyan(&self, text: &str) -> String {
        self.paint("36;1", text)
    }

    fn green(&self, text: &str) -> String {
        self.paint("32;1", text)
    }

    fn yellow(&self, text: &str) -> String {
        self.paint("33;1", text)
    }

    fn blue(&self, text: &str) -> String {
        self.paint("34;1", text)
    }

    fn red(&self, text: &str) -> String {
        self.paint("31;1", text)
    }

    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }

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
        if self.color {
            self.yellow("\u{25cb} open")
        } else {
            "open".to_string()
        }
    }

    fn scheduled_label(&self, date: &str) -> String {
        if self.color {
            self.blue(&format!("\u{1f4c5} {date}"))
        } else {
            format!("scheduled {date}")
        }
    }

    fn success_label(&self, label: &str) -> String {
        if self.color {
            self.green(&format!("\u{2713} {label}"))
        } else {
            label.to_string()
        }
    }

    fn canceled_label(&self, label: &str) -> String {
        if self.color {
            self.dim(&format!("\u{2715} {label}"))
        } else {
            label.to_string()
        }
    }

    fn warning_label(&self, label: &str) -> String {
        if self.color {
            self.yellow(&format!("\u{26a0} {label}"))
        } else {
            label.to_string()
        }
    }

    fn error_label(&self, label: &str) -> String {
        if self.color {
            self.red(&format!("\u{2717} {label}"))
        } else {
            label.to_string()
        }
    }

    fn success_prefix(&self, dry_run: bool) -> String {
        let label = if dry_run { "[dry-run] ok" } else { "ok" };
        if self.color {
            self.green(label)
        } else {
            label.to_string()
        }
    }

    fn schedule_prefix(&self, dry_run: bool) -> String {
        let label = if dry_run {
            "[dry-run] scheduled"
        } else {
            "scheduled"
        };
        if self.color {
            self.blue(label)
        } else {
            label.to_string()
        }
    }

    fn warning_prefix(&self) -> String {
        if self.color {
            self.yellow("warning")
        } else {
            "warning".to_string()
        }
    }
}

fn project_name(relative_path: &Path) -> String {
    let mut path = relative_path.to_path_buf();
    path.set_extension("");
    display_path(&path)
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

fn is_markdown_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "md")
}

fn is_excluded_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | ".obsidian" | "_generated" | "_templates" | "done"
            )
        })
}

fn trim_cr(value: &str) -> &str {
    value.strip_suffix('\r').unwrap_or(value)
}

fn pad_right(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{text}{}", " ".repeat(padding))
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn project_parser_accepts_project_type_variants_and_counts_tasks() {
        let contents = r#"---
type: "[[project]]"
status: wip
---
- [ ] #task Finish the project [p::2] ^prj
- [ ] #task open implicit p0
- [/] #task in progress [p:: 1]
- [B] #task blocked with no p
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
        assert_eq!(project.open_task_count, 4);
        assert_eq!(project.open_p0_count, 2);
        assert_eq!(project.prj_task.state, PrjTaskState::Open);
        assert_eq!(project.prj_task.priority, Some(2));
        assert_eq!(project.prj_task.description, "Finish the project");
    }

    #[test]
    fn project_parser_accepts_bare_project_type_and_prj_states() {
        let mut issues = Vec::new();
        let done = parse_project(
            Path::new("Done.md"),
            "---\ntype: [[project]]\nstatus: done\n---\n- [X] #task Ship [p::2] ^prj\n",
            &mut issues,
        )
        .expect("bare project note");
        assert_eq!(done.status, ProjectStatus::Done);
        assert_eq!(done.prj_task.state, PrjTaskState::Done);

        let canceled = parse_project(
            Path::new("Canceled.md"),
            "---\ntype: [[project]]\nstatus: canceled\n---\n- [-] #task Stop [p::2] ^prj\n",
            &mut issues,
        )
        .expect("canceled project note");
        assert_eq!(canceled.prj_task.state, PrjTaskState::Canceled);
        assert!(issues.is_empty());
    }

    #[test]
    fn project_parser_records_scheduled_and_placeholder_prj() {
        let contents = format!(
            "---\ntype: [[project]]\n---\n- [ ] #task {PLACEHOLDER_CRITERIA} [p::2] [scheduled::2026-06-11] ^prj\n"
        );
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Placeholder.md"), &contents, &mut issues)
                .expect("project note");

        assert!(issues.is_empty());
        assert_eq!(project.prj_task.state, PrjTaskState::Open);
        assert_eq!(project.prj_task.scheduled.as_deref(), Some("2026-06-11"));
        assert!(project.prj_task.placeholder);
        assert_eq!(project.prj_task.column(&Styler::plain()), "placeholder");
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
            "---\ntype: [[project]]\n---\n- [ ] #task One [p::2] ^prj\n- [ ] #task Two [p::2] ^prj\n",
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
    fn project_sync_plan_flips_status_and_schedules_after_effective_status() {
        let contents = "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Ship [p::2] ^prj\n";
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Alpha.md"), contents, &mut issues)
                .expect("project");
        let plan = plan_project_sync(&project, "2026-06-11");

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
    fn project_sync_plan_warns_without_auto_fixing_drift_and_placeholder() {
        let contents = format!(
            "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task {PLACEHOLDER_CRITERIA} [p::2] ^prj\n"
        );
        let mut issues = Vec::new();
        let project =
            parse_project(Path::new("Alpha.md"), &contents, &mut issues)
                .expect("project");
        let plan = plan_project_sync(&project, "2026-06-11");

        assert!(issues.is_empty());
        assert!(plan.changes.is_empty());
        assert_eq!(plan.warnings.len(), 2);
        assert!(
            plan.warnings
                .iter()
                .any(|event| matches!(event, SyncEvent::Warning { message, .. } if message.contains("still open")))
        );
        assert!(
            plan.warnings
                .iter()
                .any(|event| matches!(event, SyncEvent::Warning { message, .. } if message.contains("placeholder")))
        );
    }

    #[test]
    fn project_changes_replace_status_append_missing_status_and_schedule() {
        let contents = "---\ntype: [[project]]\nstatus: waiting\n---\n- [ ] #task Ship [p::2] ^prj\n";
        let output = apply_project_changes(
            contents,
            &[
                ProjectChange::Status {
                    from: "waiting".to_string(),
                    to: TargetProjectStatus::Canceled,
                },
                ProjectChange::Schedule {
                    date: "2026-06-11".to_string(),
                },
            ],
        )
        .expect("apply edits");
        assert_eq!(
            output,
            "---\ntype: [[project]]\nstatus: canceled\n---\n- [ ] #task Ship [p::2] [scheduled::2026-06-11] ^prj\n"
        );

        let output = apply_project_changes(
            "---\ntype: [[project]]\n---\n- [x] #task Ship [p::2] ^prj\n",
            &[ProjectChange::Status {
                from: "wip".to_string(),
                to: TargetProjectStatus::Done,
            }],
        )
        .expect("append status");
        assert_eq!(
            output,
            "---\ntype: [[project]]\nstatus: done\n---\n- [x] #task Ship [p::2] ^prj\n"
        );
    }

    #[test]
    fn project_changes_preserve_crlf_when_appending_status() {
        let output = apply_project_changes(
            "---\r\ntype: [[project]]\r\n---\r\n- [x] #task Ship [p::2] ^prj\r\n",
            &[ProjectChange::Status {
                from: "wip".to_string(),
                to: TargetProjectStatus::Done,
            }],
        )
        .expect("append status");
        assert_eq!(
            output,
            "---\r\ntype: [[project]]\r\nstatus: done\r\n---\r\n- [x] #task Ship [p::2] ^prj\r\n"
        );
    }
}

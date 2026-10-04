//! List and sync-report rendering and display helpers.
use super::*;

pub(super) fn print_project_list(projects: &[Project], styler: &Styler) {
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
            project.dash_visible_count,
            project.prj_task.column(styler)
        );
    }
}

pub(super) fn print_sync_report(
    report: &SyncReport,
    dry_run: bool,
    styler: &Styler,
) {
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
    if report.task_schedule_count() > 0 {
        println!(
            "  hint: run `bob task reconcile` to reconcile derived [?] Blocked markers"
        );
    }
    let mut summary = format!(
        "{} projects {separator} {} status updated {separator} {} ^prj edited {separator} {} task schedules updated {separator} {} warnings",
        report.project_count,
        report.status_update_count(),
        report.prj_edit_count(),
        report.task_schedule_count(),
        report.warning_count()
    );
    if !report.issues.is_empty() {
        summary
            .push_str(&format!(" {separator} {} errors", report.issues.len()));
    }
    println!("{summary}");
}

impl SyncEvent {
    pub(super) fn project_name(&self) -> &str {
        match self {
            Self::Status { project_name, .. }
            | Self::PrjEdit { project_name, .. }
            | Self::TaskSchedules { project_name, .. }
            | Self::Warning { project_name, .. } => project_name,
        }
    }

    pub(super) fn render(
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
            Self::TaskSchedules {
                scheduled,
                future,
                scheduled_task_count,
                removed_hide_count,
                prj_hide_changed,
                ..
            } => {
                let prefix = styler.success_prefix(dry_run);
                let mut lines = Vec::new();
                if *scheduled_task_count > 0 {
                    let verb = if dry_run {
                        "would schedule"
                    } else {
                        "scheduled"
                    };
                    let noun = if *scheduled_task_count == 1 {
                        "task"
                    } else {
                        "tasks"
                    };
                    let direction = if *future { "future" } else { "due" };
                    lines.push(format!(
                        "  {prefix} {project_name}  {verb} {scheduled_task_count} {noun} {scheduled}  frontmatter scheduled is {direction}"
                    ));
                }
                if *removed_hide_count > 0 {
                    let verb = if dry_run { "would remove" } else { "removed" };
                    let noun = if *removed_hide_count == 1 {
                        "task"
                    } else {
                        "tasks"
                    };
                    lines.push(format!(
                        "  {prefix} {project_name}  {verb} #hide from {removed_hide_count} {noun}  task schedules replace #hide"
                    ));
                }
                if *prj_hide_changed {
                    let verb = if dry_run {
                        "would normalize"
                    } else {
                        "normalized"
                    };
                    lines.push(format!(
                        "  {prefix} {project_name}  {verb} #hide on ^prj  scheduled {scheduled}"
                    ));
                }
                lines.join("\n")
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
pub(super) struct Summary {
    pub(super) active: usize,
    pub(super) waiting: usize,
    pub(super) done: usize,
    pub(super) canceled: usize,
}

impl Summary {
    pub(super) fn from_projects(projects: &[Project]) -> Self {
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
    pub(super) fn column(&self, styler: &Styler) -> String {
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

pub(super) trait ProjectStyleExt {
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

pub(super) fn project_name(relative_path: &Path) -> String {
    let mut path = relative_path.to_path_buf();
    path.set_extension("");
    display_path(&path)
}

pub(super) fn project_link_name(relative_path: &Path) -> String {
    project_link_stem(relative_path).to_ascii_lowercase()
}

pub(super) fn project_link_stem(relative_path: &Path) -> String {
    relative_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub(super) fn relative_or_original(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

pub(super) fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

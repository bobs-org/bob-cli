//! List and sync-report rendering and display helpers.
use super::*;
use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    catalog::{CatalogChild, CatalogPlan, CatalogState},
    catalog_rows::CatalogRowState,
};

pub(super) fn print_project_list(
    projects: &[Project],
    catalogs_needing_repair: usize,
    styler: &Styler,
) {
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
        let project_name = styler.cyan(&project.name);
        let cell_width = display_width(&project.name);
        let padding = " ".repeat(project_width.saturating_sub(cell_width));
        let status = styler
            .status(&pad_right(project.status.label(), 8), &project.status);
        println!(
            "  {}{}  {}  {:>4}  {:>5}  {}",
            project_name,
            padding,
            status,
            project.open_task_count,
            project.dash_visible_count,
            project.prj_task.column(styler)
        );
    }

    if catalogs_needing_repair > 0 {
        println!(
            "\n{}",
            styler.dim(&format!(
                "{catalogs_needing_repair} Projects catalogs need `bob projects sync`"
            ))
        );
    }
}

pub(super) struct ProjectListRow<'a> {
    pub(super) project: &'a Project,
    pub(super) detail: Option<&'a CatalogChild>,
    pub(super) parent: Option<String>,
    pub(super) children: Vec<String>,
    pub(super) today: Option<usize>,
}

pub(super) fn project_list_json(
    today: NaiveDate,
    rows: &[ProjectListRow<'_>],
    catalogs: &CatalogPlan,
    scan_issues: &[ScanIssue],
) -> Value {
    let projects = rows
        .iter()
        .map(|row| project_to_json(row))
        .collect::<Vec<_>>();
    let catalogs_json = catalogs
        .hosts
        .iter()
        .map(|catalog| {
            json!({
                "host": catalog.stem,
                "path": display_slash(&catalog.host),
                "kind": catalog.kind.as_str(),
                "state": catalog.state.as_str(),
                "rows": {
                    "open": catalog.open,
                    "blocked": catalog.blocked,
                    "warning": catalog.warning,
                    "closed": catalog.closed,
                },
                "issues": catalog.issues.iter().map(|issue| {
                    json!({
                        "code": issue.code,
                        "severity": catalog_issue_severity(catalog.state),
                        "line": issue.line,
                        "message": issue.message,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    let mut findings = scan_issues
        .iter()
        .map(|issue| {
            json!({
                "path": display_slash(&issue.relative_path),
                "code": "projects_scan_issue",
                "severity": "error",
                "line": issue.line_number,
                "message": issue.message,
            })
        })
        .collect::<Vec<_>>();
    for catalog in &catalogs.hosts {
        for issue in &catalog.issues {
            findings.push(json!({
                "host": catalog.stem,
                "path": display_slash(&catalog.host),
                "code": issue.code,
                "severity": catalog_issue_severity(catalog.state),
                "line": issue.line,
                "message": issue.message,
            }));
        }
    }

    let errors = scan_issues.len()
        + catalogs
            .hosts
            .iter()
            .filter(|catalog| catalog.state == CatalogState::Damaged)
            .map(|catalog| catalog.issues.len())
            .sum::<usize>();
    let warnings = catalogs
        .hosts
        .iter()
        .filter(|catalog| catalog.state != CatalogState::Damaged)
        .map(|catalog| catalog.issues.len())
        .sum::<usize>();

    json!({
        "schema_version": 1,
        "ok": errors == 0,
        "command": "list",
        "today": today.format("%Y-%m-%d").to_string(),
        "projects": projects,
        "catalogs": catalogs_json,
        "findings": findings,
        "counts": {
            "projects": rows.len(),
            "catalogs": catalogs.hosts.len(),
            "errors": errors,
            "warnings": warnings,
        },
    })
}

fn project_to_json(row: &ProjectListRow<'_>) -> Value {
    let project = row.project;
    let detail = row.detail;
    let state = detail.map(|detail| row_state_name(detail.state));
    let prj_valid = detail.is_some_and(|detail| detail.prj_line > 0);
    let blocked = detail.and_then(|detail| match detail.state {
        CatalogRowState::Scheduled => Some(json!({
            "kind": "scheduled",
            "date": detail.effective_scheduled.map(|date| date.to_string()),
        })),
        CatalogRowState::Blocked if detail.prj_blocked_tag => {
            Some(json!({"kind": "inherited"}))
        }
        CatalogRowState::Blocked => Some(json!({
            "kind": "dependency",
            "waits_on": detail.waits_on,
        })),
        _ => None,
    });
    json!({
        "path": display_slash(&project.relative_path),
        "stem": project.relative_path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(&project.name),
        "status": project.status.label(),
        "state": state,
        "criterion": detail.and_then(|detail| detail.criterion.clone()),
        "parent": row.parent,
        "children": row.children,
        "closed_on": detail.and_then(|detail| detail.closed_on.map(|date| date.to_string())),
        "prj": {
            "line": prj_valid.then(|| detail.map(|detail| detail.prj_line)).flatten(),
            "mark": prj_valid.then(|| detail.map(|detail| detail.prj_mark.to_string())).flatten(),
            "hidden": prj_valid.then(|| detail.map(|detail| detail.prj_hidden)).flatten(),
            "scheduled": prj_valid.then(|| detail.and_then(|detail| detail.effective_scheduled.map(|date| date.to_string()))).flatten(),
        },
        "blocked": blocked,
        "tasks": {
            "open": project.open_task_count,
            "shown": project.dash_visible_count,
            "today": row.today,
        },
    })
}

fn row_state_name(state: CatalogRowState) -> &'static str {
    match state {
        CatalogRowState::Open => "open",
        CatalogRowState::Scheduled => "scheduled",
        CatalogRowState::Blocked => "blocked",
        CatalogRowState::Warning => "warning",
        CatalogRowState::Done => "done",
        CatalogRowState::Canceled => "canceled",
    }
}

fn catalog_issue_severity(state: CatalogState) -> &'static str {
    if state == CatalogState::Damaged {
        "error"
    } else {
        "warning"
    }
}

fn display_slash(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

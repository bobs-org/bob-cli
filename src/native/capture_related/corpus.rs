//! Candidate discovery: open tasks in routable notes plus the closed window.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Duration, Local, NaiveDate};

use super::super::{
    capture_active_tasks, capture_link_tasks, capture_targets, capture_tasks,
    collect_done, env as bob_env, is_always_excluded_note_directory_name,
    note_tasks, ref_tasks, task_complete, task_fields, task_status_hooks,
};
use super::tokenize::{self, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CandidateKind {
    Task,
    Project,
    Reading,
}

impl CandidateKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Project => "project",
            Self::Reading => "reading",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClosedInfo {
    pub(super) status: ClosedStatus,
    pub(super) date: NaiveDate,
    pub(super) days_ago: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClosedStatus {
    Done,
    Canceled,
}

impl ClosedStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Candidate {
    pub(super) key: String,
    pub(super) title: String,
    pub(super) tokens: Vec<Token>,
    pub(super) kind: CandidateKind,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) closed: Option<ClosedInfo>,
    pub(super) route: String,
    pub(super) locator: String,
    pub(super) note_path: String,
    pub(super) target: PathBuf,
    pub(super) line: usize,
    pub(super) task_ref: String,
    pub(super) block_id: Option<String>,
    pub(super) block_id_suggestions: Vec<String>,
    pub(super) created: Option<NaiveDate>,
    pub(super) open: bool,
}

impl Candidate {
    pub(super) fn kind_name(&self) -> &'static str {
        self.kind.as_str()
    }
}

pub(super) struct Discovery {
    pub(super) candidates: Vec<Candidate>,
    pub(super) warnings: Vec<String>,
    pub(super) global_filter: String,
    /// `(alias, exact routable path)` pairs from the capture route catalog.
    pub(super) route_aliases: Vec<(String, String)>,
}

pub(super) fn discover(bob_dir: &Path, days: u32) -> Discovery {
    let today = bob_env::current_datetime().date();
    let earliest = today - Duration::days(i64::from(days));
    let mut warnings = Vec::new();
    let report = capture_targets::scan_capture_targets(bob_dir);
    for issue in &report.issues {
        warnings.push(capture_active_tasks::bounded_warning(issue.display()));
    }
    let settings = note_tasks::read_settings(bob_dir);
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for target in &report.targets {
        let relative = &target.relative_path;
        let path = bob_dir.join(relative);
        match fs::read_to_string(&path) {
            Ok(contents) => collect_from_note(
                bob_dir,
                relative,
                &path,
                &contents,
                &target.route,
                &target.route,
                &settings,
                today,
                earliest,
                true,
                &mut seen,
                &mut candidates,
                &mut warnings,
            ),
            Err(error)
                if target.kind == capture_targets::CaptureTargetKind::Inbox
                    && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => warnings.push(capture_active_tasks::bounded_warning(
                format!("failed to read {relative}: {error}"),
            )),
        }
    }

    collect_from_archives(
        bob_dir,
        &settings,
        today,
        earliest,
        &report,
        &mut seen,
        &mut candidates,
        &mut warnings,
    );

    let route_aliases = report
        .targets
        .iter()
        .flat_map(|target| {
            target.project_name_aliases.iter().map(|alias| {
                (alias.to_ascii_lowercase(), target.relative_path.clone())
            })
        })
        .collect();

    Discovery {
        candidates,
        warnings,
        global_filter: settings.global_filter.clone(),
        route_aliases,
    }
}

fn collect_from_note(
    _bob_dir: &Path,
    relative: &str,
    absolute: &Path,
    contents: &str,
    route: &str,
    locator: &str,
    settings: &note_tasks::NoteTaskSettings,
    today: NaiveDate,
    earliest: NaiveDate,
    include_open: bool,
    seen: &mut HashSet<String>,
    candidates: &mut Vec<Candidate>,
    _warnings: &mut Vec<String>,
) {
    if task_status_hooks::canonical_daily_date(Path::new(relative)).is_some() {
        return;
    }
    let scan = note_tasks::scan(contents, settings);
    let used_ids = scan
        .tasks()
        .iter()
        .filter_map(|task| task.block_id.clone())
        .collect::<Vec<_>>();
    let lines: Vec<&str> = contents.lines().collect();
    for task in scan.tasks() {
        let raw = lines.get(task.line_index).copied().unwrap_or("");
        if is_gtd_checklist(raw) {
            continue;
        }
        let open = task.status_type.is_open()
            && capture_link_tasks::is_linkable_status(task.status_symbol);
        if open && !include_open {
            continue;
        }
        let closed = if open {
            None
        } else {
            if !task.status_type.is_terminal() {
                continue;
            }
            if task_complete::is_recurring_task_line(raw) {
                continue;
            }
            let Some(closed) = closed_info(
                raw,
                task.status_symbol,
                task.status_type,
                today,
                earliest,
            ) else {
                continue;
            };
            Some(closed)
        };
        let title = tokenize::display_title(
            &task.description,
            &settings.global_filter,
            task.block_id.as_deref(),
        );
        let block_id_suggestions = if task.block_id.is_none() {
            super::super::capture_block_ids::suggest_ids(&title, '^', &used_ids)
        } else {
            Vec::new()
        };
        let key =
            candidate_key(relative, task.block_id.as_deref(), &task.digest);
        if !seen.insert(key.clone()) {
            continue;
        }
        candidates.push(Candidate {
            key,
            tokens: tokenize::tokenize(&title),
            title,
            kind: candidate_kind(task),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: capture_tasks::status_type_label(task.status_type),
            closed,
            route: route.to_string(),
            locator: locator.to_string(),
            note_path: relative.to_string(),
            target: absolute.to_path_buf(),
            line: task.line_index + 1,
            task_ref: task.task_ref(),
            block_id: task.block_id.clone(),
            block_id_suggestions,
            created: created_date(raw),
            open,
        });
    }
}

fn collect_from_archives(
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    today: NaiveDate,
    earliest: NaiveDate,
    report: &capture_targets::CaptureTargetsReport,
    seen: &mut HashSet<String>,
    candidates: &mut Vec<Candidate>,
    warnings: &mut Vec<String>,
) {
    let done_root = bob_dir.join("done");
    let mut archives = Vec::new();
    collect_done_archives(&done_root, &mut archives);
    let routes: Vec<(String, String)> = report
        .targets
        .iter()
        .map(|target| (target.relative_path.clone(), target.route.clone()))
        .collect();
    for archive in archives {
        let Ok(relative) = archive.strip_prefix(bob_dir) else {
            continue;
        };
        if !archive_mtime_in_window(&archive, earliest) {
            continue;
        }
        let Some(source) =
            collect_done::source_relative_path_from_archive(relative)
        else {
            continue;
        };
        if task_status_hooks::canonical_daily_date(&source).is_some() {
            continue;
        }
        let source_text = source.to_string_lossy().replace('\\', "/");
        let locator = routes
            .iter()
            .find(|(path, _)| path == &source_text)
            .map(|(_, route)| route.clone())
            .unwrap_or_else(|| {
                source
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("done")
                    .to_string()
            });
        match fs::read_to_string(&archive) {
            Ok(contents) => collect_from_note(
                bob_dir,
                &relative.to_string_lossy().replace('\\', "/"),
                &archive,
                &contents,
                &locator,
                &locator,
                settings,
                today,
                earliest,
                false,
                seen,
                candidates,
                warnings,
            ),
            Err(error) => warnings.push(capture_active_tasks::bounded_warning(
                format!("failed to read {}: {error}", relative.display()),
            )),
        }
    }
}

fn collect_done_archives(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if is_always_excluded_note_directory_name(&name) {
                continue;
            }
            collect_done_archives(&path, out);
        } else if name.to_str().is_some_and(|name| name.ends_with("_done.md")) {
            out.push(path);
        }
    }
}

fn archive_mtime_in_window(path: &Path, earliest: NaiveDate) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    file_mtime_date(modified).is_some_and(|date| date >= earliest)
}

fn file_mtime_date(modified: SystemTime) -> Option<NaiveDate> {
    Some(DateTime::<Local>::from(modified).naive_local().date())
}

fn candidate_key(
    note_path: &str,
    block_id: Option<&str>,
    digest: &str,
) -> String {
    match block_id {
        Some(id) => format!("{note_path}#{id}"),
        None => format!("{note_path}#{digest}"),
    }
}

fn candidate_kind(task: &note_tasks::NoteTask) -> CandidateKind {
    if task
        .block_id
        .as_deref()
        .is_some_and(|id| id.eq_ignore_ascii_case("prj"))
    {
        CandidateKind::Project
    } else if task.task_kind == Some("ref") {
        CandidateKind::Reading
    } else {
        CandidateKind::Task
    }
}

fn is_gtd_checklist(line: &str) -> bool {
    line.split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("#gtd"))
}

fn created_date(line: &str) -> Option<NaiveDate> {
    if !line.contains("[created::") {
        return None;
    }
    let field = task_fields::inline_fields(line, "created")
        .into_iter()
        .next()?;
    task_fields::parse_strict_calendar_date(field.value.trim())
}

fn closed_info(
    line: &str,
    status_symbol: char,
    status_type: note_tasks::TaskStatusType,
    today: NaiveDate,
    earliest: NaiveDate,
) -> Option<ClosedInfo> {
    let date_text = ref_tasks::close_date(line)?;
    let date = task_fields::parse_strict_calendar_date(&date_text)?;
    if date < earliest || date > today {
        return None;
    }
    let status = if status_type == note_tasks::TaskStatusType::Cancelled
        || status_symbol == '-'
        || line.contains("[cancelled::")
        || line.contains("❌")
        || line.contains('✖')
    {
        ClosedStatus::Canceled
    } else {
        ClosedStatus::Done
    };
    Some(ClosedInfo {
        status,
        date,
        days_ago: (today - date).num_days(),
    })
}

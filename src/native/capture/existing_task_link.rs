//! Crate-visible existing-task Pomodoro link for `bob ref today`.
//!
//! Plans one `(route, block_id)` link through the same planner, link
//! guard, dependency drop, and atomic commit as
//! `bob capture '^route:block-id[#name]'`.
use super::*;
use crate::native::capture_pomodoros_agenda;
use crate::native::capture_task_toggle::PomodoroLinkTarget;
use crate::native::style::Styler;
use chrono::NaiveDate;
use serde::Serialize;
use std::path::Path;

/// One open Pomodoro row in the `ref today` envelope. `homes` is the
/// derived R9 object for unnamed rows (forecast: open rows never carry
/// minutes); named rows do no resolution and omit it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TodayPomodoroRow {
    #[serde(rename = "ref")]
    pub pomodoro_ref: String,
    pub line: usize,
    pub name: Option<String>,
    pub slug: String,
    pub time_range: Option<String>,
    pub placeholder: bool,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub running: bool,
    pub default: bool,
    pub linked: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub suggested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion_label: Option<String>,
    pub task_link_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homes: Option<crate::native::pomodoro_homes::Homes>,
}

/// Result of planning (and optionally committing) one existing-task link.
#[derive(Debug, Clone)]
pub(crate) struct ExistingTaskLinkOutcome {
    pub capture: serde_json::Value,
    pub changed: bool,
    pub kind: String,
    pub human_details: String,
}

/// Plan one existing-task Pomodoro link and, unless `dry_run`, commit it.
pub(crate) fn link_existing_task(
    bob_dir: &Path,
    route: &str,
    block_id: &str,
    ledger_target: PomodoroLinkTarget<'_>,
    dry_run: bool,
    styler: &Styler,
) -> Result<ExistingTaskLinkOutcome, CaptureError> {
    let now = bob_env::current_datetime();
    let today = now.date();
    let relative = relative_target(Some(route));
    let target = bob_dir.join(&relative);
    let mut planner = CaptureBatchPlanner::default();
    let mut warnings = Vec::new();
    let mut dependency_ctx = DependencyContext::new(bob_dir, today);
    let mut task_theme_context =
        CaptureTaskThemeContext::new(bob_dir, today, None);
    planner.begin_item(None);
    let planned = plan_existing_task_pomodoro_link_item(
        &mut planner,
        bob_dir,
        dry_run,
        &target,
        route,
        block_id,
        ledger_target,
        None,
        now,
        today,
        &mut warnings,
        &mut dependency_ctx,
        Some(&mut task_theme_context),
        false,
    )?;
    let human_details =
        format_human_pomodoro_link_details(&planned.result, styler);
    let kind = planned.result.kind.to_string();
    let capture =
        serde_json::to_value(&planned.result).expect("capture item serializes");
    let text_files = planner.into_staged_files();
    let changed = !text_files.is_empty();
    if !dry_run && changed {
        let mut batch = PlannedCaptureBatch {
            items: vec![planned],
            text_files,
            global_destination: None,
            warnings,
            plan_budget: None,
            pomodoro_blocks: Vec::new(),
            task_blocks: Vec::new(),
            inbox_route: InboxRouteReport::empty(),
            routed_landings: Vec::new(),
            template: None,
            template_rows: Vec::new(),
            staged_jobs: Vec::new(),
        };
        commit_capture_batch(&mut batch, bob_dir)?;
    }
    Ok(ExistingTaskLinkOutcome {
        capture,
        changed,
        kind,
        human_details,
    })
}

/// Open top-level Pomodoro rows in document order, with `running` /
/// `default` / `linked` flags from the same rules as the write path.
/// Unnamed rows also carry their derived forecast `homes`; named rows
/// do no resolution. `daily_path` is the vault-relative day file.
pub(crate) fn list_today_link_pomodoros(
    bob_dir: &Path,
    daily_path: &Path,
    day_contents: &str,
    block_link: &str,
    today: NaiveDate,
) -> Vec<TodayPomodoroRow> {
    let mut resolver = crate::native::pomodoro_homes::HomeResolver::new(
        crate::native::pomodoro_homes::DiskHomeSource::new(bob_dir),
    );
    let scan = capture_pomodoros::scan(day_contents);
    if !scan.has_section {
        return Vec::new();
    }
    let lines = line_spans(day_contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let Some(section) = pomodoro::pomodoros_section_range(&line_texts) else {
        return Vec::new();
    };
    let movable = capture_task_toggle::find_movable_task_links(
        &lines, &scan, section, block_link,
    );
    let queue_line = match movable.as_slice() {
        [source] => Some(source.owner.line),
        _ => None,
    };
    let default_line = capture_task_toggle::default_pomodoro_link_destination(
        &scan, queue_line,
    )
    .map(|entry| entry.line);
    let open_links = capture_task_toggle::list_open_entry_links(day_contents);
    scan.entries
        .iter()
        .filter(|entry| entry.state == capture_pomodoros::PomodoroState::Open)
        .map(|entry| {
            let (starts_at, ends_at) =
                capture_pomodoros_agenda::agenda_datetimes(
                    today,
                    entry.time_range.as_deref(),
                );
            let task_link_count = open_links
                .iter()
                .find(|open| open.line == entry.line)
                .map(|open| open.links.len())
                .unwrap_or(0);
            let named =
                entry.name.as_deref().is_some_and(|name| !name.is_empty());
            let homes = (!named).then(|| {
                crate::native::pomodoro_homes::attribute_ledger_entry(
                    day_contents,
                    entry.line,
                    None,
                    false,
                    None,
                    daily_path,
                    &mut resolver,
                )
                .homes
            });
            TodayPomodoroRow {
                pomodoro_ref: entry.pomodoro_ref.to_string(),
                line: entry.line,
                name: entry.name.clone(),
                slug: entry.slug.clone(),
                time_range: entry.time_range.clone(),
                placeholder: entry.placeholder,
                starts_at,
                ends_at,
                running: entry.is_current,
                default: default_line == Some(entry.line),
                linked: queue_line == Some(entry.line),
                suggested: false,
                suggestion_label: None,
                task_link_count,
                homes,
            }
        })
        .collect()
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub(crate) fn today_error_code(error: &CaptureError) -> &'static str {
    match error.typed {
        Some(CaptureFailureKind::NoDailyNote) => "no_daily_note",
        Some(CaptureFailureKind::NoPomodorosSection) => "no_pomodoros_section",
        Some(CaptureFailureKind::NoOpenPomodoro) => "no_open_pomodoro",
        Some(CaptureFailureKind::MultipleRunning) => "multiple_running",
        Some(CaptureFailureKind::InvalidPomodoroName) => {
            "invalid_pomodoro_name"
        }
        Some(CaptureFailureKind::StalePomodoro) => "stale_pomodoro",
        Some(CaptureFailureKind::PomodoroClosed) => "pomodoro_closed",
        Some(CaptureFailureKind::LedgerInvariant) => "ledger_invariant",
        Some(CaptureFailureKind::NoteChanged) => "note_changed",
        None => "io",
    }
}

//! Pure freshness evaluation, computed at read time and never stored.
//!
//! Owned by `docs/freshness.md`. The JavaScript mirror lives in
//! bob-ledger-tools (`api.freshness.state`, `queue`, `counts`); both
//! sides run the state conformance vectors in that doc verbatim.

use chrono::NaiveDate;

use super::placement::read_freshness;
use crate::native::config::freshness::FreshnessConfig;

/// A task's freshness state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FreshState {
    New,
    Resurfaced,
    Rotten,
    Fresh,
}

impl FreshState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Resurfaced => "resurfaced",
            Self::Rotten => "rotten",
            Self::Fresh => "fresh",
        }
    }

    /// Stable read-time bucket for dashboard gating (`docs/freshness.md`
    /// §4): `new` tasks surface in NEW, `resurfaced` and `rotten` tasks
    /// surface in ROTTEN review, and `fresh` tasks stay in READY.
    pub(crate) fn bucket(self) -> Option<&'static str> {
        match self {
            Self::New => Some("new"),
            Self::Resurfaced | Self::Rotten => Some("rotten"),
            Self::Fresh => None,
        }
    }
}

/// Stable bucket for an evaluated state: out-of-scope (`None`) and
/// `Fresh` map to no bucket, so a null bucket alone never proves a
/// task is Ready — callers must also apply the Ready predicate.
pub(crate) fn bucket_for_state(
    state: Option<FreshState>,
) -> Option<&'static str> {
    state.and_then(FreshState::bucket)
}

/// Where the effective interval came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntervalSource {
    Task,
    Note,
    Config,
    Default,
}

impl IntervalSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Note => "note",
            Self::Config => "config",
            Self::Default => "default",
        }
    }
}

/// One input row for the evaluator.
///
/// Callers precompute the scope inputs: `is_todo` is the Tasks status
/// type TODO, `lane_visible` is the NEXT/PENDING lane predicate (not
/// done, not dependency-blocked, no `#hide`, not under `_templates`
/// or `_conflicts`, no scheduled date after today), `is_daily_note`
/// is a canonical daily note (`YYYY/YYYYMMDD.md`), and `is_today` is
/// membership in today's open Pomodoro Task Links.
#[derive(Debug, Clone)]
pub(crate) struct FreshnessRow {
    pub(crate) path: String,
    /// 1-based line number, as in JSON and docs.
    pub(crate) line: u32,
    // Contract field: written by the scanner for the vectors, not read yet.
    #[allow(dead_code)]
    pub(crate) status: char,
    pub(crate) is_todo: bool,
    pub(crate) recurring: bool,
    pub(crate) lane_visible: bool,
    pub(crate) is_daily_note: bool,
    pub(crate) is_today: bool,
    pub(crate) scheduled: Option<NaiveDate>,
    // Contract field: written by the scanner for the vectors, not read yet.
    #[allow(dead_code)]
    pub(crate) created: Option<NaiveDate>,
    pub(crate) raw_line: String,
    /// The note's raw `task_refresh` frontmatter value, if present.
    pub(crate) note_refresh_raw: Option<String>,
}

/// The evaluated result for one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evaluated {
    /// `None` when out of scope (see S13).
    pub(crate) state: Option<FreshState>,
    pub(crate) fresh: Option<NaiveDate>,
    pub(crate) interval_days: u16,
    pub(crate) interval_source: IntervalSource,
    pub(crate) due_on: Option<NaiveDate>,
    pub(crate) days_overdue: Option<i64>,
    pub(crate) lints: Vec<String>,
}

/// Evaluate one row for `today` under `config`.
pub(crate) fn evaluate(
    row: &FreshnessRow,
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Evaluated {
    let read = read_freshness(&row.raw_line, today);
    let mut lints = read.lints.clone();

    let (note_interval, note_lint) =
        parse_note_refresh(row.note_refresh_raw.as_deref());
    if let Some(lint) = note_lint {
        push_lint(&mut lints, &lint);
    }

    let (interval_days, interval_source) =
        interval_for(read.refresh, note_interval, config);

    let in_scope = row.is_todo
        && row.lane_visible
        && !row.recurring
        && !row.is_daily_note
        && !row.is_today;

    if !in_scope {
        return Evaluated {
            state: None,
            fresh: read.fresh,
            interval_days,
            interval_source,
            due_on: None,
            days_overdue: None,
            lints,
        };
    }

    let Some(fresh) = read.fresh else {
        return Evaluated {
            state: Some(FreshState::New),
            fresh: None,
            interval_days,
            interval_source,
            due_on: None,
            days_overdue: None,
            lints,
        };
    };

    // RESURFACED beats ROTTEN: a deferral that returned is due as soon
    // as it returns, however old the stamp is.
    if let Some(scheduled) = row.scheduled
        && fresh < scheduled
        && scheduled <= today
    {
        let days_overdue = today.signed_duration_since(scheduled).num_days();
        return Evaluated {
            state: Some(FreshState::Resurfaced),
            fresh: Some(fresh),
            interval_days,
            interval_source,
            due_on: Some(scheduled),
            days_overdue: Some(days_overdue),
            lints,
        };
    }

    let due_on = fresh
        .checked_add_days(chrono::Days::new(u64::from(interval_days)))
        .unwrap_or(fresh);
    if today >= due_on {
        let days_overdue = today.signed_duration_since(due_on).num_days();
        return Evaluated {
            state: Some(FreshState::Rotten),
            fresh: Some(fresh),
            interval_days,
            interval_source,
            due_on: Some(due_on),
            days_overdue: Some(days_overdue),
            lints,
        };
    }

    Evaluated {
        state: Some(FreshState::Fresh),
        fresh: Some(fresh),
        interval_days,
        interval_source,
        due_on: Some(due_on),
        days_overdue: None,
        lints,
    }
}

/// Effective interval and where it came from: the task's
/// `[refresh:: N]`, then the note's `task_refresh`, then
/// `freshness.interval`, then 7.
fn interval_for(
    task_refresh: Option<u16>,
    note_refresh: Option<u16>,
    config: &FreshnessConfig,
) -> (u16, IntervalSource) {
    if let Some(days) = task_refresh {
        return (days, IntervalSource::Task);
    }
    if let Some(days) = note_refresh {
        return (days, IntervalSource::Note);
    }
    if config.interval_from_config {
        return (config.interval, IntervalSource::Config);
    }
    if config.interval != 7 {
        // A programmatically built config with a non-default interval
        // but no explicit flag still counts as configured.
        return (config.interval, IntervalSource::Config);
    }
    (7, IntervalSource::Default)
}

/// Parse a note's raw `task_refresh` frontmatter value.
fn parse_note_refresh(raw: Option<&str>) -> (Option<u16>, Option<String>) {
    let Some(raw) = raw else {
        return (None, None);
    };
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    if trimmed.is_empty() {
        return (None, None);
    }
    match trimmed.parse::<i64>() {
        Ok(number) if (1..=365).contains(&number) => {
            (Some(number as u16), None)
        }
        _ => (None, Some("task_refresh_invalid".to_string())),
    }
}

/// One entry of the review queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueEntry {
    pub(crate) rank: u32,
    /// `new` or `due` (resurfaced and rotten together).
    pub(crate) tier: &'static str,
    pub(crate) state: FreshState,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) due_on: Option<NaiveDate>,
    pub(crate) days_overdue: Option<i64>,
}

/// The review queue: NEW by (path, line), then DUE by
/// (due_on, path, line).
pub(crate) fn queue(
    rows: &[FreshnessRow],
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Vec<QueueEntry> {
    let mut new_entries: Vec<QueueEntry> = Vec::new();
    let mut due_entries: Vec<QueueEntry> = Vec::new();

    for row in rows {
        let evaluated = evaluate(row, today, config);
        let Some(state) = evaluated.state else {
            continue;
        };
        if state == FreshState::Fresh {
            continue;
        }
        let entry = QueueEntry {
            rank: 0,
            tier: if state == FreshState::New {
                "new"
            } else {
                "due"
            },
            state,
            path: row.path.clone(),
            line: row.line,
            due_on: evaluated.due_on,
            days_overdue: evaluated.days_overdue,
        };
        if state == FreshState::New {
            new_entries.push(entry);
        } else {
            due_entries.push(entry);
        }
    }

    new_entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    due_entries.sort_by(|a, b| {
        a.due_on
            .cmp(&b.due_on)
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });

    let mut ordered = Vec::with_capacity(new_entries.len() + due_entries.len());
    ordered.extend(new_entries);
    ordered.extend(due_entries);
    for (index, entry) in ordered.iter_mut().enumerate() {
        entry.rank = index as u32 + 1;
    }
    ordered
}

/// Whole-vault counts for the review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) due: u32,
    pub(crate) new: u32,
    pub(crate) resurfaced: u32,
    pub(crate) rotten: u32,
    /// In-scope FRESH tasks.
    pub(crate) fresh: u32,
    /// Tasks of any status outside `_templates` / `_conflicts` whose
    /// `fresh` equals today.
    pub(crate) refreshed_today: u32,
    pub(crate) budget: Option<u32>,
    pub(crate) budget_met: bool,
}

/// Count the review states over `rows`.
pub(crate) fn counts(
    rows: &[FreshnessRow],
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Counts {
    let mut due = 0;
    let mut new = 0;
    let mut resurfaced = 0;
    let mut rotten = 0;
    let mut fresh = 0;
    let mut refreshed_today = 0;

    for row in rows {
        let evaluated = evaluate(row, today, config);
        if !is_excluded_count_path(&row.path) && evaluated.fresh == Some(today)
        {
            refreshed_today += 1;
        }
        let Some(state) = evaluated.state else {
            continue;
        };
        match state {
            FreshState::New => {
                new += 1;
                due += 1;
            }
            FreshState::Resurfaced => {
                resurfaced += 1;
                due += 1;
            }
            FreshState::Rotten => {
                rotten += 1;
                due += 1;
            }
            FreshState::Fresh => {
                fresh += 1;
            }
        }
    }

    let budget = config.rotten_daily_budget;
    let budget_met =
        budget.is_some_and(|goal| refreshed_today >= goal && new == 0);

    Counts {
        due,
        new,
        resurfaced,
        rotten,
        fresh,
        refreshed_today,
        budget,
        budget_met,
    }
}

/// Paths under `_templates` or `_conflicts` never count toward
/// `refreshed_today`.
fn is_excluded_count_path(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "_templates" || segment == "_conflicts")
}

fn push_lint(lints: &mut Vec<String>, code: &str) {
    if !lints.iter().any(|lint| lint == code) {
        lints.push(code.to_string());
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod state_tests;

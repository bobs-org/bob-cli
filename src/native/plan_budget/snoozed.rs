//! Snoozed meter and nonbinding wake forecast for `bob plan`.
//!
//! Contract: `docs/plan.md` § Snoozed forecast. The meter counts unique
//! planned identities (Asleep plus Due plus Blocked); `due` and `blocked`
//! count those states. `on_clock` counts only Asleep/Due identities whose
//! current link-budget classification is on-clock. Blocked rows stay visible
//! in the meter but never join the automatic-wake forecast. The forecast is
//! `links.count + on_clock` against `max_links`; it never changes `status`,
//! `links`, `today`, `today_tasks`, or the whole-lane meters. The
//! `snooze_wake_would_exceed_cap` lint is advisory only.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use chrono::{Datelike, NaiveDate, Timelike};
use serde::Serialize;

use super::{
    off_clock::DiskCloseVault, PlanLint, PlanReport,
    LINT_SNOOZE_WAKE_WOULD_EXCEED_CAP,
};
use crate::native::{
    capture_pomodoro_start::LinkClockContext, env as bob_env, note_tasks,
    pomodoro as native_pomodoro, snooze, vault_links::VaultLinkResolver,
};

/// Nonbinding wake forecast: what the on-clock link budget would hold if
/// every currently workable snoozed task woke now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SnoozedForecast {
    pub(crate) count: usize,
    pub(crate) cap: u32,
    pub(crate) over: bool,
}

/// Snoozed meter plus forecast. `count` is unique planned identities (live
/// plus blocked); `due` and `blocked` count those states. `on_clock` counts
/// only Asleep/Due identities whose link-budget clock is on-clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SnoozedBudget {
    pub(crate) count: usize,
    pub(crate) due: usize,
    pub(crate) blocked: usize,
    pub(crate) on_clock: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_wake_at: Option<String>,
    pub(crate) forecast: SnoozedForecast,
}

impl SnoozedBudget {
    pub(crate) fn empty(links_count: usize, cap: u32) -> Self {
        let over =
            u64::try_from(links_count).unwrap_or(u64::MAX) > u64::from(cap);
        Self {
            count: 0,
            due: 0,
            blocked: 0,
            on_clock: 0,
            next_wake_at: None,
            forecast: SnoozedForecast {
                count: links_count,
                cap,
                over,
            },
        }
    }
}

/// Shared report-enrichment path. Callers pass the already-assembled report
/// plus the vault location, the vault-relative daily path, and the daily
/// note contents when the note exists. Missing notes yield an empty snoozed
/// view; a Snoozed section without a Pomodoros section still reads.
pub(crate) fn with_snoozed_forecast(
    mut report: PlanReport,
    bob_dir: &Path,
    daily_relative: &str,
    contents: Option<&str>,
) -> PlanReport {
    let Some(contents) = contents else {
        report.snoozed =
            SnoozedBudget::empty(report.links.count, report.caps.max_links);
        return report;
    };
    let day_path = PathBuf::from(daily_relative);
    let note_date = canonical_daily_date(&day_path)
        .or_else(|| report.date.parse::<NaiveDate>().ok())
        .unwrap_or_else(|| bob_env::current_datetime().date());
    let today = bob_env::current_datetime().date();
    let wall = bob_env::current_datetime();
    let zone = bob_env::configured_timezone()
        .or_else(snooze::host_timezone)
        .unwrap_or(chrono_tz::UTC);
    let now_minutes =
        u16::try_from(wall.hour() * 60 + wall.minute()).unwrap_or(u16::MAX);
    let clock = snooze::when::SnoozeClock {
        date: today,
        now_minutes,
        zone,
    };
    let snoozed = compute_snoozed(
        bob_dir,
        &day_path,
        contents,
        note_date,
        &clock,
        report.links.count,
        report.caps.max_links,
    );
    let over = snoozed.forecast.over && snoozed.on_clock > 0;
    let forecast_count = snoozed.forecast.count;
    let forecast_cap = snoozed.forecast.cap;
    let on_clock = snoozed.on_clock;
    report.snoozed = snoozed;
    if over && on_clock > 0 {
        report.warnings.push(PlanLint {
            code: LINT_SNOOZE_WAKE_WOULD_EXCEED_CAP.to_string(),
            message: format!(
                "snoozed wake would exceed the link cap: {forecast_count}/{forecast_cap} links when all wake; wake is never refused",
            ),
            line: None,
            link: None,
            entry: None,
            task_type: None,
            entry_types: None,
        });
    }
    report
}

fn compute_snoozed(
    bob_dir: &Path,
    day_path: &Path,
    contents: &str,
    note_date: NaiveDate,
    clock: &snooze::when::SnoozeClock,
    links_count: usize,
    cap: u32,
) -> SnoozedBudget {
    let settings = note_tasks::read_settings(bob_dir);
    let resolver = VaultLinkResolver::new(bob_dir.to_path_buf());
    let note_index = resolver.note_index();
    let mut cache: BTreeMap<PathBuf, String> = BTreeMap::new();
    cache.insert(day_path.to_path_buf(), contents.to_string());
    let lines: Vec<&str> = contents.lines().collect();
    if let Some(section) = snooze::parse::parse_section(&lines) {
        for row in &section.rows {
            let (Some(target), Some(_)) =
                (row.target.as_deref(), row.block_id.as_deref())
            else {
                continue;
            };
            let resolved = note_index.resolve_detailed(Some(day_path), target);
            if let crate::native::vault_links::LinkResolution::Found(path) =
                resolved
                && !cache.contains_key(&path)
                && let Ok(text) = std::fs::read_to_string(bob_dir.join(&path))
            {
                cache.insert(path, text);
            }
        }
    }
    let view = snooze::read::read(
        contents,
        day_path,
        note_date,
        clock,
        note_index,
        &settings,
        |path| cache.get(path).map(String::as_str),
    );
    let mut due = 0usize;
    let mut blocked = 0usize;
    let mut live_identities = Vec::new();
    for row in &view.classified.rows {
        match row.state {
            snooze::state::RowState::Due => {
                due += 1;
                if let Some(identity) = row.identity.clone() {
                    live_identities.push(identity);
                }
            }
            snooze::state::RowState::Asleep => {
                if let Some(identity) = row.identity.clone() {
                    live_identities.push(identity);
                }
            }
            snooze::state::RowState::Blocked => {
                blocked += 1;
            }
            snooze::state::RowState::History
            | snooze::state::RowState::Ignored(_) => {}
        }
    }
    let planned = snooze::state::live_targets(&view.classified).planned.len();
    let vault = DiskCloseVault::new(bob_dir.to_path_buf());
    let mut context = LinkClockContext::new(Some(&vault), day_path, contents);
    let mut on_clock = 0usize;
    for identity in &live_identities {
        let target = if identity.route == day_path {
            String::new()
        } else {
            identity
                .route
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/")
        };
        let fact = context.resolve(&target, &identity.block_id, false);
        if fact.fact.off_clock != Some(true) {
            on_clock += 1;
        }
    }
    let forecast_count = links_count.saturating_add(on_clock);
    let over =
        u64::try_from(forecast_count).unwrap_or(u64::MAX) > u64::from(cap);
    SnoozedBudget {
        count: planned,
        due,
        blocked,
        on_clock,
        next_wake_at: view.classified.next_wake_at.clone(),
        forecast: SnoozedForecast {
            count: forecast_count,
            cap,
            over,
        },
    }
}

fn canonical_daily_date(relative_path: &Path) -> Option<NaiveDate> {
    let mut components = relative_path.components();
    let Component::Normal(year_component) = components.next()? else {
        return None;
    };
    let Component::Normal(file_component) = components.next()? else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    let year_text = year_component.to_str()?;
    let file_name = file_component.to_str()?;
    if year_text.len() != 4
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
        || file_name.len() != 11
        || !file_name.ends_with(".md")
    {
        return None;
    }
    let date = native_pomodoro::parse_day_file_date(file_name)?;
    (year_text.parse::<i32>().ok()? == date.year()).then_some(date)
}

//! `bob bank report` with the totals semantics.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use super::{
    config::{AccountKind, BankConfig},
    dates::{
        month_range, parse_month_arg, parse_week_arg, parse_year_arg,
        resolve_day, year_range,
    },
    grammar::{render_day_heading, YearMonth},
    model::{compute_totals, month_of, month_path, row_json, BankRow},
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeriodKind {
    Day,
    Week,
    Month,
    Year,
    Quarter,
    IsoYear,
    AllTime,
}

impl PeriodKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
            Self::Quarter => "quarter",
            Self::IsoYear => "iso_year",
            Self::AllTime => "all_time",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Period {
    pub(crate) kind: PeriodKind,
    pub(crate) key: String,
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
    pub(crate) label: String,
    pub(crate) so_far: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct BreakdownEntry {
    pub(crate) key: String,
    pub(crate) emoji: String,
    pub(crate) label: String,
    pub(crate) out_cents: i64,
    pub(crate) share: f64,
    pub(crate) rows: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ReportDay {
    pub(crate) date: NaiveDate,
    pub(crate) heading: String,
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) out_cents: i64,
    pub(crate) in_cents: i64,
    pub(crate) rows: Vec<BankRow>,
    pub(crate) waiting: usize,
    pub(crate) flagged: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Coverage {
    pub(crate) tracked_since: Option<NaiveDate>,
    pub(crate) imported_through: Option<NaiveDate>,
    pub(crate) start: Option<NaiveDate>,
    pub(crate) through: NaiveDate,
    pub(crate) complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavTarget {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) date: NaiveDate,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Navigation {
    pub(crate) waiting: Option<NavTarget>,
    pub(crate) first: Option<NavTarget>,
    pub(crate) months: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct BankReport {
    pub(crate) today: NaiveDate,
    pub(crate) period: Period,
    pub(crate) totals: super::model::Totals,
    pub(crate) breakdown: Vec<BreakdownEntry>,
    pub(crate) days: Vec<ReportDay>,
    pub(crate) waiting_period: usize,
    pub(crate) waiting_all: usize,
    pub(crate) waiting_oldest: Option<NaiveDate>,
    pub(crate) flagged: usize,
    pub(crate) categories: Vec<super::config::Category>,
    pub(crate) accounts: Vec<super::config::AccountStatus>,
    pub(crate) default_alias: String,
    pub(crate) coverage: Coverage,
    pub(crate) navigation: Navigation,
}

pub(crate) fn resolve_period(
    day: Option<&str>,
    month: Option<&str>,
    week: Option<&str>,
    year: Option<&str>,
    period: Option<&str>,
    today: NaiveDate,
) -> Result<Period, String> {
    let selected = [
        day.is_some(),
        month.is_some(),
        week.is_some(),
        year.is_some(),
        period.is_some(),
    ]
    .iter()
    .filter(|present| **present)
    .count();
    if selected > 1 {
        return Err(
            "pass at most one of -D/--day, -m/--month, -p/--period, -w/--week, -y/--year"
                .to_owned(),
        );
    }
    if let Some(raw) = day {
        let date = resolve_day(raw, today)?;
        return Ok(Period {
            kind: PeriodKind::Day,
            key: date.format("%Y-%m-%d").to_string(),
            start: date,
            end: date,
            label: render_day_heading(date)
                .trim_start_matches('#')
                .trim()
                .to_owned(),
            so_far: date == today,
        });
    }
    if let Some(raw) = week {
        let (start, end) = parse_week_arg(raw)?;
        return Ok(Period {
            kind: PeriodKind::Week,
            key: raw.trim().to_owned(),
            start,
            end,
            label: format!(
                "{} – {}",
                start.format("%b %-d"),
                end.format("%b %-d, %Y")
            ),
            so_far: start <= today && today <= end,
        });
    }
    if let Some(raw) = year {
        let year = parse_year_arg(raw)?;
        let (start, end) = year_range(year);
        return Ok(Period {
            kind: PeriodKind::Year,
            key: format!("{year:04}"),
            start,
            end,
            label: format!("{year:04}"),
            so_far: start <= today && today <= end,
        });
    }
    if let Some(raw) = period {
        return resolve_stats_strip_period(raw, today);
    }
    let month = match month {
        Some(raw) => parse_month_arg(raw)?,
        None => YearMonth {
            year: today.year(),
            month: today.month(),
        },
    };
    let (start, end) = month_range(month);
    let label = month.label();
    Ok(Period {
        kind: PeriodKind::Month,
        key: month.as_str(),
        start,
        end,
        label,
        so_far: start <= today && today <= end,
    })
}

/// Resolve `-p/--period`: a day form goes through `resolve_day` (identical
/// to `-D`), and every other form goes through the periods calendar
/// (`PeriodId::resolve`). `-p 2026` is the ISO week-year; `-y 2026` stays
/// the calendar year. All time is provisional here (today to today) and is
/// finalized by `all_time_period` once rows are loaded.
fn resolve_stats_strip_period(
    raw: &str,
    today: NaiveDate,
) -> Result<Period, String> {
    use crate::native::periods::{calendar::PeriodId, labels::range_label};
    if let Ok(date) = resolve_day(raw, today) {
        return Ok(Period {
            kind: PeriodKind::Day,
            key: date.format("%Y-%m-%d").to_string(),
            start: date,
            end: date,
            label: render_day_heading(date)
                .trim_start_matches('#')
                .trim()
                .to_owned(),
            so_far: date == today,
        });
    }
    let id = PeriodId::resolve(raw, today)?;
    match id {
        PeriodId::Week { .. } => {
            let (start, end) = id
                .start()
                .zip(id.end())
                .ok_or_else(|| format!("invalid period {raw:?}"))?;
            Ok(Period {
                kind: PeriodKind::Week,
                key: id.to_string(),
                start,
                end,
                label: format!(
                    "{} – {}",
                    start.format("%b %-d"),
                    end.format("%b %-d, %Y")
                ),
                so_far: start <= today && today <= end,
            })
        }
        PeriodId::Quarter { .. } => {
            let (start, end) = id
                .start()
                .zip(id.end())
                .ok_or_else(|| format!("invalid period {raw:?}"))?;
            Ok(Period {
                kind: PeriodKind::Quarter,
                key: id.to_string(),
                start,
                end,
                label: format!("{id} · {}", range_label(start, end)),
                so_far: start <= today && today <= end,
            })
        }
        PeriodId::Year(year) => {
            let (start, end) = id
                .start()
                .zip(id.end())
                .ok_or_else(|| format!("invalid period {raw:?}"))?;
            Ok(Period {
                kind: PeriodKind::IsoYear,
                key: format!("{year:04}"),
                start,
                end,
                label: format!("{id} · {}", range_label(start, end)),
                so_far: start <= today && today <= end,
            })
        }
        PeriodId::AllTime => Ok(Period {
            kind: PeriodKind::AllTime,
            key: "all_time".to_owned(),
            start: today,
            end: today,
            label: "All time".to_owned(),
            so_far: true,
        }),
    }
}

/// Finalize an all-time period over the bank's own history: it starts at
/// the earliest `start` of a non-cash account or the earliest row date,
/// whichever is earlier (today when neither exists), and ends at the later
/// of today and the latest row date, so no row is ever excluded.
pub(crate) fn all_time_period(
    config: Option<&BankConfig>,
    rows: &[BankRow],
    today: NaiveDate,
) -> Period {
    let mut earliest: Option<NaiveDate> = None;
    if let Some(config) = config {
        for account in &config.accounts {
            if account.kind.importable()
                && let Some(start) = account.start
            {
                earliest = Some(earliest.map_or(start, |date| date.min(start)));
            }
        }
    }
    let mut latest: Option<NaiveDate> = None;
    for row in rows {
        earliest = Some(earliest.map_or(row.date, |date| date.min(row.date)));
        latest = Some(latest.map_or(row.date, |date| date.max(row.date)));
    }
    let start = earliest.unwrap_or(today);
    let end = latest.map_or(today, |date| date.max(today));
    Period {
        kind: PeriodKind::AllTime,
        key: "all_time".to_owned(),
        start,
        end,
        label: format!("All time · since {}", start.format("%b %-d, %Y")),
        so_far: start <= today && today <= end,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    all_rows: Vec<BankRow>,
    today: NaiveDate,
    period: Period,
    config: Option<&BankConfig>,
    categories: Vec<super::config::Category>,
    accounts: Vec<super::config::AccountStatus>,
    default_alias: String,
    heading_lines: &HashMap<(String, NaiveDate), (String, usize)>,
    journal: &super::journal::JournalSnapshot,
    summary: bool,
) -> BankReport {
    let waiting_all = all_rows.iter().filter(|row| !row.acknowledged).count();
    let waiting_oldest = all_rows
        .iter()
        .filter(|row| !row.acknowledged)
        .map(|row| row.date)
        .min();
    let coverage = coverage_for(&period, config, journal, &all_rows);
    let period_rows: Vec<BankRow> = all_rows
        .into_iter()
        .filter(|row| row.date >= period.start && row.date <= period.end)
        .collect();
    let totals = compute_totals(&period_rows, config);
    let breakdown = breakdown_for(&period_rows, config);
    let flagged = period_rows.iter().filter(|row| row.flagged).count();
    let waiting_period =
        period_rows.iter().filter(|row| !row.acknowledged).count();
    let navigation = navigation_for(&period_rows, config, heading_lines);

    let mut by_date: BTreeMap<NaiveDate, Vec<BankRow>> = BTreeMap::new();
    for row in &period_rows {
        by_date.entry(row.date).or_default().push(row.clone());
    }
    let mut days = Vec::new();
    if !summary {
        let mut cursor = period.start;
        loop {
            let rows = by_date.remove(&cursor).unwrap_or_default();
            let month = month_of(cursor);
            let path = month_path(month);
            let (heading_path, line) = heading_lines
                .get(&(path.clone(), cursor))
                .cloned()
                .or_else(|| {
                    rows.first().map(|row| (row.path.clone(), row.line))
                })
                .unwrap_or_else(|| (path.clone(), 1));
            let heading = rows
                .first()
                .map(|row| row.heading.clone())
                .unwrap_or_else(|| render_day_heading(cursor));
            // Day out/in run through the same accounting path as totals.
            let day_totals = compute_totals(&rows, config);
            days.push(ReportDay {
                date: cursor,
                heading,
                path: heading_path,
                line,
                out_cents: day_totals.out_cents,
                in_cents: day_totals.in_cents,
                rows,
                waiting: day_totals.waiting,
                flagged: day_totals.flagged,
            });
            if cursor == period.end {
                break;
            }
            let Some(next) = cursor.succ_opt() else {
                break;
            };
            cursor = next;
        }
    }

    BankReport {
        today,
        period,
        totals,
        breakdown,
        days,
        waiting_period,
        waiting_all,
        waiting_oldest,
        flagged,
        categories,
        accounts,
        default_alias,
        coverage,
        navigation,
    }
}

/// Whether a row carries counted money: it lands in `in_cents` or
/// `out_cents` under `compute_totals`.
fn is_counted_money(row: &BankRow, config: Option<&BankConfig>) -> bool {
    if row.cents == 0 {
        return false;
    }
    let kind = config.and_then(|config| {
        config.account(&row.account).map(|account| account.kind)
    });
    if kind == Some(AccountKind::Cash)
        || row.category.as_deref() == Some("transfer")
    {
        return false;
    }
    if row.cents < 0 {
        return true;
    }
    matches!(row.category.as_deref(), Some("income") | Some("refund"))
}

fn coverage_for(
    period: &Period,
    config: Option<&BankConfig>,
    journal: &super::journal::JournalSnapshot,
    all_rows: &[BankRow],
) -> Coverage {
    let tracked_since = {
        let mut earliest: Option<NaiveDate> = None;
        if let Some(config) = config {
            for account in &config.accounts {
                if account.kind.importable()
                    && let Some(start) = account.start
                {
                    earliest =
                        Some(earliest.map_or(start, |date| date.min(start)));
                }
            }
        }
        match earliest {
            Some(date) => Some(date),
            None => all_rows.iter().map(|row| row.date).min(),
        }
    };
    let imported_through = {
        let mut through: Option<NaiveDate> = None;
        let mut importable = false;
        if let Some(config) = config {
            for account in &config.accounts {
                if !account.kind.importable() {
                    continue;
                }
                importable = true;
                let Some(start) = account.start else {
                    continue;
                };
                let latest_end = journal
                    .imports
                    .iter()
                    .filter(|event| event.account == account.alias)
                    .map(|event| event.end)
                    .max();
                let account_through =
                    latest_end.or_else(|| start.pred_opt()).unwrap_or(start);
                through =
                    Some(through.map_or(account_through, |date| {
                        date.min(account_through)
                    }));
            }
        }
        if importable {
            through
        } else {
            None
        }
    };
    let start = match tracked_since {
        Some(tracked) if tracked <= period.end => {
            Some(tracked.max(period.start))
        }
        _ => None,
    };
    let through = match imported_through {
        Some(imported) => imported.min(period.end),
        None => period.end,
    };
    let complete = tracked_since.is_none_or(|tracked| tracked <= period.start)
        && imported_through.is_none_or(|imported| imported >= period.end);
    Coverage {
        tracked_since,
        imported_through,
        start,
        through,
        complete,
    }
}

fn navigation_for(
    period_rows: &[BankRow],
    config: Option<&BankConfig>,
    heading_lines: &HashMap<(String, NaiveDate), (String, usize)>,
) -> Navigation {
    let waiting = period_rows
        .iter()
        .filter(|row| !row.acknowledged)
        .min_by(|left, right| {
            (left.date, &left.path, left.line).cmp(&(
                right.date,
                &right.path,
                right.line,
            ))
        })
        .map(|row| NavTarget {
            path: row.path.clone(),
            line: row.line,
            date: row.date,
        });
    let first_date = period_rows
        .iter()
        .filter(|row| is_counted_money(row, config))
        .map(|row| row.date)
        .min();
    let first = first_date.map(|date| {
        let (path, line) = heading_lines
            .get(&(month_path(month_of(date)), date))
            .cloned()
            .or_else(|| {
                period_rows
                    .iter()
                    .find(|row| row.date == date)
                    .map(|row| (row.path.clone(), row.line))
            })
            .unwrap_or_else(|| (month_path(month_of(date)), 1));
        NavTarget { path, line, date }
    });
    let mut months_set = std::collections::BTreeSet::new();
    for row in period_rows {
        months_set.insert(month_path(month_of(row.date)));
    }
    Navigation {
        waiting,
        first,
        months: months_set.into_iter().collect(),
    }
}

fn breakdown_for(
    rows: &[BankRow],
    config: Option<&BankConfig>,
) -> Vec<BreakdownEntry> {
    let mut per_key: HashMap<String, (i64, usize)> = HashMap::new();
    let mut unclassified_out = 0i64;
    let mut unclassified_rows = 0usize;
    for row in rows {
        if row.cents >= 0 {
            continue;
        }
        let kind = config.and_then(|config| {
            config.account(&row.account).map(|account| account.kind)
        });
        if kind == Some(AccountKind::Cash)
            || row.category.as_deref() == Some("transfer")
        {
            continue;
        }
        let magnitude = -row.cents;
        match row.category.as_deref() {
            Some("cash") => {
                let entry = per_key.entry("cash".to_owned()).or_insert((0, 0));
                entry.0 += magnitude;
                entry.1 += 1;
            }
            Some("income") | Some("refund") | None => {
                unclassified_out += magnitude;
                unclassified_rows += 1;
            }
            Some(key) => {
                let entry = per_key.entry(key.to_owned()).or_insert((0, 0));
                entry.0 += magnitude;
                entry.1 += 1;
            }
        }
    }
    let total = per_key.values().map(|(cents, _)| *cents).sum::<i64>()
        + unclassified_out;
    let mut ordered: Vec<BreakdownEntry> = Vec::new();
    if let Some(config) = config {
        for category in &config.categories {
            if category.builtin && category.key != "cash" {
                continue;
            }
            if category.key == "cash" {
                continue;
            }
            if let Some((cents, count)) = per_key.remove(&category.key) {
                ordered.push(BreakdownEntry {
                    key: category.key.clone(),
                    emoji: category.emoji.clone(),
                    label: category.label.clone(),
                    out_cents: cents,
                    share: share_of(cents, total),
                    rows: count,
                });
            }
        }
    } else {
        let mut keys: Vec<String> = per_key
            .keys()
            .filter(|key| *key != "cash")
            .cloned()
            .collect();
        keys.sort();
        for key in keys {
            if let Some((cents, count)) = per_key.remove(&key) {
                ordered.push(BreakdownEntry {
                    key: key.clone(),
                    emoji: String::new(),
                    label: key.clone(),
                    out_cents: cents,
                    share: share_of(cents, total),
                    rows: count,
                });
            }
        }
    }
    if let Some((cents, count)) = per_key.remove("cash") {
        let (emoji, label) = config
            .and_then(|config| config.category("cash"))
            .map(|category| (category.emoji.clone(), category.label.clone()))
            .unwrap_or_else(|| ("🏧".to_owned(), "Cash out".to_owned()));
        ordered.push(BreakdownEntry {
            key: "cash".to_owned(),
            emoji,
            label,
            out_cents: cents,
            share: share_of(cents, total),
            rows: count,
        });
    }
    // Any leftover builtin outflow (should not happen) folds into
    // unclassified so no outflow is hidden.
    for (_, (cents, count)) in per_key {
        unclassified_out += cents;
        unclassified_rows += count;
    }
    if unclassified_out > 0 || unclassified_rows > 0 {
        ordered.push(BreakdownEntry {
            key: "unclassified".to_owned(),
            emoji: "·".to_owned(),
            label: "Unclassified".to_owned(),
            out_cents: unclassified_out,
            share: share_of(unclassified_out, total),
            rows: unclassified_rows,
        });
    }
    ordered
}

fn share_of(part: i64, total: i64) -> f64 {
    if total <= 0 {
        0.0
    } else {
        part as f64 / total as f64
    }
}

fn option_date(value: Option<NaiveDate>) -> Value {
    match value {
        Some(date) => Value::String(date.format("%Y-%m-%d").to_string()),
        None => Value::Null,
    }
}

fn nav_target_json(target: &Option<NavTarget>) -> Value {
    match target {
        Some(target) => json!({
            "path": target.path,
            "line": target.line,
            "date": target.date.format("%Y-%m-%d").to_string(),
        }),
        None => Value::Null,
    }
}

pub(crate) fn to_json(report: &BankReport) -> Value {
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "today": report.today.format("%Y-%m-%d").to_string(),
        "categories": report.categories.iter().map(super::config::Category::to_json).collect::<Vec<_>>(),
        "accounts": report.accounts.iter().map(|account| account.to_json()).collect::<Vec<_>>(),
        "issues": {"errors": 0, "warnings": 0},
        "period": {
            "kind": report.period.kind.as_str(),
            "key": report.period.key,
            "start": report.period.start.format("%Y-%m-%d").to_string(),
            "end": report.period.end.format("%Y-%m-%d").to_string(),
            "label": report.period.label,
            "so_far": report.period.so_far,
            "note": Value::Null,
        },
        "totals": {
            "out_cents": report.totals.out_cents,
            "in_cents": report.totals.in_cents,
            "net_cents": report.totals.net_cents,
            "refund_cents": report.totals.refund_cents,
            "transfer_out_cents": report.totals.transfer_out_cents,
            "transfer_in_cents": report.totals.transfer_in_cents,
            "cash_out_cents": report.totals.cash_out_cents,
            "cash_traced_cents": report.totals.cash_traced_cents,
            "unclassified_out_cents": report.totals.unclassified_out_cents,
            "unclassified_in_cents": report.totals.unclassified_in_cents,
        },
        "breakdown": report.breakdown.iter().map(|entry| json!({
            "key": entry.key,
            "emoji": entry.emoji,
            "label": entry.label,
            "out_cents": entry.out_cents,
            "share": entry.share,
            "rows": entry.rows,
        })).collect::<Vec<_>>(),
        "days": report.days.iter().map(|day| json!({
            "date": day.date.format("%Y-%m-%d").to_string(),
            "heading": day.heading,
            "path": day.path,
            "line": day.line,
            "out_cents": day.out_cents,
            "in_cents": day.in_cents,
            "rows": day.rows.iter().map(|row| row_json(row, &report.default_alias)).collect::<Vec<_>>(),
            "waiting": day.waiting,
            "flagged": day.flagged,
        })).collect::<Vec<_>>(),
        "waiting": {
            "period": report.waiting_period,
            "all": report.waiting_all,
            "oldest": option_date(report.waiting_oldest),
        },
        "coverage": {
            "tracked_since": option_date(report.coverage.tracked_since),
            "imported_through": option_date(report.coverage.imported_through),
            "start": option_date(report.coverage.start),
            "through": report.coverage.through.format("%Y-%m-%d").to_string(),
            "complete": report.coverage.complete,
        },
        "navigation": {
            "waiting": nav_target_json(&report.navigation.waiting),
            "first": nav_target_json(&report.navigation.first),
            "months": report.navigation.months,
        },
        "flagged": report.flagged,
    })
}

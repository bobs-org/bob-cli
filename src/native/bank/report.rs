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
}

impl PeriodKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
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
}

pub(crate) fn resolve_period(
    day: Option<&str>,
    month: Option<&str>,
    week: Option<&str>,
    year: Option<&str>,
    today: NaiveDate,
) -> Result<Period, String> {
    let selected = [
        day.is_some(),
        month.is_some(),
        week.is_some(),
        year.is_some(),
    ]
    .iter()
    .filter(|present| **present)
    .count();
    if selected > 1 {
        return Err(
            "pass at most one of -D/--day, -m/--month, -w/--week, -y/--year"
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
) -> BankReport {
    let waiting_all = all_rows.iter().filter(|row| !row.acknowledged).count();
    let waiting_oldest = all_rows
        .iter()
        .filter(|row| !row.acknowledged)
        .map(|row| row.date)
        .min();
    let period_rows: Vec<BankRow> = all_rows
        .into_iter()
        .filter(|row| row.date >= period.start && row.date <= period.end)
        .collect();
    let totals = compute_totals(&period_rows, config);
    let breakdown = breakdown_for(&period_rows, config);
    let flagged = period_rows.iter().filter(|row| row.flagged).count();
    let waiting_period =
        period_rows.iter().filter(|row| !row.acknowledged).count();

    let mut by_date: BTreeMap<NaiveDate, Vec<BankRow>> = BTreeMap::new();
    for row in &period_rows {
        by_date.entry(row.date).or_default().push(row.clone());
    }
    let mut days = Vec::new();
    let mut cursor = period.start;
    while cursor <= period.end {
        let rows = by_date.remove(&cursor).unwrap_or_default();
        let month = month_of(cursor);
        let path = month_path(month);
        let (heading_path, line) = heading_lines
            .get(&(path.clone(), cursor))
            .cloned()
            .or_else(|| rows.first().map(|row| (row.path.clone(), row.line)))
            .unwrap_or_else(|| (path.clone(), 1));
        let heading = rows
            .first()
            .map(|row| row.heading.clone())
            .unwrap_or_else(|| render_day_heading(cursor));
        let mut out_cents = 0i64;
        let mut in_cents = 0i64;
        let mut waiting = 0usize;
        let mut flagged_count = 0usize;
        for row in &rows {
            let kind = config.and_then(|config| {
                config.account(&row.account).map(|account| account.kind)
            });
            if kind == Some(AccountKind::Cash)
                || row.category.as_deref() == Some("transfer")
                || row.cents == 0
            {
                // Day out/in follow the same keep-apart rules as totals:
                // cash-account and transfer rows never land in day out/in.
            } else if row.cents < 0 {
                out_cents += -row.cents;
            } else if row.category.as_deref() == Some("income")
                || row.category.as_deref() == Some("refund")
            {
                in_cents += row.cents;
            }
            if !row.acknowledged {
                waiting += 1;
            }
            if row.flagged {
                flagged_count += 1;
            }
        }
        days.push(ReportDay {
            date: cursor,
            heading,
            path: heading_path,
            line,
            out_cents,
            in_cents,
            rows,
            waiting,
            flagged: flagged_count,
        });
        let Some(next) = cursor.succ_opt() else {
            break;
        };
        cursor = next;
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

pub(crate) fn to_json(report: &BankReport) -> Value {
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "today": report.today.format("%Y-%m-%d").to_string(),
        "categories": report.categories.iter().map(|category| json!({
            "key": category.key,
            "emoji": category.emoji,
            "label": category.label,
            "builtin": category.builtin,
        })).collect::<Vec<_>>(),
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
            "oldest": report.waiting_oldest.map(|date| date.format("%Y-%m-%d").to_string()),
        },
        "flagged": report.flagged,
    })
}

//! Bare `bob bank` waiting queue (`list`).

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    config::{AccountStatus, Category},
    dates::parse_month_arg,
    grammar::YearMonth,
    model::{row_json, BankRow},
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct ListFilter {
    pub(crate) all: bool,
    pub(crate) month: Option<YearMonth>,
}

#[derive(Debug, Clone)]
pub(crate) struct ListDay {
    pub(crate) date: NaiveDate,
    pub(crate) heading: String,
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) rows: Vec<BankRow>,
}

#[derive(Debug, Clone)]
pub(crate) struct ListReport {
    pub(crate) today: NaiveDate,
    pub(crate) filter: &'static str,
    pub(crate) month: Option<YearMonth>,
    pub(crate) days: Vec<ListDay>,
    pub(crate) rows_total: usize,
    pub(crate) waiting: usize,
    pub(crate) flagged: usize,
    pub(crate) categories: Vec<Category>,
    pub(crate) accounts: Vec<AccountStatus>,
}

pub(crate) fn parse_month_filter(
    value: Option<&str>,
) -> Result<Option<YearMonth>, String> {
    match value {
        None => Ok(None),
        Some(raw) => parse_month_arg(raw).map(Some),
    }
}

pub(crate) fn build(
    rows: Vec<BankRow>,
    today: NaiveDate,
    filter: ListFilter,
    categories: Vec<Category>,
    accounts: Vec<AccountStatus>,
    heading_lines: std::collections::HashMap<
        (String, NaiveDate),
        (String, usize),
    >,
) -> ListReport {
    let want_all = filter.all;
    let mut by_day: std::collections::BTreeMap<
        NaiveDate,
        (String, String, usize, Vec<BankRow>),
    > = std::collections::BTreeMap::new();
    let mut rows_total = 0;
    let mut waiting = 0;
    let mut flagged = 0;
    for row in rows {
        if let Some(month) = filter.month
            && (row.date.year() != month.year
                || row.date.month() != month.month)
        {
            continue;
        }
        if !want_all && row.acknowledged {
            continue;
        }
        rows_total += 1;
        if !row.acknowledged {
            waiting += 1;
        }
        if row.flagged {
            flagged += 1;
        }
        let key = (row.path.clone(), row.date);
        let (heading_path, _) = heading_lines
            .get(&key)
            .cloned()
            .unwrap_or_else(|| (row.path.clone(), row.line));
        by_day
            .entry(row.date)
            .or_insert_with(|| {
                (heading_path, row.heading.clone(), 0, Vec::new())
            })
            .3
            .push(row);
    }
    // Fill heading line numbers from the map when the first row was not the
    // heading line.
    let mut days = Vec::new();
    for (date, (path, heading, _, day_rows)) in by_day {
        let line = heading_lines
            .get(&(path.clone(), date))
            .map(|(_, line)| *line)
            .unwrap_or_else(|| {
                day_rows.first().map(|row| row.line).unwrap_or(1)
            });
        let heading_text = day_rows
            .first()
            .map(|row| row.heading.clone())
            .unwrap_or(heading);
        days.push(ListDay {
            date,
            heading: heading_text,
            path,
            line,
            rows: day_rows,
        });
    }
    days.sort_by_key(|day| day.date);
    ListReport {
        today,
        filter: if want_all { "all" } else { "waiting" },
        month: filter.month,
        days,
        rows_total,
        waiting,
        flagged,
        categories,
        accounts,
    }
}

use chrono::Datelike;

pub(crate) fn to_json(report: &ListReport, default_alias: &str) -> Value {
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
        "accounts": report.accounts.iter().map(|account| json!({
            "alias": account.alias,
            "label": account.label,
            "kind": account.kind.as_str(),
            "default": account.default,
            "start": account.start.map(|date| date.format("%Y-%m-%d").to_string()),
            "status": account.status,
            "last_import": Value::Null,
            "balance_cents": Value::Null,
            "balance_asof": Value::Null,
            "delta_cents": Value::Null,
            "hint": account.hint,
        })).collect::<Vec<_>>(),
        "issues": {"errors": 0, "warnings": 0},
        "filter": report.filter,
        "month": report.month.map(|month| month.as_str()),
        "days": report.days.iter().map(|day| json!({
            "date": day.date.format("%Y-%m-%d").to_string(),
            "heading": day.heading,
            "path": day.path,
            "line": day.line,
            "rows": day.rows.iter().map(|row| row_json(row, default_alias)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "counts": {
            "rows": report.rows_total,
            "waiting": report.waiting,
            "flagged": report.flagged,
        },
    })
}

pub(crate) fn collect_heading_lines(
    scan: &super::scan::BankScan,
) -> std::collections::HashMap<(String, NaiveDate), (String, usize)> {
    let mut map = std::collections::HashMap::new();
    for month_file in &scan.months {
        let note = super::month::parse_month_note(
            &month_file.path,
            &month_file.contents,
            month_file.month,
        );
        for day in &note.days {
            map.insert(
                (note.path.clone(), day.date),
                (note.path.clone(), day.heading_line),
            );
        }
    }
    map
}

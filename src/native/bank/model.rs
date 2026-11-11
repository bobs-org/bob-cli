//! Shared bank model: config, rows, and totals helpers.

use chrono::NaiveDate;

use super::{
    config::{AccountKind, BankConfig},
    grammar::{render_day_heading, YearMonth},
    journal::{JournalSnapshot, TxnOutcome},
    month::{parse_month_note, LocatedRowKind, MonthNote},
    scan::BankScan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BankRow {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) date: NaiveDate,
    pub(crate) heading: String,
    pub(crate) account: String,
    pub(crate) cents: i64,
    pub(crate) payee: String,
    pub(crate) category: Option<String>,
    pub(crate) links: Vec<String>,
    pub(crate) note: Option<String>,
    pub(crate) flagged: bool,
    pub(crate) acknowledged: bool,
    pub(crate) id: Option<String>,
    pub(crate) origin: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) out_cents: i64,
    pub(crate) in_cents: i64,
    pub(crate) net_cents: i64,
    pub(crate) refund_cents: i64,
    pub(crate) transfer_out_cents: i64,
    pub(crate) transfer_in_cents: i64,
    pub(crate) cash_out_cents: i64,
    pub(crate) cash_traced_cents: i64,
    pub(crate) unclassified_out_cents: i64,
    pub(crate) unclassified_in_cents: i64,
    pub(crate) waiting: usize,
    pub(crate) flagged: usize,
}

pub(crate) fn collect_rows(
    scan: &BankScan,
    config: Option<&BankConfig>,
    journal: Option<&JournalSnapshot>,
) -> Vec<BankRow> {
    let mut rows = Vec::new();
    for month_file in &scan.months {
        let note: MonthNote = parse_month_note(
            &month_file.path,
            &month_file.contents,
            month_file.month,
        );
        for day in &note.days {
            let heading = render_day_heading(day.date);
            for row in &day.rows {
                let LocatedRowKind::Parsed(parsed) = &row.kind else {
                    continue;
                };
                let account = parsed.account.clone().unwrap_or_else(|| {
                    config
                        .and_then(|config| config.default_account())
                        .map(|account| account.alias.clone())
                        .unwrap_or_else(|| "checking".to_owned())
                });
                // A matched journal outcome reports as matched; an ID
                // without journal evidence keeps the imported fallback.
                let origin = match parsed.id.as_deref() {
                    None => "typed",
                    Some(id) => {
                        let matched = journal
                            .and_then(|snapshot| snapshot.ids.get(id))
                            .and_then(|key| {
                                journal.and_then(|snapshot| {
                                    snapshot.transactions.get(key)
                                })
                            })
                            .is_some_and(|event| {
                                event.outcome == TxnOutcome::Matched
                            });
                        if matched {
                            "matched"
                        } else {
                            "imported"
                        }
                    }
                };
                rows.push(BankRow {
                    path: note.path.clone(),
                    line: row.line,
                    date: day.date,
                    heading: heading.clone(),
                    account,
                    cents: parsed.cents,
                    payee: parsed.payee.clone(),
                    category: parsed.category.clone(),
                    links: parsed.links.clone(),
                    note: parsed.note.clone(),
                    flagged: parsed.flagged,
                    acknowledged: parsed.acknowledged,
                    id: parsed.id.clone(),
                    origin,
                });
            }
        }
    }
    rows.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then(left.path.cmp(&right.path))
            .then(left.line.cmp(&right.line))
    });
    rows
}

/// Compute every total from rows at read time.
///
/// Rows on `cash`-kind accounts and rows with `[cat:: transfer]` are kept
/// apart. Amounts are signed integer cents; totals below are magnitudes
/// except `net_cents`, which is `in - out`.
pub(crate) fn compute_totals(
    rows: &[BankRow],
    config: Option<&BankConfig>,
) -> Totals {
    let mut totals = Totals::default();
    for row in rows {
        if !row.acknowledged {
            // Waiting counts every `[ ]` row, including transfers and cash.
        }
        if !row.acknowledged {
            totals.waiting += 1;
        }
        if row.flagged {
            totals.flagged += 1;
        }
        if row.cents == 0 {
            continue;
        }
        let kind = config.and_then(|config| {
            config.account(&row.account).map(|account| account.kind)
        });
        if kind == Some(AccountKind::Cash) {
            if row.cents < 0 {
                totals.cash_traced_cents += -row.cents;
            }
            continue;
        }
        if row.category.as_deref() == Some("transfer") {
            if row.cents < 0 {
                totals.transfer_out_cents += -row.cents;
            } else {
                totals.transfer_in_cents += row.cents;
            }
            continue;
        }
        if row.cents < 0 {
            let magnitude = -row.cents;
            totals.out_cents += magnitude;
            match row.category.as_deref() {
                Some("cash") => {
                    totals.cash_out_cents += magnitude;
                }
                Some("income") | Some("refund") => {
                    // An outflow carrying an inflow-only meaning is still
                    // out, but it is unclassified for the breakdown.
                    totals.unclassified_out_cents += magnitude;
                }
                Some(_) => {}
                None => {
                    totals.unclassified_out_cents += magnitude;
                }
            }
        } else {
            match row.category.as_deref() {
                Some("income") => {
                    totals.in_cents += row.cents;
                }
                Some("refund") => {
                    totals.in_cents += row.cents;
                    totals.refund_cents += row.cents;
                }
                // A credit alone is never income: an inflow with a spending
                // category, an unknown category, or no category is
                // unclassified in (outside In).
                _ => {
                    totals.unclassified_in_cents += row.cents;
                }
            }
        }
    }
    totals.net_cents = totals.in_cents - totals.out_cents;
    totals
}

pub(crate) fn month_of(date: NaiveDate) -> YearMonth {
    use chrono::Datelike;
    YearMonth {
        year: date.year(),
        month: date.month(),
    }
}

pub(crate) fn month_path(month: YearMonth) -> String {
    format!("bank/{}.md", month.as_str())
}

pub(crate) fn row_json(
    row: &BankRow,
    default_alias: &str,
) -> serde_json::Value {
    let _ = default_alias;
    serde_json::json!({
        "path": row.path,
        "line": row.line,
        "date": row.date.format("%Y-%m-%d").to_string(),
        "account": row.account,
        "cents": row.cents,
        "payee": row.payee,
        "category": row.category,
        "links": row.links,
        "note": row.note,
        "flagged": row.flagged,
        "acknowledged": row.acknowledged,
        "id": row.id,
        "origin": row.origin,
    })
}

//! `bob bank report -c`: the child-period cashflow and category aggregate.
//!
//! A week, quarter, ISO week-year, or all-time report splits into the same
//! child spans as its Stats Strip (the shared periods-calendar partition):
//! days of a week, weeks of a quarter or ISO year, and quarters of all
//! time. Each bucket's money runs through `compute_totals` over the rows
//! dated inside its span, exactly like the report's own `totals`, so the
//! buckets always add up to the period. Coverage states describe how much
//! of a bucket bob can vouch for; a state never zeroes money. The category
//! series is folded here, so a client never adds cents itself.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    config::BankConfig,
    model::{compute_totals, month_of, month_path, BankRow},
    report::{
        breakdown_for, rows_in_span, share_of, BreakdownEntry, Coverage,
        Period, PeriodKind,
    },
};
use crate::native::periods::{
    calendar::PeriodId,
    children::{child_specs, column_specs, group_specs, ChildSpec, Span},
};

pub(crate) const CHARTS_VERSION: u32 = 1;

/// The most buckets one aggregate carries; all time coarsens to ISO years
/// past this many quarters and reports `null` past this many years.
pub(crate) const MAX_BUCKETS: usize = 64;

/// Ranked category rows before the rest folds into `N more`.
pub(crate) const CATEGORY_LIMIT: usize = 5;

/// Top outflow categories carried per bucket.
const TOP_OUT_LIMIT: usize = 3;

/// The option error for `-c` on a period without stats-strip children.
pub(crate) const PERIOD_ERROR: &str =
    "-c/--charts needs a week, quarter, ISO year, or all-time period (-w or -p)";

/// The child grain of the buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Granularity {
    Day,
    Week,
    Quarter,
    Year,
}

impl Granularity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Quarter => "quarter",
            Self::Year => "year",
        }
    }
}

/// How much of a bucket's span bob can vouch for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BucketState {
    Tracked,
    Partial,
    Untracked,
    AwaitingImport,
    Future,
}

impl BucketState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tracked => "tracked",
            Self::Partial => "partial",
            Self::Untracked => "untracked",
            Self::AwaitingImport => "awaiting_import",
            Self::Future => "future",
        }
    }

    /// The human words for a non-tracked state.
    pub(crate) fn words(self) -> Option<&'static str> {
        match self {
            Self::Tracked => None,
            Self::Partial => Some("partly covered"),
            Self::Untracked => Some("not tracked"),
            Self::AwaitingImport => Some("not imported yet"),
            Self::Future => Some("future"),
        }
    }
}

/// A read-only navigation target: a day heading in a bank month note, or
/// line 1 of a period note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) path: String,
    pub(crate) line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopOut {
    pub(crate) key: String,
    pub(crate) emoji: String,
    pub(crate) label: String,
    pub(crate) out_cents: i64,
}

/// One bucket (a child period) or one group of buckets.
#[derive(Debug, Clone)]
pub(crate) struct Bucket {
    pub(crate) id: String,
    pub(crate) kind: Granularity,
    pub(crate) label: String,
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
    pub(crate) group: Option<String>,
    pub(crate) so_far: bool,
    pub(crate) state: BucketState,
    pub(crate) in_cents: i64,
    pub(crate) out_cents: i64,
    pub(crate) net_cents: i64,
    pub(crate) waiting: usize,
    pub(crate) rows: usize,
    pub(crate) top_out: Vec<TopOut>,
    pub(crate) target: Option<Target>,
}

#[derive(Debug, Clone)]
pub(crate) struct SeriesEntry {
    pub(crate) key: String,
    pub(crate) emoji: String,
    pub(crate) label: String,
    pub(crate) kind: &'static str,
    pub(crate) out_cents: i64,
    pub(crate) share: f64,
    pub(crate) rows: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Rest {
    pub(crate) count: usize,
    pub(crate) out_cents: i64,
    pub(crate) share: f64,
    pub(crate) rows: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Categories {
    pub(crate) limit: usize,
    pub(crate) series: Vec<SeriesEntry>,
    pub(crate) rest: Option<Rest>,
}

#[derive(Debug, Clone)]
pub(crate) struct Charts {
    pub(crate) granularity: Granularity,
    pub(crate) buckets: Vec<Bucket>,
    pub(crate) groups: Vec<Bucket>,
    pub(crate) categories: Categories,
}

/// The stats-strip period behind a report period: weeks (from `-w` or
/// `-p`), quarters, ISO week-years, and all time. Days, calendar months,
/// and calendar years have no strip children.
pub(crate) fn period_id_for(period: &Period) -> Option<PeriodId> {
    match period.kind {
        PeriodKind::Week => Some(PeriodId::week_for_date(period.start)),
        PeriodKind::Quarter => Some(PeriodId::quarter_for_date(period.start)),
        PeriodKind::IsoYear => Some(PeriodId::year_for_date(period.start)),
        PeriodKind::AllTime => Some(PeriodId::AllTime),
        PeriodKind::Day | PeriodKind::Month | PeriodKind::Year => None,
    }
}

/// Everything a bucket's money, state, and target read.
struct BucketInputs<'a> {
    rows: &'a [BankRow],
    config: Option<&'a BankConfig>,
    coverage: &'a Coverage,
    heading_lines: &'a HashMap<(String, NaiveDate), (String, usize)>,
    today: NaiveDate,
}

/// Build the aggregate for `period` from its rows and its breakdown.
///
/// Returns `None` when the period has no strip children (the CLI refuses
/// those before building) or when all time spans more than
/// [`MAX_BUCKETS`] ISO years.
pub(crate) fn build(
    period: &Period,
    period_rows: &[BankRow],
    config: Option<&BankConfig>,
    breakdown: &[BreakdownEntry],
    coverage: &Coverage,
    heading_lines: &HashMap<(String, NaiveDate), (String, usize)>,
    today: NaiveDate,
) -> Option<Charts> {
    let id = period_id_for(period)?;
    let span = Span {
        start: period.start,
        end: period.end,
    };
    let categories = fold_categories(breakdown);
    let inputs = BucketInputs {
        rows: period_rows,
        config,
        coverage,
        heading_lines,
        today,
    };
    // An empty or reversed span (a future-only all-time history) has no
    // children at all.
    let empty = span.start > span.end;
    let granularity = match id {
        PeriodId::Week { .. } => Granularity::Day,
        PeriodId::Quarter { .. } | PeriodId::Year(_) => Granularity::Week,
        PeriodId::AllTime if empty || quarter_count(span) <= MAX_BUCKETS => {
            Granularity::Quarter
        }
        PeriodId::AllTime if year_count(span) <= MAX_BUCKETS => {
            Granularity::Year
        }
        PeriodId::AllTime => return None,
    };
    let (children, groups) = if empty {
        (Vec::new(), Vec::new())
    } else if granularity == Granularity::Year {
        (child_specs(PeriodId::AllTime, span), Vec::new())
    } else {
        (column_specs(id, span), group_specs(id, span))
    };
    let buckets = children
        .iter()
        .map(|spec| child_bucket(spec, &inputs))
        .collect();
    let groups = groups
        .iter()
        .map(|group| {
            period_bucket(
                group.id,
                group.caption.clone(),
                group.span,
                None,
                &inputs,
            )
        })
        .collect();
    Some(Charts {
        granularity,
        buckets,
        groups,
        categories,
    })
}

fn child_bucket(spec: &ChildSpec, inputs: &BucketInputs<'_>) -> Bucket {
    match (spec.day, spec.id) {
        (None, Some(id)) => period_bucket(
            id,
            spec.caption(),
            spec.span,
            spec.group.clone(),
            inputs,
        ),
        _ => {
            let target = spec.day.and_then(|day| {
                inputs
                    .heading_lines
                    .get(&(month_path(month_of(day)), day))
                    .map(|(path, line)| Target {
                        path: path.clone(),
                        line: *line,
                    })
            });
            money_bucket(
                spec.key(),
                Granularity::Day,
                spec.caption(),
                spec.span,
                spec.group.clone(),
                target,
                inputs,
            )
        }
    }
}

fn period_bucket(
    id: PeriodId,
    label: String,
    span: Span,
    group: Option<String>,
    inputs: &BucketInputs<'_>,
) -> Bucket {
    let kind = match id {
        PeriodId::Week { .. } => Granularity::Week,
        PeriodId::Quarter { .. } => Granularity::Quarter,
        PeriodId::Year(_) | PeriodId::AllTime => Granularity::Year,
    };
    let target = Target {
        path: id.path().to_string_lossy().into_owned(),
        line: 1,
    };
    money_bucket(
        id.to_string(),
        kind,
        label,
        span,
        group,
        Some(target),
        inputs,
    )
}

fn money_bucket(
    id: String,
    kind: Granularity,
    label: String,
    span: Span,
    group: Option<String>,
    target: Option<Target>,
    inputs: &BucketInputs<'_>,
) -> Bucket {
    let rows = rows_in_span(inputs.rows, span.start, span.end);
    // Bucket money runs through the same accounting path as `totals`.
    let totals = compute_totals(&rows, inputs.config);
    Bucket {
        id,
        kind,
        label,
        start: span.start,
        end: span.end,
        group,
        so_far: span.start <= inputs.today && inputs.today <= span.end,
        state: bucket_state(span, inputs.today, inputs.coverage),
        in_cents: totals.in_cents,
        out_cents: totals.out_cents,
        net_cents: totals.net_cents,
        waiting: totals.waiting,
        rows: rows.len(),
        top_out: top_out(&rows, inputs.config),
        target,
    }
}

/// The first matching coverage rule for `[s, e]`, with `T = today`,
/// `TS = tracked_since`, and `IT = imported_through`.
pub(crate) fn bucket_state(
    span: Span,
    today: NaiveDate,
    coverage: &Coverage,
) -> BucketState {
    if span.start > today {
        return BucketState::Future;
    }
    if coverage
        .tracked_since
        .is_some_and(|tracked| span.end < tracked)
    {
        return BucketState::Untracked;
    }
    if coverage
        .imported_through
        .is_some_and(|imported| span.start > imported)
    {
        return BucketState::AwaitingImport;
    }
    let starts_untracked = coverage
        .tracked_since
        .is_some_and(|tracked| span.start < tracked);
    let short_import = coverage
        .imported_through
        .is_some_and(|imported| imported < span.end.min(today));
    if starts_untracked || short_import {
        BucketState::Partial
    } else {
        BucketState::Tracked
    }
}

fn top_out(rows: &[BankRow], config: Option<&BankConfig>) -> Vec<TopOut> {
    let mut entries = breakdown_for(rows, config);
    entries.sort_by(|left, right| {
        right
            .out_cents
            .cmp(&left.out_cents)
            .then_with(|| left.key.cmp(&right.key))
    });
    entries
        .into_iter()
        .take(TOP_OUT_LIMIT)
        .map(|entry| TopOut {
            key: entry.key,
            emoji: entry.emoji,
            label: entry.label,
            out_cents: entry.out_cents,
        })
        .collect()
}

fn is_pinned(entry: &BreakdownEntry) -> bool {
    matches!(entry.key.as_str(), "cash" | "unclassified")
        && entry.out_cents != 0
}

fn entry_kind(key: &str) -> &'static str {
    match key {
        "cash" => "cash",
        "unclassified" => "unclassified",
        _ => "category",
    }
}

/// Fold the period breakdown into at most [`CATEGORY_LIMIT`] ranked rows
/// plus a `rest` total. Nonzero `cash` and `unclassified` are pinned; a
/// single leftover joins the series instead of becoming `1 more`.
pub(crate) fn fold_categories(breakdown: &[BreakdownEntry]) -> Categories {
    let total: i64 = breakdown.iter().map(|entry| entry.out_cents).sum();
    let mut ranked: Vec<&BreakdownEntry> = breakdown.iter().collect();
    ranked.sort_by(|left, right| {
        right
            .out_cents
            .cmp(&left.out_cents)
            .then_with(|| left.key.cmp(&right.key))
    });
    let pinned = ranked.iter().filter(|entry| is_pinned(entry)).count();
    let others = ranked.len() - pinned;
    let room = CATEGORY_LIMIT.saturating_sub(pinned);
    let keep = if others == room + 1 {
        others
    } else {
        room.min(others)
    };
    let mut series = Vec::new();
    let mut kept_others = 0usize;
    let mut rest_count = 0usize;
    let mut rest_out = 0i64;
    let mut rest_rows = 0usize;
    for entry in ranked {
        let shown = if is_pinned(entry) {
            true
        } else if kept_others < keep {
            kept_others += 1;
            true
        } else {
            false
        };
        if shown {
            series.push(SeriesEntry {
                key: entry.key.clone(),
                emoji: entry.emoji.clone(),
                label: entry.label.clone(),
                kind: entry_kind(&entry.key),
                out_cents: entry.out_cents,
                share: entry.share,
                rows: entry.rows,
            });
        } else {
            rest_count += 1;
            rest_out += entry.out_cents;
            rest_rows += entry.rows;
        }
    }
    let rest = (rest_count > 0).then(|| Rest {
        count: rest_count,
        out_cents: rest_out,
        share: share_of(rest_out, total),
        rows: rest_rows,
    });
    Categories {
        limit: CATEGORY_LIMIT,
        series,
        rest,
    }
}

/// Quarters from the one holding `span.start` through the one holding
/// `span.end`, counted without walking them.
fn quarter_count(span: Span) -> usize {
    let index = |date: NaiveDate| match PeriodId::quarter_for_date(date) {
        PeriodId::Quarter { year, quarter } => {
            i64::from(year) * 4 + i64::from(quarter)
        }
        _ => 0,
    };
    usize::try_from(index(span.end) - index(span.start) + 1).unwrap_or(0)
}

/// ISO week-years from the one holding `span.start` through the one
/// holding `span.end`, counted without walking them.
fn year_count(span: Span) -> usize {
    let year = |date: NaiveDate| match PeriodId::year_for_date(date) {
        PeriodId::Year(year) => i64::from(year),
        _ => 0,
    };
    usize::try_from(year(span.end) - year(span.start) + 1).unwrap_or(0)
}

fn date_text(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn bucket_json(bucket: &Bucket) -> Value {
    json!({
        "id": bucket.id,
        "kind": bucket.kind.as_str(),
        "label": bucket.label,
        "start": date_text(bucket.start),
        "end": date_text(bucket.end),
        "group": bucket.group,
        "so_far": bucket.so_far,
        "state": bucket.state.as_str(),
        "in_cents": bucket.in_cents,
        "out_cents": bucket.out_cents,
        "net_cents": bucket.net_cents,
        "waiting": bucket.waiting,
        "rows": bucket.rows,
        "top_out": bucket.top_out.iter().map(|entry| json!({
            "key": entry.key,
            "emoji": entry.emoji,
            "label": entry.label,
            "out_cents": entry.out_cents,
        })).collect::<Vec<_>>(),
        "target": bucket.target.as_ref().map_or(Value::Null, |target| json!({
            "path": target.path,
            "line": target.line,
        })),
    })
}

pub(crate) fn to_json(charts: &Charts) -> Value {
    json!({
        "version": CHARTS_VERSION,
        "granularity": charts.granularity.as_str(),
        "buckets": charts.buckets.iter().map(bucket_json).collect::<Vec<_>>(),
        "groups": charts.groups.iter().map(bucket_json).collect::<Vec<_>>(),
        "categories": {
            "limit": charts.categories.limit,
            "series": charts.categories.series.iter().map(|entry| json!({
                "key": entry.key,
                "emoji": entry.emoji,
                "label": entry.label,
                "kind": entry.kind,
                "out_cents": entry.out_cents,
                "share": entry.share,
                "rows": entry.rows,
            })).collect::<Vec<_>>(),
            "rest": charts.categories.rest.as_ref().map_or(Value::Null, |rest| json!({
                "count": rest.count,
                "out_cents": rest.out_cents,
                "share": rest.share,
                "rows": rest.rows,
            })),
        },
    })
}

#[cfg(test)]
#[path = "charts_tests.rs"]
mod tests;

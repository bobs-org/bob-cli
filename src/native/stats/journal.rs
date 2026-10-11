//! Read-only evaluators for journal metrics over recorded review answers.
//!
//! The journal writer lives in the Obsidian plugins (`bob-ledger-tools`
//! `api.stats` appends, `bob-navigation-hotkeys` records); this module never
//! writes journal files. Event parsing and health counters live in `index`;
//! this module selects events by tier, counts distinct row keys, and detects
//! review-close gaps from the registry's own `review_closed` evaluation.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, FixedOffset, NaiveDate};

use super::{
    index::StatsIndex,
    metrics::{effective_since, MetricEvaluation},
    registry::{JournalSelect, JournalTiers, MetricDef, MetricSpec},
    report::StatsWarning,
    value::{DailyValue, DataQuality},
};

/// Distinct walk row keys answered on one day, with the last selected event
/// per key. Ordering follows the index reader: by instant, then path, then
/// line, so equal-timestamp ties resolve deterministically.
#[derive(Default)]
struct DaySelection<'a> {
    /// Last selected event per row key.
    last_by_key: BTreeMap<String, &'a super::index::IndexedJournalEvent>,
    /// Distinct row keys with at least one selected event per tier.
    keys_by_tier: BTreeMap<String, BTreeSet<String>>,
}

/// Evaluate a journal metric over the requested dates. Returns `None` for any
/// non-journal metric.
///
/// `evaluations` carries the already-evaluated metrics this selector needs:
/// the registry's `review_closed` definition for gap detection. Callers must
/// evaluate `review_closed` first (even when it is hidden or unselected) and
/// pass it through; when it is missing or invalid the days report a
/// `journal_limitation` warning instead of gap warnings. There is no second
/// built-in review-closed checker here.
pub(crate) fn evaluate_journal(
    metric: &MetricDef,
    index: &StatsIndex,
    dates: &[NaiveDate],
    today: NaiveDate,
    global_since: Option<NaiveDate>,
    evaluations: &BTreeMap<String, MetricEvaluation>,
) -> Option<MetricEvaluation> {
    let MetricSpec::Journal { select, tiers } = &metric.spec else {
        return None;
    };
    let inferred_since = index.health.journal.since;
    let since = effective_since(metric, inferred_since, global_since);
    let closed_days = review_closed_days(evaluations);
    let days_with_events = days_with_any_event(index);

    let mut warnings = Vec::new();
    if closed_days.is_none() {
        warnings.push(StatsWarning {
            code: "journal_limitation".to_string(),
            metric: Some(metric.id.clone()),
            date: None,
            message: format!(
                "review_closed is not available ({}), so journal gaps cannot \
                 be detected",
                review_closed_unavailable_reason(evaluations),
            ),
        });
    }

    let values = dates
        .iter()
        .map(|date| {
            let provisional = *date == today;
            if since.is_some_and(|start| *date < start) {
                return daily_value(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::NotTracked,
                    Some("before tracking began".to_string()),
                    provisional,
                    BTreeMap::new(),
                );
            }
            // A known closed review with no journaled event in any tier from
            // any host is a plugin gap, never a silent zero. Days before the
            // journal started keep not_tracked above instead.
            if closed_days
                .as_ref()
                .is_some_and(|closed| closed.contains(date))
                && !days_with_events.contains(date)
            {
                warnings.push(StatsWarning {
                    code: "journal_gap".to_string(),
                    metric: Some(metric.id.clone()),
                    date: Some(*date),
                    message: format!(
                        "Review closed on {} but no answers were recorded: \
                         the review plugin may be outdated or a write failed",
                        date.format("%a, %b %-d"),
                    ),
                });
                return daily_value(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some(
                        "review closed but no answers were recorded"
                            .to_string(),
                    ),
                    provisional,
                    BTreeMap::new(),
                );
            }
            let selection = select_day(index, *select, tiers, *date);
            match select {
                JournalSelect::Reviewed => {
                    reviewed_value(*date, provisional, &selection)
                }
                JournalSelect::KeepRate => {
                    keep_rate_value(*date, provisional, tiers, &selection)
                }
            }
        })
        .collect();

    Some(MetricEvaluation {
        since,
        values,
        warnings,
    })
}

/// Days the registry's `review_closed` definition reports as met
/// (nonzero value), or `None` when that definition has no evaluation.
fn review_closed_days(
    evaluations: &BTreeMap<String, MetricEvaluation>,
) -> Option<BTreeSet<NaiveDate>> {
    let evaluation = evaluations.get("review_closed")?;
    Some(
        evaluation
            .values
            .iter()
            .filter(|value| value.value.is_some_and(|met| met != 0))
            .map(|value| value.date)
            .collect(),
    )
}

fn review_closed_unavailable_reason(
    evaluations: &BTreeMap<String, MetricEvaluation>,
) -> String {
    if evaluations.contains_key("review_closed") {
        "it was not evaluated before its journal dependents".to_string()
    } else {
        "its definition is invalid or removed".to_string()
    }
}

fn days_with_any_event(index: &StatsIndex) -> BTreeSet<NaiveDate> {
    index.journal_events.iter().map(|event| event.day).collect()
}

fn select_day<'a>(
    index: &'a StatsIndex,
    select: JournalSelect,
    tiers: &JournalTiers,
    date: NaiveDate,
) -> DaySelection<'a> {
    let mut selection = DaySelection::default();
    for event in index.journal_events.iter().filter(|event| {
        event.day == date && event_selected(select, tiers, event)
    }) {
        selection
            .keys_by_tier
            .entry(event.tier.clone())
            .or_default()
            .insert(event.key.clone());
        let replace =
            selection
                .last_by_key
                .get(&event.key)
                .is_none_or(|previous| {
                    event_sort_key(event) >= event_sort_key(previous)
                });
        if replace {
            selection.last_by_key.insert(event.key.clone(), event);
        }
    }
    selection
}

/// Comparable sort key matching the index reader: by instant, then path,
/// then line. Offsets differ between hosts, so timestamps compare by
/// instant; unparseable timestamps (never produced by the reader) fall back
/// to string order and stay deterministic.
fn event_sort_key(
    event: &super::index::IndexedJournalEvent,
) -> (Option<DateTime<FixedOffset>>, &str, &str, u32) {
    (
        DateTime::parse_from_rfc3339(&event.ts).ok(),
        event.ts.as_str(),
        event.path.as_str(),
        event.line,
    )
}

/// Tier filter for the `reviewed` selector: `decision` is every tier except
/// `pre`/`post`, `all` is every tier, and a list is an exact tier filter.
/// Tier strings compare case-insensitively; the walk writes them lowercase.
fn tier_selected(tier: &str, tiers: &JournalTiers) -> bool {
    let tier = tier.trim().to_lowercase();
    match tiers {
        JournalTiers::Decision => tier != "pre" && tier != "post",
        JournalTiers::All => true,
        JournalTiers::List(list) => {
            list.iter().any(|item| item.trim().to_lowercase() == tier)
        }
    }
}

/// Keep rate answers only PENDING and NEXT rows, intersected with any
/// configured tier restriction.
fn event_selected(
    select: JournalSelect,
    tiers: &JournalTiers,
    event: &super::index::IndexedJournalEvent,
) -> bool {
    if !tier_selected(&event.tier, tiers) {
        return false;
    }
    match select {
        JournalSelect::Reviewed => true,
        JournalSelect::KeepRate => {
            matches!(
                event.tier.trim().to_lowercase().as_str(),
                "pending" | "next"
            )
        }
    }
}

fn reviewed_value(
    date: NaiveDate,
    provisional: bool,
    selection: &DaySelection<'_>,
) -> DailyValue {
    let distinct = selection.last_by_key.len();
    daily_value(
        date,
        Some(i64::try_from(distinct).unwrap_or(i64::MAX)),
        None,
        None,
        DataQuality::Ok,
        None,
        provisional,
        journal_parts(selection),
    )
}

fn keep_rate_value(
    date: NaiveDate,
    provisional: bool,
    tiers: &JournalTiers,
    selection: &DaySelection<'_>,
) -> DailyValue {
    let denominator = selection.last_by_key.len();
    if denominator == 0 {
        return daily_value(
            date,
            None,
            None,
            None,
            DataQuality::Missing,
            Some(tiers_empty_reason(tiers)),
            provisional,
            BTreeMap::new(),
        );
    }
    let keeps = selection
        .last_by_key
        .values()
        .filter(|event| event.outcome == "keep")
        .count();
    let denominator_i64 = i64::try_from(denominator).unwrap_or(i64::MAX);
    let keeps_i64 = i64::try_from(keeps).unwrap_or(i64::MAX);
    // Basis points pool num and den; periods never average daily percentages.
    let basis_points = (i128::from(keeps_i64) * 10_000
        + i128::from(denominator_i64) / 2)
        / i128::from(denominator_i64);
    daily_value(
        date,
        Some(i64::try_from(basis_points).unwrap_or(i64::MAX)),
        Some(keeps_i64),
        Some(denominator_i64),
        DataQuality::Ok,
        None,
        provisional,
        journal_parts(selection),
    )
}

fn tiers_empty_reason(tiers: &JournalTiers) -> String {
    match tiers {
        JournalTiers::Decision | JournalTiers::All => {
            "no pending or next answers were recorded".to_string()
        }
        JournalTiers::List(list) => {
            let names = list
                .iter()
                .map(|tier| tier.trim())
                .filter(|tier| !tier.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            format!("no answers in tiers [{names}] were recorded")
        }
    }
}

/// Parts shared by both selectors: `tier` counts distinct keys per tier and
/// `outcome` buckets each key's last selected answer. Unknown outcomes land
/// under `other` so the outcome vocabulary stays additive.
fn journal_parts(
    selection: &DaySelection<'_>,
) -> BTreeMap<String, BTreeMap<String, i64>> {
    let mut parts = BTreeMap::new();
    for (tier, keys) in &selection.keys_by_tier {
        add_dimension_part(
            &mut parts,
            "tier",
            tier,
            i64::try_from(keys.len()).unwrap_or(i64::MAX),
        );
    }
    for event in selection.last_by_key.values() {
        add_dimension_part(
            &mut parts,
            "outcome",
            &normalized_outcome(&event.outcome),
            1,
        );
    }
    parts
}

/// Recognized outcome spellings: the documented writer vocabulary plus both
/// the shipped decay spellings (`decay:notNow`, `decay:level:0`,
/// `decay:lessOften`) and the design's normalized spellings. Any other
/// `decay:<action>` also counts as recognized so new decay actions stay
/// additive; everything else is `other`.
fn normalized_outcome(outcome: &str) -> String {
    const BASE: &[&str] = &[
        "keep",
        "done",
        "drop",
        "defer",
        "sequence",
        "lane",
        "route",
        "today",
        "card",
        "checklist",
    ];
    if BASE.contains(&outcome) {
        return outcome.to_string();
    }
    if outcome.len() > "decay:".len()
        && outcome[..6].eq_ignore_ascii_case("decay:")
        && !outcome[6..].trim().is_empty()
        && !outcome[6..].chars().any(char::is_whitespace)
    {
        return outcome.to_string();
    }
    "other".to_string()
}

fn add_dimension_part(
    parts: &mut BTreeMap<String, BTreeMap<String, i64>>,
    dimension: &str,
    key: &str,
    value: i64,
) {
    *parts
        .entry(dimension.to_string())
        .or_default()
        .entry(key.to_string())
        .or_default() += value;
}

#[allow(clippy::too_many_arguments)]
fn daily_value(
    date: NaiveDate,
    value: Option<i64>,
    num: Option<i64>,
    den: Option<i64>,
    quality: DataQuality,
    reason: Option<String>,
    provisional: bool,
    parts: BTreeMap<String, BTreeMap<String, i64>>,
) -> DailyValue {
    DailyValue {
        date,
        value,
        num,
        den,
        quality,
        reason,
        provisional,
        parts,
    }
}

#[cfg(test)]
mod tests;

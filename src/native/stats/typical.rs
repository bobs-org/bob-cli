//! Additive `typical` day reference for day-grouped count and minutes points.
//!
//! The typical day is the median of the seven days before D (D−7…D−1),
//! needing at least four usable days. It shares the usable-value rule with
//! rolling medians so the two can never drift apart.

use std::collections::BTreeMap;

use chrono::{Days, NaiveDate};

use super::registry::MetricUnit;
use super::report::{GroupName, SeriesPoint, TypicalDay, TypicalRelation};
use super::series::{
    format_date_span, format_minutes, format_value, is_usable_value, median,
    DateSpan, DAY_WINDOW, DAY_WINDOW_MIN,
};

/// Add the optional `typical` object to day-grouped count/minutes points.
///
/// The window is selected by calendar date (D−7…D−1), never by index, so a
/// gap can never shift it. Callers pass the full pre-drop point list
/// (including rolling-median lead-in) so the first displayed point still has
/// a full window.
pub(crate) fn add_typical_days(
    points: &mut [SeriesPoint],
    group: GroupName,
    unit: MetricUnit,
) {
    if group != GroupName::Day {
        return;
    }
    if !matches!(unit, MetricUnit::Count | MetricUnit::Minutes) {
        return;
    }
    let by_date: BTreeMap<NaiveDate, i64> = points
        .iter()
        .filter(|point| is_usable_value(point))
        .filter_map(|point| Some((point.date, point.value?)))
        .collect();
    let computed: Vec<Option<TypicalDay>> = points
        .iter()
        .map(|point| {
            let start =
                point.date.checked_sub_days(Days::new(DAY_WINDOW as u64))?;
            let end = point.date.checked_sub_days(Days::new(1))?;
            let mut usable = Vec::new();
            let mut day = start;
            loop {
                if let Some(value) = by_date.get(&day) {
                    usable.push(*value);
                }
                if day == end {
                    break;
                }
                let Some(next) = day.succ_opt() else {
                    break;
                };
                day = next;
            }
            Some(build_typical(point, start, end, &usable, unit))
        })
        .collect();
    for (point, tip) in points.iter_mut().zip(computed) {
        point.typical = tip;
    }
}

fn build_typical(
    point: &SeriesPoint,
    start: NaiveDate,
    end: NaiveDate,
    usable: &[i64],
    unit: MetricUnit,
) -> TypicalDay {
    let days = usable.len() as u32;
    let span = DateSpan { start, end };
    if usable.len() < DAY_WINDOW_MIN {
        let verb = if usable.len() == 1 { "has" } else { "have" };
        return TypicalDay {
            start,
            end,
            days,
            value: None,
            display: "–".to_string(),
            relation: None,
            words: "no typical day yet".to_string(),
            detail: format!(
                "No typical day yet: {days} of the previous 7 days {verb} data (needs 4)."
            ),
        };
    }
    let value = median(usable).expect("ready window has a median");
    let display = format_value(Some(value), unit);
    let span_text = format_date_span(span);
    let history = format!("{span_text} ({days} of 7 days with data)");
    if point.provisional {
        return TypicalDay {
            start,
            end,
            days,
            value: Some(value),
            display: display.clone(),
            relation: None,
            words: format!("typical full day {display}"),
            detail: format!(
                "Typical full day {display}: median of {history}. Today is still in progress, so this is context, not a pace."
            ),
        };
    }
    let Some(current) = point.value else {
        return TypicalDay {
            start,
            end,
            days,
            value: Some(value),
            display: display.clone(),
            relation: None,
            words: format!("typical day {display}"),
            detail: format!("Typical day {display}: median of {history}."),
        };
    };
    if current == value {
        return TypicalDay {
            start,
            end,
            days,
            value: Some(value),
            display: display.clone(),
            relation: Some(TypicalRelation::Equal),
            words: format!("the same as the typical day of {display}"),
            detail: format!(
                "Typical day {display}: median of {history}. This day matches it."
            ),
        };
    }
    let amount = format_amount(current.saturating_sub(value).abs(), unit);
    let (relation, direction) = if current > value {
        (TypicalRelation::Above, "above")
    } else {
        (TypicalRelation::Below, "below")
    };
    TypicalDay {
        start,
        end,
        days,
        value: Some(value),
        display: display.clone(),
        relation: Some(relation),
        words: format!("{amount} {direction} the typical day of {display}"),
        detail: format!(
            "Typical day {display}: median of {history}. This day is {amount} {direction}."
        ),
    }
}

fn format_amount(diff: i64, unit: MetricUnit) -> String {
    match unit {
        MetricUnit::Minutes => format_minutes(diff),
        _ => diff.to_string(),
    }
}

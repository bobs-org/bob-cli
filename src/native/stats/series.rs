use std::collections::BTreeMap;

use chrono::{Datelike, Days, NaiveDate, TimeDelta};

use super::range::RangeSpec;
use super::registry::MetricUnit;
use super::report::{
    ChartBand, ChoreSummary, Delta, GroupName, Headline, MetricSummary,
    PartSummary, PriorHeadline, PriorRange, RangeReport, SeriesPoint,
    ValueDisplay,
};
use super::value::{DailyValue, DataQuality};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DateSpan {
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
}

impl DateSpan {
    fn days(self) -> Option<u32> {
        let days = (self.end - self.start).num_days().checked_add(1)?;
        u32::try_from(days).ok()
    }
}

/// The normalized window shared by range metadata, series points, and summaries.
/// The displayed today point is kept separate from the complete-day span so no
/// summary can accidentally include provisional data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedRange {
    pub(crate) report: RangeReport,
    pub(crate) complete: Option<DateSpan>,
    pub(crate) today: NaiveDate,
    pub(crate) includes_today: bool,
    pub(crate) prior: Option<DateSpan>,
    pub(crate) group: GroupName,
    pub(crate) comparison_label: String,
}

impl ResolvedRange {
    fn display_span(&self) -> Option<DateSpan> {
        match (self.complete, self.includes_today) {
            (Some(span), true) => Some(DateSpan {
                start: span.start,
                end: self.today,
            }),
            (Some(span), false) => Some(span),
            (None, true) => Some(DateSpan {
                start: self.today,
                end: self.today,
            }),
            (None, false) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetricSeries {
    pub(crate) summary: MetricSummary,
    pub(crate) chart: ChartBand,
    pub(crate) points: Vec<SeriesPoint>,
    pub(crate) prior_points: Vec<SeriesPoint>,
    pub(crate) daily_points: Vec<SeriesPoint>,
    pub(crate) parts_summary: BTreeMap<String, Vec<PartSummary>>,
    pub(crate) chores: Vec<ChoreSummary>,
}

/// Resolve a user/config range using a fixed local date. For an all-range,
/// all_since is the earliest effective since date among selected metrics (after
/// applying the global stats.since floor).
pub(crate) fn resolve_range(
    spec: &RangeSpec,
    today: NaiveDate,
    offset: u32,
    grouping: super::request::Grouping,
    compare: bool,
    all_since: Option<NaiveDate>,
) -> Result<ResolvedRange, String> {
    let (complete, includes_today, preset, comparison_days) = match spec {
        RangeSpec::Days(count) => {
            let days = *count;
            let complete = preset_span(today, days, offset)?;
            (Some(complete), offset == 0, Some(spec.label()), days)
        }
        RangeSpec::Weeks(count) => {
            let days = count
                .checked_mul(7)
                .ok_or_else(|| "stats range is too large".to_string())?;
            let complete = preset_span(today, days, offset)?;
            (Some(complete), offset == 0, Some(spec.label()), days)
        }
        RangeSpec::Months(count) => {
            // Calendar months are converted to a whole number of weeks so
            // weekday counts remain balanced in every window.
            let weeks = count
                .checked_mul(52)
                .and_then(|weeks| weeks.checked_add(11))
                .map(|weeks| weeks / 12)
                .ok_or_else(|| "stats range is too large".to_string())?;
            let days = weeks
                .checked_mul(7)
                .ok_or_else(|| "stats range is too large".to_string())?;
            let complete = preset_span(today, days, offset)?;
            (Some(complete), offset == 0, Some(spec.label()), days)
        }
        RangeSpec::Years(count) => {
            let days = count
                .checked_mul(52)
                .and_then(|weeks| weeks.checked_mul(7))
                .ok_or_else(|| "stats range is too large".to_string())?;
            let complete = preset_span(today, days, offset)?;
            (Some(complete), offset == 0, Some(spec.label()), days)
        }
        RangeSpec::All => {
            if offset != 0 {
                return Err("stats.range=all cannot be shifted with an offset"
                    .to_string());
            }
            let yesterday = checked_add_days(today, -1)?;
            let complete = all_since.and_then(|since| {
                let start = since.min(today);
                (start <= yesterday).then_some(DateSpan {
                    start,
                    end: yesterday,
                })
            });
            let days = complete
                .map(|span| {
                    span.days()
                        .ok_or_else(|| "stats range is too large".to_string())
                })
                .transpose()?
                .unwrap_or(0);
            (complete, true, Some("all".to_string()), days)
        }
        RangeSpec::Explicit { start, end } => {
            if *start > today {
                return Err("stats.range cannot start after today".to_string());
            }
            let clamped_end = (*end).min(today);
            let includes_today = clamped_end == today;
            let complete_end = if includes_today {
                checked_add_days(today, -1)?
            } else {
                clamped_end
            };
            let base = (*start <= complete_end).then_some(DateSpan {
                start: *start,
                end: complete_end,
            });
            let shifted = if offset == 0 {
                base
            } else {
                let Some(base) = base else {
                    return Err("cannot offset a range with no complete days"
                        .to_string());
                };
                let days = base
                    .days()
                    .ok_or_else(|| "stats range is too large".to_string())?;
                Some(shift_span(base, days, offset)?)
            };
            let shifted_includes_today = includes_today && offset == 0;
            let days = shifted.map_or(0, |span| span.days().unwrap_or(0));
            (shifted, shifted_includes_today, None, days)
        }
    };

    let complete = match complete {
        Some(span) if span.start <= span.end => Some(span),
        _ => None,
    };
    let days = complete.map_or(0, |span| {
        span.days().expect("resolved range day count was checked")
    });
    let group = match grouping {
        super::request::Grouping::Day => GroupName::Day,
        super::request::Grouping::Week => GroupName::Week,
        super::request::Grouping::Month => GroupName::Month,
        super::request::Grouping::Auto if days <= 91 => GroupName::Day,
        super::request::Grouping::Auto if days <= 371 => GroupName::Week,
        super::request::Grouping::Auto => GroupName::Month,
    };

    let (start, end) = match complete {
        Some(span) => (span.start, span.end),
        None if matches!(spec, RangeSpec::All) => {
            let yesterday = checked_add_days(today, -1)?;
            (yesterday, yesterday)
        }
        None => (today, today),
    };
    let prior = if compare && days > 0 {
        let prior_end = checked_add_days(start, -1)?;
        let prior_start = checked_add_days(prior_end, -(i64::from(days) - 1))?;
        Some(DateSpan {
            start: prior_start,
            end: prior_end,
        })
    } else {
        None
    };
    let prior_report = prior.map(|span| PriorRange {
        start: span.start,
        end: span.end,
        label: format_date_span(span),
    });
    let label_span = complete.or_else(|| {
        includes_today.then_some(DateSpan {
            start: today,
            end: today,
        })
    });
    let label =
        label_span.map_or_else(|| "No dates".to_string(), format_date_span);
    let description_days = if comparison_days > 0 {
        comparison_days
    } else {
        days
    };
    Ok(ResolvedRange {
        report: RangeReport {
            preset,
            offset,
            start,
            end,
            days,
            includes_today,
            group,
            label,
            prior: prior_report,
        },
        complete,
        today,
        includes_today,
        prior,
        group,
        comparison_label: comparison_description(spec, description_days),
    })
}

fn preset_span(
    today: NaiveDate,
    days: u32,
    offset: u32,
) -> Result<DateSpan, String> {
    let days = i64::from(days);
    let shift = days
        .checked_mul(i64::from(offset))
        .ok_or_else(|| "stats range is too large".to_string())?;
    let end_offset = 1_i64
        .checked_add(shift)
        .ok_or_else(|| "stats range is too large".to_string())?;
    let end = checked_add_days(today, -end_offset)?;
    let start = checked_add_days(end, -(days - 1))?;
    Ok(DateSpan { start, end })
}

fn shift_span(
    span: DateSpan,
    days: u32,
    offset: u32,
) -> Result<DateSpan, String> {
    let shift = i64::from(days)
        .checked_mul(i64::from(offset))
        .ok_or_else(|| "stats range is too large".to_string())?;
    Ok(DateSpan {
        start: checked_add_days(span.start, -shift)?,
        end: checked_add_days(span.end, -shift)?,
    })
}

fn checked_add_days(date: NaiveDate, days: i64) -> Result<NaiveDate, String> {
    let delta = TimeDelta::try_days(days)
        .ok_or_else(|| "stats range is too large".to_string())?;
    date.checked_add_signed(delta)
        .ok_or_else(|| "stats range exceeds the supported calendar".to_string())
}

/// Earliest date whose values feed the first displayed point's rolling
/// median: 6 days, 3 ISO weeks, or 2 calendar months before `start`.
pub(crate) fn lead_in_start(
    start: NaiveDate,
    group: GroupName,
) -> Option<NaiveDate> {
    match group {
        GroupName::Day => start.checked_sub_days(Days::new(6)),
        GroupName::Week => {
            let week_start = bucket_start(start, GroupName::Week);
            week_start.checked_sub_days(Days::new(21))
        }
        GroupName::Month => shift_calendar_months(start, -2).and_then(|date| {
            NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
        }),
    }
}

fn shift_calendar_months(date: NaiveDate, months: i32) -> Option<NaiveDate> {
    let month0 = i32::try_from(date.month0()).ok()?.checked_add(months)?;
    let year = date.year().checked_add(month0.div_euclid(12))?;
    let month = u32::try_from(month0.rem_euclid(12)).ok()? + 1;
    NaiveDate::from_ymd_opt(year, month, date.day()).or_else(|| {
        NaiveDate::from_ymd_opt(year, month, 1).and_then(|first| {
            let next = shift_calendar_months(first, 1)?;
            next.pred_opt()
        })
    })
}

fn format_date_span(span: DateSpan) -> String {
    let start = span.start;
    let end = span.end;
    if start == end {
        return start.format("%b %-d").to_string();
    }
    if start.year() == end.year() {
        format!("{} – {}", start.format("%b %-d"), end.format("%b %-d"))
    } else {
        format!(
            "{} – {}",
            start.format("%b %-d, %Y"),
            end.format("%b %-d, %Y")
        )
    }
}

fn comparison_description(spec: &RangeSpec, days: u32) -> String {
    match spec {
        RangeSpec::All => "all history".to_string(),
        RangeSpec::Explicit { .. } => "selected period".to_string(),
        _ if days == 1 => "day".to_string(),
        _ if days.is_multiple_of(7) && days / 7 == 1 => "week".to_string(),
        _ if days.is_multiple_of(7) => format!("{} weeks", days / 7),
        _ => format!("{days} days"),
    }
}

/// Build all display points and complete-day statistics for one metric from
/// evaluator output. Input values may include both the current window and its
/// prior window; dates outside those windows are ignored.
pub(crate) fn build_metric_series(
    values: &[DailyValue],
    metric_id: &str,
    unit: MetricUnit,
    range: &ResolvedRange,
    is_checklist: bool,
) -> MetricSeries {
    let current_span = range.display_span();
    let current_daily = normalize(
        values,
        span_with_lead_in(current_span, range.group),
        range.today,
        range.includes_today,
    );
    let prior_daily = normalize(
        values,
        span_with_lead_in(range.prior, range.group),
        range.today,
        false,
    );
    let points = make_points(
        &current_daily,
        unit,
        range.group,
        range.complete,
        current_span.map(|span| span.start),
    );
    let prior_points = make_points(
        &prior_daily,
        unit,
        range.group,
        range.prior,
        range.prior.map(|span| span.start),
    );
    let complete_daily: Vec<_> = current_daily
        .iter()
        .filter(|value| !value.provisional)
        .filter(|value| {
            range.complete.is_some_and(|span| {
                span.start <= value.date && value.date <= span.end
            })
        })
        .cloned()
        .collect();
    let prior_for_summary: Vec<_> = prior_daily
        .iter()
        .filter(|value| {
            range.prior.is_some_and(|span| {
                span.start <= value.date && value.date <= span.end
            })
        })
        .cloned()
        .collect();
    let prior_summary = range
        .prior
        .map(|_| summarize(&prior_for_summary, unit, metric_id == "net"));
    let summary = summarize_with_prior(
        &complete_daily,
        unit,
        prior_summary.as_ref(),
        &range.comparison_label,
        metric_id == "net",
    );
    let parts_summary = summarize_parts(&complete_daily, unit);
    let daily_points = display_daily_points(&current_daily, unit, range);
    let chart = chart_band(&points, range.group);
    let chores = if is_checklist {
        summarize_chores(&complete_daily)
    } else {
        Vec::new()
    };
    MetricSeries {
        summary,
        chart,
        points,
        prior_points,
        daily_points,
        parts_summary,
        chores,
    }
}

fn normalize(
    values: &[DailyValue],
    span: Option<DateSpan>,
    today: NaiveDate,
    includes_today: bool,
) -> Vec<DailyValue> {
    let Some(span) = span else {
        return Vec::new();
    };
    let by_date: BTreeMap<_, _> = values
        .iter()
        .filter(|value| span.start <= value.date && value.date <= span.end)
        .map(|value| (value.date, value))
        .collect();
    let mut normalized = Vec::new();
    let mut date = span.start;
    loop {
        let provisional = includes_today && date == today;
        if let Some(value) = by_date.get(&date) {
            let mut value = (*value).clone();
            value.provisional = provisional;
            normalized.push(value);
        } else {
            normalized.push(DailyValue {
                date,
                value: None,
                num: None,
                den: None,
                quality: DataQuality::Missing,
                reason: Some("no data".to_string()),
                provisional,
                parts: BTreeMap::new(),
                apportioned: BTreeMap::new(),
                checklist_state: None,
            });
        }
        if date == span.end {
            break;
        }
        let Some(next) = date.succ_opt() else {
            break;
        };
        date = next;
    }
    normalized
}

fn span_with_lead_in(
    span: Option<DateSpan>,
    group: GroupName,
) -> Option<DateSpan> {
    let span = span?;
    let start = lead_in_start(span.start, group).unwrap_or(span.start);
    Some(DateSpan {
        start: start.min(span.start),
        end: span.end,
    })
}

fn make_points(
    values: &[DailyValue],
    unit: MetricUnit,
    group: GroupName,
    clip: Option<DateSpan>,
    drop_before: Option<NaiveDate>,
) -> Vec<SeriesPoint> {
    let mut points = match group {
        GroupName::Day => values
            .iter()
            .map(|value| daily_point(value, unit))
            .collect(),
        GroupName::Week | GroupName::Month => {
            let mut points = grouped_points(values, unit, group, clip);
            if let Some(today) = values.iter().find(|value| value.provisional) {
                points.push(daily_point(today, unit));
            }
            points
        }
    };
    add_rolling_medians(&mut points, group);
    if let Some(start) = drop_before {
        points.retain(|point| point.end >= start);
    }
    points
}

fn daily_point(value: &DailyValue, unit: MetricUnit) -> SeriesPoint {
    let display = if value.provisional
        && unit == MetricUnit::Bool
        && value.value == Some(0)
    {
        "not yet".to_string()
    } else {
        format_value(value.value, unit)
    };
    SeriesPoint {
        date: value.date,
        end: value.date,
        value: value.value,
        display,
        quality: value.quality,
        reason: value.reason.clone(),
        provisional: value.provisional,
        rolling_median: None,
        num: value.num,
        den: value.den,
        partial: None,
        parts: value.parts.clone(),
        parts_display: format_parts(&value.parts, unit),
        apportioned: value.apportioned.clone(),
        checklist_state: value.checklist_state,
    }
}

fn grouped_points(
    values: &[DailyValue],
    unit: MetricUnit,
    group: GroupName,
    clip: Option<DateSpan>,
) -> Vec<SeriesPoint> {
    // The provisional today point stays out of grouped buckets so it can
    // never contaminate a completed-bucket aggregate. Day grouping still
    // reports today as its own point; summaries already exclude it.
    let complete: Vec<&DailyValue> =
        values.iter().filter(|value| !value.provisional).collect();
    let Some(first) = complete.first() else {
        return Vec::new();
    };
    let Some(last) = complete.last() else {
        return Vec::new();
    };
    let data_start = first.date;
    let data_end = last.date;
    let mut buckets: BTreeMap<NaiveDate, Vec<&DailyValue>> = BTreeMap::new();
    for value in complete {
        buckets
            .entry(bucket_start(value.date, group))
            .or_default()
            .push(value);
    }
    buckets
        .into_iter()
        .map(|(natural_start, days)| {
            let natural_end = bucket_end(natural_start, group);
            let displayed = clip.filter(|span| {
                natural_end >= span.start && natural_start <= span.end
            });
            let start = displayed
                .map_or(natural_start.max(data_start), |span| {
                    natural_start.max(span.start)
                });
            let end = displayed.map_or(natural_end.min(data_end), |span| {
                natural_end.min(span.end)
            });
            let partial = start != natural_start || end != natural_end;
            let days: Vec<&DailyValue> = days
                .into_iter()
                .filter(|day| day.date >= start && day.date <= end)
                .collect();
            aggregate_bucket(start, end, &days, unit, partial)
        })
        .collect()
}

fn bucket_start(date: NaiveDate, group: GroupName) -> NaiveDate {
    match group {
        GroupName::Day => date,
        GroupName::Week => {
            let offset = u64::from(date.weekday().num_days_from_monday());
            date.checked_sub_days(Days::new(offset)).unwrap_or(date)
        }
        GroupName::Month => {
            NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
                .unwrap_or(date)
        }
    }
}

fn bucket_end(start: NaiveDate, group: GroupName) -> NaiveDate {
    match group {
        GroupName::Day => start,
        GroupName::Week => {
            start.checked_add_days(Days::new(6)).unwrap_or(start)
        }
        GroupName::Month => shift_calendar_months(start, 1)
            .and_then(|date| date.pred_opt())
            .unwrap_or(start),
    }
}

fn aggregate_bucket(
    start: NaiveDate,
    end: NaiveDate,
    days: &[&DailyValue],
    unit: MetricUnit,
    partial_bucket: bool,
) -> SeriesPoint {
    let valid: Vec<_> = days.iter().filter(|day| day.value.is_some()).collect();
    let provisional = days.iter().any(|day| day.provisional);
    let mut parts = BTreeMap::<String, BTreeMap<String, i64>>::new();
    let mut apportioned = BTreeMap::<String, BTreeMap<String, i64>>::new();
    for day in days {
        for (dimension, entries) in &day.parts {
            let grouped = parts.entry(dimension.clone()).or_default();
            for (key, value) in entries {
                let total = grouped.entry(key.clone()).or_default();
                *total = total.saturating_add(*value);
            }
        }
        for (dimension, entries) in &day.apportioned {
            let grouped = apportioned.entry(dimension.clone()).or_default();
            for (key, value) in entries {
                let total = grouped.entry(key.clone()).or_default();
                *total = total.saturating_add(*value);
            }
        }
    }

    let (value, num, den, bool_total) = if valid.is_empty() {
        (None, None, None, None)
    } else if unit == MetricUnit::Ratio {
        let (num, den) = valid.iter().fold((0_i64, 0_i64), |acc, day| {
            match (day.num, day.den) {
                (Some(num), Some(den)) => {
                    (acc.0.saturating_add(num), acc.1.saturating_add(den))
                }
                _ => acc,
            }
        });
        let value = (den > 0).then(|| ratio_basis_points(num, den));
        (value, Some(num), Some(den), None)
    } else {
        let total = valid.iter().fold(0_i64, |sum, day| {
            sum.saturating_add(day.value.unwrap_or_default())
        });
        let bool_total = (unit == MetricUnit::Bool).then(|| {
            valid
                .iter()
                .filter(|day| day.value.unwrap_or_default() > 0)
                .count()
        });
        (Some(total), None, None, bool_total)
    };
    let (quality, reason) = aggregate_quality(days, valid.len());
    let display = if unit == MetricUnit::Bool {
        if valid.is_empty() {
            "–".to_string()
        } else {
            let met = bool_total.unwrap_or_default();
            format!("{met} of {}", valid.len())
        }
    } else {
        format_value(value, unit)
    };
    SeriesPoint {
        date: start,
        end,
        value,
        display,
        quality,
        reason,
        provisional,
        rolling_median: None,
        num,
        den,
        partial: partial_bucket.then_some(true),
        parts_display: format_parts(&parts, unit),
        parts,
        apportioned,
        checklist_state: None,
    }
}

fn aggregate_quality(
    days: &[&DailyValue],
    valid_count: usize,
) -> (DataQuality, Option<String>) {
    if days.is_empty() {
        return (DataQuality::Missing, Some("no data".to_string()));
    }
    if valid_count == 0 {
        if days
            .iter()
            .all(|day| day.quality == DataQuality::NotTracked)
        {
            let reason = days.iter().find_map(|day| day.reason.clone());
            return (DataQuality::NotTracked, reason);
        }
        let reason = days.iter().find_map(|day| day.reason.clone());
        return (
            DataQuality::Missing,
            reason.or_else(|| Some("no data".to_string())),
        );
    }
    let incomplete = days.len() > valid_count;
    let partial_input =
        days.iter().any(|day| day.quality == DataQuality::Partial);
    if incomplete || partial_input {
        let missing = days.iter().filter(|day| day.value.is_none()).count();
        let reason = if missing > 0 {
            Some(format!(
                "incomplete data on {missing} of {} days",
                days.len()
            ))
        } else {
            days.iter()
                .find(|day| day.quality == DataQuality::Partial)
                .and_then(|day| day.reason.clone())
                .or_else(|| Some("includes partial data".to_string()))
        };
        (DataQuality::Partial, reason)
    } else {
        (DataQuality::Ok, None)
    }
}

fn add_rolling_medians(points: &mut [SeriesPoint], group: GroupName) {
    let window: usize = match group {
        GroupName::Day => 7,
        GroupName::Week => 4,
        GroupName::Month => 3,
    };
    let minimum = window.div_ceil(2);
    for index in 0..points.len() {
        if points[index].provisional {
            points[index].rolling_median = None;
            continue;
        }
        let start = index.saturating_add(1).saturating_sub(window);
        let values: Vec<_> = points[start..=index]
            .iter()
            .filter(|point| !point.provisional)
            .filter_map(|point| point.value)
            .collect();
        points[index].rolling_median = if values.len() >= minimum {
            median(&values)
        } else {
            None
        };
    }
}

fn summarize_with_prior(
    values: &[DailyValue],
    unit: MetricUnit,
    prior: Option<&MetricSummary>,
    comparison_label: &str,
    is_net: bool,
) -> MetricSummary {
    let mut current = summarize(values, unit, is_net);
    if let Some(prior) = prior {
        current.prior = Some(PriorHeadline {
            headline: prior.headline.clone(),
        });
        current.delta =
            Some(make_delta(&current, prior, unit, comparison_label, is_net));
    }
    current
}

fn summarize(
    values: &[DailyValue],
    unit: MetricUnit,
    is_net: bool,
) -> MetricSummary {
    let values: Vec<_> = values
        .iter()
        .filter(|day| !day.provisional)
        .filter_map(|day| day.value.map(|value| (day, value)))
        .collect();
    let numbers: Vec<_> = values.iter().map(|(_, value)| *value).collect();
    let days_with_data = u32::try_from(numbers.len()).unwrap_or(u32::MAX);
    let days_met = (unit == MetricUnit::Bool).then(|| {
        u32::try_from(numbers.iter().filter(|value| **value > 0).count())
            .unwrap_or(u32::MAX)
    });

    let headline = if is_net {
        if numbers.is_empty() {
            Headline {
                value: None,
                display: "–".to_string(),
                suffix: None,
                stat: None,
            }
        } else {
            let total = numbers
                .iter()
                .fold(0_i64, |sum, value| sum.saturating_add(*value));
            Headline {
                value: Some(total),
                display: format_value(Some(total), unit),
                suffix: Some("total".to_string()),
                stat: Some("total".to_string()),
            }
        }
    } else {
        match unit {
            MetricUnit::Bool if days_with_data == 0 => Headline {
                value: None,
                display: "–".to_string(),
                suffix: None,
                stat: Some("days".to_string()),
            },
            MetricUnit::Bool => {
                let met = days_met.unwrap_or_default();
                Headline {
                    value: Some(i64::from(met)),
                    display: format!("{met} of {days_with_data} days"),
                    suffix: None,
                    stat: Some("days".to_string()),
                }
            }
            MetricUnit::Ratio => {
                let (num, den) = values.iter().fold(
                    (0_i64, 0_i64),
                    |acc, (day, _)| match (day.num, day.den) {
                        (Some(num), Some(den)) => (
                            acc.0.saturating_add(num),
                            acc.1.saturating_add(den),
                        ),
                        _ => acc,
                    },
                );
                let value = (den > 0).then(|| ratio_basis_points(num, den));
                Headline {
                    value,
                    display: format_value(value, unit),
                    suffix: None,
                    stat: Some("pooled".to_string()),
                }
            }
            _ => {
                let value = median(&numbers);
                Headline {
                    value,
                    display: format_value(value, unit),
                    suffix: value.map(|_| "/day".to_string()),
                    stat: value.map(|_| "median".to_string()),
                }
            }
        }
    };
    let total = if unit == MetricUnit::Ratio {
        None
    } else {
        let total = numbers
            .iter()
            .fold(0_i64, |sum, value| sum.saturating_add(*value));
        (!numbers.is_empty()).then(|| ValueDisplay {
            value: Some(total),
            display: if unit == MetricUnit::Bool {
                format!("{} days", days_met.unwrap_or_default())
            } else {
                format_value(Some(total), unit)
            },
        })
    };
    MetricSummary {
        headline,
        total,
        mean: mean(&numbers),
        p25: percentile_quarters(&numbers, 1),
        p75: percentile_quarters(&numbers, 3),
        days_with_data,
        days_met,
        prior: None,
        delta: None,
    }
}

fn make_delta(
    current: &MetricSummary,
    prior: &MetricSummary,
    unit: MetricUnit,
    comparison_label: &str,
    is_net: bool,
) -> Delta {
    if is_net {
        if current.days_with_data == 0 {
            return empty_delta();
        }
        let total = current
            .total
            .as_ref()
            .and_then(|total| total.value)
            .unwrap_or(0);
        let (display, words, sign) = if total < 0 {
            ("shrinking", "net flow is shrinking", -1)
        } else if total > 0 {
            ("growing", "net flow is growing", 1)
        } else {
            ("flat", "net flow is flat", 0)
        };
        return Delta {
            display: display.to_string(),
            words: words.to_string(),
            sign,
        };
    }

    if current.days_with_data == 0 || prior.days_with_data == 0 {
        return empty_delta();
    }
    let (current_value, prior_value) = if unit == MetricUnit::Bool {
        (
            current.days_met.map(i64::from),
            prior.days_met.map(i64::from),
        )
    } else {
        (current.headline.value, prior.headline.value)
    };
    let Some(current_value) = current_value else {
        return empty_delta();
    };
    let Some(prior_value) = prior_value else {
        return empty_delta();
    };
    let difference = current_value.saturating_sub(prior_value);
    if difference == 0 {
        return same_delta(comparison_label);
    }
    if unit == MetricUnit::Ratio {
        let amount = format_points(difference.unsigned_abs() as i64);
        if amount == "0" {
            return same_delta(comparison_label);
        }
        let (display, direction, sign) = if difference > 0 {
            (format!("▲ {amount} pts"), "up", 1)
        } else {
            (format!("▼ {amount} pts"), "down", -1)
        };
        return Delta {
            display,
            words: format!("{direction} {amount} percentage points vs the prior {comparison_label}"),
            sign,
        };
    }
    if unit == MetricUnit::Bool {
        let (display, direction, sign) = if difference > 0 {
            (format!("▲ {difference} days"), "up", 1)
        } else {
            (format!("▼ {} days", difference.unsigned_abs()), "down", -1)
        };
        return Delta {
            display,
            words: format!(
                "{direction} {} days vs the prior {comparison_label}",
                difference.unsigned_abs()
            ),
            sign,
        };
    }
    if prior_value == 0 {
        let sign = if current_value < 0 { -1 } else { 1 };
        return new_delta(comparison_label, sign);
    }
    if current_value == 0 {
        return Delta {
            display: "▼ 100%".to_string(),
            words: format!("down 100% vs the prior {comparison_label}"),
            sign: -1,
        };
    }
    if prior_value < 0 || current_value.signum() != prior_value.signum() {
        let amount = difference.unsigned_abs();
        let (display, direction, sign) = if difference > 0 {
            (format!("▲ {amount}"), "up", 1)
        } else {
            (format!("▼ {amount}"), "down", -1)
        };
        return Delta {
            display,
            words: format!(
                "{direction} {amount} vs the prior {comparison_label}"
            ),
            sign,
        };
    }
    let abs_current = i128::from(current_value).abs();
    let denominator = i128::from(prior_value).abs();
    if abs_current >= denominator.saturating_mul(2)
        || abs_current.saturating_mul(2) <= denominator
    {
        let factor_tenths =
            round_div(abs_current.saturating_mul(10), denominator) as i64;
        let factor = format_factor(factor_tenths);
        let (display, direction, sign) = if difference > 0 {
            (format!("▲ {factor}×"), "up", 1)
        } else {
            (format!("▼ {factor}×"), "down", -1)
        };
        return Delta {
            display,
            words: format!(
                "{direction} {factor} times vs the prior {comparison_label}"
            ),
            sign,
        };
    }
    let numerator = i128::from(difference).abs().saturating_mul(100);
    if numerator <= denominator {
        return same_delta(comparison_label);
    }
    let percent = round_div(numerator, denominator) as i64;
    let amount = format!("{percent}%");
    let (display, direction, sign) = if difference > 0 {
        (format!("▲ {amount}"), "up", 1)
    } else {
        (format!("▼ {amount}"), "down", -1)
    };
    Delta {
        display,
        words: format!("{direction} {amount} vs the prior {comparison_label}"),
        sign,
    }
}

fn empty_delta() -> Delta {
    Delta {
        display: "–".to_string(),
        words: "no comparable data".to_string(),
        sign: 0,
    }
}

fn same_delta(comparison_label: &str) -> Delta {
    Delta {
        display: "≈ same".to_string(),
        words: format!("about the same as the prior {comparison_label}"),
        sign: 0,
    }
}

fn new_delta(comparison_label: &str, sign: i8) -> Delta {
    Delta {
        display: "new".to_string(),
        words: format!("new vs the prior {comparison_label}"),
        sign,
    }
}

fn summarize_parts(
    values: &[DailyValue],
    unit: MetricUnit,
) -> BTreeMap<String, Vec<PartSummary>> {
    let mut totals = BTreeMap::<String, BTreeMap<String, i64>>::new();
    let mut apportioned_totals =
        BTreeMap::<String, BTreeMap<String, i64>>::new();
    for day in values.iter().filter(|day| !day.provisional) {
        for (dimension, entries) in &day.parts {
            let dimension_totals = totals.entry(dimension.clone()).or_default();
            for (key, value) in entries {
                let total = dimension_totals.entry(key.clone()).or_default();
                *total = total.saturating_add(*value);
            }
        }
        for (dimension, entries) in &day.apportioned {
            let dimension_totals =
                apportioned_totals.entry(dimension.clone()).or_default();
            for (key, value) in entries {
                let total = dimension_totals.entry(key.clone()).or_default();
                *total = total.saturating_add(*value);
            }
        }
    }
    totals
        .into_iter()
        .filter_map(|(dimension, entries)| {
            let all_total = entries.values().fold(0_i64, |sum, value| {
                sum.saturating_add(value.saturating_abs())
            });
            if entries.is_empty() {
                return None;
            }
            let mut ranked: Vec<_> = entries.into_iter().collect();
            ranked.sort_by(
                |(left_key, left_value), (right_key, right_value)| {
                    right_value
                        .saturating_abs()
                        .cmp(&left_value.saturating_abs())
                        .then_with(|| left_key.cmp(right_key))
                },
            );
            if dimension == "note" || dimension == "theme" {
                let overflow = ranked.split_off(ranked.len().min(5));
                if !overflow.is_empty() {
                    let other =
                        overflow.iter().fold(0_i64, |sum, (_, value)| {
                            sum.saturating_add(*value)
                        });
                    if let Some(existing) =
                        ranked.iter_mut().find(|(key, _)| key == "other")
                    {
                        existing.1 = existing.1.saturating_add(other);
                    } else {
                        ranked.push(("other".to_string(), other));
                    }
                }
            }
            let dimension_apportioned = apportioned_totals
                .get(&dimension)
                .cloned()
                .unwrap_or_default();
            Some((
                dimension,
                ranked
                    .into_iter()
                    .map(|(key, value)| {
                        let share = if all_total == 0 {
                            0
                        } else {
                            round_div(
                                i128::from(value.saturating_abs()) * 100,
                                i128::from(all_total),
                            ) as i64
                        };
                        let apportioned = dimension_apportioned
                            .get(&key)
                            .copied()
                            .filter(|minutes| *minutes > 0);
                        let minutes_mode = unit == MetricUnit::Minutes;
                        PartSummary {
                            key,
                            value,
                            display: if minutes_mode {
                                format_value(Some(value), unit)
                            } else {
                                value.to_string()
                            },
                            share: format!("{share}%"),
                            label: None,
                            apportioned: minutes_mode
                                .then_some(())
                                .and(apportioned),
                            apportioned_display: minutes_mode
                                .then_some(())
                                .and(apportioned)
                                .map(|minutes| {
                                    format_value(Some(minutes), unit)
                                }),
                        }
                    })
                    .collect(),
            ))
        })
        .collect()
}

fn display_daily_points(
    values: &[DailyValue],
    unit: MetricUnit,
    range: &ResolvedRange,
) -> Vec<SeriesPoint> {
    match range.group {
        GroupName::Day => Vec::new(),
        GroupName::Week | GroupName::Month => values
            .iter()
            .filter(|value| in_display_window(value.date, range))
            .map(|value| daily_point(value, unit))
            .collect(),
    }
}

fn in_display_window(date: NaiveDate, range: &ResolvedRange) -> bool {
    if range.includes_today && date == range.today {
        return true;
    }
    range
        .complete
        .is_some_and(|span| span.start <= date && date <= span.end)
}

fn chart_band(points: &[SeriesPoint], group: GroupName) -> ChartBand {
    let numbers: Vec<i64> = points
        .iter()
        .filter(|point| !point.provisional)
        .filter_map(|point| point.value)
        .collect();
    ChartBand {
        group,
        p25: percentile_quarters(&numbers, 1),
        p75: percentile_quarters(&numbers, 3),
    }
}

fn summarize_chores(values: &[DailyValue]) -> Vec<ChoreSummary> {
    let mut roster = BTreeMap::<String, (u32, u32)>::new();
    for day in values {
        if let Some(met) = day.parts.get("met") {
            for key in met.keys() {
                let entry = roster.entry(key.clone()).or_default();
                entry.0 = entry.0.saturating_add(1);
                entry.1 = entry.1.saturating_add(1);
            }
        }
        if let Some(missed) = day.parts.get("missed") {
            for key in missed.keys() {
                let entry = roster.entry(key.clone()).or_default();
                entry.1 = entry.1.saturating_add(1);
            }
        }
    }
    roster
        .into_iter()
        .map(|(key, (met, due))| ChoreSummary {
            key,
            met,
            due,
            display: format!("{met} of {due} days"),
        })
        .collect()
}

fn format_parts(
    parts: &BTreeMap<String, BTreeMap<String, i64>>,
    unit: MetricUnit,
) -> BTreeMap<String, BTreeMap<String, String>> {
    parts
        .iter()
        .map(|(dimension, entries)| {
            (
                dimension.clone(),
                entries
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), format_part_display(*value, unit))
                    })
                    .collect(),
            )
        })
        .collect()
}

fn format_part_display(value: i64, unit: MetricUnit) -> String {
    if unit == MetricUnit::Minutes {
        format_value(Some(value), unit)
    } else {
        value.to_string()
    }
}

fn format_value(value: Option<i64>, unit: MetricUnit) -> String {
    let Some(value) = value else {
        return "–".to_string();
    };
    match unit {
        MetricUnit::Count => value.to_string(),
        MetricUnit::Signed => {
            if value > 0 {
                format!("+{value}")
            } else if value < 0 {
                format!("−{}", value.unsigned_abs())
            } else {
                "0".to_string()
            }
        }
        MetricUnit::Minutes => format_minutes(value),
        MetricUnit::Bool => {
            if value > 0 {
                "met".to_string()
            } else {
                "missed".to_string()
            }
        }
        MetricUnit::Ratio => format!("{}%", round_div(i128::from(value), 100)),
    }
}

pub(crate) fn format_minutes(value: i64) -> String {
    let sign = if value < 0 { "−" } else { "" };
    let minutes = value.unsigned_abs();
    if minutes < 60 {
        format!("{sign}{minutes}m")
    } else {
        format!("{sign}{}h {:02}m", minutes / 60, minutes % 60)
    }
}

fn format_points(value: i64) -> String {
    let tenths = round_div(i128::from(value), 10) as i64;
    if tenths % 10 == 0 {
        (tenths / 10).to_string()
    } else {
        format!("{}.{:01}", tenths / 10, tenths % 10)
    }
}

fn format_factor(tenths: i64) -> String {
    if tenths % 10 == 0 {
        (tenths / 10).to_string()
    } else {
        format!("{}.{:01}", tenths / 10, tenths % 10)
    }
}

fn ratio_basis_points(num: i64, den: i64) -> i64 {
    if den <= 0 {
        return 0;
    }
    round_div(i128::from(num).saturating_mul(10_000), i128::from(den)) as i64
}

fn mean(values: &[i64]) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    let sum = values
        .iter()
        .fold(0_i128, |sum, value| sum.saturating_add(i128::from(*value)));
    Some(round_div(sum, values.len() as i128) as i64)
}

fn median(values: &[i64]) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[middle])
    } else {
        Some(round_div(
            i128::from(sorted[middle - 1]) + i128::from(sorted[middle]),
            2,
        ) as i64)
    }
}

fn percentile_quarters(values: &[i64], quartile: u8) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let numerator = (sorted.len() - 1) * usize::from(quartile);
    let lower = numerator / 4;
    let remainder = numerator % 4;
    if remainder == 0 {
        return Some(sorted[lower]);
    }
    let upper = (lower + 1).min(sorted.len() - 1);
    let interpolated = i128::from(sorted[lower]) * (4 - remainder) as i128
        + i128::from(sorted[upper]) * remainder as i128;
    Some(round_div(interpolated, 4) as i64)
}

fn round_div(numerator: i128, denominator: i128) -> i128 {
    debug_assert!(denominator > 0);
    if numerator < 0 {
        -((-numerator + denominator / 2) / denominator)
    } else {
        (numerator + denominator / 2) / denominator
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, NaiveDate};

    use super::*;
    use crate::native::stats::request::Grouping;
    use crate::native::stats::value::ChecklistState;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn daily(date: NaiveDate, value: Option<i64>) -> DailyValue {
        DailyValue {
            date,
            value,
            num: None,
            den: None,
            quality: if value.is_some() {
                DataQuality::Ok
            } else {
                DataQuality::Missing
            },
            reason: value.is_none().then(|| "missing".to_string()),
            provisional: false,
            parts: BTreeMap::new(),
            apportioned: BTreeMap::new(),
            checklist_state: None,
        }
    }

    fn request_range(
        text: &str,
        today: NaiveDate,
        compare: bool,
    ) -> ResolvedRange {
        resolve_range(
            &RangeSpec::parse(text).unwrap(),
            today,
            0,
            Grouping::Auto,
            compare,
            None,
        )
        .unwrap()
    }

    #[test]
    fn presets_include_today_as_provisional_and_prior_is_equal_length() {
        let range = request_range("4w", date(2026, 10, 11), true);
        assert_eq!(range.report.start, date(2026, 9, 13));
        assert_eq!(range.report.end, date(2026, 10, 10));
        assert_eq!(range.report.days, 28);
        assert!(range.report.includes_today);
        assert_eq!(
            range.prior.unwrap(),
            DateSpan {
                start: date(2026, 8, 16),
                end: date(2026, 9, 12),
            }
        );
        assert_eq!(range.report.label, "Sep 13 – Oct 10");
    }

    #[test]
    fn offsets_shift_full_windows_and_never_include_today() {
        let range = resolve_range(
            &RangeSpec::Weeks(4),
            date(2026, 10, 11),
            2,
            Grouping::Auto,
            true,
            None,
        )
        .unwrap();
        assert_eq!(
            range.complete.unwrap(),
            DateSpan {
                start: date(2026, 7, 19),
                end: date(2026, 8, 15),
            }
        );
        assert!(!range.includes_today);
        assert_eq!(
            range.prior.unwrap(),
            DateSpan {
                start: date(2026, 6, 21),
                end: date(2026, 7, 18),
            }
        );
    }

    #[test]
    fn month_and_year_windows_snap_to_weeks_and_explicit_ranges_clamp_today() {
        let today = date(2026, 3, 1);
        let three_months = request_range("3m", today, false);
        assert_eq!(three_months.report.days, 91);
        let year = request_range("1y", today, false);
        assert_eq!(year.report.days, 364);
        let explicit = resolve_range(
            &RangeSpec::parse("2026-02-27..2026-03-12").unwrap(),
            today,
            0,
            Grouping::Auto,
            false,
            None,
        )
        .unwrap();
        assert_eq!(explicit.report.start, date(2026, 2, 27));
        assert_eq!(explicit.report.end, date(2026, 2, 28));
        assert_eq!(explicit.report.days, 2);
        assert!(explicit.includes_today);
        assert_eq!(explicit.report.preset, None);
    }

    #[test]
    fn all_uses_effective_since_and_rejects_offsets() {
        let range = resolve_range(
            &RangeSpec::All,
            date(2026, 10, 11),
            0,
            Grouping::Auto,
            false,
            Some(date(2026, 9, 1)),
        )
        .unwrap();
        assert_eq!(range.report.start, date(2026, 9, 1));
        assert_eq!(range.report.days, 40);
        assert!(resolve_range(
            &RangeSpec::All,
            date(2026, 10, 11),
            1,
            Grouping::Auto,
            false,
            None,
        )
        .is_err());
    }

    #[test]
    fn auto_grouping_uses_inclusive_thresholds_and_iso_weeks() {
        assert_eq!(
            request_range("13w", date(2026, 10, 11), false).group,
            GroupName::Day
        );
        assert_eq!(
            request_range("1y", date(2026, 10, 11), false).group,
            GroupName::Week
        );
        assert_eq!(
            request_range("54w", date(2026, 10, 11), false).group,
            GroupName::Month
        );
        let monday = date(2026, 12, 28);
        assert_eq!(bucket_start(monday, GroupName::Week), monday);
        assert_eq!(bucket_end(monday, GroupName::Week), date(2027, 1, 3));
    }

    #[test]
    fn grouped_ratios_pool_counts_and_boolean_days() {
        let start = date(2026, 9, 28); // Monday
        let mut ratio = Vec::new();
        let mut boolean = Vec::new();
        for (offset, (num, den, met)) in [
            (1, 2, 1),
            (1, 4, 0),
            (0, 0, 1),
            (3, 4, 1),
            (1, 1, 0),
            (1, 2, 1),
            (0, 1, 0),
        ]
        .into_iter()
        .enumerate()
        {
            let day = start + Duration::days(offset as i64);
            let mut r = daily(
                day,
                Some(if den > 0 {
                    ratio_basis_points(num, den)
                } else {
                    0
                }),
            );
            r.num = Some(num);
            r.den = Some(den);
            ratio.push(r);
            boolean.push(daily(day, Some(met)));
        }
        let ratio_point =
            grouped_points(&ratio, MetricUnit::Ratio, GroupName::Week, None)
                .remove(0);
        assert_eq!(ratio_point.num, Some(7));
        assert_eq!(ratio_point.den, Some(14));
        assert_eq!(ratio_point.value, Some(5_000));
        assert_eq!(ratio_point.display, "50%");
        let bool_point =
            grouped_points(&boolean, MetricUnit::Bool, GroupName::Week, None)
                .remove(0);
        assert_eq!(bool_point.value, Some(4));
        assert_eq!(bool_point.display, "4 of 7");
    }

    #[test]
    fn grouped_buckets_exclude_the_provisional_today_point() {
        let today = date(2026, 10, 11);
        let mut values = vec![
            daily(date(2026, 10, 9), Some(4)),
            daily(date(2026, 10, 10), Some(6)),
            daily(today, Some(100)),
        ];
        values.last_mut().unwrap().provisional = true;
        let points =
            grouped_points(&values, MetricUnit::Count, GroupName::Week, None);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].value, Some(10));
        assert_eq!(points[0].display, "10");
        assert_eq!(points[0].end, date(2026, 10, 10));
        assert!(!points[0].provisional);
        assert_eq!(points[0].partial, Some(true));
    }

    #[test]
    fn summaries_exclude_today_and_build_headline_stats_and_deltas() {
        let today = date(2026, 10, 11);
        let range = request_range("7d", today, true);
        let mut values = Vec::new();
        for (offset, value) in [1, 2, 3, 4, 5, 6, 100].into_iter().enumerate() {
            values.push(daily(
                date(2026, 10, 4) + Duration::days(offset as i64),
                Some(value),
            ));
        }
        for offset in 0..7 {
            let date = range.prior.unwrap().start + Duration::days(offset);
            values.push(daily(date, Some(2)));
        }
        values.push(daily(today, Some(500)));
        let built = build_metric_series(
            &values,
            "tasks",
            MetricUnit::Count,
            &range,
            false,
        );
        assert_eq!(built.summary.headline.value, Some(4));
        assert_eq!(built.summary.headline.display, "4");
        assert_eq!(built.summary.total.as_ref().unwrap().value, Some(121));
        assert_eq!(built.summary.days_with_data, 7);
        assert_eq!(
            built.summary.prior.as_ref().unwrap().headline.value,
            Some(2)
        );
        assert_eq!(built.summary.delta.as_ref().unwrap().display, "▲ 2×");
        assert_eq!(built.points.last().unwrap().value, Some(500));
        assert!(built.points.last().unwrap().provisional);
        assert_eq!(built.points.last().unwrap().rolling_median, None);
    }

    #[test]
    fn rolling_median_requires_half_window_and_excludes_provisional() {
        let start = date(2026, 1, 1);
        let mut values: Vec<_> = (0..7)
            .map(|offset| daily(start + Duration::days(offset), Some(offset)))
            .collect();
        values.push(daily(start + Duration::days(7), Some(100)));
        values.last_mut().unwrap().provisional = true;
        let points =
            make_points(&values, MetricUnit::Count, GroupName::Day, None, None);
        assert_eq!(points[2].rolling_median, None);
        assert_eq!(points[3].rolling_median, Some(2));
        assert_eq!(points[6].rolling_median, Some(3));
        assert_eq!(points[7].rolling_median, None);
    }

    #[test]
    fn formatting_covers_every_metric_unit() {
        assert_eq!(format_value(Some(0), MetricUnit::Minutes), "0m");
        assert_eq!(format_value(Some(59), MetricUnit::Minutes), "59m");
        assert_eq!(format_value(Some(245), MetricUnit::Minutes), "4h 05m");
        assert_eq!(format_value(Some(0), MetricUnit::Count), "0");
        assert_eq!(format_value(Some(-6), MetricUnit::Signed), "−6");
        assert_eq!(format_value(Some(9), MetricUnit::Signed), "+9");
        assert_eq!(format_value(Some(1), MetricUnit::Bool), "met");
        assert_eq!(format_value(Some(0), MetricUnit::Bool), "missed");
        assert_eq!(format_value(Some(8_250), MetricUnit::Ratio), "83%");
        assert_eq!(format_value(None, MetricUnit::Count), "–");
    }

    #[test]
    fn unmet_provisional_bool_point_displays_not_yet() {
        let mut today = daily(date(2026, 10, 24), Some(0));
        today.provisional = true;
        assert_eq!(daily_point(&today, MetricUnit::Bool).display, "not yet");
        let complete = daily(date(2026, 10, 23), Some(0));
        assert_eq!(daily_point(&complete, MetricUnit::Bool).display, "missed");
        let mut met = daily(date(2026, 10, 24), Some(1));
        met.provisional = true;
        assert_eq!(daily_point(&met, MetricUnit::Bool).display, "met");
        let mut null = daily(date(2026, 10, 24), None);
        null.provisional = true;
        assert_eq!(daily_point(&null, MetricUnit::Bool).display, "–");
        assert_eq!(daily_point(&today, MetricUnit::Count).display, "0");
    }

    #[test]
    fn boolean_and_ratio_summaries_use_days_and_pooled_parts() {
        let values = vec![
            DailyValue {
                value: Some(1),
                num: Some(1),
                den: Some(2),
                ..daily(date(2026, 1, 1), Some(1))
            },
            DailyValue {
                value: Some(0),
                num: Some(1),
                den: Some(4),
                ..daily(date(2026, 1, 2), Some(0))
            },
            DailyValue {
                value: Some(1),
                num: Some(1),
                den: Some(1),
                ..daily(date(2026, 1, 3), Some(1))
            },
        ];
        let bool_summary = summarize(&values, MetricUnit::Bool, false);
        assert_eq!(bool_summary.headline.display, "2 of 3 days");
        assert_eq!(bool_summary.days_met, Some(2));
        let ratio_summary = summarize(&values, MetricUnit::Ratio, false);
        assert_eq!(ratio_summary.headline.value, Some(4_286));
        assert_eq!(ratio_summary.headline.display, "43%");
        assert_eq!(ratio_summary.total, None);
    }

    #[test]
    fn parts_summary_keeps_top_five_and_rolls_the_rest_into_other() {
        let mut value = daily(date(2026, 1, 1), Some(21));
        value.parts.insert(
            "note".to_string(),
            [("a", 6), ("b", 5), ("c", 4), ("d", 3), ("e", 2), ("f", 1)]
                .into_iter()
                .map(|(key, count)| (key.to_string(), count))
                .collect(),
        );
        let summary = summarize_parts(&[value], MetricUnit::Count);
        let notes = &summary["note"];
        assert_eq!(notes.len(), 6);
        assert_eq!(notes[0].key, "a");
        assert_eq!(notes[4].key, "e");
        assert_eq!(notes[5].key, "other");
        assert_eq!(notes[5].value, 1);
        assert_eq!(notes[0].share, "29%");
    }

    #[test]
    fn parts_summary_keeps_every_area_and_project_key() {
        let mut value = daily(date(2026, 1, 1), Some(21));
        value.parts.insert(
            "area".to_string(),
            [("a", 6), ("b", 5), ("c", 4), ("d", 3), ("e", 2), ("f", 1)]
                .into_iter()
                .map(|(key, count)| (key.to_string(), count))
                .collect(),
        );
        let summary = summarize_parts(&[value], MetricUnit::Minutes);
        let areas = &summary["area"];
        assert_eq!(areas.len(), 6);
        assert!(areas.iter().all(|part| part.key != "other"));
        assert_eq!(areas[0].display, "6m");
    }

    #[test]
    fn signed_net_delta_uses_range_total_words() {
        let current = summarize(
            &[daily(date(2026, 1, 1), Some(-3))],
            MetricUnit::Signed,
            true,
        );
        let previous = summarize(
            &[daily(date(2025, 12, 1), Some(4))],
            MetricUnit::Signed,
            true,
        );
        let delta = make_delta(
            &current,
            &previous,
            MetricUnit::Signed,
            "4 weeks",
            true,
        );
        assert_eq!(delta.display, "shrinking");
        assert_eq!(delta.words, "net flow is shrinking");
    }

    #[test]
    fn net_headline_is_window_total_not_median() {
        // Screenshot repro: positive median (+3/+4/+5/-116 → median +4)
        // but negative total (-104) must read shrinking.
        let days = [3, 4, 5, -116];
        let values: Vec<_> = days
            .into_iter()
            .enumerate()
            .map(|(offset, value)| {
                daily(
                    date(2026, 1, 1) + Duration::days(offset as i64),
                    Some(value),
                )
            })
            .collect();
        let summary = summarize(&values, MetricUnit::Signed, true);
        assert_eq!(summary.headline.value, Some(-104));
        assert_eq!(summary.headline.display, "−104");
        assert_eq!(summary.headline.suffix.as_deref(), Some("total"));
        assert_eq!(summary.headline.stat.as_deref(), Some("total"));
        let prior = summarize(
            &[daily(date(2025, 12, 1), Some(1))],
            MetricUnit::Signed,
            true,
        );
        let delta =
            make_delta(&summary, &prior, MetricUnit::Signed, "4 weeks", true);
        assert_eq!(delta.display, "shrinking");
        assert_eq!(delta.sign, -1);
        assert_eq!(summary.headline.value.is_some_and(|v| v < 0), true);
        assert_eq!(delta.sign < 0, summary.headline.value.unwrap() < 0);

        // Mirror: negative median but positive total → growing.
        let mirror = [(-5), (-4), (-3), 116];
        let mirror_values: Vec<_> = mirror
            .into_iter()
            .enumerate()
            .map(|(offset, value)| {
                daily(
                    date(2026, 2, 1) + Duration::days(offset as i64),
                    Some(value),
                )
            })
            .collect();
        let mirror_summary =
            summarize(&mirror_values, MetricUnit::Signed, true);
        assert_eq!(mirror_summary.headline.value, Some(104));
        assert_eq!(mirror_summary.headline.display, "+104");
        assert_eq!(mirror_summary.headline.suffix.as_deref(), Some("total"));
        assert_eq!(mirror_summary.headline.stat.as_deref(), Some("total"));
        let mirror_delta = make_delta(
            &mirror_summary,
            &prior,
            MetricUnit::Signed,
            "4 weeks",
            true,
        );
        assert_eq!(mirror_delta.display, "growing");
        assert_eq!(mirror_delta.sign, 1);

        // Zero total → flat.
        let zero = [3, -3];
        let zero_values: Vec<_> = zero
            .into_iter()
            .enumerate()
            .map(|(offset, value)| {
                daily(
                    date(2026, 3, 1) + Duration::days(offset as i64),
                    Some(value),
                )
            })
            .collect();
        let zero_summary = summarize(&zero_values, MetricUnit::Signed, true);
        assert_eq!(zero_summary.headline.value, Some(0));
        assert_eq!(zero_summary.headline.display, "0");
        assert_eq!(zero_summary.headline.suffix.as_deref(), Some("total"));
        let zero_delta = make_delta(
            &zero_summary,
            &prior,
            MetricUnit::Signed,
            "4 weeks",
            true,
        );
        assert_eq!(zero_delta.display, "flat");
        assert_eq!(zero_delta.sign, 0);

        // Empty net window keeps the empty shape.
        let empty = summarize(&[], MetricUnit::Signed, true);
        assert_eq!(empty.headline.value, None);
        assert_eq!(empty.headline.display, "–");
        assert_eq!(empty.headline.suffix, None);
    }

    #[test]
    fn non_net_signed_and_count_metrics_keep_median_headline() {
        let values: Vec<_> = [3, 4, 5, -116]
            .into_iter()
            .enumerate()
            .map(|(offset, value)| {
                daily(
                    date(2026, 1, 1) + Duration::days(offset as i64),
                    Some(value),
                )
            })
            .collect();
        let signed = summarize(&values, MetricUnit::Signed, false);
        assert_eq!(signed.headline.value, Some(4));
        assert_eq!(signed.headline.display, "+4");
        assert_eq!(signed.headline.suffix.as_deref(), Some("/day"));
        assert_eq!(signed.headline.stat.as_deref(), Some("median"));
        let count_values: Vec<_> = [1, 2, 3, 100]
            .into_iter()
            .enumerate()
            .map(|(offset, value)| {
                daily(
                    date(2026, 1, 1) + Duration::days(offset as i64),
                    Some(value),
                )
            })
            .collect();
        let counted = summarize(&count_values, MetricUnit::Count, false);
        assert_eq!(counted.headline.suffix.as_deref(), Some("/day"));
        assert_eq!(counted.headline.stat.as_deref(), Some("median"));
    }

    #[test]
    fn net_prior_headline_uses_window_total() {
        let today = date(2026, 10, 11);
        let range = request_range("7d", today, true);
        let mut values = Vec::new();
        for (offset, value) in [3, 4, 5, -116, 1, 2, 3].into_iter().enumerate()
        {
            values.push(daily(
                date(2026, 10, 4) + Duration::days(offset as i64),
                Some(value),
            ));
        }
        for offset in 0..7 {
            let day = range.prior.unwrap().start + Duration::days(offset);
            values.push(daily(day, Some(10)));
        }
        let built = build_metric_series(
            &values,
            "net",
            MetricUnit::Signed,
            &range,
            false,
        );
        // Current total: 3+4+5-116+1+2+3 = -98.
        assert_eq!(built.summary.headline.value, Some(-98));
        assert_eq!(built.summary.headline.suffix.as_deref(), Some("total"));
        assert_eq!(built.summary.headline.stat.as_deref(), Some("total"));
        let prior = built.summary.prior.as_ref().expect("prior headline");
        assert_eq!(prior.headline.value, Some(70));
        assert_eq!(prior.headline.suffix.as_deref(), Some("total"));
        assert_eq!(prior.headline.stat.as_deref(), Some("total"));
        assert_eq!(built.summary.delta.as_ref().unwrap().display, "shrinking");
    }

    #[test]
    fn delta_uses_multipliers_for_large_changes_and_points_for_ratios() {
        let current = summarize(
            &[daily(date(2026, 1, 1), Some(5))],
            MetricUnit::Count,
            false,
        );
        let prior = summarize(
            &[daily(date(2025, 12, 1), Some(2))],
            MetricUnit::Count,
            false,
        );
        let delta =
            make_delta(&current, &prior, MetricUnit::Count, "4 weeks", false);
        assert_eq!(delta.display, "▲ 2.5×");

        let mut current_day = daily(date(2026, 1, 1), Some(6_000));
        current_day.num = Some(3);
        current_day.den = Some(5);
        let mut prior_day = daily(date(2025, 12, 1), Some(5_600));
        prior_day.num = Some(14);
        prior_day.den = Some(25);
        let ratio_current = summarize(&[current_day], MetricUnit::Ratio, false);
        let ratio_prior = summarize(&[prior_day], MetricUnit::Ratio, false);
        let delta = make_delta(
            &ratio_current,
            &ratio_prior,
            MetricUnit::Ratio,
            "4 weeks",
            false,
        );
        assert_eq!(delta.display, "▲ 4 pts");
        assert!(delta.words.contains("4 percentage points"));
    }

    #[test]
    fn weekly_and_monthly_buckets_flag_clipped_edges_and_partial_data() {
        let start = date(2026, 1, 2); // Friday: clipped start of ISO week
        let values = vec![
            daily(start, Some(2)),
            daily(start + Duration::days(1), None),
        ];
        let points =
            grouped_points(&values, MetricUnit::Count, GroupName::Week, None);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].date, start);
        assert_eq!(points[0].end, date(2026, 1, 3));
        assert_eq!(points[0].partial, Some(true));
        assert_eq!(points[0].quality, DataQuality::Partial);
        assert_eq!(points[0].value, Some(2));

        let month =
            grouped_points(&values, MetricUnit::Count, GroupName::Month, None);
        assert_eq!(month[0].date, start);
        assert_eq!(month[0].end, date(2026, 1, 3));
        assert_eq!(month[0].partial, Some(true));
    }

    #[test]
    fn explicit_offset_moves_a_full_complete_window_back() {
        let range = resolve_range(
            &RangeSpec::parse("2026-10-01..2026-10-11").unwrap(),
            date(2026, 10, 11),
            1,
            Grouping::Day,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            range.complete.unwrap(),
            DateSpan {
                start: date(2026, 9, 21),
                end: date(2026, 9, 30),
            }
        );
        assert!(!range.includes_today);
    }

    #[test]
    fn summary_quartiles_interpolate_and_round_half_away_from_zero() {
        assert_eq!(median(&[1, 2]), Some(2));
        assert_eq!(median(&[-2, -1]), Some(-2));
        assert_eq!(percentile_quarters(&[1, 2, 3, 4], 1), Some(2));
        assert_eq!(percentile_quarters(&[-4, -3, -2, -1], 3), Some(-2));
    }

    #[test]
    fn leap_day_is_a_valid_range_day_and_month_bucket() {
        let leap = date(2028, 2, 29);
        let range = resolve_range(
            &RangeSpec::parse("2028-02-28..2028-03-01").unwrap(),
            date(2028, 3, 2),
            0,
            Grouping::Month,
            false,
            None,
        )
        .unwrap();
        let complete = range.complete.expect("complete days");
        assert!(complete.start <= leap && leap <= complete.end);

        let values = vec![
            daily(date(2028, 2, 28), Some(1)),
            daily(leap, Some(2)),
            daily(date(2028, 3, 1), Some(4)),
        ];
        let points =
            grouped_points(&values, MetricUnit::Count, GroupName::Month, None);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].date, date(2028, 2, 28));
        assert_eq!(points[0].end, leap);
        assert_eq!(points[0].value, Some(3));
        assert_eq!(points[1].date, date(2028, 3, 1));
        assert_eq!(points[1].value, Some(4));
    }

    #[test]
    fn pooled_one_of_one_and_zero_of_nine_is_ten_percent() {
        let mut first = daily(date(2026, 1, 1), Some(10_000));
        first.num = Some(1);
        first.den = Some(1);
        let mut second = daily(date(2026, 1, 2), Some(0));
        second.num = Some(0);
        second.den = Some(9);
        let summary = summarize(&[first, second], MetricUnit::Ratio, false);
        assert_eq!(summary.headline.value, Some(1_000));
        assert_eq!(summary.headline.display, "10%");
    }

    #[test]
    fn auto_grouping_thresholds_at_ninety_two_three_seventy_one_and_two() {
        assert_eq!(
            request_range("92d", date(2026, 10, 11), false).group,
            GroupName::Week
        );
        assert_eq!(
            request_range("371d", date(2026, 10, 11), false).group,
            GroupName::Week
        );
        assert_eq!(
            request_range("372d", date(2026, 10, 11), false).group,
            GroupName::Month
        );
    }

    #[test]
    fn month_buckets_split_december_and_january() {
        let values = vec![
            daily(date(2026, 12, 31), Some(1)),
            daily(date(2027, 1, 1), Some(2)),
        ];
        let points =
            grouped_points(&values, MetricUnit::Count, GroupName::Month, None);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].date, date(2026, 12, 31));
        assert_eq!(points[0].end, date(2026, 12, 31));
        assert_eq!(points[1].date, date(2027, 1, 1));
    }

    #[test]
    fn iso_week_spanning_a_year_end_is_one_grouped_bucket() {
        let start = date(2026, 12, 28);
        let values: Vec<_> = (0..7)
            .map(|offset| daily(start + Duration::days(offset), Some(1)))
            .collect();
        let points =
            grouped_points(&values, MetricUnit::Count, GroupName::Week, None);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].date, start);
        assert_eq!(points[0].end, date(2027, 1, 3));
        assert_eq!(points[0].value, Some(7));
    }

    fn count_delta(current: Option<i64>, prior: Option<i64>) -> Delta {
        make_delta(
            &summarize(
                &[daily(date(2026, 1, 1), current)],
                MetricUnit::Count,
                false,
            ),
            &summarize(
                &[daily(date(2025, 12, 1), prior)],
                MetricUnit::Count,
                false,
            ),
            MetricUnit::Count,
            "4 weeks",
            false,
        )
    }

    #[test]
    fn every_delta_rule_has_a_hand_derived_case() {
        assert_eq!(count_delta(Some(100), Some(100)).display, "≈ same");
        assert_eq!(count_delta(Some(101), Some(100)).display, "≈ same");
        assert_eq!(count_delta(Some(110), Some(100)).display, "▲ 10%");
        assert_eq!(count_delta(Some(5), Some(2)).display, "▲ 2.5×");
        assert_eq!(count_delta(Some(1), Some(2)).display, "▼ 0.5×");
        assert_eq!(count_delta(Some(5), Some(0)).display, "new");
        assert_eq!(count_delta(Some(5), Some(0)).sign, 1);
        let negative_new = count_delta(Some(-3), Some(0));
        assert_eq!(negative_new.display, "new");
        assert_eq!(negative_new.sign, -1);
        assert_eq!(count_delta(Some(5), None).display, "–");
        assert_eq!(count_delta(Some(0), Some(5)).display, "▼ 100%");
        let crossing = count_delta(Some(5), Some(-3));
        assert_eq!(crossing.display, "▲ 8");
        assert_eq!(crossing.sign, 1);

        let mut current_day = daily(date(2026, 1, 1), Some(5_000));
        current_day.num = Some(1);
        current_day.den = Some(2);
        let mut prior_day = daily(date(2025, 12, 1), Some(5_004));
        prior_day.num = Some(1251);
        prior_day.den = Some(2500);
        let ratio = make_delta(
            &summarize(&[current_day], MetricUnit::Ratio, false),
            &summarize(&[prior_day], MetricUnit::Ratio, false),
            MetricUnit::Ratio,
            "4 weeks",
            false,
        );
        assert_eq!(ratio.display, "≈ same");

        let bool_delta = make_delta(
            &summarize(
                &[daily(date(2026, 1, 1), Some(1))],
                MetricUnit::Bool,
                false,
            ),
            &summarize(
                &[daily(date(2025, 12, 1), Some(0))],
                MetricUnit::Bool,
                false,
            ),
            MetricUnit::Bool,
            "4 weeks",
            false,
        );
        assert_eq!(bool_delta.display, "▲ 1 days");

        let empty_bool = summarize(&[], MetricUnit::Bool, false);
        assert_eq!(empty_bool.headline.display, "–");
        assert_eq!(empty_bool.headline.value, None);
    }

    #[test]
    fn huge_offset_is_a_range_error_not_a_panic() {
        assert!(resolve_range(
            &RangeSpec::Days(7),
            date(2026, 10, 11),
            u32::MAX,
            Grouping::Day,
            false,
            None,
        )
        .is_err());
        assert!(resolve_range(
            &RangeSpec::Days(u32::MAX),
            date(2026, 10, 11),
            u32::MAX,
            Grouping::Day,
            false,
            None,
        )
        .is_err());
    }

    #[test]
    fn all_with_no_history_matches_preset_end_convention() {
        let range = resolve_range(
            &RangeSpec::All,
            date(2026, 10, 11),
            0,
            Grouping::Auto,
            false,
            None,
        )
        .unwrap();
        assert_eq!(range.report.start, date(2026, 10, 10));
        assert_eq!(range.report.end, date(2026, 10, 10));
        assert_eq!(range.report.days, 0);
        assert!(range.includes_today);
        assert!(range.complete.is_none());
    }

    #[test]
    fn grouped_today_is_a_separate_provisional_point() {
        let today = date(2026, 10, 11);
        let mut values = vec![
            daily(date(2026, 10, 8), Some(1)),
            daily(date(2026, 10, 9), Some(2)),
            daily(date(2026, 10, 10), Some(3)),
            daily(today, Some(99)),
        ];
        values.last_mut().unwrap().provisional = true;
        let clip = DateSpan {
            start: date(2026, 10, 8),
            end: date(2026, 10, 10),
        };
        let points = make_points(
            &values,
            MetricUnit::Count,
            GroupName::Month,
            Some(clip),
            Some(date(2026, 10, 8)),
        );
        assert_eq!(points.len(), 2);
        assert!(!points[0].provisional);
        assert_eq!(points[0].end, date(2026, 10, 10));
        assert_eq!(points[0].value, Some(6));
        assert!(points[1].provisional);
        assert_eq!(points[1].date, today);
        assert_eq!(points[1].value, Some(99));
        assert_eq!(points[1].rolling_median, None);
    }

    #[test]
    fn first_displayed_point_uses_lead_in_for_rolling_median() {
        let start = date(2026, 1, 7);
        let values: Vec<_> = (0..14)
            .map(|offset| {
                daily(date(2026, 1, 1) + Duration::days(offset), Some(offset))
            })
            .collect();
        let clip = DateSpan {
            start,
            end: date(2026, 1, 13),
        };
        let points = make_points(
            &values,
            MetricUnit::Count,
            GroupName::Day,
            Some(clip),
            Some(start),
        );
        assert_eq!(points[0].date, start);
        assert_eq!(points[0].rolling_median, Some(3));
    }

    #[test]
    fn parts_collapse_only_note_and_theme() {
        let mut value = daily(date(2026, 1, 1), Some(21));
        let six = [("a", 6), ("b", 5), ("c", 4), ("d", 3), ("e", 2), ("f", 1)];
        value.parts.insert(
            "kind".to_string(),
            six.into_iter()
                .map(|(key, count)| (key.to_string(), count))
                .collect(),
        );
        value.parts.insert(
            "note".to_string(),
            six.into_iter()
                .map(|(key, count)| (key.to_string(), count))
                .collect(),
        );
        let summary = summarize_parts(&[value], MetricUnit::Count);
        assert_eq!(summary["kind"].len(), 6);
        assert!(summary["kind"].iter().all(|part| part.key != "other"));
        assert_eq!(summary["note"].len(), 6);
        assert_eq!(summary["note"][5].key, "other");
        assert_eq!(summary["note"][5].display, "1");
    }

    fn grouped_range(
        text: &str,
        today: NaiveDate,
        grouping: Grouping,
    ) -> ResolvedRange {
        resolve_range(
            &RangeSpec::parse(text).unwrap(),
            today,
            0,
            grouping,
            false,
            None,
        )
        .unwrap()
    }

    fn checklist_day(
        date: NaiveDate,
        state: ChecklistState,
        met: &[&str],
        missed: &[&str],
    ) -> DailyValue {
        let mut value = daily(
            date,
            match state {
                ChecklistState::Met => Some(10_000),
                ChecklistState::Missed => Some(0),
                _ => None,
            },
        );
        value.quality = match state {
            ChecklistState::NotTracked => DataQuality::NotTracked,
            ChecklistState::Missing | ChecklistState::NotDue => {
                DataQuality::Missing
            }
            ChecklistState::Met | ChecklistState::Missed => DataQuality::Ok,
        };
        value.reason = match state {
            ChecklistState::NotDue => Some("nothing due".to_string()),
            ChecklistState::Missing => Some("no data".to_string()),
            ChecklistState::NotTracked => {
                Some("before tracking began".to_string())
            }
            _ => None,
        };
        if !met.is_empty() {
            value.parts.insert(
                "met".to_string(),
                met.iter().map(|key| (key.to_string(), 1)).collect(),
            );
        }
        if !missed.is_empty() {
            value.parts.insert(
                "missed".to_string(),
                missed.iter().map(|key| (key.to_string(), 1)).collect(),
            );
        }
        value.num = (!met.is_empty() || !missed.is_empty())
            .then_some(i64::try_from(met.len()).unwrap_or(0));
        value.den = (!met.is_empty() || !missed.is_empty())
            .then_some(i64::try_from(met.len() + missed.len()).unwrap_or(0));
        value.checklist_state = Some(state);
        value
    }

    #[test]
    fn day_chart_quartiles_match_the_daily_summary() {
        let today = date(2026, 10, 11);
        let range = grouped_range("7d", today, Grouping::Day);
        let values: Vec<_> = (0..7)
            .map(|offset| {
                daily(
                    date(2026, 10, 4) + Duration::days(offset),
                    Some(offset * 10),
                )
            })
            .collect();
        let built = build_metric_series(
            &values,
            "worked",
            MetricUnit::Minutes,
            &range,
            false,
        );
        assert_eq!(built.chart.group, GroupName::Day);
        assert_eq!(built.chart.p25, built.summary.p25);
        assert_eq!(built.chart.p75, built.summary.p75);
        assert!(built.daily_points.is_empty());
        // Hand: complete values 0,10,20,30,40,50,60; p25 at (6)*1/4 = 1.5
        // interpolates 10 and 20 → 15. p75 at 4.5 interpolates 40 and 50 → 45.
        assert_eq!(built.chart.p25, Some(15));
        assert_eq!(built.chart.p75, Some(45));
    }

    #[test]
    fn week_and_month_chart_bands_use_bucket_values() {
        let today = date(2026, 10, 11);
        let mut values = Vec::new();
        // Two complete ISO weeks of constant daily 1, then 2, plus today.
        for offset in 0..14 {
            let day = date(2026, 9, 27) + Duration::days(offset);
            let value = if offset < 7 { 1 } else { 10 };
            values.push(daily(day, Some(value)));
        }
        values.push(daily(today, Some(100)));
        let week =
            grouped_range("2026-09-27..2026-10-11", today, Grouping::Week);
        let built = build_metric_series(
            &values,
            "tasks",
            MetricUnit::Count,
            &week,
            false,
        );
        assert_eq!(built.chart.group, GroupName::Week);
        // Displayed weeks: Sep 27 only (1); Sep 28–Oct 4 (6×1 + 10 = 16);
        // Oct 5–10 (6×10 = 60). Today is provisional and excluded.
        let complete: Vec<_> = built
            .points
            .iter()
            .filter(|point| !point.provisional)
            .filter_map(|point| point.value)
            .collect();
        assert_eq!(complete, vec![1, 16, 60]);
        assert_eq!(built.chart.p25, percentile_quarters(&[1, 16, 60], 1));
        assert_eq!(built.chart.p75, percentile_quarters(&[1, 16, 60], 3));
        assert_ne!(built.chart.p25, built.summary.p25);

        let month =
            grouped_range("2026-09-27..2026-10-11", today, Grouping::Month);
        let built = build_metric_series(
            &values,
            "tasks",
            MetricUnit::Count,
            &month,
            false,
        );
        assert_eq!(built.chart.group, GroupName::Month);
        let complete: Vec<_> = built
            .points
            .iter()
            .filter(|point| !point.provisional)
            .filter_map(|point| point.value)
            .collect();
        // Sep 27–30: 4 days of 1 = 4; Oct 1–10: 3 days of 1 + 7 days of 10 = 73.
        assert_eq!(complete, vec![4, 73]);
        assert_eq!(built.chart.p25, percentile_quarters(&[4, 73], 1));
        assert_eq!(built.chart.p75, percentile_quarters(&[4, 73], 3));
    }

    #[test]
    fn chart_bands_cover_signed_values_and_all_null_windows() {
        let today = date(2026, 10, 11);
        let range = grouped_range("7d", today, Grouping::Day);
        let signed: Vec<_> = [-6, -2, 0, 4, 9, -1, 3]
            .into_iter()
            .enumerate()
            .map(|(offset, value)| {
                daily(
                    date(2026, 10, 4) + Duration::days(offset as i64),
                    Some(value),
                )
            })
            .collect();
        let built = build_metric_series(
            &signed,
            "net",
            MetricUnit::Signed,
            &range,
            false,
        );
        // Sorted: -6,-2,-1,0,3,4,9. p25 at 1.5 → (-2 + -1)/2 = -2 (half-away
        // on -1.5 is -2). p75 at 4.5 → (3+4)/2 = 4.
        assert_eq!(built.chart.p25, Some(-2));
        assert_eq!(built.chart.p75, Some(4));

        let nulls: Vec<_> = (0..7)
            .map(|offset| {
                daily(date(2026, 10, 4) + Duration::days(offset), None)
            })
            .collect();
        let built = build_metric_series(
            &nulls,
            "worked",
            MetricUnit::Minutes,
            &range,
            false,
        );
        assert_eq!(built.chart.p25, None);
        assert_eq!(built.chart.p75, None);
        assert_eq!(built.summary.p25, None);
        assert_eq!(built.summary.p75, None);
    }

    #[test]
    fn chart_and_daily_points_exclude_today_and_lead_in() {
        let today = date(2026, 10, 11);
        let range = grouped_range("7d", today, Grouping::Week);
        let start = range.complete.unwrap().start;
        let lead = lead_in_start(start, GroupName::Week).unwrap();
        let mut values = Vec::new();
        let mut date = lead;
        while date <= today {
            let mut value = daily(date, Some(5));
            value.provisional = date == today;
            values.push(value);
            date += Duration::days(1);
        }
        let built = build_metric_series(
            &values,
            "tasks",
            MetricUnit::Count,
            &range,
            false,
        );
        assert!(built.points.iter().all(|point| point.date >= start));
        assert!(built
            .chart
            .p25
            .is_some_and(|_| built.points.iter().any(|p| !p.provisional)));
        assert!(built.daily_points.iter().all(|point| {
            point.date >= start
                && (point.date <= range.complete.unwrap().end
                    || point.date == today)
        }));
        assert!(built.daily_points.iter().all(|point| point.date >= start));
        assert!(!built.daily_points.iter().any(|point| point.date < start));
        assert_eq!(built.daily_points.last().unwrap().date, today);
        assert!(built.daily_points.last().unwrap().provisional);
        assert_eq!(
            built.daily_points.len(),
            usize::try_from(range.report.days).unwrap() + 1
        );
    }

    #[test]
    fn grouped_daily_points_clip_to_the_display_window() {
        let today = date(2026, 10, 11);
        let range =
            grouped_range("2026-10-08..2026-10-11", today, Grouping::Month);
        let mut values = Vec::new();
        for offset in 0..10 {
            let day = date(2026, 10, 2) + Duration::days(offset);
            let mut value = daily(day, Some(1));
            value.provisional = day == today;
            values.push(value);
        }
        let built = build_metric_series(
            &values,
            "tasks",
            MetricUnit::Count,
            &range,
            false,
        );
        let dates: Vec<_> =
            built.daily_points.iter().map(|point| point.date).collect();
        assert_eq!(
            dates,
            vec![
                date(2026, 10, 8),
                date(2026, 10, 9),
                date(2026, 10, 10),
                date(2026, 10, 11),
            ]
        );
        assert!(built.daily_points[3].provisional);
        assert!(built.points[0].checklist_state.is_none());
        assert_eq!(built.chart.group, GroupName::Month);
    }

    #[test]
    fn checklist_states_and_chore_denominators_use_complete_days() {
        let today = date(2026, 10, 11);
        let range =
            grouped_range("2026-10-08..2026-10-11", today, Grouping::Day);
        let mut values = vec![
            checklist_day(
                date(2026, 10, 8),
                ChecklistState::NotTracked,
                &[],
                &[],
            ),
            checklist_day(
                date(2026, 10, 9),
                ChecklistState::Missed,
                &[],
                &["journal", "meditate"],
            ),
            checklist_day(
                date(2026, 10, 10),
                ChecklistState::Missed,
                &["meditate"],
                &["journal"],
            ),
            checklist_day(
                date(2026, 10, 11),
                ChecklistState::Missed,
                &[],
                &["journal", "meditate"],
            ),
        ];
        values[3].provisional = true;
        let built = build_metric_series(
            &values,
            "habits",
            MetricUnit::Ratio,
            &range,
            true,
        );
        assert_eq!(
            built.points[0].checklist_state,
            Some(ChecklistState::NotTracked)
        );
        assert_eq!(
            built.points[1].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(
            built.points[2].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(
            built.points[3].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(built.chores.len(), 2);
        assert_eq!(built.chores[0].key, "journal");
        assert_eq!(built.chores[0].met, 0);
        assert_eq!(built.chores[0].due, 2);
        assert_eq!(built.chores[0].display, "0 of 2 days");
        assert_eq!(built.chores[1].key, "meditate");
        assert_eq!(built.chores[1].met, 1);
        assert_eq!(built.chores[1].due, 2);
        assert_eq!(built.chores[1].display, "1 of 2 days");

        let mut not_due =
            checklist_day(date(2026, 10, 9), ChecklistState::NotDue, &[], &[]);
        not_due.reason = Some("nothing due".to_string());
        let missing = checklist_day(
            date(2026, 10, 10),
            ChecklistState::Missing,
            &[],
            &[],
        );
        let built = build_metric_series(
            &[
                checklist_day(
                    date(2026, 10, 8),
                    ChecklistState::Met,
                    &["journal"],
                    &[],
                ),
                not_due,
                missing,
            ],
            "habits",
            MetricUnit::Ratio,
            &range,
            true,
        );
        assert_eq!(built.points[0].checklist_state, Some(ChecklistState::Met));
        assert_eq!(
            built.points[1].checklist_state,
            Some(ChecklistState::NotDue)
        );
        assert_eq!(
            built.points[2].checklist_state,
            Some(ChecklistState::Missing)
        );
        assert_eq!(built.chores.len(), 1);
        assert_eq!(built.chores[0].key, "journal");
        assert_eq!(built.chores[0].due, 1);
        assert_eq!(built.chores[0].met, 1);
    }

    #[test]
    fn parts_display_formats_minutes_and_leaves_counts() {
        let today = date(2026, 10, 11);
        let range =
            grouped_range("2026-10-08..2026-10-11", today, Grouping::Day);
        let mut minutes = daily(date(2026, 10, 9), Some(32));
        minutes.parts.insert(
            "theme".to_string(),
            BTreeMap::from([("GTD".to_string(), 29), ("SASE".to_string(), 3)]),
        );
        let built = build_metric_series(
            &[minutes],
            "worked",
            MetricUnit::Minutes,
            &range,
            false,
        );
        assert_eq!(built.points[1].parts_display["theme"]["GTD"], "29m");
        assert_eq!(built.points[1].parts_display["theme"]["SASE"], "3m");

        let mut counts = daily(date(2026, 10, 9), Some(5));
        counts.parts.insert(
            "kind".to_string(),
            BTreeMap::from([("task".to_string(), 4), ("ref".to_string(), 1)]),
        );
        let built = build_metric_series(
            &[counts],
            "done",
            MetricUnit::Count,
            &range,
            false,
        );
        assert_eq!(built.points[1].parts_display["kind"]["task"], "4");
        assert_eq!(built.points[1].parts_display["kind"]["ref"], "1");
    }
}

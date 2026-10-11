use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate};

use super::range::RangeSpec;
use super::registry::MetricUnit;
use super::report::{
    Delta, GroupName, Headline, MetricSummary, PartSummary, PriorHeadline,
    PriorRange, RangeReport, SeriesPoint, ValueDisplay,
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
    pub(crate) points: Vec<SeriesPoint>,
    pub(crate) prior_points: Vec<SeriesPoint>,
    pub(crate) parts_summary: BTreeMap<String, Vec<PartSummary>>,
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
            let start = all_since.unwrap_or(today).min(today);
            let yesterday = checked_add_days(today, -1)?;
            let complete = (start <= yesterday).then_some(DateSpan {
                start,
                end: yesterday,
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

    let start = complete.map_or(today, |span| span.start);
    let end = complete.map_or(today, |span| span.end);
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
    date.checked_add_signed(Duration::days(days))
        .ok_or_else(|| "stats range exceeds the supported calendar".to_string())
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
) -> MetricSeries {
    let current_span = range.display_span();
    let current_daily =
        normalize(values, current_span, range.today, range.includes_today);
    let prior_daily = normalize(values, range.prior, range.today, false);
    let points = make_points(&current_daily, unit, range.group);
    let prior_points = make_points(&prior_daily, unit, range.group);
    let complete_daily: Vec<_> = current_daily
        .iter()
        .filter(|value| !value.provisional)
        .cloned()
        .collect();
    let prior_summary = range.prior.map(|_| summarize(&prior_daily, unit));
    let summary = summarize_with_prior(
        &complete_daily,
        unit,
        prior_summary.as_ref(),
        &range.comparison_label,
        metric_id == "net",
    );
    let parts_summary = summarize_parts(&complete_daily, unit);
    MetricSeries {
        summary,
        points,
        prior_points,
        parts_summary,
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

fn make_points(
    values: &[DailyValue],
    unit: MetricUnit,
    group: GroupName,
) -> Vec<SeriesPoint> {
    let mut points = match group {
        GroupName::Day => values
            .iter()
            .map(|value| daily_point(value, unit))
            .collect(),
        GroupName::Week | GroupName::Month => {
            grouped_points(values, unit, group)
        }
    };
    add_rolling_medians(&mut points, group);
    points
}

fn daily_point(value: &DailyValue, unit: MetricUnit) -> SeriesPoint {
    SeriesPoint {
        date: value.date,
        end: value.date,
        value: value.value,
        display: format_value(value.value, unit),
        quality: value.quality,
        reason: value.reason.clone(),
        provisional: value.provisional,
        rolling_median: None,
        num: value.num,
        den: value.den,
        partial: None,
        parts: value.parts.clone(),
    }
}

fn grouped_points(
    values: &[DailyValue],
    unit: MetricUnit,
    group: GroupName,
) -> Vec<SeriesPoint> {
    let Some(first) = values.first() else {
        return Vec::new();
    };
    let Some(last) = values.last() else {
        return Vec::new();
    };
    let range_start = first.date;
    let range_end = last.date;
    let mut buckets: BTreeMap<NaiveDate, Vec<&DailyValue>> = BTreeMap::new();
    for value in values {
        buckets
            .entry(bucket_start(value.date, group))
            .or_default()
            .push(value);
    }
    buckets
        .into_iter()
        .map(|(natural_start, days)| {
            let natural_end = bucket_end(natural_start, group);
            let start = natural_start.max(range_start);
            let end = natural_end.min(range_end);
            let partial = start != natural_start || end != natural_end;
            aggregate_bucket(start, end, &days, unit, partial)
        })
        .collect()
}

fn bucket_start(date: NaiveDate, group: GroupName) -> NaiveDate {
    match group {
        GroupName::Day => date,
        GroupName::Week => {
            date - Duration::days(i64::from(
                date.weekday().num_days_from_monday(),
            ))
        }
        GroupName::Month => {
            NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
                .expect("a date has a valid first of month")
        }
    }
}

fn bucket_end(start: NaiveDate, group: GroupName) -> NaiveDate {
    match group {
        GroupName::Day => start,
        GroupName::Week => start + Duration::days(6),
        GroupName::Month => {
            let next_month = if start.month() == 12 {
                NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
            };
            next_month.and_then(|date| date.pred_opt()).unwrap_or(start)
        }
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
    for day in days {
        for (dimension, entries) in &day.parts {
            let grouped = parts.entry(dimension.clone()).or_default();
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
        let met = bool_total.unwrap_or_default();
        format!("{met} of {}", valid.len())
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
        parts,
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
    let mut current = summarize(values, unit);
    if let Some(prior) = prior {
        current.prior = Some(PriorHeadline {
            headline: prior.headline.clone(),
        });
        current.delta =
            Some(make_delta(&current, prior, unit, comparison_label, is_net));
    }
    current
}

fn summarize(values: &[DailyValue], unit: MetricUnit) -> MetricSummary {
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

    let headline = match unit {
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
            let (num, den) =
                values.iter().fold((0_i64, 0_i64), |acc, (day, _)| {
                    match (day.num, day.den) {
                        (Some(num), Some(den)) => (
                            acc.0.saturating_add(num),
                            acc.1.saturating_add(den),
                        ),
                        _ => acc,
                    }
                });
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
        return if current_value != 0 {
            new_delta(comparison_label)
        } else {
            empty_delta()
        };
    };
    let difference = current_value.saturating_sub(prior_value);
    if difference == 0 {
        return same_delta(comparison_label);
    }
    if unit == MetricUnit::Ratio {
        let amount = format_points(difference.unsigned_abs() as i64);
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
        return new_delta(comparison_label);
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

fn new_delta(comparison_label: &str) -> Delta {
    Delta {
        display: "new".to_string(),
        words: format!("new vs the prior {comparison_label}"),
        sign: 1,
    }
}

fn summarize_parts(
    values: &[DailyValue],
    unit: MetricUnit,
) -> BTreeMap<String, Vec<PartSummary>> {
    let mut totals = BTreeMap::<String, BTreeMap<String, i64>>::new();
    for day in values.iter().filter(|day| !day.provisional) {
        for (dimension, entries) in &day.parts {
            let dimension_totals = totals.entry(dimension.clone()).or_default();
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
            let overflow = ranked.split_off(ranked.len().min(5));
            if !overflow.is_empty() {
                let other = overflow
                    .iter()
                    .fold(0_i64, |sum, (_, value)| sum.saturating_add(*value));
                ranked.push(("other".to_string(), other));
            }
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
                        PartSummary {
                            key,
                            value,
                            display: format_value(Some(value), unit),
                            share: format!("{share}%"),
                        }
                    })
                    .collect(),
            ))
        })
        .collect()
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

fn format_minutes(value: i64) -> String {
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
    use chrono::NaiveDate;

    use super::*;
    use crate::native::stats::request::Grouping;

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
            grouped_points(&ratio, MetricUnit::Ratio, GroupName::Week)
                .remove(0);
        assert_eq!(ratio_point.num, Some(7));
        assert_eq!(ratio_point.den, Some(14));
        assert_eq!(ratio_point.value, Some(5_000));
        assert_eq!(ratio_point.display, "50%");
        let bool_point =
            grouped_points(&boolean, MetricUnit::Bool, GroupName::Week)
                .remove(0);
        assert_eq!(bool_point.value, Some(4));
        assert_eq!(bool_point.display, "4 of 7");
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
        let built =
            build_metric_series(&values, "tasks", MetricUnit::Count, &range);
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
        let points = make_points(&values, MetricUnit::Count, GroupName::Day);
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
        let bool_summary = summarize(&values, MetricUnit::Bool);
        assert_eq!(bool_summary.headline.display, "2 of 3 days");
        assert_eq!(bool_summary.days_met, Some(2));
        let ratio_summary = summarize(&values, MetricUnit::Ratio);
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
    fn signed_net_delta_uses_range_total_words() {
        let current =
            summarize(&[daily(date(2026, 1, 1), Some(-3))], MetricUnit::Signed);
        let previous =
            summarize(&[daily(date(2025, 12, 1), Some(4))], MetricUnit::Signed);
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
    fn delta_uses_multipliers_for_large_changes_and_points_for_ratios() {
        let current =
            summarize(&[daily(date(2026, 1, 1), Some(5))], MetricUnit::Count);
        let prior =
            summarize(&[daily(date(2025, 12, 1), Some(2))], MetricUnit::Count);
        let delta =
            make_delta(&current, &prior, MetricUnit::Count, "4 weeks", false);
        assert_eq!(delta.display, "▲ 2.5×");

        let mut current_day = daily(date(2026, 1, 1), Some(6_000));
        current_day.num = Some(3);
        current_day.den = Some(5);
        let mut prior_day = daily(date(2025, 12, 1), Some(5_600));
        prior_day.num = Some(14);
        prior_day.den = Some(25);
        let ratio_current = summarize(&[current_day], MetricUnit::Ratio);
        let ratio_prior = summarize(&[prior_day], MetricUnit::Ratio);
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
            grouped_points(&values, MetricUnit::Count, GroupName::Week);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].date, start);
        assert_eq!(points[0].end, date(2026, 1, 3));
        assert_eq!(points[0].partial, Some(true));
        assert_eq!(points[0].quality, DataQuality::Partial);
        assert_eq!(points[0].value, Some(2));

        let month =
            grouped_points(&values, MetricUnit::Count, GroupName::Month);
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
}

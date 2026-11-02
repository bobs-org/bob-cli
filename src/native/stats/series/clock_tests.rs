//! Worked/off-clock breakdowns on series points, part summaries, and
//! complete-day totals.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use super::*;
use crate::native::stats::report::{ClockBreakdown, SeriesPoint};
use crate::native::stats::request::Grouping;

const TODAY: (i32, u32, u32) = (2026, 10, 14);

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn sept(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).unwrap()
}

fn blank(date: NaiveDate) -> DailyValue {
    DailyValue {
        date,
        value: None,
        num: None,
        den: None,
        quality: DataQuality::Missing,
        reason: Some("no daily note".to_string()),
        provisional: false,
        parts: BTreeMap::new(),
        apportioned: BTreeMap::new(),
        checklist_state: None,
        off_clock: None,
        off_clock_parts: BTreeMap::new(),
        clock: None,
    }
}

fn theme_parts(entries: &[(&str, i64)]) -> BTreeMap<String, i64> {
    entries
        .iter()
        .filter(|(_, value)| *value > 0)
        .map(|(key, value)| (key.to_string(), *value))
        .collect()
}

/// A Total day, shaped like the Pomodoro evaluator's output: `value` holds
/// both clocks, `off_clock` and `off_clock_parts` hold the off-clock side
/// only when it is positive, and every dimension splits the same minutes.
fn total_day(date: NaiveDate, worked: i64, off: i64) -> DailyValue {
    let mut day = blank(date);
    day.clock = Some(StatsClock::Total);
    day.value = Some(worked + off);
    day.quality = DataQuality::Ok;
    day.reason = None;
    for (dimension, work_key, off_key) in
        [("theme", "WORK", "MEET"), ("project", "app.md", "meet.md")]
    {
        let split = theme_parts(&[(work_key, worked), (off_key, off)]);
        if !split.is_empty() {
            day.parts.insert(dimension.to_string(), split);
        }
        let off_split = theme_parts(&[(off_key, off)]);
        if !off_split.is_empty() {
            day.off_clock_parts.insert(dimension.to_string(), off_split);
        }
    }
    if worked + off > 0 {
        day.parts.insert(
            "area".to_string(),
            theme_parts(&[("dev.md", worked + off)]),
        );
    }
    if off > 0 {
        day.off_clock = Some(off);
        day.off_clock_parts
            .insert("area".to_string(), theme_parts(&[("dev.md", off)]));
    }
    day
}

fn worked_day(date: NaiveDate, worked: i64, excluded: i64) -> DailyValue {
    let mut day = blank(date);
    day.clock = Some(StatsClock::Worked);
    day.value = Some(worked);
    day.quality = DataQuality::Ok;
    day.reason = None;
    day.off_clock = (excluded > 0).then_some(excluded);
    day.parts = BTreeMap::from([(
        "theme".to_string(),
        theme_parts(&[("WORK", worked)]),
    )]);
    day
}

fn untracked_day(date: NaiveDate, clock: StatsClock) -> DailyValue {
    let mut day = blank(date);
    day.clock = Some(clock);
    day.quality = DataQuality::NotTracked;
    day.reason = Some("before tracking began".to_string());
    day
}

fn provisional(mut day: DailyValue) -> DailyValue {
    day.provisional = true;
    day
}

fn split(point: &SeriesPoint) -> Option<(i64, i64)> {
    point
        .clock_breakdown
        .as_ref()
        .map(|split| (split.worked.value, split.off_clock.value))
}

fn point_on(points: &[SeriesPoint], day: NaiveDate) -> &SeriesPoint {
    points
        .iter()
        .find(|point| point.date == day)
        .unwrap_or_else(|| panic!("no point starting {day}"))
}

fn range(text: &str, grouping: Grouping, compare: bool) -> ResolvedRange {
    resolve_range(
        &RangeSpec::parse(text).unwrap(),
        today(),
        0,
        grouping,
        compare,
        None,
    )
    .unwrap()
}

/// Total days from Sep 26 to today. Adoption is Oct 1, Oct 8 has no note,
/// and today is provisional.
fn total_history() -> Vec<DailyValue> {
    let mut days: Vec<DailyValue> = (26..=30)
        .map(|day| untracked_day(sept(day), StatsClock::Total))
        .collect();
    // (worked, off): a zero day, an off-only day, and a worked-only day sit
    // among mixed days.
    let table: [(u32, Option<(i64, i64)>); 14] = [
        (1, Some((30, 10))),
        (2, Some((0, 0))),
        (3, Some((45, 0))),
        (4, Some((20, 60))),
        (5, Some((60, 0))),
        (6, Some((30, 30))),
        (7, Some((0, 90))),
        (8, None),
        (9, Some((45, 15))),
        (10, Some((0, 0))),
        (11, Some((15, 0))),
        (12, Some((50, 25))),
        (13, Some((20, 0))),
        (14, Some((10, 20))),
    ];
    for (day, split) in table {
        let mut value = match split {
            Some((worked, off)) => total_day(date(day), worked, off),
            None => blank(date(day)),
        };
        value.provisional = day == 14;
        days.push(value);
    }
    days
}

/// The same history on a series that does not know its clock.
fn clockless(days: &[DailyValue]) -> Vec<DailyValue> {
    days.iter()
        .cloned()
        .map(|mut day| {
            day.clock = None;
            day
        })
        .collect()
}

fn without_breakdowns(point: &SeriesPoint) -> SeriesPoint {
    SeriesPoint {
        clock_breakdown: None,
        parts_clock_breakdown: BTreeMap::new(),
        ..point.clone()
    }
}

fn assert_adds_up(split: &ClockBreakdown, whole: i64, label: &str) {
    assert_eq!(
        split.worked.value + split.off_clock.value,
        whole,
        "{label} components add to the whole"
    );
}

#[test]
fn total_day_points_split_value_and_every_part() {
    let mixed = total_day(date(9), 45, 15);
    let point = daily_point(&mixed, MetricUnit::Minutes);
    let breakdown = point.clock_breakdown.as_ref().expect("Total split");
    assert_eq!(breakdown.worked.value, 45);
    assert_eq!(breakdown.worked.display, "45m");
    assert_eq!(breakdown.off_clock.value, 15);
    assert_eq!(breakdown.off_clock.display, "15m");
    assert_adds_up(breakdown, point.value.unwrap(), "point");
    assert_eq!(point.off_clock, Some(15));

    for dimension in ["theme", "project", "area"] {
        let parts = &point.parts[dimension];
        let splits = &point.parts_clock_breakdown[dimension];
        assert_eq!(
            parts.keys().collect::<Vec<_>>(),
            splits.keys().collect::<Vec<_>>(),
            "{dimension} split covers exactly the existing parts"
        );
        for (key, whole) in parts {
            assert_adds_up(&splits[key], *whole, key);
        }
    }
    let theme = &point.parts_clock_breakdown["theme"];
    assert_eq!(
        (theme["WORK"].worked.value, theme["WORK"].off_clock.value),
        (45, 0)
    );
    assert_eq!(
        (theme["MEET"].worked.value, theme["MEET"].off_clock.value),
        (0, 15)
    );
    assert_eq!(theme["MEET"].off_clock.display, "15m");
    assert_eq!(
        point.parts_clock_breakdown["area"]["dev.md"].worked.value,
        45
    );

    // Hour formatting comes from the shared Bob formatter.
    let long = daily_point(&total_day(date(9), 390, 90), MetricUnit::Minutes);
    let long = long.clock_breakdown.unwrap();
    assert_eq!(long.worked.display, "6h 30m");
    assert_eq!(long.off_clock.display, "1h 30m");
}

#[test]
fn total_zero_days_report_two_explicit_zero_components() {
    let zero = total_day(date(10), 0, 0);
    let point = daily_point(&zero, MetricUnit::Minutes);
    assert_eq!(split(&point), Some((0, 0)));
    let breakdown = point.clock_breakdown.unwrap();
    assert_eq!(breakdown.worked.display, "0m");
    assert_eq!(breakdown.off_clock.display, "0m");
    assert_eq!(point.off_clock, None);
    assert!(point.parts_clock_breakdown.is_empty());

    // An all-worked Total day reports zero off-clock, not a missing side.
    let worked_only =
        daily_point(&total_day(date(3), 45, 0), MetricUnit::Minutes);
    assert_eq!(split(&worked_only), Some((45, 0)));
    assert_eq!(worked_only.off_clock, None);
    assert_eq!(
        worked_only.parts_clock_breakdown["theme"]["WORK"]
            .off_clock
            .value,
        0
    );
}

#[test]
fn missing_and_untracked_days_never_invent_a_split() {
    for clock in [StatsClock::Worked, StatsClock::OffClock, StatsClock::Total] {
        let untracked = untracked_day(date(5), clock);
        let point = daily_point(&untracked, MetricUnit::Minutes);
        assert_eq!(point.clock_breakdown, None, "{clock:?}");
        assert!(point.parts_clock_breakdown.is_empty(), "{clock:?}");
    }
    let mut missing = total_day(date(8), 10, 5);
    missing.value = None;
    missing.quality = DataQuality::Missing;
    let point = daily_point(&missing, MetricUnit::Minutes);
    assert_eq!(point.clock_breakdown, None);
    assert!(point.parts_clock_breakdown.is_empty());
    assert_eq!(
        daily_point(&blank(date(8)), MetricUnit::Minutes).clock_breakdown,
        None
    );
}

#[test]
fn worked_points_split_only_when_off_clock_time_was_left_out() {
    let hidden = daily_point(&worked_day(date(9), 32, 60), MetricUnit::Minutes);
    let breakdown = hidden.clock_breakdown.as_ref().expect("Worked split");
    assert_eq!(breakdown.worked.value, 32);
    assert_eq!(breakdown.worked.display, "32m");
    assert_eq!(breakdown.off_clock.value, 60);
    assert_eq!(breakdown.off_clock.display, "1h 00m");
    assert_eq!(hidden.value, Some(32));
    assert_eq!(hidden.off_clock, Some(60));
    assert!(hidden.parts_clock_breakdown.is_empty());

    let clean = daily_point(&worked_day(date(10), 32, 0), MetricUnit::Minutes);
    assert_eq!(clean.clock_breakdown, None);
    assert_eq!(clean.off_clock, None);
}

#[test]
fn off_clock_lens_and_non_pomodoro_metrics_carry_no_split() {
    let mut off = worked_day(date(9), 60, 0);
    off.clock = Some(StatsClock::OffClock);
    let point = daily_point(&off, MetricUnit::Minutes);
    assert_eq!(point.clock_breakdown, None);
    assert!(point.parts_clock_breakdown.is_empty());

    // Formulas, tasks, and journals leave `clock` empty, whatever else they
    // carry. Even a stray off-clock amount never produces a split.
    let mut other = total_day(date(9), 45, 15);
    other.clock = None;
    let point = daily_point(&other, MetricUnit::Minutes);
    assert_eq!(point.clock_breakdown, None);
    assert!(point.parts_clock_breakdown.is_empty());
    let mut ratio = total_day(date(9), 45, 15);
    ratio.clock = None;
    assert_eq!(daily_point(&ratio, MetricUnit::Ratio).clock_breakdown, None);
}

#[test]
fn a_corrupt_total_is_left_unsplit_instead_of_negative() {
    let mut bad = total_day(date(9), 10, 5);
    bad.value = Some(4);
    bad.off_clock = Some(5);
    let point = daily_point(&bad, MetricUnit::Minutes);
    assert_eq!(point.clock_breakdown, None);
    // Parts that are individually consistent still split.
    assert!(point.parts_clock_breakdown.contains_key("theme"));
    let mut worse = total_day(date(9), 10, 5);
    worse
        .parts
        .get_mut("theme")
        .unwrap()
        .insert("MEET".into(), 2);
    let point = daily_point(&worse, MetricUnit::Minutes);
    assert!(!point.parts_clock_breakdown["theme"].contains_key("MEET"));
    assert!(point.parts_clock_breakdown["theme"].contains_key("WORK"));
}

#[test]
fn count_units_split_in_sessions_not_minutes() {
    let mut day = total_day(date(9), 2, 1);
    day.parts.clear();
    day.off_clock_parts.clear();
    day.parts.insert(
        "theme".to_string(),
        theme_parts(&[("WORK", 2), ("MEET", 1)]),
    );
    day.off_clock_parts
        .insert("theme".to_string(), theme_parts(&[("MEET", 1)]));
    let point = daily_point(&day, MetricUnit::Count);
    let breakdown = point.clock_breakdown.unwrap();
    assert_eq!(point.display, "3");
    assert_eq!(breakdown.worked.display, "2");
    assert_eq!(breakdown.off_clock.display, "1");
    let meet = &point.parts_clock_breakdown["theme"]["MEET"];
    assert_eq!((meet.worked.value, meet.off_clock.value), (0, 1));
    assert_eq!(meet.off_clock.display, "1");

    let worked = daily_point(&worked_day(date(9), 2, 1), MetricUnit::Count);
    assert_eq!(worked.clock_breakdown.unwrap().off_clock.display, "1");

    // Units outside minutes and sessions never split.
    assert_eq!(
        daily_point(&total_day(date(9), 2, 1), MetricUnit::Signed)
            .clock_breakdown,
        None
    );
}

#[test]
fn daily_series_split_today_and_adoption_gaps_without_changing_values() {
    let days = total_history();
    let window = range("2026-10-05..2026-10-14", Grouping::Day, true);
    let built = build_metric_series(
        &days,
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );
    let baseline = build_metric_series(
        &clockless(&days),
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );

    // Existing facts are byte for byte what a clockless series reports.
    assert_eq!(built.summary.headline, baseline.summary.headline);
    assert_eq!(built.summary.total, baseline.summary.total);
    assert_eq!(built.summary.delta, baseline.summary.delta);
    assert_eq!(built.summary.mean, baseline.summary.mean);
    assert_eq!(built.chart, baseline.chart);
    let stripped: Vec<_> =
        built.points.iter().map(without_breakdowns).collect();
    assert_eq!(stripped, baseline.points);
    let stripped_prior: Vec<_> =
        built.prior_points.iter().map(without_breakdowns).collect();
    assert_eq!(stripped_prior, baseline.prior_points);

    assert_eq!(split(point_on(&built.points, date(5))), Some((60, 0)));
    assert_eq!(split(point_on(&built.points, date(6))), Some((30, 30)));
    // Off-only and zero days.
    assert_eq!(split(point_on(&built.points, date(7))), Some((0, 90)));
    assert_eq!(split(point_on(&built.points, date(10))), Some((0, 0)));
    // The day without a note has no split.
    assert_eq!(split(point_on(&built.points, date(8))), None);
    // Provisional today keeps its split and its marker.
    let live = point_on(&built.points, date(14));
    assert!(live.provisional);
    assert_eq!(split(live), Some((10, 20)));
    assert_eq!(
        live.parts_clock_breakdown["theme"]["MEET"].off_clock.value,
        20
    );

    // Prior points: adoption clips Sep 26–30; Oct 1–4 split.
    assert_eq!(built.prior_points.len(), 9);
    assert_eq!(split(point_on(&built.prior_points, sept(28))), None);
    assert_eq!(
        split(point_on(&built.prior_points, date(1))),
        Some((30, 10))
    );
    assert_eq!(
        split(point_on(&built.prior_points, date(4))),
        Some((20, 60))
    );
    assert!(built.prior_points.iter().all(|point| !point.provisional));

    // Every split point adds up, and so does every part.
    for point in built.points.iter().chain(&built.prior_points) {
        if let Some(breakdown) = &point.clock_breakdown {
            assert_adds_up(breakdown, point.value.unwrap(), "point");
        } else {
            assert!(point.parts_clock_breakdown.is_empty());
        }
        for (dimension, splits) in &point.parts_clock_breakdown {
            for (key, split) in splits {
                assert_adds_up(split, point.parts[dimension][key], key);
            }
        }
    }
}

#[test]
fn summary_total_split_covers_exactly_the_complete_days() {
    let days = total_history();
    let window = range("2026-10-05..2026-10-14", Grouping::Day, true);
    let built = build_metric_series(
        &days,
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );
    // Complete days Oct 5–13 without the note-less Oct 8:
    // value 60+60+90+60+0+15+75+20 = 380, off 0+30+90+15+0+0+25+0 = 160.
    let total = built.summary.total.as_ref().expect("total");
    assert_eq!(total.value, Some(380));
    let breakdown = built.summary.total_clock_breakdown.as_ref().unwrap();
    assert_eq!(breakdown.worked.value, 220);
    assert_eq!(breakdown.worked.display, "3h 40m");
    assert_eq!(breakdown.off_clock.value, 160);
    assert_eq!(breakdown.off_clock.display, "2h 40m");
    assert_adds_up(breakdown, 380, "summary total");
    // Today (30 total) is excluded, exactly as in the summary total.
    assert_ne!(breakdown.off_clock.value, 180);

    // The headline is a median and stays undecomposed.
    assert_eq!(built.summary.headline.stat.as_deref(), Some("median"));
    let json = serde_json::to_value(&built.summary).unwrap();
    assert!(json["headline"].get("clock_breakdown").is_none());
    assert_eq!(json["total_clock_breakdown"]["worked"]["value"], 220);
    assert_eq!(
        json["total_clock_breakdown"]["off_clock"]["display"],
        "2h 40m"
    );

    // Parts add up per key and across the dimension.
    for (dimension, parts) in &built.parts_summary {
        let mut worked = 0;
        let mut off = 0;
        for part in parts {
            let split =
                part.clock_breakdown.as_ref().expect("Total part split");
            assert_adds_up(split, part.value, &part.key);
            assert_eq!(
                split.off_clock.value,
                part.off_clock.unwrap_or(0),
                "{dimension} {} off-clock agrees with the sub-share",
                part.key
            );
            worked += split.worked.value;
            off += split.off_clock.value;
        }
        assert_eq!((worked, off), (220, 160), "{dimension}");
    }
    let meet = built.parts_summary["theme"]
        .iter()
        .find(|part| part.key == "MEET")
        .unwrap();
    assert_eq!(meet.value, 160);
    let meet = meet.clock_breakdown.as_ref().unwrap();
    assert_eq!((meet.worked.value, meet.off_clock.value), (0, 160));
    let work = built.parts_summary["theme"]
        .iter()
        .find(|part| part.key == "WORK")
        .unwrap();
    assert_eq!(work.off_clock, None);
    assert_eq!(
        work.clock_breakdown.as_ref().unwrap().off_clock.display,
        "0m"
    );
}

#[test]
fn week_and_month_buckets_sum_valid_days_and_keep_daily_points() {
    let days = total_history();
    let window = range("2026-10-05..2026-10-14", Grouping::Week, true);
    let built = build_metric_series(
        &days,
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );
    // Oct 5–11 (days with a note: 5,6,7,9,10,11) and Oct 12–13, then the
    // provisional today as its own daily point.
    let first = point_on(&built.points, date(5));
    assert_eq!(first.end, date(11));
    assert_eq!(first.value, Some(285));
    assert_eq!(split(first), Some((150, 135)));
    assert_eq!(first.off_clock, Some(135));
    let second = point_on(&built.points, date(12));
    assert_eq!(split(second), Some((70, 25)));
    let live = point_on(&built.points, date(14));
    assert!(live.provisional);
    assert_eq!(split(live), Some((10, 20)));
    assert_eq!(built.points.len(), 3);

    // Bucket parts add up and equal the sum of their daily parts.
    for (dimension, splits) in &first.parts_clock_breakdown {
        for (key, split) in splits {
            assert_adds_up(split, first.parts[dimension][key], key);
        }
    }
    let meet = &first.parts_clock_breakdown["theme"]["MEET"];
    assert_eq!((meet.worked.value, meet.off_clock.value), (0, 135));
    let work = &first.parts_clock_breakdown["theme"]["WORK"];
    assert_eq!((work.worked.value, work.off_clock.value), (150, 0));

    // Daily points under a grouped chart carry the same days.
    let daily = point_on(&built.daily_points, date(6));
    assert_eq!(split(daily), Some((30, 30)));
    assert_eq!(split(point_on(&built.daily_points, date(8))), None);
    assert_eq!(built.daily_points.len(), 10);

    // The prior week is clipped by adoption: only Oct 1–4 contribute.
    let prior = point_on(&built.prior_points, sept(28));
    assert_eq!(prior.value, Some(165));
    assert_eq!(split(prior), Some((95, 70)));
    assert_eq!(
        split(point_on(&built.prior_points, sept(26))),
        None,
        "a bucket made only of untracked days has no split"
    );

    let month = range("2026-10-05..2026-10-14", Grouping::Month, false);
    let monthly = build_metric_series(
        &days,
        "worked",
        MetricUnit::Minutes,
        &month,
        false,
    );
    let october = point_on(&monthly.points, date(5));
    assert_eq!(october.value, Some(380));
    assert_eq!(split(october), Some((220, 160)));
    assert_eq!(
        monthly.summary.total_clock_breakdown,
        built.summary.total_clock_breakdown
    );

    // Grouping never changes existing values.
    let baseline = build_metric_series(
        &clockless(&days),
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );
    let stripped: Vec<_> =
        built.points.iter().map(without_breakdowns).collect();
    assert_eq!(stripped, baseline.points);
    let stripped_daily: Vec<_> =
        built.daily_points.iter().map(without_breakdowns).collect();
    assert_eq!(stripped_daily, baseline.daily_points);
}

#[test]
fn worked_series_split_points_and_totals_by_excluded_time() {
    let days = vec![
        worked_day(date(9), 32, 60),
        worked_day(date(10), 40, 0),
        worked_day(date(11), 20, 15),
        worked_day(date(12), 5, 5),
        provisional(worked_day(date(14), 7, 3)),
    ];
    let window = range("2026-10-09..2026-10-14", Grouping::Day, false);
    let built = build_metric_series(
        &days,
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );
    assert_eq!(split(point_on(&built.points, date(9))), Some((32, 60)));
    assert_eq!(split(point_on(&built.points, date(10))), None);
    assert_eq!(split(point_on(&built.points, date(11))), Some((20, 15)));
    assert_eq!(split(point_on(&built.points, date(12))), Some((5, 5)));
    assert_eq!(split(point_on(&built.points, date(13))), None);
    let live = point_on(&built.points, date(14));
    assert!(live.provisional);
    assert_eq!(split(live), Some((7, 3)));
    // Complete days only: today's 7 and 3 stay out of the total.
    let breakdown = built.summary.total_clock_breakdown.as_ref().unwrap();
    assert_eq!(
        (breakdown.worked.value, breakdown.off_clock.value),
        (97, 80)
    );
    assert_eq!(built.summary.total.as_ref().unwrap().value, Some(97));
    // Worked parts have no per-part off-clock detail.
    assert!(built.parts_summary["theme"]
        .iter()
        .all(|part| part.clock_breakdown.is_none()));
    assert!(built
        .points
        .iter()
        .all(|point| point.parts_clock_breakdown.is_empty()));

    let clean = vec![worked_day(date(10), 40, 0)];
    let window = range("2026-10-10..2026-10-10", Grouping::Day, false);
    let built = build_metric_series(
        &clean,
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );
    assert_eq!(built.summary.total_clock_breakdown, None);
}

#[test]
fn off_clock_and_unrelated_series_omit_every_breakdown() {
    let mut off: Vec<_> = (9..=11)
        .map(|day| {
            let mut value = worked_day(date(day), 60, 0);
            value.clock = Some(StatsClock::OffClock);
            value
        })
        .collect();
    off.push(untracked_day(date(8), StatsClock::OffClock));
    let window = range("2026-10-08..2026-10-11", Grouping::Day, false);
    let built = build_metric_series(
        &off,
        "worked",
        MetricUnit::Minutes,
        &window,
        false,
    );
    assert_eq!(built.summary.total_clock_breakdown, None);
    assert!(built
        .points
        .iter()
        .all(|point| point.clock_breakdown.is_none()));
    assert!(built
        .parts_summary
        .values()
        .flatten()
        .all(|part| part.clock_breakdown.is_none()));

    // A non-Pomodoro metric with the same numbers (clock empty) stays plain.
    let tasks = clockless(
        &(9..=11)
            .map(|day| total_day(date(day), 3, 1))
            .collect::<Vec<_>>(),
    );
    let built =
        build_metric_series(&tasks, "done", MetricUnit::Count, &window, false);
    assert_eq!(built.summary.total_clock_breakdown, None);
    let json = serde_json::to_value(&built.points[2]).unwrap();
    assert!(json.get("clock_breakdown").is_none());
    assert!(json.get("parts_clock_breakdown").is_none());
}

#[test]
fn serialized_breakdown_shape_is_additive_and_skips_empty_fields() {
    let point = daily_point(&total_day(date(9), 45, 15), MetricUnit::Minutes);
    let json = serde_json::to_value(&point).unwrap();
    assert_eq!(
        json["clock_breakdown"],
        serde_json::json!({
            "worked": {"value": 45, "display": "45m"},
            "off_clock": {"value": 15, "display": "15m"},
        })
    );
    assert_eq!(
        json["parts_clock_breakdown"]["theme"]["MEET"],
        serde_json::json!({
            "worked": {"value": 0, "display": "0m"},
            "off_clock": {"value": 15, "display": "15m"},
        })
    );
    // Existing keys are untouched.
    assert_eq!(json["off_clock"], 15);
    assert_eq!(json["parts"]["theme"]["MEET"], 15);
    assert_eq!(json["parts_display"]["theme"]["MEET"], "15m");

    let plain = daily_point(&blank(date(9)), MetricUnit::Minutes);
    let json = serde_json::to_value(&plain).unwrap();
    assert!(json.get("clock_breakdown").is_none());
    assert!(json.get("parts_clock_breakdown").is_none());
}

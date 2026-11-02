//! Worked/off-clock breakdowns through the real compute path. Every Total
//! split is checked against the Worked and Off-clock lenses computed
//! independently over the same vault, filters, and window.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use tempfile::TempDir;

use crate::native::stats::clock::StatsClock;
use crate::native::stats::compute::compute;
use crate::native::stats::report::{MetricReport, SeriesPoint, StatsReport};
use crate::native::stats::request::{Grouping, MetricSelection, StatsRequest};

const IDS: [&str; 11] = [
    "worked",
    "sessions",
    "done",
    "sase_time",
    "sase_in",
    "sase_count_in",
    "exclude_sase",
    "gtd_daily_in",
    "family_off",
    "family_worked",
    "inner",
];

fn homes_vault() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stats/homes_vault"
    ))
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let dest = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// The homes fixture plus four off-clock rows on Oct 16: a multi-home row
/// that shares its theme with a worked row, a single-area row, a
/// multi-theme row under a project, and an unlinked row. Oct 14 and Oct 15
/// precede the first off-clock session.
fn vault_and_config() -> (TempDir, PathBuf, PathBuf) {
    let workspace = TempDir::new().unwrap();
    let vault = workspace.path().join("vault");
    copy_tree(&homes_vault(), &vault);
    let note = vault.join("2026/20261016.md");
    let mut text = std::fs::read_to_string(&note).unwrap();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(concat!(
        "- [x] (**14:00 - 14:30** [off:: 30m]) — HACK\n",
        "  - [[bob#^t]]\n",
        "  - [[sase#^t]]\n",
        "- [x] (**15:00 - 15:20** [off:: 20m]) — MEET\n",
        "  - [[job#^t]]\n",
        "- [x] (**16:00 - 16:40** [off:: 40m]) — SASE + BLOG\n",
        "  - [[sase_blog#^t]]\n",
        "- [x] (**17:00 - 17:10** [off:: 10m]) — MISC\n",
    ));
    std::fs::write(&note, text).unwrap();
    let config = workspace.path().join("config.yml");
    std::fs::write(
        &config,
        [
            "stats:",
            "  metrics:",
            "    sase_time:",
            "      label: SASE time",
            "      kind: pomodoro",
            "      measure: minutes",
            "      themes: [SASE]",
            "    sase_in:",
            "      label: SASE in",
            "      kind: pomodoro",
            "      measure: minutes",
            "      in: [sase]",
            "    sase_count_in:",
            "      label: SASE session count",
            "      kind: pomodoro",
            "      measure: count",
            "      in: [sase]",
            "    exclude_sase:",
            "      label: Exclude sase",
            "      kind: pomodoro",
            "      measure: minutes",
            "      exclude_in: [sase]",
            "    gtd_daily_in:",
            "      label: gtd_daily in",
            "      kind: pomodoro",
            "      measure: minutes",
            "      in: [gtd_daily]",
            "    family_off:",
            "      label: Family off",
            "      kind: pomodoro",
            "      clock: off-clock",
            "    family_worked:",
            "      label: Family worked",
            "      kind: pomodoro",
            "      clock: worked",
            "    inner:",
            "      label: Inner",
            "      kind: formula",
            "      expr: worked",
            "      unit: minutes",
            "",
        ]
        .join("\n"),
    )
    .unwrap();
    (workspace, vault, config)
}

fn report(
    vault: &Path,
    config: &Path,
    clock: StatsClock,
    grouping: Grouping,
) -> StatsReport {
    let now = NaiveDate::from_ymd_opt(2026, 10, 18)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    let mut request =
        StatsRequest::at(vault.to_path_buf(), config.to_path_buf(), now);
    request.selection = MetricSelection::Explicit(
        IDS.into_iter().map(str::to_string).collect(),
    );
    request.range = Some("2026-10-14..2026-10-18".to_string());
    request.grouping = grouping;
    request.compare = true;
    request.clock = clock;
    compute(&request).expect("clock report computes")
}

fn metric<'a>(report: &'a StatsReport, id: &str) -> &'a MetricReport {
    report
        .metrics
        .iter()
        .find(|metric| metric.id == id)
        .unwrap_or_else(|| panic!("missing metric {id}"))
}

fn part(point: &SeriesPoint, dimension: &str, key: &str) -> i64 {
    point
        .parts
        .get(dimension)
        .and_then(|parts| parts.get(key))
        .copied()
        .unwrap_or(0)
}

/// One metric in the three lenses of the same request.
struct Trio<'a> {
    id: &'a str,
    total: &'a MetricReport,
    worked: &'a MetricReport,
    off: &'a MetricReport,
}

/// The per-day points behind a metric's complete-day summary: the daily
/// points of a grouped chart, otherwise the points themselves.
fn complete_days(metric: &MetricReport) -> &[SeriesPoint] {
    if metric.daily_points.is_empty() {
        &metric.points
    } else {
        &metric.daily_points
    }
}

/// Check the displayed points. A Total point must split into the values of
/// the other two lenses over the same tracked days, whole and part by part.
/// A bucket that straddles Off-clock adoption holds only the tracked days
/// in Total, so expectations come from the aligned daily points.
fn check_points(trio: &Trio<'_>) -> usize {
    let Trio {
        id,
        total,
        worked,
        off,
    } = trio;
    let (days_t, days_w, days_o) = (
        complete_days(total),
        complete_days(worked),
        complete_days(off),
    );
    assert_eq!(total.points.len(), worked.points.len(), "{id}");
    assert_eq!(total.points.len(), off.points.len(), "{id}");
    let mut splits = 0;
    for ((t, w), o) in total.points.iter().zip(&worked.points).zip(&off.points)
    {
        let at = format!("{id} {}..{}", t.date, t.end);
        assert_eq!((t.date, t.end), (w.date, w.end), "{at}");
        assert_eq!(t.provisional, w.provisional, "{at}");
        let rows: Vec<usize> = (0..days_t.len())
            .filter(|&index| {
                let day = &days_t[index];
                t.date <= day.date
                    && day.date <= t.end
                    && day.value.is_some()
                    && day.provisional == t.provisional
            })
            .collect();
        let sum_value = |days: &[SeriesPoint]| -> i64 {
            rows.iter().filter_map(|&index| days[index].value).sum()
        };
        let sum_part = |days: &[SeriesPoint], dimension: &str, key: &str| {
            rows.iter()
                .map(|&index| part(&days[index], dimension, key))
                .sum::<i64>()
        };
        match (&t.clock_breakdown, t.value) {
            (Some(split), Some(value)) => {
                splits += 1;
                assert!(!rows.is_empty(), "{at} has tracked days");
                assert_eq!(
                    split.worked.value + split.off_clock.value,
                    value,
                    "{at} adds up"
                );
                assert_eq!(
                    split.worked.value,
                    sum_value(days_w),
                    "{at} worked"
                );
                assert_eq!(
                    split.off_clock.value,
                    sum_value(days_o),
                    "{at} off"
                );
                assert_eq!(
                    t.off_clock.unwrap_or(0),
                    split.off_clock.value,
                    "{at} sub-share"
                );
                if rows.len() == 1 {
                    assert_eq!(
                        split.worked.display, days_w[rows[0]].display,
                        "{at} worked text"
                    );
                    assert_eq!(
                        split.off_clock.display, days_o[rows[0]].display,
                        "{at} off text"
                    );
                }
            }
            (None, None) => {
                assert!(t.parts_clock_breakdown.is_empty(), "{at}");
                assert!(t.off_clock.is_none(), "{at}");
            }
            (split, value) => {
                panic!("{at}: split {split:?} for value {value:?}")
            }
        }
        // The exact existing parts are split, and nothing else.
        let dims: Vec<_> = t.parts_clock_breakdown.keys().collect();
        if t.value.is_some() {
            assert_eq!(dims, t.parts.keys().collect::<Vec<_>>(), "{at} dims");
        } else {
            assert!(dims.is_empty(), "{at}");
        }
        for (dimension, entries) in &t.parts_clock_breakdown {
            assert_eq!(
                entries.keys().collect::<Vec<_>>(),
                t.parts[dimension].keys().collect::<Vec<_>>(),
                "{at} {dimension} keys"
            );
            for (key, split) in entries {
                let at = format!("{at} {dimension}/{key}");
                assert_eq!(
                    split.worked.value + split.off_clock.value,
                    t.parts[dimension][key],
                    "{at} adds up"
                );
                assert_eq!(
                    split.worked.value,
                    sum_part(days_w, dimension, key),
                    "{at} worked"
                );
                assert_eq!(
                    split.off_clock.value,
                    sum_part(days_o, dimension, key),
                    "{at} off"
                );
            }
        }
        // The Worked lens only splits when it left off-clock time out, with
        // the same off-clock amount the Off-clock lens reports.
        match &w.clock_breakdown {
            Some(split) => {
                assert_eq!(Some(split.worked.value), w.value, "{at}");
                assert_eq!(Some(split.off_clock.value), o.value, "{at}");
                assert_eq!(w.off_clock, Some(split.off_clock.value), "{at}");
                assert!(split.off_clock.value > 0, "{at}");
            }
            None => assert!(w.off_clock.is_none(), "{at}"),
        }
        assert!(w.parts_clock_breakdown.is_empty(), "{at}");
        assert!(o.clock_breakdown.is_none(), "{at}");
        assert!(o.parts_clock_breakdown.is_empty(), "{at}");
    }
    splits
}

/// Prior windows end before adoption in this vault: Total has no tracked
/// day there, so no point splits while the Worked lens still has values.
fn check_prior(trio: &Trio<'_>) {
    let Trio { id, total, .. } = trio;
    assert!(!total.prior_points.is_empty(), "{id} has prior points");
    for point in &total.prior_points {
        assert_eq!(point.value, None, "{id} prior {}", point.date);
        assert!(point.clock_breakdown.is_none(), "{id}");
        assert!(point.parts_clock_breakdown.is_empty(), "{id}");
    }
}

fn check_summaries(
    id: &str,
    total: &MetricReport,
    worked: &MetricReport,
    off: &MetricReport,
) {
    let total_of = |metric: &MetricReport| {
        metric.summary.total.as_ref().and_then(|total| total.value)
    };
    // Total is not tracked before adoption, so its complete days are the
    // tracked ones; the other lenses are summed over exactly those days.
    let tracked: Vec<usize> = complete_days(total)
        .iter()
        .enumerate()
        .filter(|(_, point)| !point.provisional && point.value.is_some())
        .map(|(index, _)| index)
        .collect();
    let over_tracked = |metric: &MetricReport| -> i64 {
        tracked
            .iter()
            .filter_map(|&index| complete_days(metric)[index].value)
            .sum()
    };
    match &total.summary.total_clock_breakdown {
        Some(split) => {
            assert_eq!(split.worked.value, over_tracked(worked), "{id}");
            assert_eq!(split.off_clock.value, over_tracked(off), "{id}");
            assert_eq!(
                Some(split.worked.value + split.off_clock.value),
                total_of(total),
                "{id} complete days add up"
            );
        }
        None => assert!(tracked.is_empty(), "{id}"),
    }
    match &worked.summary.total_clock_breakdown {
        Some(split) => {
            assert_eq!(Some(split.worked.value), total_of(worked), "{id}");
            assert_eq!(Some(split.off_clock.value), total_of(off), "{id}");
            assert!(split.off_clock.value > 0, "{id}");
        }
        None => assert_eq!(total_of(off).unwrap_or(0), 0, "{id}"),
    }
    assert!(off.summary.total_clock_breakdown.is_none(), "{id}");

    for (dimension, parts) in &total.parts_summary {
        for part in parts {
            let at = format!("{id} {dimension}/{}", part.key);
            let split = part.clock_breakdown.as_ref().expect(&at);
            assert_eq!(
                split.worked.value + split.off_clock.value,
                part.value,
                "{at} adds up"
            );
            let over_days = |metric: &MetricReport| -> i64 {
                tracked
                    .iter()
                    .map(|&index| {
                        self::part(
                            &complete_days(metric)[index],
                            dimension,
                            &part.key,
                        )
                    })
                    .sum()
            };
            assert_eq!(split.worked.value, over_days(worked), "{at} worked");
            assert_eq!(split.off_clock.value, over_days(off), "{at} off");
            assert_eq!(part.off_clock.unwrap_or(0), split.off_clock.value);
        }
    }
    for metric in [worked, off] {
        assert!(
            metric
                .parts_summary
                .values()
                .flatten()
                .all(|part| part.clock_breakdown.is_none()),
            "{id} only Total parts split"
        );
    }
}

fn check_grouping(grouping: Grouping) -> usize {
    let (_workspace, vault, config) = vault_and_config();
    let worked = report(&vault, &config, StatsClock::Worked, grouping);
    let off = report(&vault, &config, StatsClock::OffClock, grouping);
    let total = report(&vault, &config, StatsClock::Total, grouping);
    let mut splits = 0;
    for id in IDS {
        let (t, w, o) =
            (metric(&total, id), metric(&worked, id), metric(&off, id));
        let pomodoro = t.kind == "pomodoro";
        let effective = t
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.clock.as_deref());
        match (pomodoro, effective) {
            (false, _) => {
                // Formulas and tasks never follow the lens or split.
                for report in [t, w, o] {
                    assert!(report.summary.total_clock_breakdown.is_none());
                    assert!(report
                        .points
                        .iter()
                        .chain(&report.daily_points)
                        .chain(&report.prior_points)
                        .all(|point| point.clock_breakdown.is_none()
                            && point.parts_clock_breakdown.is_empty()));
                    assert!(report
                        .parts_summary
                        .values()
                        .flatten()
                        .all(|part| part.clock_breakdown.is_none()));
                }
            }
            (true, Some("off-clock")) => {
                // Pinned Off-clock metrics are the same in every lens and
                // carry no breakdown.
                for report in [t, w, o] {
                    assert!(report.summary.total_clock_breakdown.is_none());
                    assert!(report
                        .points
                        .iter()
                        .chain(&report.daily_points)
                        .chain(&report.prior_points)
                        .all(|point| point.clock_breakdown.is_none()
                            && point.parts_clock_breakdown.is_empty()));
                }
            }
            (true, Some("worked")) if id == "family_worked" => {
                // A Worked pin uses its own facts in every lens: it splits
                // exactly like the default Worked metric.
                let default = metric(&worked, "worked");
                for report in [t, w, o] {
                    assert_eq!(
                        report.summary.total_clock_breakdown,
                        default.summary.total_clock_breakdown
                    );
                    let splits: Vec<_> = report
                        .points
                        .iter()
                        .map(|point| point.clock_breakdown.clone())
                        .collect();
                    let expected: Vec<_> = default
                        .points
                        .iter()
                        .map(|point| point.clock_breakdown.clone())
                        .collect();
                    assert_eq!(splits, expected);
                }
            }
            _ => {
                let trio = Trio {
                    id,
                    total: t,
                    worked: w,
                    off: o,
                };
                splits += check_points(&trio);
                check_prior(&trio);
                // Grouped charts also carry the days behind each bucket.
                for ((day_t, day_w), day_o) in t
                    .daily_points
                    .iter()
                    .zip(&w.daily_points)
                    .zip(&o.daily_points)
                {
                    assert_eq!(day_t.date, day_w.date, "{id}");
                    assert_eq!(day_t.date, day_o.date, "{id}");
                    if let Some(split) = &day_t.clock_breakdown {
                        splits += 1;
                        assert_eq!(Some(split.worked.value), day_w.value);
                        assert_eq!(Some(split.off_clock.value), day_o.value);
                    }
                }
                check_summaries(id, t, w, o);
            }
        }
    }
    splits
}

#[test]
fn total_splits_match_the_independent_lenses_for_every_filter_and_grouping() {
    for grouping in [Grouping::Day, Grouping::Week, Grouping::Month] {
        let splits = check_grouping(grouping);
        assert!(splits > 20, "{grouping:?} exercised {splits} splits");
    }
}

#[test]
fn multi_home_mixed_day_splits_by_hand_derived_minutes() {
    let (_workspace, vault, config) = vault_and_config();
    let total = report(&vault, &config, StatsClock::Total, Grouping::Day);
    let worked = metric(&total, "worked");

    // Oct 14 and Oct 15 precede adoption (Oct 16): no value, no split.
    for day in [14, 15] {
        let point = &worked.points[day - 14];
        assert_eq!(
            point.date,
            NaiveDate::from_ymd_opt(2026, 10, day as u32).unwrap()
        );
        assert_eq!(point.value, None);
        assert_eq!(point.clock_breakdown, None);
        assert!(point.parts_clock_breakdown.is_empty());
    }

    // Oct 16: worked 132 (BLOG 50, HACK 30, JOB 7, MISC 15, PLAN+SASE 20,
    // unnamed 10) plus off-clock 100 (30 + 20 + 40 + 10).
    let busy = &worked.points[2];
    assert_eq!(busy.value, Some(232));
    let split = busy.clock_breakdown.as_ref().expect("Oct 16 split");
    assert_eq!((split.worked.value, split.off_clock.value), (132, 100));
    assert_eq!(split.worked.display, "2h 12m");
    assert_eq!(split.off_clock.display, "1h 40m");
    // HACK is worked and off-clock: the theme part holds both clocks.
    let hack = &busy.parts_clock_breakdown["theme"]["HACK"];
    assert_eq!((hack.worked.value, hack.off_clock.value), (30, 30));
    let meet = &busy.parts_clock_breakdown["theme"]["MEET"];
    assert_eq!((meet.worked.value, meet.off_clock.value), (0, 20));
    // The multi-home off-clock HACK (bob + sase) lands in both homes.
    let bob = &busy.parts_clock_breakdown["project"]["bob.md"];
    assert!(bob.off_clock.value > 0 && bob.worked.value > 0);

    // Oct 17 has a Pomodoros heading and no rows: explicit zeros.
    let quiet = &worked.points[3];
    assert_eq!(quiet.value, Some(0));
    let split = quiet.clock_breakdown.as_ref().expect("zero day split");
    assert_eq!((split.worked.value, split.off_clock.value), (0, 0));
    assert_eq!(split.worked.display, "0m");
    assert!(quiet.parts_clock_breakdown.is_empty());

    // Today (Oct 18) is provisional, worked-only, and still split.
    let live = &worked.points[4];
    assert!(live.provisional);
    let split = live.clock_breakdown.as_ref().expect("provisional split");
    assert_eq!((split.worked.value, split.off_clock.value), (15, 0));

    // Complete days only: today stays out of the summary split.
    let summary = worked.summary.total_clock_breakdown.as_ref().unwrap();
    assert_eq!((summary.worked.value, summary.off_clock.value), (132, 100));

    // Count metrics split in sessions: 6 worked rows and 4 off-clock rows.
    let sessions = metric(&total, "sessions");
    let split = sessions.points[2].clock_breakdown.as_ref().unwrap();
    assert_eq!((split.worked.value, split.off_clock.value), (6, 4));
    assert_eq!(split.worked.display, "6");
    assert_eq!(split.off_clock.display, "4");

    // A home filter and a theme filter use their own facts.
    let sase_in = metric(&total, "sase_in");
    let sase_split = sase_in.points[2].clock_breakdown.as_ref().unwrap();
    assert!(sase_split.off_clock.value > 0);
    assert!(sase_split.off_clock.value < split_value(busy));
    let sase_time = metric(&total, "sase_time");
    let themed = sase_time.points[2].clock_breakdown.as_ref().unwrap();
    // SASE is 20 worked (PLAN+SASE half is not in this filter) plus the
    // off-clock SASE + BLOG half; only SASE-themed minutes count.
    assert!(themed.off_clock.value > 0);
    assert!(themed.worked.value > 0);

    // The JSON shape is additive.
    let json = serde_json::to_value(busy).unwrap();
    assert_eq!(json["clock_breakdown"]["worked"]["value"], 132);
    assert_eq!(json["clock_breakdown"]["off_clock"]["display"], "1h 40m");
    assert_eq!(
        json["parts_clock_breakdown"]["theme"]["HACK"]["worked"]["value"],
        30
    );
    let summary = serde_json::to_value(&worked.summary).unwrap();
    assert_eq!(summary["total_clock_breakdown"]["off_clock"]["value"], 100);
    assert!(summary["headline"].get("worked").is_none());
}

fn split_value(point: &SeriesPoint) -> i64 {
    point.clock_breakdown.as_ref().unwrap().off_clock.value
}

#[test]
fn clockless_reports_and_pre_adoption_windows_stay_plain() {
    let workspace = TempDir::new().unwrap();
    let vault = workspace.path().join("vault");
    copy_tree(&homes_vault(), &vault);
    let config = workspace.path().join("config.yml");
    std::fs::write(&config, "stats:\n").unwrap();
    for clock in StatsClock::ALL {
        let report = report_for(&vault, &config, clock);
        let worked = metric(&report, "worked");
        assert_eq!(report.clock.since, None);
        assert!(worked.summary.total_clock_breakdown.is_none());
        assert!(worked
            .points
            .iter()
            .all(|point| point.clock_breakdown.is_none()
                && point.parts_clock_breakdown.is_empty()));
        assert!(worked
            .parts_summary
            .values()
            .flatten()
            .all(|part| part.clock_breakdown.is_none()));
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("clock_breakdown"), "{clock:?}");
    }
}

fn report_for(vault: &Path, config: &Path, clock: StatsClock) -> StatsReport {
    let now = NaiveDate::from_ymd_opt(2026, 10, 18)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    let mut request =
        StatsRequest::at(vault.to_path_buf(), config.to_path_buf(), now);
    request.selection = MetricSelection::Explicit(vec!["worked".to_string()]);
    request.range = Some("2026-10-14..2026-10-18".to_string());
    request.grouping = Grouping::Day;
    request.clock = clock;
    compute(&request).expect("plain report computes")
}

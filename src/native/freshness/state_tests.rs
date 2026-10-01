//! State conformance vectors S1–S15 from `docs/freshness.md`.
//! Today `2026-10-08`, config interval 7 unless noted.

use chrono::NaiveDate;

use super::{
    bucket_for_state, counts, evaluate, queue, Counts, FreshState,
    FreshnessRow, IntervalSource,
};
use crate::native::config::freshness::FreshnessConfig;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 8).expect("valid today")
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
}

fn default_config() -> FreshnessConfig {
    FreshnessConfig::default()
}

fn config_with_interval(days: u16) -> FreshnessConfig {
    FreshnessConfig {
        interval: days,
        rotten_daily_budget: None,
        interval_from_config: true,
        stale_budget_deprecated: false,
    }
}

fn config_with_budget(budget: u32) -> FreshnessConfig {
    FreshnessConfig {
        interval: 7,
        rotten_daily_budget: Some(budget),
        interval_from_config: false,
        stale_budget_deprecated: false,
    }
}

fn row(line: &str) -> FreshnessRow {
    FreshnessRow {
        path: "a.md".to_string(),
        line: 1,
        status: ' ',
        is_todo: true,
        recurring: false,
        lane_visible: true,
        is_daily_note: false,
        is_today: false,
        scheduled: None,
        created: None,
        raw_line: line.to_string(),
        note_refresh_raw: None,
    }
}

fn fresh_line(day: &str) -> String {
    format!("- [ ] #task Do it [fresh:: {day}]")
}

#[test]
fn s01_new_without_stamp() {
    let evaluated =
        evaluate(&row("- [ ] #task Do it"), today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::New));
    assert_eq!(evaluated.fresh, None);
    assert_eq!(evaluated.due_on, None);
    assert_eq!(evaluated.days_overdue, None);
}

#[test]
fn s02_fresh_with_due_on() {
    let evaluated =
        evaluate(&row(&fresh_line("2026-10-02")), today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::Fresh));
    assert_eq!(evaluated.due_on, Some(date(2026, 10, 9)));
    assert_eq!(evaluated.interval_days, 7);
    assert_eq!(evaluated.interval_source, IntervalSource::Default);
}

#[test]
fn s03_boundary_is_rotten_with_zero_overdue() {
    let evaluated =
        evaluate(&row(&fresh_line("2026-10-01")), today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::Rotten));
    assert_eq!(evaluated.due_on, Some(date(2026, 10, 8)));
    assert_eq!(evaluated.days_overdue, Some(0));
}

#[test]
fn s04_overdue_counts_days() {
    let evaluated =
        evaluate(&row(&fresh_line("2026-09-20")), today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::Rotten));
    assert_eq!(evaluated.due_on, Some(date(2026, 9, 27)));
    assert_eq!(evaluated.days_overdue, Some(11));
}

#[test]
fn s05_task_interval_beats_note() {
    let mut input =
        row("- [ ] #task Do it [fresh:: 2026-10-01] [refresh:: 14]");
    input.note_refresh_raw = Some("3".to_string());
    let evaluated = evaluate(&input, today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::Fresh));
    assert_eq!(evaluated.interval_days, 14);
    assert_eq!(evaluated.interval_source, IntervalSource::Task);
    assert_eq!(evaluated.due_on, Some(date(2026, 10, 15)));
}

#[test]
fn s06_note_interval_beats_config() {
    let mut input = row(&fresh_line("2026-10-05"));
    input.note_refresh_raw = Some("3".to_string());
    let evaluated = evaluate(&input, today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::Rotten));
    assert_eq!(evaluated.interval_days, 3);
    assert_eq!(evaluated.interval_source, IntervalSource::Note);
    assert_eq!(evaluated.due_on, Some(date(2026, 10, 8)));
}

#[test]
fn s07_config_interval() {
    let evaluated = evaluate(
        &row(&fresh_line("2026-10-01")),
        today(),
        &config_with_interval(10),
    );
    assert_eq!(evaluated.state, Some(FreshState::Fresh));
    assert_eq!(evaluated.interval_days, 10);
    assert_eq!(evaluated.interval_source, IntervalSource::Config);
    assert_eq!(evaluated.due_on, Some(date(2026, 10, 11)));
}

#[test]
fn s08_invalid_overrides_fall_through_with_lints() {
    let mut input = row("- [ ] #task Do it [fresh:: 2026-10-01] [refresh:: 0]");
    input.note_refresh_raw = Some("soon".to_string());
    let evaluated = evaluate(&input, today(), &default_config());
    assert!(evaluated.lints.contains(&"refresh_invalid".to_string()));
    assert!(evaluated
        .lints
        .contains(&"task_refresh_invalid".to_string()));
    assert_eq!(evaluated.interval_days, 7);
    assert_eq!(evaluated.interval_source, IntervalSource::Default);
    // 2026-10-01 + 7d is due today: rotten.
    assert_eq!(evaluated.state, Some(FreshState::Rotten));
}

#[test]
fn s09_malformed_fresh_is_new_with_lint() {
    let evaluated = evaluate(
        &row("- [ ] #task Do it [fresh:: 2026-13-01]"),
        today(),
        &default_config(),
    );
    assert_eq!(evaluated.state, Some(FreshState::New));
    assert_eq!(evaluated.fresh, None);
    assert!(evaluated.lints.contains(&"fresh_malformed".to_string()));
}

#[test]
fn s10_future_fresh_is_new_with_lint() {
    let evaluated =
        evaluate(&row(&fresh_line("2026-10-09")), today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::New));
    assert_eq!(evaluated.fresh, None);
    assert!(evaluated.lints.contains(&"fresh_future".to_string()));
}

#[test]
fn s11_resurfaced_when_schedule_returns_after_stamp() {
    let mut input = row(&fresh_line("2026-10-05"));
    input.scheduled = Some(date(2026, 10, 7));
    let evaluated = evaluate(&input, today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::Resurfaced));
    assert_eq!(evaluated.due_on, Some(date(2026, 10, 7)));
}

#[test]
fn s12_equal_schedule_is_not_resurfaced() {
    let mut input = row(&fresh_line("2026-10-07"));
    input.scheduled = Some(date(2026, 10, 7));
    let evaluated = evaluate(&input, today(), &default_config());
    assert_eq!(evaluated.state, Some(FreshState::Fresh));
}

#[test]
fn s13_out_of_scope_is_null() {
    let fresh = fresh_line("2026-10-02");
    // Recurring.
    let mut recurring = row(&fresh);
    recurring.recurring = true;
    // A `[?]` with a future scheduled: not a TODO status type.
    let mut blocked_future = row(
        "- [?] #task Deferred [fresh:: 2026-10-02] [scheduled:: 2026-10-20]",
    );
    blocked_future.status = '?';
    blocked_future.is_todo = false;
    blocked_future.scheduled = Some(date(2026, 10, 20));
    // `[*]` and `[/]`: not TODO.
    let mut on_hold = row("- [*] #task Next-ish [fresh:: 2026-10-02]");
    on_hold.status = '*';
    on_hold.is_todo = false;
    let mut in_progress = row("- [/] #task Started [fresh:: 2026-10-02]");
    in_progress.status = '/';
    in_progress.is_todo = false;
    // `#hide`: not lane-visible.
    let mut hidden = row(&fresh);
    hidden.lane_visible = false;
    // Under `_templates`: not lane-visible.
    let mut templated = row(&fresh);
    templated.path = "_templates/x.md".to_string();
    templated.lane_visible = false;
    // Canonical daily note.
    let mut daily = row(&fresh);
    daily.path = "2026/20261008.md".to_string();
    daily.is_daily_note = true;
    // Linked under today's open Pomodoro.
    let mut today_member = row(&fresh);
    today_member.is_today = true;
    // A `[ ]` whose `dependsOn` names an open task: blocked, so not
    // lane-visible.
    let mut blocked_dep =
        row("- [ ] #task Gated [fresh:: 2026-10-02] [dependsOn:: abc]");
    blocked_dep.lane_visible = false;

    for (name, candidate) in [
        ("recurring", recurring),
        ("blocked-future", blocked_future),
        ("on-hold", on_hold),
        ("in-progress", in_progress),
        ("hidden", hidden),
        ("templates", templated),
        ("daily", daily),
        ("today", today_member),
        ("blocked-dep", blocked_dep),
    ] {
        let evaluated = evaluate(&candidate, today(), &default_config());
        assert_eq!(evaluated.state, None, "{name} must be out of scope");
    }
}

#[test]
fn s14_queue_order_new_then_due() {
    let config = default_config();
    let new_b = FreshnessRow {
        path: "b.md".to_string(),
        line: 3,
        ..row("- [ ] #task New bee")
    };
    let new_a = FreshnessRow {
        path: "a.md".to_string(),
        line: 9,
        ..row("- [ ] #task New aye")
    };
    let rotten_c = FreshnessRow {
        path: "c.md".to_string(),
        line: 2,
        ..row("- [ ] #task Old [fresh:: 2026-09-24]")
    };
    let mut resurfaced_a = FreshnessRow {
        path: "a.md".to_string(),
        line: 4,
        ..row("- [ ] #task Back [fresh:: 2026-10-05]")
    };
    resurfaced_a.scheduled = Some(date(2026, 10, 7));
    let rotten_a = FreshnessRow {
        path: "a.md".to_string(),
        line: 2,
        ..row("- [ ] #task Older [fresh:: 2026-09-30]")
    };
    // Sanity on the crafted due dates: c.md:2 due 2026-10-01, a.md:2
    // due 2026-10-07, a.md:4 resurfaced due 2026-10-07.
    let ordered = queue(
        &[new_b, new_a, rotten_c, resurfaced_a, rotten_a],
        today(),
        &config,
    );
    let keys: Vec<(String, u32)> = ordered
        .iter()
        .map(|entry| (entry.path.clone(), entry.line))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("a.md".to_string(), 9),
            ("b.md".to_string(), 3),
            ("c.md".to_string(), 2),
            ("a.md".to_string(), 2),
            ("a.md".to_string(), 4),
        ]
    );
    assert_eq!(
        ordered.iter().map(|entry| entry.rank).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        ordered.iter().map(|entry| entry.tier).collect::<Vec<_>>(),
        vec!["new", "new", "due", "due", "due"]
    );
}

fn stamped_row(
    path: &str,
    line: u32,
    status: char,
    is_todo: bool,
    fresh: &str,
) -> FreshnessRow {
    FreshnessRow {
        path: path.to_string(),
        line,
        status,
        is_todo,
        recurring: false,
        lane_visible: is_todo,
        is_daily_note: false,
        is_today: false,
        scheduled: None,
        created: None,
        raw_line: format!("- [{status}] #task Counted [fresh:: {fresh}]"),
        note_refresh_raw: None,
    }
}

/// Bucket conformance (`docs/freshness.md` §4): S1 maps to `new`;
/// S3, S4, and S11 map to `rotten`; S2, S5, S7, and S12 map to no
/// bucket; every S13 row maps to no bucket.
#[test]
fn bucket_partition_vectors() {
    let config = default_config();
    let bucketed = |input: &FreshnessRow| {
        let evaluated = evaluate(input, today(), &config);
        bucket_for_state(evaluated.state)
    };

    // S1 new.
    assert_eq!(bucketed(&row("- [ ] #task Do it")), Some("new"));

    // S3 boundary, S4 overdue, S11 resurfaced: rotten.
    assert_eq!(bucketed(&row(&fresh_line("2026-10-01"))), Some("rotten"));
    assert_eq!(bucketed(&row(&fresh_line("2026-09-20"))), Some("rotten"));
    let mut resurfaced = row(&fresh_line("2026-10-05"));
    resurfaced.scheduled = Some(date(2026, 10, 7));
    assert_eq!(bucketed(&resurfaced), Some("rotten"));

    // S2 fresh, S5 task interval, S7 config interval, S12 equal
    // schedule: no bucket.
    assert_eq!(bucketed(&row(&fresh_line("2026-10-02"))), None);
    let mut task_interval =
        row("- [ ] #task Do it [fresh:: 2026-10-01] [refresh:: 14]");
    task_interval.note_refresh_raw = Some("3".to_string());
    assert_eq!(bucketed(&task_interval), None);
    // S7's date is rotten under the default interval (see S3 above);
    // the null case needs the config interval.
    let s7 = evaluate(
        &row(&fresh_line("2026-10-01")),
        today(),
        &config_with_interval(10),
    );
    assert_eq!(s7.state, Some(FreshState::Fresh));
    assert_eq!(bucket_for_state(s7.state), None);
    let mut equal_schedule = row(&fresh_line("2026-10-07"));
    equal_schedule.scheduled = Some(date(2026, 10, 7));
    assert_eq!(bucketed(&equal_schedule), None);

    // Every S13 out-of-scope row: no bucket, and a null bucket alone
    // never proves Ready.
    let mut hidden = row(&fresh_line("2026-10-02"));
    hidden.lane_visible = false;
    let mut today_member = row(&fresh_line("2026-10-02"));
    today_member.is_today = true;
    let mut recurring = row(&fresh_line("2026-10-02"));
    recurring.recurring = true;
    for candidate in [&hidden, &today_member, &recurring] {
        let evaluated = evaluate(candidate, today(), &config);
        assert_eq!(evaluated.state, None);
        assert_eq!(bucket_for_state(evaluated.state), None);
    }
    assert_eq!(bucket_for_state(None), None);
    assert_eq!(FreshState::Fresh.bucket(), None);
}

#[test]
fn s15_counts_and_budget_meter() {
    // 13 in-scope Ready tasks stamped today plus one [*] and one [x]
    // stamped today: refreshed_today counts all 15. A Ready task
    // stamped yesterday does not count.
    let mut rows = Vec::new();
    for index in 0..13 {
        rows.push(stamped_row("a.md", index + 1, ' ', true, "2026-10-08"));
    }
    rows.push(stamped_row("b.md", 1, '*', false, "2026-10-08"));
    rows.push(stamped_row("c.md", 1, 'x', false, "2026-10-08"));
    rows.push(stamped_row("a.md", 20, ' ', true, "2026-10-07"));

    let met: Counts = counts(&rows, today(), &config_with_budget(15));
    assert_eq!(met.refreshed_today, 15);
    assert_eq!(met.new, 0);
    assert_eq!(met.budget, Some(15));
    assert!(met.budget_met);

    // One NEW capture unmeets the budget even at 15 refreshed.
    let mut with_new = rows.clone();
    with_new.push(row("- [ ] #task Fresh capture"));
    let unmet: Counts = counts(&with_new, today(), &config_with_budget(15));
    assert_eq!(unmet.refreshed_today, 15);
    assert_eq!(unmet.new, 1);
    assert!(!unmet.budget_met);
}

//! State conformance vectors S1–S15 from `docs/freshness.md`.
//! Today `2026-10-08`, config interval 7 unless noted.

use chrono::NaiveDate;

use super::{
    bucket_for_state, counts, evaluate, queue, Counts, FreshState,
    FreshnessRow, IntervalSource, Lane, Tier,
};
use crate::native::config::freshness::{DecayConfig, FreshnessConfig};

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
        pending_interval: Some(1),
        next_interval: Some(1),
        rotten_daily_budget: None,
        interval_from_config: true,
        stale_budget_deprecated: false,
        decay: DecayConfig::default(),
    }
}

fn config_with_budget(budget: u32) -> FreshnessConfig {
    FreshnessConfig {
        interval: 7,
        pending_interval: Some(1),
        next_interval: Some(1),
        rotten_daily_budget: Some(budget),
        interval_from_config: false,
        stale_budget_deprecated: false,
        decay: DecayConfig::default(),
    }
}

fn config_with_lanes(
    pending: Option<u16>,
    next: Option<u16>,
) -> FreshnessConfig {
    FreshnessConfig {
        interval: 7,
        pending_interval: pending,
        next_interval: next,
        rotten_daily_budget: None,
        interval_from_config: false,
        stale_budget_deprecated: false,
        decay: DecayConfig::default(),
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
            ("a.md".to_string(), 4),
            ("c.md".to_string(), 2),
            ("a.md".to_string(), 2),
        ]
    );
    assert_eq!(
        ordered.iter().map(|entry| entry.rank).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        ordered
            .iter()
            .map(|entry| entry.tier.as_str())
            .collect::<Vec<_>>(),
        vec!["new", "new", "returned", "rotten", "rotten"]
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
    // 14 in-scope Ready tasks stamped today plus one [*] and one [x]
    // stamped today: refreshed_today counts all 16, upkeep_today
    // counts the 14 Ready plus the [x] but not the [*]. A Ready task
    // stamped yesterday counts for neither.
    let mut rows = Vec::new();
    for index in 0..14 {
        rows.push(stamped_row("a.md", index + 1, ' ', true, "2026-10-08"));
    }
    rows.push(stamped_row("b.md", 1, '*', false, "2026-10-08"));
    rows.push(stamped_row("c.md", 1, 'x', false, "2026-10-08"));
    rows.push(stamped_row("a.md", 20, ' ', true, "2026-10-07"));

    let met: Counts = counts(&rows, today(), &config_with_budget(15));
    assert_eq!(met.refreshed_today, 16);
    assert_eq!(met.upkeep_today, 15);
    assert_eq!(met.new, 0);
    assert_eq!(met.budget, Some(15));
    assert!(met.budget_met);

    // One NEW capture unmeets the budget even at 15 upkeep.
    let mut with_new = rows.clone();
    with_new.push(row("- [ ] #task Fresh capture"));
    let unmet: Counts = counts(&with_new, today(), &config_with_budget(15));
    assert_eq!(unmet.refreshed_today, 16);
    assert_eq!(unmet.upkeep_today, 15);
    assert_eq!(unmet.new, 1);
    assert!(!unmet.budget_met);
}

/// Build a lane row: `status` is the Tasks symbol (`/`, `*`, ` `,
/// `?`, `x`), `fresh` the stamp day or `None` for never stamped.
fn lane_row(
    path: &str,
    line: u32,
    status: char,
    fresh: Option<&str>,
    created: Option<NaiveDate>,
) -> FreshnessRow {
    let is_todo = status == ' ';
    let raw_line = match fresh {
        Some(day) => format!("- [{status}] #task Walk [fresh:: {day}]"),
        None => format!("- [{status}] #task Walk"),
    };
    FreshnessRow {
        path: path.to_string(),
        line,
        status,
        is_todo,
        recurring: false,
        lane_visible: true,
        is_daily_note: false,
        is_today: false,
        scheduled: None,
        created,
        raw_line,
        note_refresh_raw: None,
    }
}

fn ready_row(
    path: &str,
    line: u32,
    fresh: Option<&str>,
    created: Option<NaiveDate>,
) -> FreshnessRow {
    lane_row(path, line, ' ', fresh, created)
}

fn queue_keys(entries: &[super::QueueEntry]) -> Vec<(String, u32)> {
    entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.line))
        .collect()
}

fn queue_tiers(entries: &[super::QueueEntry]) -> Vec<&'static str> {
    entries.iter().map(|entry| entry.tier.as_str()).collect()
}

#[test]
fn q1_bryan_example_orders_adjc() {
    // All four tasks are ROTTEN (today 2026-10-08, interval 7
    // unless the task refresh says otherwise).
    let config = default_config();
    let mut d =
        ready_row("d.md", 1, Some("2026-10-07"), Some(date(2026, 9, 1)));
    d.raw_line = "- [ ] #task A [fresh:: 2026-10-07] [refresh:: 1]".to_string();
    let c = ready_row("c.md", 1, Some("2026-09-28"), Some(date(2026, 9, 4)));
    let b = ready_row("b.md", 1, Some("2026-09-28"), Some(date(2026, 9, 1)));
    let a = ready_row("a.md", 1, Some("2026-09-30"), Some(date(2026, 9, 1)));
    let ordered = queue(&[d, c, b, a], today(), &config);
    assert_eq!(
        queue_keys(&ordered),
        vec![
            ("d.md".to_string(), 1),
            ("c.md".to_string(), 1),
            ("b.md".to_string(), 1),
            ("a.md".to_string(), 1),
        ]
    );
    assert_eq!(queue_tiers(&ordered), vec!["rotten"; 4]);
    // d.md:1 runs on its 1-day task interval.
    assert_eq!(ordered[0].interval_days, 1);
    assert_eq!(ordered[0].due_on, Some(date(2026, 10, 8)));
}

#[test]
fn q2_tier_order_beats_path_order() {
    let config = default_config();
    let new = ready_row("e.md", 1, None, None);
    let pending = lane_row("d.md", 1, '/', Some("2026-10-07"), None);
    let next = lane_row("c.md", 1, '*', Some("2026-10-07"), None);
    let mut returned = ready_row("b.md", 1, Some("2026-10-05"), None);
    returned.scheduled = Some(date(2026, 10, 7));
    let rotten = ready_row("a.md", 1, Some("2026-09-20"), None);
    let ordered =
        queue(&[rotten, returned, next, pending, new], today(), &config);
    assert_eq!(
        queue_keys(&ordered),
        vec![
            ("e.md".to_string(), 1),
            ("d.md".to_string(), 1),
            ("c.md".to_string(), 1),
            ("b.md".to_string(), 1),
            ("a.md".to_string(), 1),
        ]
    );
    assert_eq!(
        queue_tiers(&ordered),
        vec!["new", "pending", "next", "returned", "rotten"]
    );
}

#[test]
fn l1_lane_due_and_stamped_today() {
    let config = default_config();
    // Stamped today: in no tier.
    let today_row = lane_row("a.md", 1, '*', Some("2026-10-08"), None);
    let evaluated = evaluate(&today_row, today(), &config);
    assert_eq!(evaluated.state, None);
    assert_eq!(evaluated.tier, None);
    assert_eq!(evaluated.lane, Some(Lane::Next));
    // Stamped yesterday: tier next, due today.
    let due = lane_row("a.md", 2, '*', Some("2026-10-07"), None);
    let evaluated = evaluate(&due, today(), &config);
    assert_eq!(evaluated.state, None);
    assert_eq!(bucket_for_state(evaluated.state), None);
    assert_eq!(evaluated.tier, Some(Tier::Next));
    assert_eq!(evaluated.lane, Some(Lane::Next));
    assert_eq!(evaluated.due_on, Some(date(2026, 10, 8)));
    assert_eq!(evaluated.days_overdue, Some(0));
    assert_eq!(evaluated.interval_days, 1);
    assert_eq!(evaluated.interval_source, IntervalSource::Next);
}

#[test]
fn l2_lane_overrides_refresh() {
    let config = default_config();
    let mut input = lane_row("a.md", 1, '/', Some("2026-10-07"), None);
    input.raw_line =
        "- [/] #task Lane [fresh:: 2026-10-07] [refresh:: 30]".to_string();
    let evaluated = evaluate(&input, today(), &config);
    assert_eq!(evaluated.tier, Some(Tier::Pending));
    assert_eq!(evaluated.interval_days, 1);
    assert_eq!(evaluated.interval_source, IntervalSource::Pending);
}

#[test]
fn l3_lane_exclusions_have_no_tier() {
    let config = default_config();
    let fresh = Some("2026-10-07");
    let mut recurring = lane_row("a.md", 1, '*', fresh, None);
    recurring.recurring = true;
    let mut today_member = lane_row("a.md", 2, '*', fresh, None);
    today_member.is_today = true;
    let mut daily = lane_row("2026/20261008.md", 1, '*', fresh, None);
    daily.is_daily_note = true;
    let mut hidden = lane_row("a.md", 3, '*', fresh, None);
    hidden.lane_visible = false;
    for (name, candidate) in [
        ("recurring", recurring),
        ("today", today_member),
        ("daily", daily),
        ("hidden", hidden),
    ] {
        let evaluated = evaluate(&candidate, today(), &config);
        assert_eq!(evaluated.tier, None, "{name} must be in no tier");
        assert_eq!(evaluated.state, None, "{name} keeps a null state");
    }
}

#[test]
fn l4_lane_off_switch_and_null_default() {
    // With next_interval: false, a never-stamped [*] is in no tier
    // and its interval falls back to the Ready chain (7, default).
    let off = config_with_lanes(Some(1), None);
    let never = lane_row("a.md", 1, '*', None, None);
    let evaluated = evaluate(&never, today(), &off);
    assert_eq!(evaluated.tier, None);
    assert_eq!(evaluated.interval_days, 7);
    assert_eq!(evaluated.interval_source, IntervalSource::Default);
    // An explicit null means the default 1.
    let null_case = config_with_lanes(Some(1), Some(1));
    let evaluated = evaluate(&never, today(), &null_case);
    assert_eq!(evaluated.tier, Some(Tier::Next));
    assert_eq!(evaluated.interval_days, 1);
}

#[test]
fn l5_lane_order_never_stamped_first() {
    let config = default_config();
    let z = lane_row("z.md", 9, '/', None, Some(date(2026, 9, 1)));
    let b =
        lane_row("b.md", 1, '/', Some("2026-10-01"), Some(date(2026, 9, 15)));
    let a5 =
        lane_row("a.md", 5, '/', Some("2026-10-07"), Some(date(2026, 9, 10)));
    let a2 =
        lane_row("a.md", 2, '/', Some("2026-10-07"), Some(date(2026, 9, 20)));
    let a1 = lane_row("a.md", 1, '/', Some("2026-10-07"), None);
    let ordered = queue(&[a1, a2, a5, b, z], today(), &config);
    assert_eq!(
        queue_keys(&ordered),
        vec![
            ("z.md".to_string(), 9),
            ("b.md".to_string(), 1),
            ("a.md".to_string(), 5),
            ("a.md".to_string(), 2),
            ("a.md".to_string(), 1),
        ]
    );
    assert_eq!(queue_tiers(&ordered), vec!["pending"; 5]);
}

#[test]
fn r1_returned_beats_older_rotten() {
    let config = default_config();
    let mut returned = ready_row("b.md", 1, Some("2026-10-05"), None);
    returned.scheduled = Some(date(2026, 10, 7));
    let rotten = ready_row("a.md", 1, Some("2026-09-20"), None);
    let ordered = queue(&[rotten, returned], today(), &config);
    assert_eq!(
        queue_keys(&ordered),
        vec![("b.md".to_string(), 1), ("a.md".to_string(), 1)]
    );
    assert_eq!(queue_tiers(&ordered), vec!["returned", "rotten"]);
}

#[test]
fn r2_returned_orders_by_schedule_then_newest_created() {
    let config = default_config();
    let mut x =
        ready_row("x.md", 1, Some("2026-10-05"), Some(date(2026, 9, 1)));
    x.scheduled = Some(date(2026, 10, 6));
    let mut w =
        ready_row("w.md", 1, Some("2026-10-05"), Some(date(2026, 9, 5)));
    w.scheduled = Some(date(2026, 10, 7));
    let mut y =
        ready_row("y.md", 1, Some("2026-10-05"), Some(date(2026, 9, 1)));
    y.scheduled = Some(date(2026, 10, 7));
    let ordered = queue(&[y, w, x], today(), &config);
    assert_eq!(
        queue_keys(&ordered),
        vec![
            ("x.md".to_string(), 1),
            ("w.md".to_string(), 1),
            ("y.md".to_string(), 1),
        ]
    );
}

#[test]
fn missing_created_sorts_after_dated_peers() {
    let config = default_config();
    // Ascending (pending): dated first, missing last.
    let dated =
        lane_row("a.md", 1, '/', Some("2026-10-07"), Some(date(2026, 9, 1)));
    let missing = lane_row("a.md", 2, '/', Some("2026-10-07"), None);
    let ordered = queue(&[missing, dated], today(), &config);
    assert_eq!(
        queue_keys(&ordered),
        vec![("a.md".to_string(), 1), ("a.md".to_string(), 2)]
    );
    // Descending (rotten): dated first (newest), missing last.
    let old = ready_row("a.md", 3, Some("2026-09-28"), Some(date(2026, 9, 1)));
    let new = ready_row("a.md", 4, Some("2026-09-28"), Some(date(2026, 9, 4)));
    let missing = ready_row("a.md", 5, Some("2026-09-28"), None);
    let ordered = queue(&[missing, old, new], today(), &config);
    assert_eq!(
        queue_keys(&ordered),
        vec![
            ("a.md".to_string(), 4),
            ("a.md".to_string(), 3),
            ("a.md".to_string(), 5),
        ]
    );
}

#[test]
fn b1_upkeep_counts_outside_the_lanes() {
    // 20 lane stamps plus 5 Ready stamps today, budget 15:
    // upkeep 5, refreshed 25, budget not met.
    let mut rows = Vec::new();
    for index in 0..10 {
        rows.push(stamped_row("lane.md", index + 1, '/', false, "2026-10-08"));
    }
    for index in 0..10 {
        rows.push(stamped_row("lane.md", index + 11, '*', false, "2026-10-08"));
    }
    for index in 0..5 {
        rows.push(stamped_row("a.md", index + 1, ' ', true, "2026-10-08"));
    }
    let budget = FreshnessConfig {
        interval: 7,
        pending_interval: Some(1),
        next_interval: Some(1),
        rotten_daily_budget: Some(15),
        interval_from_config: false,
        stale_budget_deprecated: false,
        decay: DecayConfig::default(),
    };
    let report: Counts = counts(&rows, today(), &budget);
    assert_eq!(report.refreshed_today, 25);
    assert_eq!(report.upkeep_today, 5);
    assert!(!report.budget_met);
    // A Blocked [?] and an [x] stamped today are upkeep too.
    let mut more = rows.clone();
    more.push(stamped_row("b.md", 1, '?', false, "2026-10-08"));
    more.push(stamped_row("c.md", 1, 'x', false, "2026-10-08"));
    let report: Counts = counts(&more, today(), &budget);
    assert_eq!(report.refreshed_today, 27);
    assert_eq!(report.upkeep_today, 7);
}

/// A post-activation date: the trial ends 2026-10-18, so 2026-10-20
/// is active (`docs/freshness.md` §2a).
fn active_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 20).expect("valid active day")
}

fn decay_config(keeps: u16, enabled: bool) -> FreshnessConfig {
    FreshnessConfig {
        interval: 7,
        pending_interval: Some(1),
        next_interval: Some(1),
        rotten_daily_budget: None,
        interval_from_config: false,
        stale_budget_deprecated: false,
        decay: DecayConfig {
            enabled,
            keeps,
            enter: None,
        },
    }
}

fn kept_row(keeps: u32) -> FreshnessRow {
    row(&format!(
        "- [ ] #task Kept [fresh:: 2026-10-01] [keeps:: {keeps}]"
    ))
}

/// D1: before activation there is no decision — count and show pips
/// only — even at the limit.
#[test]
fn decide_silent_before_activation() {
    let evaluated = evaluate(&kept_row(3), today(), &decay_config(3, true));
    assert_eq!(evaluated.tier, Some(Tier::Rotten));
    assert_eq!(evaluated.keeps, 3);
    assert!(!evaluated.decide);
}

/// D2/D3: after activation a due Ready row at the limit decides;
/// below it only stamps.
#[test]
fn decide_at_limit_but_not_below() {
    let config = decay_config(3, true);
    let at_limit = evaluate(&kept_row(3), active_day(), &config);
    assert_eq!(at_limit.tier, Some(Tier::Rotten));
    assert!(at_limit.decide);
    let below = evaluate(&kept_row(2), active_day(), &config);
    assert_eq!(below.tier, Some(Tier::Rotten));
    assert!(!below.decide);
    // The annotation is read-time only: the row still queues.
    let queued = queue(&[kept_row(3), kept_row(2)], active_day(), &config);
    assert_eq!(queued.len(), 2);
    assert!(queued[0].decide);
    assert!(!queued[1].decide);
}

/// D4/D5: RETURNED at the limit decides; NEW never does.
#[test]
fn decide_covers_returned_but_never_new() {
    let config = decay_config(3, true);
    let mut returned = row(
        "- [ ] #task Week habits [fresh:: 2026-10-05] [scheduled:: 2026-10-07] [keeps:: 5]",
    );
    returned.scheduled = Some(date(2026, 10, 7));
    let evaluated = evaluate(&returned, active_day(), &config);
    assert_eq!(evaluated.tier, Some(Tier::Returned));
    assert!(evaluated.decide);
    let new = evaluate(
        &row("- [ ] #task New capture [keeps:: 3]"),
        active_day(),
        &config,
    );
    assert_eq!(new.tier, Some(Tier::New));
    assert!(!new.decide);
}

/// D6/D7/D8: a zero limit asks on every due Ready re-confirmation;
/// `decay: false` never asks; lane rows never decide.
#[test]
fn decide_zero_off_and_lane_rows() {
    let rotten = kept_row(0);
    let zero = evaluate(&rotten, active_day(), &decay_config(0, true));
    assert_eq!(zero.tier, Some(Tier::Rotten));
    assert!(zero.decide);
    let off = evaluate(&kept_row(9), active_day(), &decay_config(3, false));
    assert_eq!(off.tier, Some(Tier::Rotten));
    assert!(!off.decide);
    let lane = evaluate(
        &lane_row("c.md", 1, '*', Some("2026-09-20"), None),
        active_day(),
        &decay_config(3, true),
    );
    assert_eq!(lane.tier, Some(Tier::Next));
    assert!(!lane.decide);
}

/// Counts carry `decide` over the full queue input.
#[test]
fn counts_carry_decide() {
    let config = decay_config(3, true);
    let report = counts(&[kept_row(3), kept_row(2)], active_day(), &config);
    assert_eq!(report.walk, 2);
    assert_eq!(report.decide, 1);
    assert_eq!(report.rotten, 2);
    let silent = counts(&[kept_row(3)], today(), &config);
    assert_eq!(silent.decide, 0);
}

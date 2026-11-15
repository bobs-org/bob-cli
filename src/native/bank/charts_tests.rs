//! `bob bank report -c` aggregate: invariants, states, folding, targets,
//! and bounds.

use std::collections::{BTreeSet, HashMap};

use chrono::NaiveDate;

use super::*;
use crate::native::bank::{
    config::{parse_bank_config, ConfigParse},
    grammar::render_day_heading,
    journal::JournalSnapshot,
    report::{all_time_period, resolve_period, to_json, BankReport},
};

type Headings = HashMap<(String, NaiveDate), (String, usize)>;

const HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-10-01\n    pocket:\n      label: Pocket cash\n      kind: cash\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n    fuel: ⛽ Fuel\n    books: 📚 Books\n    gifts: 🎁 Gifts\n    pets: 🐶 Pets\n    games: 🎲 Games\n---\n\n# 🏦 Bank\n";

fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("test date")
}

fn today() -> NaiveDate {
    date("2026-11-12")
}

fn config_from(hub: &str) -> BankConfig {
    match parse_bank_config(hub) {
        ConfigParse::Ok(config) => config,
        other => panic!("expected a valid hub, got {other:?}"),
    }
}

fn config() -> BankConfig {
    config_from(HUB)
}

fn hub_starting(start: &str) -> String {
    HUB.replace("start: 2026-10-01", &format!("start: {start}"))
}

fn row(
    day: &str,
    cents: i64,
    category: Option<&str>,
    account: &str,
    acknowledged: bool,
) -> BankRow {
    let date = date(day);
    BankRow {
        path: month_path(month_of(date)),
        line: 10,
        date,
        heading: render_day_heading(date),
        account: account.to_owned(),
        cents,
        payee: "Payee".to_owned(),
        category: category.map(str::to_owned),
        links: Vec::new(),
        note: None,
        flagged: false,
        acknowledged,
        id: None,
        origin: "typed",
        suggestion: None,
        subscription: None,
        bank_recurring: false,
        subscription_candidate: false,
    }
}

fn paid(day: &str, cents: i64, category: Option<&str>) -> BankRow {
    row(day, cents, category, "checking", true)
}

fn waiting(day: &str, cents: i64, category: Option<&str>) -> BankRow {
    row(day, cents, category, "checking", false)
}

/// Rows spanning ISO 2026 and its neighbours, with every excluded-money
/// shape on Nov 10 and Nov 11.
fn ledger() -> Vec<BankRow> {
    vec![
        paid("2025-12-20", -1_200, Some("groceries")),
        paid("2025-12-29", -500, Some("dining")),
        paid("2026-01-04", 100_000, Some("income")),
        paid("2026-03-30", -4_000, Some("fuel")),
        paid("2026-06-28", -800, Some("books")),
        paid("2026-09-28", -10_000, Some("groceries")),
        waiting("2026-11-09", -2_000, Some("groceries")),
        paid("2026-11-10", -50_000, Some("transfer")),
        paid("2026-11-10", 50_000, Some("transfer")),
        row("2026-11-11", -4_000, None, "pocket", true),
        paid("2026-11-11", -6_000, Some("cash")),
        paid("2026-11-11", 1_500, Some("refund")),
        paid("2026-11-11", 999, None),
        paid("2026-11-11", 0, Some("dining")),
        paid("2026-11-11", -300, Some("dining")),
        waiting("2026-11-11", -700, None),
        waiting("2026-11-12", -24_977, Some("groceries")),
        paid("2026-11-12", 250_000, Some("income")),
        paid("2026-12-28", -3_000, Some("gifts")),
        paid("2027-01-03", -1_100, Some("pets")),
        paid("2027-01-04", -900, Some("games")),
    ]
}

fn report_at(
    config: &BankConfig,
    rows: &[BankRow],
    mut period: Period,
    journal: &JournalSnapshot,
    headings: &Headings,
) -> BankReport {
    if period.kind == PeriodKind::AllTime {
        period = all_time_period(Some(config), rows, today());
    }
    crate::native::bank::report::build(
        rows.to_vec(),
        today(),
        period,
        Some(config),
        config.categories.clone(),
        Vec::new(),
        "checking".to_owned(),
        headings,
        journal,
        true,
        true,
    )
}

fn strip_period(raw: &str) -> Period {
    resolve_period(None, None, None, None, Some(raw), today())
        .unwrap_or_else(|error| panic!("-p {raw}: {error}"))
}

fn report_for(raw: &str) -> BankReport {
    report_at(
        &config(),
        &ledger(),
        strip_period(raw),
        &JournalSnapshot::default(),
        &Headings::new(),
    )
}

fn charts_of(report: &BankReport) -> &Charts {
    report
        .charts
        .as_ref()
        .and_then(Option::as_ref)
        .expect("a charts aggregate")
}

fn assert_partition(buckets: &[Bucket], period: &Period) {
    if period.start > period.end {
        assert!(buckets.is_empty());
        return;
    }
    assert_eq!(buckets.first().expect("buckets").start, period.start);
    assert_eq!(buckets.last().expect("buckets").end, period.end);
    for bucket in buckets {
        assert!(bucket.start <= bucket.end, "{}", bucket.id);
    }
    for pair in buckets.windows(2) {
        assert_eq!(
            pair[0].end.succ_opt(),
            Some(pair[1].start),
            "{} then {}",
            pair[0].id,
            pair[1].id
        );
    }
}

/// Every invariant from the contract, for buckets and (when present)
/// groups.
fn assert_invariants(report: &BankReport, rows: &[BankRow]) {
    let charts = charts_of(report);
    let period = &report.period;
    let period_rows = rows
        .iter()
        .filter(|row| row.date >= period.start && row.date <= period.end)
        .count();
    assert!(charts.buckets.len() <= MAX_BUCKETS);
    for set in [&charts.buckets, &charts.groups] {
        if set.is_empty() {
            continue;
        }
        let ids: BTreeSet<&str> =
            set.iter().map(|bucket| bucket.id.as_str()).collect();
        assert_eq!(ids.len(), set.len(), "unique ids");
        assert_partition(set, period);
        for bucket in set.iter() {
            assert_eq!(
                bucket.net_cents,
                bucket.in_cents - bucket.out_cents,
                "{}",
                bucket.id
            );
        }
        let sum_in: i64 = set.iter().map(|bucket| bucket.in_cents).sum();
        let sum_out: i64 = set.iter().map(|bucket| bucket.out_cents).sum();
        let sum_waiting: usize = set.iter().map(|bucket| bucket.waiting).sum();
        let sum_rows: usize = set.iter().map(|bucket| bucket.rows).sum();
        assert_eq!(sum_in, report.totals.in_cents, "Σ in");
        assert_eq!(sum_out, report.totals.out_cents, "Σ out");
        assert_eq!(sum_waiting, report.waiting_period, "Σ waiting");
        assert_eq!(sum_rows, period_rows, "Σ rows");
    }
    for group in &charts.groups {
        assert!(group.group.is_none(), "{}", group.id);
    }
    let series_out: i64 = charts
        .categories
        .series
        .iter()
        .map(|entry| entry.out_cents)
        .sum();
    let rest_out = charts
        .categories
        .rest
        .as_ref()
        .map_or(0, |rest| rest.out_cents);
    assert_eq!(series_out + rest_out, report.totals.out_cents);
}

#[test]
fn every_strip_period_keeps_the_invariants() {
    let rows = ledger();
    for raw in ["2026-W46", "2026-Q4", "2026", "2026-W53", "2026-W01", "all"] {
        let report = report_for(raw);
        assert_invariants(&report, &rows);
    }
    // -w resolves the same week as -p.
    let week = report_at(
        &config(),
        &rows,
        resolve_period(None, None, Some("2026-W46"), None, None, today())
            .expect("week"),
        &JournalSnapshot::default(),
        &Headings::new(),
    );
    assert_invariants(&week, &rows);
    assert_eq!(charts_of(&week).granularity, Granularity::Day);
    assert_eq!(charts_of(&week).buckets.len(), 7);
}

#[test]
fn bucket_ids_and_labels_follow_the_shared_calendar() {
    for (raw, id, granularity, kind, count) in [
        (
            "2026-W46",
            PeriodId::Week {
                year: 2026,
                week: 46,
            },
            Granularity::Day,
            "day",
            7,
        ),
        (
            "2026-W53",
            PeriodId::Week {
                year: 2026,
                week: 53,
            },
            Granularity::Day,
            "day",
            7,
        ),
        (
            "2026-Q4",
            PeriodId::Quarter {
                year: 2026,
                quarter: 4,
            },
            Granularity::Week,
            "week",
            14,
        ),
        ("2026", PeriodId::Year(2026), Granularity::Week, "week", 53),
    ] {
        let report = report_for(raw);
        let charts = charts_of(&report);
        assert_eq!(charts.granularity, granularity, "{raw}");
        let span = Span {
            start: report.period.start,
            end: report.period.end,
        };
        let specs = column_specs(id, span);
        assert_eq!(charts.buckets.len(), count, "{raw}");
        for (bucket, spec) in charts.buckets.iter().zip(&specs) {
            assert_eq!(bucket.id, spec.key(), "{raw}");
            assert_eq!(bucket.label, spec.caption(), "{raw}");
            assert_eq!(bucket.group, spec.group, "{raw}");
            assert_eq!(bucket.kind.as_str(), kind, "{raw}");
        }
    }
    let week = report_for("2026-W46");
    let labels: Vec<&str> = charts_of(&week)
        .buckets
        .iter()
        .map(|bucket| bucket.label.as_str())
        .collect();
    assert_eq!(
        labels,
        ["Mon 9", "Tue 10", "Wed 11", "Thu 12", "Fri 13", "Sat 14", "Sun 15"]
    );
    assert_eq!(charts_of(&week).buckets[2].id, "2026-11-11");
    assert!(charts_of(&week).groups.is_empty());
    assert!(charts_of(&report_for("2026-Q4")).groups.is_empty());
}

#[test]
fn iso_year_groups_weeks_into_quarters() {
    let report = report_for("2026");
    let charts = charts_of(&report);
    let span = Span {
        start: report.period.start,
        end: report.period.end,
    };
    let specs = group_specs(PeriodId::Year(2026), span);
    let ids: Vec<&str> = charts
        .groups
        .iter()
        .map(|group| group.id.as_str())
        .collect();
    assert_eq!(ids, ["2026-Q1", "2026-Q2", "2026-Q3", "2026-Q4"]);
    for (group, spec) in charts.groups.iter().zip(&specs) {
        assert_eq!(group.label, spec.caption);
        assert_eq!(group.kind, Granularity::Quarter);
        assert_eq!(group.start, spec.span.start);
        assert_eq!(group.end, spec.span.end);
        assert_eq!(
            group.target,
            Some(Target {
                path: format!("2026/{}.md", group.id),
                line: 1
            })
        );
    }
    assert_eq!(charts.buckets[0].group.as_deref(), Some("Q1"));
    assert_eq!(
        charts.buckets[0].target,
        Some(Target {
            path: "2026/2026-W01.md".to_owned(),
            line: 1
        })
    );
    assert_eq!(charts.buckets[52].id, "2026-W53");
    assert_eq!(charts.buckets[52].group.as_deref(), Some("Q4"));
    // The Q4 group holds W40–W53, including Dec 28 and Jan 3.
    let q4 = &charts.groups[3];
    assert_eq!(
        q4.out_cents,
        10_000 + 2_000 + 6_000 + 300 + 700 + 24_977 + 3_000 + 1_100
    );
    assert_eq!(q4.in_cents, 1_500 + 250_000);
    // Rows before W01 and after W53 never leak into the year.
    assert_eq!(charts.groups[0].out_cents, 500);
    assert_eq!(charts.groups[0].in_cents, 100_000);
}

#[test]
fn all_time_uses_bank_history_quarters_grouped_by_year() {
    let report = report_for("all");
    assert_eq!(report.period.start, date("2025-12-20"));
    assert_eq!(report.period.end, date("2027-01-04"));
    let charts = charts_of(&report);
    assert_eq!(charts.granularity, Granularity::Quarter);
    let ids: Vec<&str> = charts
        .buckets
        .iter()
        .map(|bucket| bucket.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["2025-Q4", "2026-Q1", "2026-Q2", "2026-Q3", "2026-Q4", "2027-Q1"]
    );
    let first = &charts.buckets[0];
    assert_eq!(first.label, "Q4");
    assert_eq!(first.group.as_deref(), Some("2025"));
    assert_eq!(first.start, date("2025-12-20"));
    assert_eq!(first.state, BucketState::Untracked);
    assert_eq!(first.out_cents, 1_200, "untracked money is never zeroed");
    let last = charts.buckets.last().expect("last");
    assert_eq!(last.end, date("2027-01-04"));
    assert_eq!(last.state, BucketState::Future);
    let groups: Vec<(&str, &str, Granularity)> = charts
        .groups
        .iter()
        .map(|group| (group.id.as_str(), group.label.as_str(), group.kind))
        .collect();
    assert_eq!(
        groups,
        [
            ("2025", "2025", Granularity::Year),
            ("2026", "2026", Granularity::Year),
            ("2027", "2027", Granularity::Year)
        ]
    );
    assert_eq!(
        charts.groups[1].target,
        Some(Target {
            path: "2026.md".to_owned(),
            line: 1
        })
    );
    assert_eq!(
        first.target,
        Some(Target {
            path: "2025/2025-Q4.md".to_owned(),
            line: 1
        })
    );
}

#[test]
fn excluded_money_lands_in_buckets_exactly_as_in_totals() {
    let report = report_for("2026-W46");
    let charts = charts_of(&report);
    let by_id = |id: &str| {
        charts
            .buckets
            .iter()
            .find(|bucket| bucket.id == id)
            .expect("bucket")
    };
    // Transfers in and out count as rows, never as In or Out.
    let transfers = by_id("2026-11-10");
    assert_eq!((transfers.in_cents, transfers.out_cents), (0, 0));
    assert_eq!(transfers.rows, 2);
    assert!(transfers.top_out.is_empty());
    // Cash-account rows stay out; [cat:: cash] is Out; a refund is In; a
    // bare credit is never In; a zero-cent row counts only as a row.
    let mixed = by_id("2026-11-11");
    assert_eq!(mixed.out_cents, 6_000 + 300 + 700);
    assert_eq!(mixed.in_cents, 1_500);
    assert_eq!(mixed.net_cents, 1_500 - 7_000);
    assert_eq!(mixed.rows, 7);
    assert_eq!(mixed.waiting, 1);
    let day_rows: Vec<BankRow> = ledger()
        .into_iter()
        .filter(|row| row.date == date("2026-11-11"))
        .collect();
    let config = config();
    let expected = compute_totals(&day_rows, Some(&config));
    assert_eq!(mixed.in_cents, expected.in_cents);
    assert_eq!(mixed.out_cents, expected.out_cents);
    assert_eq!(mixed.waiting, expected.waiting);
    // Top outflows: largest first, at most three.
    let top: Vec<(&str, i64)> = mixed
        .top_out
        .iter()
        .map(|entry| (entry.key.as_str(), entry.out_cents))
        .collect();
    assert_eq!(
        top,
        [("cash", 6_000), ("unclassified", 700), ("dining", 300)]
    );
    assert_eq!(mixed.top_out[2].emoji, "🍽️");
    assert_eq!(mixed.top_out[2].label, "Dining");
}

#[test]
fn todays_real_shape_is_one_counted_day_awaiting_import() {
    // Tracking started this week and nothing was imported yet.
    let config = config_from(&hub_starting("2026-11-09"));
    let rows = vec![waiting("2026-11-12", -24_977, Some("groceries"))];
    let report = report_at(
        &config,
        &rows,
        strip_period("2026-W46"),
        &JournalSnapshot::default(),
        &Headings::new(),
    );
    let charts = charts_of(&report);
    let states: Vec<(&str, bool)> = charts
        .buckets
        .iter()
        .map(|bucket| (bucket.state.as_str(), bucket.so_far))
        .collect();
    assert_eq!(
        states,
        [
            ("awaiting_import", false),
            ("awaiting_import", false),
            ("awaiting_import", false),
            ("awaiting_import", true),
            ("future", false),
            ("future", false),
            ("future", false),
        ]
    );
    let today = &charts.buckets[3];
    assert_eq!(today.out_cents, 24_977, "a state never zeroes money");
    assert_eq!(today.waiting, 1);
    assert_invariants(&report, &rows);
}

fn coverage(tracked: Option<&str>, imported: Option<&str>) -> Coverage {
    Coverage {
        tracked_since: tracked.map(date),
        imported_through: imported.map(date),
        start: None,
        through: today(),
        complete: false,
    }
}

fn span(start: &str, end: &str) -> Span {
    Span {
        start: date(start),
        end: date(end),
    }
}

#[test]
fn bucket_states_apply_the_first_matching_rule_at_boundaries() {
    let today = today();
    let none = coverage(None, None);
    assert_eq!(
        bucket_state(span("2026-11-13", "2026-11-13"), today, &none),
        BucketState::Future
    );
    // A current bucket starting today is not future.
    assert_eq!(
        bucket_state(span("2026-11-12", "2026-11-15"), today, &none),
        BucketState::Tracked
    );
    // Future wins over every coverage rule.
    let late = coverage(Some("2026-12-01"), Some("2026-10-01"));
    assert_eq!(
        bucket_state(span("2026-11-16", "2026-11-22"), today, &late),
        BucketState::Future
    );
    // Untracked: the bucket ends before tracking starts.
    let tracked = coverage(Some("2026-11-11"), Some("2026-11-12"));
    assert_eq!(
        bucket_state(span("2026-11-10", "2026-11-10"), today, &tracked),
        BucketState::Untracked
    );
    // Tracking starts mid-bucket: partial; on the bucket start: tracked.
    assert_eq!(
        bucket_state(span("2026-11-09", "2026-11-15"), today, &tracked),
        BucketState::Partial
    );
    assert_eq!(
        bucket_state(span("2026-11-11", "2026-11-11"), today, &tracked),
        BucketState::Tracked
    );
    // Imported through the day before a bucket: awaiting import.
    let imported = coverage(Some("2026-10-01"), Some("2026-11-08"));
    assert_eq!(
        bucket_state(span("2026-11-09", "2026-11-15"), today, &imported),
        BucketState::AwaitingImport
    );
    // Imported through a bucket's start but short of today: partial.
    assert_eq!(
        bucket_state(span("2026-11-08", "2026-11-14"), today, &imported),
        BucketState::Partial
    );
    // A current bucket imported through today is tracked; one day short
    // is partial.
    let current = coverage(Some("2026-10-01"), Some("2026-11-12"));
    assert_eq!(
        bucket_state(span("2026-11-09", "2026-11-15"), today, &current),
        BucketState::Tracked
    );
    let short = coverage(Some("2026-10-01"), Some("2026-11-11"));
    assert_eq!(
        bucket_state(span("2026-11-09", "2026-11-15"), today, &short),
        BucketState::Partial
    );
    // A closed bucket imported through its end is tracked.
    assert_eq!(
        bucket_state(span("2026-11-02", "2026-11-08"), today, &imported),
        BucketState::Tracked
    );
    // Untracked wins over awaiting import.
    let both = coverage(Some("2026-11-11"), Some("2026-11-01"));
    assert_eq!(
        bucket_state(span("2026-11-09", "2026-11-10"), today, &both),
        BucketState::Untracked
    );
    for (state, words) in [
        (BucketState::Tracked, None),
        (BucketState::Partial, Some("partly covered")),
        (BucketState::Untracked, Some("not tracked")),
        (BucketState::AwaitingImport, Some("not imported yet")),
        (BucketState::Future, Some("future")),
    ] {
        assert_eq!(state.words(), words);
    }
}

fn entry(key: &str, out_cents: i64, rows: usize) -> BreakdownEntry {
    BreakdownEntry {
        key: key.to_owned(),
        emoji: String::new(),
        label: key.to_owned(),
        out_cents,
        share: 0.0,
        rows,
    }
}

fn with_shares(mut entries: Vec<BreakdownEntry>) -> Vec<BreakdownEntry> {
    let total: i64 = entries.iter().map(|entry| entry.out_cents).sum();
    for entry in &mut entries {
        entry.share = share_of(entry.out_cents, total);
    }
    entries
}

fn series_keys(categories: &Categories) -> Vec<&str> {
    categories
        .series
        .iter()
        .map(|entry| entry.key.as_str())
        .collect()
}

#[test]
fn categories_pin_cash_and_unclassified_then_fold_the_rest() {
    let breakdown = with_shares(vec![
        entry("a", 9_000, 1),
        entry("b", 8_000, 2),
        entry("c", 7_000, 1),
        entry("d", 6_000, 1),
        entry("e", 5_000, 3),
        entry("f", 4_000, 1),
        entry("cash", 100, 1),
        entry("unclassified", 50, 2),
    ]);
    let total: i64 = breakdown.iter().map(|entry| entry.out_cents).sum();
    let folded = fold_categories(&breakdown);
    assert_eq!(folded.limit, 5);
    // Two pinned plus the top three others, all in rank order.
    assert_eq!(
        series_keys(&folded),
        ["a", "b", "c", "cash", "unclassified"]
    );
    let kinds: Vec<&str> =
        folded.series.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        ["category", "category", "category", "cash", "unclassified"]
    );
    let rest = folded.rest.as_ref().expect("rest");
    assert_eq!(rest.count, 3);
    assert_eq!(rest.out_cents, 6_000 + 5_000 + 4_000);
    assert_eq!(rest.rows, 5);
    assert!((rest.share - 15_000.0 / total as f64).abs() < 1e-12);
    let shown: i64 = folded.series.iter().map(|entry| entry.out_cents).sum();
    assert_eq!(shown + rest.out_cents, total);
    // Series entries keep bob's own share.
    assert_eq!(folded.series[0].share, breakdown[0].share);
}

#[test]
fn a_single_leftover_joins_the_series() {
    let six = with_shares(vec![
        entry("a", 600, 1),
        entry("b", 500, 1),
        entry("c", 400, 1),
        entry("d", 300, 1),
        entry("e", 200, 1),
        entry("f", 100, 1),
    ]);
    let folded = fold_categories(&six);
    assert_eq!(series_keys(&folded), ["a", "b", "c", "d", "e", "f"]);
    assert!(folded.rest.is_none());

    let seven = with_shares(vec![
        entry("a", 700, 1),
        entry("b", 600, 1),
        entry("c", 500, 1),
        entry("d", 400, 1),
        entry("e", 300, 1),
        entry("f", 200, 1),
        entry("g", 100, 1),
    ]);
    let folded = fold_categories(&seven);
    assert_eq!(series_keys(&folded), ["a", "b", "c", "d", "e"]);
    let rest = folded.rest.as_ref().expect("rest");
    assert_eq!((rest.count, rest.out_cents), (2, 300));

    // With one pinned entry, four others fit and a fifth joins too.
    let pinned = with_shares(vec![
        entry("a", 700, 1),
        entry("b", 600, 1),
        entry("c", 500, 1),
        entry("d", 400, 1),
        entry("e", 300, 1),
        entry("cash", 50, 1),
    ]);
    let folded = fold_categories(&pinned);
    assert_eq!(series_keys(&folded), ["a", "b", "c", "d", "e", "cash"]);
    assert!(folded.rest.is_none());

    assert!(fold_categories(&[]).series.is_empty());
    assert!(fold_categories(&[]).rest.is_none());
}

#[test]
fn category_ties_break_by_key() {
    let breakdown = with_shares(vec![
        entry("zeta", 500, 1),
        entry("alpha", 500, 1),
        entry("unclassified", 500, 1),
        entry("mid", 900, 1),
    ]);
    let folded = fold_categories(&breakdown);
    assert_eq!(
        series_keys(&folded),
        ["mid", "alpha", "unclassified", "zeta"]
    );
    let report = report_for("2026");
    let charts = charts_of(&report);
    // ISO 2026: groceries, gifts, fuel, pets, books, dining plus cash and
    // unclassified; two pinned and three others shown, three folded.
    assert_eq!(
        series_keys(&charts.categories),
        ["groceries", "cash", "fuel", "gifts", "unclassified"]
    );
    let rest = charts.categories.rest.as_ref().expect("rest");
    assert_eq!(rest.count, 3);
    assert_eq!(rest.out_cents, 1_100 + 800 + 500 + 300);
}

#[test]
fn day_targets_point_at_existing_headings_only() {
    let mut headings = Headings::new();
    headings.insert(
        ("bank/2026-11.md".to_owned(), date("2026-11-11")),
        ("bank/2026-11.md".to_owned(), 42),
    );
    let report = report_at(
        &config(),
        &ledger(),
        strip_period("2026-W46"),
        &JournalSnapshot::default(),
        &headings,
    );
    let charts = charts_of(&report);
    assert_eq!(
        charts.buckets[2].target,
        Some(Target {
            path: "bank/2026-11.md".to_owned(),
            line: 42
        })
    );
    // Rows alone never stand in for a missing heading.
    assert_eq!(charts.buckets[3].target, None);
    assert_eq!(charts.buckets[4].target, None);
    let json = to_json(&report);
    assert_eq!(
        json["charts"]["buckets"][2]["target"],
        serde_json::json!({"path": "bank/2026-11.md", "line": 42})
    );
    assert!(json["charts"]["buckets"][3]["target"].is_null());
}

#[test]
fn json_carries_charts_only_when_requested() {
    let config = config();
    let rows = ledger();
    let build = |summary: bool, charts: bool| {
        crate::native::bank::report::build(
            rows.clone(),
            today(),
            strip_period("2026-W46"),
            Some(&config),
            config.categories.clone(),
            Vec::new(),
            "checking".to_owned(),
            &Headings::new(),
            &JournalSnapshot::default(),
            summary,
            charts,
        )
    };
    let plain = to_json(&build(true, false));
    assert!(plain.get("charts").is_none());
    let mut with = to_json(&build(true, true));
    assert_eq!(with["days"], serde_json::json!([]));
    let charts = with
        .as_object_mut()
        .expect("object")
        .remove("charts")
        .expect("charts key");
    assert_eq!(with, plain, "everything else is unchanged");
    assert_eq!(charts["version"], 1);
    assert_eq!(charts["granularity"], "day");
    let bucket = &charts["buckets"][3];
    for key in [
        "id",
        "kind",
        "label",
        "start",
        "end",
        "group",
        "so_far",
        "state",
        "in_cents",
        "out_cents",
        "net_cents",
        "waiting",
        "rows",
        "top_out",
        "target",
    ] {
        assert!(bucket.get(key).is_some(), "bucket.{key}");
    }
    assert_eq!(bucket["id"], "2026-11-12");
    assert_eq!(bucket["kind"], "day");
    assert_eq!(bucket["label"], "Thu 12");
    assert!(bucket["group"].is_null());
    assert_eq!(bucket["so_far"], true);
    assert_eq!(bucket["state"], "awaiting_import");
    assert_eq!(bucket["out_cents"], 24_977);
    assert_eq!(bucket["in_cents"], 250_000);
    assert_eq!(bucket["net_cents"], 250_000 - 24_977);
    assert_eq!(
        bucket["top_out"],
        serde_json::json!([{
            "key": "groceries", "emoji": "🛒", "label": "Groceries",
            "out_cents": 24_977
        }])
    );
    assert_eq!(charts["groups"], serde_json::json!([]));
    assert_eq!(charts["categories"]["limit"], 5);
    assert!(charts["categories"]["series"].is_array());
    // Full (non-summary) output keeps its days alongside charts.
    let full = to_json(&build(false, true));
    assert_eq!(full["days"].as_array().expect("days").len(), 7);
    assert!(full["charts"].is_object());
}

#[test]
fn a_future_only_history_has_no_buckets() {
    let config = config_from(&hub_starting("2027-03-01"));
    let report = report_at(
        &config,
        &[],
        strip_period("all"),
        &JournalSnapshot::default(),
        &Headings::new(),
    );
    assert!(report.period.start > report.period.end);
    let charts = charts_of(&report);
    assert_eq!(charts.granularity, Granularity::Quarter);
    assert!(charts.buckets.is_empty());
    assert!(charts.groups.is_empty());
    assert!(charts.categories.series.is_empty());
    let json = to_json(&report);
    assert_eq!(json["charts"]["buckets"], serde_json::json!([]));
    assert_eq!(json["charts"]["groups"], serde_json::json!([]));
}

#[test]
fn long_histories_coarsen_to_years_then_null() {
    // Exactly 64 quarters stays quarterly; 65 coarsens.
    let sixty_four = Span {
        start: PeriodId::Quarter {
            year: 2011,
            quarter: 1,
        }
        .start()
        .expect("start"),
        end: PeriodId::Quarter {
            year: 2026,
            quarter: 4,
        }
        .end()
        .expect("end"),
    };
    assert_eq!(quarter_count(sixty_four), 64);
    assert_eq!(year_count(sixty_four), 16);
    let sixty_five = Span {
        start: PeriodId::Quarter {
            year: 2010,
            quarter: 4,
        }
        .start()
        .expect("start"),
        end: sixty_four.end,
    };
    assert_eq!(quarter_count(sixty_five), 65);

    let rows = vec![
        paid("2000-03-01", -1_000, Some("groceries")),
        paid("2026-11-02", -2_000, Some("dining")),
    ];
    let config = config_from(&hub_starting("2000-01-01"));
    let report = report_at(
        &config,
        &rows,
        strip_period("all"),
        &JournalSnapshot::default(),
        &Headings::new(),
    );
    let charts = charts_of(&report);
    assert_eq!(charts.granularity, Granularity::Year);
    assert!(charts.groups.is_empty());
    // 2000-01-01 is in ISO week-year 1999.
    assert_eq!(charts.buckets[0].id, "1999");
    assert_eq!(charts.buckets[0].kind, Granularity::Year);
    assert_eq!(charts.buckets[0].label, "1999");
    assert!(charts.buckets[0].group.is_none());
    assert_eq!(
        charts.buckets[0].target,
        Some(Target {
            path: "1999.md".to_owned(),
            line: 1
        })
    );
    assert_eq!(charts.buckets.len(), 28);
    assert_invariants(&report, &rows);
    assert_eq!(to_json(&report)["charts"]["granularity"], "year");

    let ancient = config_from(&hub_starting("1950-01-02"));
    let report = report_at(
        &ancient,
        &rows,
        strip_period("all"),
        &JournalSnapshot::default(),
        &Headings::new(),
    );
    assert!(matches!(report.charts, Some(None)));
    let json = to_json(&report);
    assert!(json["charts"].is_null());
    assert!(json.get("charts").is_some());
}

#[test]
fn only_strip_periods_have_children() {
    let today = today();
    for raw in ["2026-W46", "2026-Q4", "2026", "all", "last-week"] {
        assert!(period_id_for(&strip_period(raw)).is_some(), "{raw}");
    }
    let week = resolve_period(None, None, Some("2026-W46"), None, None, today)
        .expect("week");
    assert_eq!(
        period_id_for(&week),
        Some(PeriodId::Week {
            year: 2026,
            week: 46
        })
    );
    let refused = [
        resolve_period(Some("today"), None, None, None, None, today),
        resolve_period(None, Some("2026-11"), None, None, None, today),
        resolve_period(None, None, None, Some("2026"), None, today),
        resolve_period(None, None, None, None, None, today),
        resolve_period(None, None, None, None, Some("2026-11-12"), today),
    ];
    for period in refused {
        let period = period.expect("period");
        assert_eq!(period_id_for(&period), None, "{:?}", period.kind);
        let config = config();
        assert!(build(
            &period,
            &[],
            Some(&config),
            &[],
            &coverage(None, None),
            &Headings::new(),
            today,
        )
        .is_none());
    }
}

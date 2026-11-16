//! Historical balance resolver: invariants and the phase-1 case matrix.

use chrono::{DateTime, FixedOffset, NaiveDate};

use super::*;
use crate::native::bank::{
    config::{parse_bank_config, ConfigParse},
    journal::{
        ImportCounts, ImportEvent, JournalContradiction, JournalSnapshot,
        MalformedLine, TxnEvent, TxnOutcome,
    },
    model::BankRow,
    report::{all_time_period, resolve_period, Period},
    source::SourceKind,
    status,
};

fn eastern() -> chrono_tz::Tz {
    "America/New_York".parse().unwrap()
}

fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("test date")
}

fn stamp_at(day: &str, hour: u32) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(&format!("{day}T{hour:02}:00:00-05:00"))
        .unwrap()
}

fn config(start: &str, stale_after: u32) -> BankConfig {
    match parse_bank_config(&format!(
        "---\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: {start}\n    pocket:\n      label: Pocket cash\n      kind: cash\n  categories:\n    groceries: Groceries\n  stale_after: {stale_after}\n---\n"
    )) {
        ConfigParse::Ok(config) => config,
        other => panic!("expected valid config, got {other:?}"),
    }
}

fn ofx(
    start: &str,
    end: &str,
    as_of: &str,
    cents: i64,
    imported_on: &str,
) -> ImportEvent {
    ofx_at(start, end, as_of, cents, imported_on, 12)
}

fn ofx_at(
    start: &str,
    end: &str,
    as_of: &str,
    cents: i64,
    imported_on: &str,
    hour: u32,
) -> ImportEvent {
    ImportEvent {
        ts: stamp_at(imported_on, hour),
        host: "athena".to_owned(),
        path: ".bob/bank/athena.jsonl".to_owned(),
        line: 1,
        account: "checking".to_owned(),
        source: SourceKind::Ofx,
        file_sha256: format!("{imported_on}{hour:02}"),
        start: date(start),
        end: date(end),
        balance_cents: Some(cents),
        balance_asof: Some(date(as_of)),
        counts: ImportCounts::default(),
    }
}

fn csv(start: &str, end: &str, imported_on: &str, hour: u32) -> ImportEvent {
    ImportEvent {
        ts: stamp_at(imported_on, hour),
        host: "athena".to_owned(),
        path: ".bob/bank/athena.jsonl".to_owned(),
        line: 2,
        account: "checking".to_owned(),
        source: SourceKind::Csv,
        file_sha256: format!("csv{imported_on}{hour:02}"),
        start: date(start),
        end: date(end),
        balance_cents: None,
        balance_asof: None,
        counts: ImportCounts::default(),
    }
}

fn txn(id: &str, posted: &str, cents: i64, imported_on: &str) -> TxnEvent {
    txn_at(id, posted, cents, imported_on, 12, TxnOutcome::Written)
}

fn txn_at(
    id: &str,
    posted: &str,
    cents: i64,
    imported_on: &str,
    hour: u32,
    outcome: TxnOutcome,
) -> TxnEvent {
    TxnEvent {
        ts: stamp_at(imported_on, hour),
        host: "athena".to_owned(),
        path: ".bob/bank/athena.jsonl".to_owned(),
        line: 3,
        account: "checking".to_owned(),
        key: format!("{id:0<64}"),
        id: id.to_owned(),
        posted: date(posted),
        cents,
        desc: "SHOPRITE".to_owned(),
        merchant: "shoprite".to_owned(),
        outcome,
        note: (outcome != TxnOutcome::BeforeStart)
            .then(|| "bank/2026-11.md".to_owned()),
    }
}

fn insert_txn(journal: &mut JournalSnapshot, event: TxnEvent) {
    journal.ids.insert(event.id.clone(), event.key.clone());
    journal.transactions.insert(event.key.clone(), event);
}

fn checking_history(
    config: &BankConfig,
    journal: &JournalSnapshot,
) -> AccountHistory {
    let built = histories(config, journal, Some(eastern()));
    built
        .into_iter()
        .find(|item| item.alias == "checking")
        .expect("checking history")
}

fn week(id: &str, today: NaiveDate) -> Period {
    resolve_period(None, None, Some(id), None, None, today).unwrap()
}

fn day_period(id: &str, today: NaiveDate) -> Period {
    resolve_period(Some(id), None, None, None, None, today).unwrap()
}

fn quarter(id: &str, today: NaiveDate) -> Period {
    resolve_period(None, None, None, None, Some(id), today).unwrap()
}

fn row(id: &str, posted: &str, cents: i64) -> BankRow {
    let posted = date(posted);
    BankRow {
        path: "bank/2026-11.md".to_owned(),
        line: 3,
        date: posted,
        heading: format!("## {posted}"),
        account: "checking".to_owned(),
        cents,
        payee: "Shoprite".to_owned(),
        category: None,
        links: Vec::new(),
        note: None,
        flagged: false,
        acknowledged: false,
        id: Some(id.to_owned()),
        origin: "imported",
        suggestion: None,
        subscription: None,
        bank_recurring: false,
        subscription_candidate: false,
    }
}

fn assert_as_of_bound(balance: &PeriodBalance, today: NaiveDate) {
    if let Some(as_of) = balance.as_of {
        assert!(as_of <= balance.wanted);
        assert!(as_of <= today);
    }
}

fn assert_cents_match_state(balance: &PeriodBalance) {
    if balance.state.is_number() {
        assert!(balance.cents.is_some());
    } else {
        assert_eq!(balance.cents, None);
    }
}

#[test]
fn invariants_on_a_balanced_chain() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-13",
    ));
    insert_txn(
        &mut journal,
        txn("aaaaaaaa", "2026-11-03", -1_000, "2026-11-13"),
    );
    let history = checking_history(&config, &journal);
    let period = day_period("2026-11-12", today);
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(10_000));
    assert_eq!(balance.as_of, Some(date("2026-11-12")));
    assert_as_of_bound(&balance, today);
    assert_cents_match_state(&balance);

    let (accounts, _) = status::accounts_with_evidence(
        &config,
        &journal,
        &[row("aaaaaaaa", "2026-11-03", -1_000)],
        today,
    );
    assert_eq!(accounts[0].status, "balanced");
    assert_eq!(accounts[0].balance_cents, balance.cents);

    let mut day = date("2026-10-31");
    while day <= date("2026-11-12") {
        let Some(next) = day.succ_opt() else {
            break;
        };
        if next > date("2026-11-12") {
            break;
        }
        let Some(before) = balance_on(&history, day) else {
            day = next;
            continue;
        };
        let Some(after) = balance_on(&history, next) else {
            day = next;
            continue;
        };
        let posted = history.counted_by_day.get(&next).copied().unwrap_or(0);
        assert_eq!(after - before, posted, "{next}");
        day = next;
    }
}

#[test]
fn closed_period_does_not_inherit_the_latest_balance() {
    let config = config("2026-05-01", 10);
    let today = date("2026-11-16");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-05-01",
        "2026-09-20",
        "2026-09-20",
        50_000,
        "2026-09-21",
    ));
    journal.imports.push(ofx(
        "2026-10-10",
        "2026-11-13",
        "2026-11-13",
        12_345,
        "2026-11-16",
    ));
    let history = checking_history(&config, &journal);
    let period = week("2026-W40", today);
    assert!(period.end < today);
    let balance = resolve(&history, &period, today);
    assert_ne!(balance.cents, Some(12_345));
    assert_eq!(balance.cents, None);
    assert!(balance.as_of.unwrap() < period.start);
    assert_eq!(balance.state, BalanceState::Gap);
    assert_as_of_bound(&balance, today);
    assert_cents_match_state(&balance);

    let mut waiting = JournalSnapshot::default();
    waiting.imports.push(ofx(
        "2026-05-01",
        "2026-09-20",
        "2026-09-20",
        50_000,
        "2026-09-21",
    ));
    let waiting_history = checking_history(&config, &waiting);
    let waiting_balance = resolve(&waiting_history, &period, today);
    assert_eq!(waiting_balance.state, BalanceState::AwaitingImport);
    assert_eq!(waiting_balance.cents, None);
}

#[test]
fn anchor_before_inside_and_after_the_period() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-20");
    let period = week("2026-W46", today);
    assert_eq!(period.start, date("2026-11-09"));
    assert_eq!(period.end, date("2026-11-15"));

    let mut before = JournalSnapshot::default();
    before.imports.push(ofx(
        "2026-11-01",
        "2026-11-15",
        "2026-11-01",
        20_000,
        "2026-11-16",
    ));
    insert_txn(
        &mut before,
        txn("aaaaaaaa", "2026-11-10", -2_000, "2026-11-16"),
    );
    let history = checking_history(&config, &before);
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(18_000));
    assert_eq!(balance_on(&history, date("2026-11-15")), Some(18_000));

    let mut inside = JournalSnapshot::default();
    inside.imports.push(ofx(
        "2026-11-01",
        "2026-11-15",
        "2026-11-12",
        19_000,
        "2026-11-16",
    ));
    insert_txn(
        &mut inside,
        txn("bbbbbbbb", "2026-11-14", -500, "2026-11-16"),
    );
    let history = checking_history(&config, &inside);
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(18_500));

    let mut after = JournalSnapshot::default();
    after.imports.push(ofx(
        "2026-11-01",
        "2026-11-20",
        "2026-11-20",
        17_000,
        "2026-11-21",
    ));
    insert_txn(
        &mut after,
        txn("cccccccc", "2026-11-18", -1_000, "2026-11-21"),
    );
    let history = checking_history(&config, &after);
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(18_000));
}

#[test]
fn before_start_transactions_count_in_reconstruction() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-10");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-10-01",
        "2026-11-05",
        "2026-11-05",
        8_000,
        "2026-11-06",
    ));
    insert_txn(
        &mut journal,
        txn_at(
            "aaaaaaaa",
            "2026-10-15",
            -2_000,
            "2026-11-06",
            12,
            TxnOutcome::BeforeStart,
        ),
    );
    let history = checking_history(&config, &journal);
    assert_eq!(balance_on(&history, date("2026-10-14")), Some(10_000));
    assert_eq!(balance_on(&history, date("2026-10-15")), Some(8_000));
    assert_eq!(balance_on(&history, date("2026-11-05")), Some(8_000));
    let period = day_period("2026-11-05", today);
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(8_000));
}

#[test]
fn csv_only_batch_is_excluded_and_mixed_batch_taints_the_span() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut excluded = JournalSnapshot::default();
    excluded.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-13",
    ));
    excluded
        .imports
        .push(csv("2026-11-01", "2026-11-12", "2026-11-13", 18));
    insert_txn(
        &mut excluded,
        txn_at(
            "aaaaaaaa",
            "2026-11-03",
            -1_000,
            "2026-11-13",
            12,
            TxnOutcome::Written,
        ),
    );
    insert_txn(
        &mut excluded,
        txn_at(
            "bbbbbbbb",
            "2026-11-03",
            -500,
            "2026-11-13",
            18,
            TxnOutcome::Written,
        ),
    );
    let history = checking_history(&config, &excluded);
    assert!(history.findings.is_empty());
    assert_eq!(balance_on(&history, date("2026-11-02")), Some(11_000));
    assert_eq!(balance_on(&history, date("2026-11-12")), Some(10_000));
    let period = day_period("2026-11-12", today);
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(10_000));

    let mut mixed = JournalSnapshot::default();
    mixed.imports.push(ofx_at(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-13",
        12,
    ));
    mixed
        .imports
        .push(csv("2026-11-01", "2026-11-12", "2026-11-13", 12));
    insert_txn(
        &mut mixed,
        txn("cccccccc", "2026-11-03", -1_000, "2026-11-13"),
    );
    let history = checking_history(&config, &mixed);
    assert!(history.findings.iter().any(|finding| {
        finding.code == "bank_balance_csv_overlap"
            && finding.message.contains("2026-11-13")
            && finding.message.contains("2026-11-01–2026-11-12")
    }));
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Unverified);
    assert_eq!(balance.reason, Some(BalanceReason::CsvOverlap));
    assert_eq!(balance.cents, None);
    assert_cents_match_state(&balance);
}

#[test]
fn mismatch_hides_only_its_segment_and_dispute_hides_its_date() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-25");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-11-01",
        "2026-11-20",
        "2026-11-01",
        10_000,
        "2026-11-21",
    ));
    journal.imports.push(ofx(
        "2026-11-01",
        "2026-11-20",
        "2026-11-10",
        9_000,
        "2026-11-21",
    ));
    journal.imports.push(ofx(
        "2026-11-01",
        "2026-11-20",
        "2026-11-20",
        5_000,
        "2026-11-21",
    ));
    insert_txn(
        &mut journal,
        txn("aaaaaaaa", "2026-11-05", -1_000, "2026-11-21"),
    );
    insert_txn(
        &mut journal,
        txn("bbbbbbbb", "2026-11-15", -1_000, "2026-11-21"),
    );
    let history = checking_history(&config, &journal);
    assert!(history.findings.iter().any(|finding| {
        finding.code == "bank_balance_anchor_mismatch"
            && finding.message.contains("2026-11-10")
            && finding.message.contains("2026-11-11–2026-11-19")
    }));
    let early = resolve(&history, &day_period("2026-11-10", today), today);
    assert_eq!(early.state, BalanceState::Settled);
    assert_eq!(early.cents, Some(9_000));
    assert!(!early.anchors.agree);
    let mid = resolve(&history, &day_period("2026-11-15", today), today);
    assert_eq!(mid.state, BalanceState::Unverified);
    assert_eq!(mid.reason, Some(BalanceReason::AnchorMismatch));
    assert_eq!(mid.cents, None);
    let late = resolve(&history, &day_period("2026-11-20", today), today);
    assert_eq!(late.state, BalanceState::Settled);
    assert_eq!(late.cents, Some(5_000));

    let mut disputed = JournalSnapshot::default();
    disputed.imports.push(ofx_at(
        "2026-11-01",
        "2026-11-20",
        "2026-11-13",
        10_000,
        "2026-11-14",
        12,
    ));
    disputed.imports.push(ofx_at(
        "2026-11-01",
        "2026-11-20",
        "2026-11-13",
        9_000,
        "2026-11-16",
        12,
    ));
    let history = checking_history(&config, &disputed);
    assert!(history
        .findings
        .iter()
        .any(|finding| finding.code == "bank_balance_anchor_mismatch"));
    let balance = resolve(&history, &day_period("2026-11-13", today), today);
    assert_eq!(balance.state, BalanceState::Unverified);
    assert_eq!(balance.reason, Some(BalanceReason::AnchorMismatch));
    assert_eq!(balance.cents, None);
    assert!(!balance.anchors.agree);
}

#[test]
fn provisional_anchor_is_ignored_once_superseded() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx_at(
        "2026-11-01",
        "2026-11-13",
        "2026-11-13",
        10_000,
        "2026-11-13",
        12,
    ));
    journal.imports.push(ofx_at(
        "2026-11-01",
        "2026-11-13",
        "2026-11-13",
        9_000,
        "2026-11-16",
        12,
    ));
    let history = checking_history(&config, &journal);
    assert!(history.findings.is_empty());
    assert_eq!(balance_on(&history, date("2026-11-13")), Some(9_000));
    let balance = resolve(&history, &day_period("2026-11-13", today), today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(9_000));
}

#[test]
fn provisional_window_day_never_settles_a_closed_period() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-11-09",
        "2026-11-13",
        "2026-11-13",
        7_760,
        "2026-11-13",
    ));
    let history = checking_history(&config, &journal);
    let period = week("2026-W46", today);
    let balance = resolve(&history, &period, today);
    assert_eq!(period.end, date("2026-11-15"));
    assert_eq!(balance.state, BalanceState::AsOf);
    assert_eq!(balance.as_of, Some(date("2026-11-12")));
    assert_eq!(balance.wanted, date("2026-11-15"));
    assert_eq!(balance.cents, Some(7_760));
    assert_ne!(balance.as_of, Some(period.end));
    assert_as_of_bound(&balance, today);
}

#[test]
fn open_all_time_and_future_periods() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-12");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-12",
    ));
    let history = checking_history(&config, &journal);
    let open = week("2026-W46", today);
    let balance = resolve(&history, &open, today);
    assert_eq!(balance.state, BalanceState::SoFar);
    assert_eq!(balance.wanted, today);
    assert_eq!(balance.as_of, Some(date("2026-11-12")));
    assert_eq!(balance.cents, Some(10_000));
    assert!(balance.open.is_some());
    assert!(!balance.stale);
    assert_as_of_bound(&balance, today);

    let all_time = all_time_period(Some(&config), &[], today);
    let balance = resolve(&history, &all_time, today);
    assert_eq!(balance.state, BalanceState::SoFar);
    assert_eq!(balance.open, None);
    assert_eq!(balance.change_cents, None);
    assert_eq!(balance.cents, Some(10_000));

    let future_today = date("2026-11-01");
    let future = week("2026-W46", future_today);
    let balance = resolve(&history, &future, future_today);
    assert_eq!(balance.state, BalanceState::None);
    assert_eq!(balance.reason, Some(BalanceReason::Future));
    assert_eq!(balance.cents, None);
    assert_cents_match_state(&balance);

    let empty = checking_history(&config, &JournalSnapshot::default());
    let past = day_period("2026-10-01", today);
    let balance = resolve(&empty, &past, today);
    assert_eq!(balance.state, BalanceState::None);
    assert_eq!(balance.reason, Some(BalanceReason::PreCoverage));
    assert_eq!(balance.cents, None);
}

#[test]
fn w53_quarter_ends_on_the_following_january() {
    let config = config("2026-10-01", 10);
    let today = date("2027-01-10");
    let period = quarter("2026-Q4", today);
    assert_eq!(period.end, date("2027-01-03"));
    assert!(!period.so_far);
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-10-05",
        "2027-01-03",
        "2027-01-03",
        42_00,
        "2027-01-04",
    ));
    let history = checking_history(&config, &journal);
    let balance = resolve(&history, &period, today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.as_of, Some(date("2027-01-03")));
    assert_eq!(balance.wanted, date("2027-01-03"));
    assert_eq!(balance.cents, Some(42_00));
    assert_as_of_bound(&balance, today);
}

#[test]
fn zero_and_negative_balances_render_as_numbers() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut zero = JournalSnapshot::default();
    zero.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        0,
        "2026-11-13",
    ));
    let history = checking_history(&config, &zero);
    let balance = resolve(&history, &day_period("2026-11-12", today), today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(0));
    assert_cents_match_state(&balance);

    let mut negative = JournalSnapshot::default();
    negative.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        -4_510,
        "2026-11-13",
    ));
    let history = checking_history(&config, &negative);
    let balance = resolve(&history, &day_period("2026-11-12", today), today);
    assert_eq!(balance.cents, Some(-4_510));
    assert_cents_match_state(&balance);
}

#[test]
fn stale_is_only_set_on_so_far() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-24");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-13",
    ));
    let history = checking_history(&config, &journal);
    let open = week("2026-W48", today);
    assert!(open.so_far);
    let balance = resolve(&history, &open, today);
    assert_eq!(balance.state, BalanceState::SoFar);
    assert!(balance.stale);
    assert_eq!(balance.as_of, Some(date("2026-11-12")));

    let closed = week("2026-W46", today);
    let balance = resolve(&history, &closed, today);
    assert_eq!(balance.state, BalanceState::AsOf);
    assert!(!balance.stale);
    assert_eq!(balance.cents, Some(10_000));
}

#[test]
fn journal_malformed_or_contradiction_is_unverified() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut malformed = JournalSnapshot::default();
    malformed.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-13",
    ));
    malformed.malformed.push(MalformedLine {
        path: ".bob/bank/athena.jsonl".to_owned(),
        line: 9,
    });
    let history = checking_history(&config, &malformed);
    let balance = resolve(&history, &day_period("2026-11-12", today), today);
    assert_eq!(balance.state, BalanceState::Unverified);
    assert_eq!(balance.reason, Some(BalanceReason::JournalUnreadable));
    assert_eq!(balance.cents, None);

    let mut contradiction = JournalSnapshot::default();
    contradiction.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-13",
    ));
    let event = txn("aaaaaaaa", "2026-11-03", -1_000, "2026-11-13");
    contradiction.contradictions.push(JournalContradiction {
        key: event.key.clone(),
        field: "cents",
    });
    insert_txn(&mut contradiction, event);
    let history = checking_history(&config, &contradiction);
    let balance = resolve(&history, &day_period("2026-11-12", today), today);
    assert_eq!(balance.state, BalanceState::Unverified);
    assert_eq!(balance.reason, Some(BalanceReason::JournalContradiction));
    assert_eq!(balance.cents, None);
    assert_cents_match_state(&balance);
}

#[test]
fn cash_accounts_are_omitted_and_checking_is_hero() {
    let config = config("2026-11-01", 10);
    let journal = JournalSnapshot::default();
    let built = histories(&config, &journal, Some(eastern()));
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].alias, "checking");
    assert!(built[0].hero);
    assert_eq!(built[0].kind, AccountKind::Checking);
}

#[test]
fn no_anchor_span_is_unverified() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut journal = JournalSnapshot::default();
    let mut event = ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        10_000,
        "2026-11-13",
    );
    event.balance_cents = None;
    event.balance_asof = None;
    journal.imports.push(event);
    let history = checking_history(&config, &journal);
    let balance = resolve(&history, &day_period("2026-11-12", today), today);
    assert_eq!(balance.state, BalanceState::Unverified);
    assert_eq!(balance.reason, Some(BalanceReason::NoAnchor));
    assert_eq!(balance.cents, None);
}

#[test]
fn recovery_transaction_without_an_import_batch_counts() {
    let config = config("2026-11-01", 10);
    let today = date("2026-11-16");
    let mut journal = JournalSnapshot::default();
    journal.imports.push(ofx(
        "2026-11-01",
        "2026-11-12",
        "2026-11-12",
        9_000,
        "2026-11-13",
    ));
    insert_txn(
        &mut journal,
        txn_at(
            "aaaaaaaa",
            "2026-11-03",
            -1_000,
            "2026-10-01",
            12,
            TxnOutcome::Written,
        ),
    );
    let history = checking_history(&config, &journal);
    assert_eq!(balance_on(&history, date("2026-11-02")), Some(10_000));
    let balance = resolve(&history, &day_period("2026-11-12", today), today);
    assert_eq!(balance.state, BalanceState::Settled);
    assert_eq!(balance.cents, Some(9_000));
}

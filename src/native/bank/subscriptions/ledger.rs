//! Per-record subscription ledger facts.
//!
//! Facts are pure functions of the registry, attributed transactions, merged
//! import coverage, and caller-supplied today. Charges are attributed
//! outflows oldest first; refunds net against spending and never enter the
//! charge list, latest, price-change, still-charging, after-cancel, or
//! timeline inputs. Timeline buckets align to `renews` when set, otherwise to
//! the latest charge, using the original-anchor cadence helper so month-end
//! clamping does not drift. A missing charge is missed only after its due
//! interval has elapsed and every observed relevant account covers that
//! interval; with no schedule or no historical account evidence, missed
//! charges are not manufactured.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::native::bank::{journal::JournalSnapshot, model::BankRow};

use super::{
    attribution::{
        attribute, AttributedTxn, AttributionDiagnostic, AttributionSnapshot,
        AttributionSource,
    },
    cadence::{
        add_cadence_steps, div_round_half_up, within_one_and_half_steps,
    },
    coverage::CoverageIndex,
    registry::{
        SubscriptionCadence, SubscriptionPricing, SubscriptionRecord,
        SubscriptionRegistry, SubscriptionStatus,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerCharge {
    pub(crate) date: NaiveDate,
    pub(crate) cents: i64,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) id: Option<String>,
    pub(crate) account: String,
    pub(crate) journal_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriceChange {
    pub(crate) from_cents: i64,
    pub(crate) to_cents: i64,
    pub(crate) since: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimelineState {
    Charged,
    Missed,
    Uncovered,
    Upcoming,
}

impl TimelineState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Charged => "charged",
            Self::Missed => "missed",
            Self::Uncovered => "uncovered",
            Self::Upcoming => "upcoming",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimelineBucket {
    pub(crate) start: NaiveDate,
    pub(crate) due: NaiveDate,
    pub(crate) state: TimelineState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordLedger {
    pub(crate) record_path: String,
    pub(crate) charges: Vec<LedgerCharge>,
    pub(crate) refunds: Vec<LedgerCharge>,
    pub(crate) gross_cents: i64,
    pub(crate) refund_cents: i64,
    pub(crate) net_cents: i64,
    pub(crate) latest: Option<LedgerCharge>,
    pub(crate) next_expected: Option<NaiveDate>,
    pub(crate) monthly_cents: i64,
    pub(crate) price_change: Option<PriceChange>,
    pub(crate) still_charging: bool,
    pub(crate) after_cancel: Option<LedgerCharge>,
    pub(crate) timeline: Vec<TimelineBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowAttribution {
    pub(crate) record_path: Option<String>,
    pub(crate) source: Option<AttributionSource>,
    pub(crate) needle: Option<String>,
    pub(crate) tied_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LedgerSnapshot {
    pub(crate) records: BTreeMap<String, RecordLedger>,
    pub(crate) by_row: BTreeMap<(String, usize), RowAttribution>,
    pub(crate) by_id: BTreeMap<String, RowAttribution>,
    pub(crate) coverage: CoverageIndex,
    pub(crate) diagnostics: Vec<AttributionDiagnostic>,
}

/// Derive coverage-aware ledger facts for one command invocation.
pub(crate) fn compute_ledger(
    registry: &SubscriptionRegistry,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    today: NaiveDate,
) -> LedgerSnapshot {
    let coverage = CoverageIndex::from_journal(journal);
    let attributed = attribute(registry, journal, rows, &coverage);
    build_ledger(registry, attributed, coverage, today)
}

fn build_ledger(
    registry: &SubscriptionRegistry,
    attributed: AttributionSnapshot,
    coverage: CoverageIndex,
    today: NaiveDate,
) -> LedgerSnapshot {
    let mut by_record = BTreeMap::<String, Vec<AttributedTxn>>::new();
    let mut by_row = BTreeMap::new();
    let mut by_id = BTreeMap::new();
    for txn in attributed.transactions {
        let projection = RowAttribution {
            record_path: txn.record_path.clone(),
            source: txn.source,
            needle: txn.needle.clone(),
            tied_paths: txn.tied_paths.clone(),
        };
        if let (Some(path), Some(line)) = (txn.path.clone(), txn.line) {
            by_row.insert((path, line), projection.clone());
        }
        if let Some(id) = txn.id.clone() {
            by_id.insert(id, projection);
        }
        if let Some(record_path) = txn.record_path.clone() {
            by_record.entry(record_path).or_default().push(txn);
        }
    }

    let mut records = BTreeMap::new();
    for record in &registry.records {
        let txns = by_record.remove(&record.path).unwrap_or_default();
        records.insert(
            record.path.clone(),
            record_ledger(record, &txns, &coverage, today),
        );
    }
    LedgerSnapshot {
        records,
        by_row,
        by_id,
        coverage,
        diagnostics: attributed.diagnostics,
    }
}

fn record_ledger(
    record: &SubscriptionRecord,
    txns: &[AttributedTxn],
    coverage: &CoverageIndex,
    today: NaiveDate,
) -> RecordLedger {
    let mut charges = Vec::new();
    let mut refunds = Vec::new();
    for txn in txns {
        if txn.is_refund {
            refunds.push(to_charge(txn));
        } else if txn.cents < 0 {
            charges.push(to_charge(txn));
        }
    }
    charges.sort_by(charge_order);
    refunds.sort_by(charge_order);
    let gross_cents = sum_magnitudes(&charges);
    let refund_cents = refunds.iter().map(|refund| refund.cents.max(0)).sum();
    let net_cents = gross_cents.saturating_sub(refund_cents);
    let latest = charges.last().cloned();
    let next_expected = next_expected_date(record, latest.as_ref());
    let monthly_cents = monthly_equivalent(record, &charges, today);
    let price_change = price_change_for(record, &charges);
    let still_charging = record.status == SubscriptionStatus::ToCancel
        && latest.as_ref().is_some_and(|charge| {
            within_one_and_half_steps(charge.date, today, record.cadence)
        });
    let after_cancel = after_cancel_charge(record, &charges);
    let timeline = timeline_for(record, &charges, coverage, today);
    RecordLedger {
        record_path: record.path.clone(),
        charges,
        refunds,
        gross_cents,
        refund_cents,
        net_cents,
        latest,
        next_expected,
        monthly_cents,
        price_change,
        still_charging,
        after_cancel,
        timeline,
    }
}

fn to_charge(txn: &AttributedTxn) -> LedgerCharge {
    LedgerCharge {
        date: txn.date,
        cents: txn.cents,
        path: txn.path.clone(),
        line: txn.line,
        id: txn.id.clone(),
        account: txn.account.clone(),
        journal_only: txn.journal_only,
    }
}

fn charge_order(
    left: &LedgerCharge,
    right: &LedgerCharge,
) -> std::cmp::Ordering {
    left.date
        .cmp(&right.date)
        .then(left.account.cmp(&right.account))
        .then(left.journal_only.cmp(&right.journal_only))
        .then(left.path.cmp(&right.path))
        .then(left.line.cmp(&right.line))
        .then(left.id.cmp(&right.id))
        .then(left.cents.cmp(&right.cents))
}

fn sum_magnitudes(charges: &[LedgerCharge]) -> i64 {
    charges.iter().fold(0, |total, charge| {
        total.saturating_add(charge.cents.saturating_neg())
    })
}

fn next_expected_date(
    record: &SubscriptionRecord,
    latest: Option<&LedgerCharge>,
) -> Option<NaiveDate> {
    if let Some(renews) = record.renews {
        return Some(renews);
    }
    latest.and_then(|charge| add_cadence_steps(charge.date, 1, record.cadence))
}

fn monthly_equivalent(
    record: &SubscriptionRecord,
    charges: &[LedgerCharge],
    today: NaiveDate,
) -> i64 {
    if record.status.is_cancelled() {
        return 0;
    }
    match record.pricing {
        SubscriptionPricing::Fixed => fixed_monthly(record),
        SubscriptionPricing::Variable => {
            variable_monthly(record.amount_cents, charges, today)
        }
    }
}

fn fixed_monthly(record: &SubscriptionRecord) -> i64 {
    match record.cadence {
        SubscriptionCadence::Monthly => record.amount_cents,
        SubscriptionCadence::FourWeekly => {
            div_round_half_up(record.amount_cents.saturating_mul(13), 12)
                .unwrap_or(0)
        }
        SubscriptionCadence::Quarterly => {
            div_round_half_up(record.amount_cents, 3).unwrap_or(0)
        }
        SubscriptionCadence::Yearly => {
            div_round_half_up(record.amount_cents, 12).unwrap_or(0)
        }
    }
}

fn variable_monthly(
    amount_cents: i64,
    charges: &[LedgerCharge],
    today: NaiveDate,
) -> i64 {
    let Some(start) =
        add_cadence_steps(today, -3, SubscriptionCadence::Monthly)
    else {
        return amount_cents;
    };
    let mut sum = 0i64;
    let mut any = false;
    for charge in charges {
        if charge.date >= start && charge.date <= today {
            any = true;
            sum = sum.saturating_add(charge.cents.saturating_neg());
        }
    }
    if !any {
        return amount_cents;
    }
    div_round_half_up(sum, 3).unwrap_or(0)
}

fn price_change_for(
    record: &SubscriptionRecord,
    charges: &[LedgerCharge],
) -> Option<PriceChange> {
    if record.pricing != SubscriptionPricing::Fixed {
        return None;
    }
    let latest = charges.last()?;
    let latest_magnitude = latest.cents.saturating_neg();
    match record.cadence {
        SubscriptionCadence::Monthly | SubscriptionCadence::FourWeekly => {
            let previous = charges.get(charges.len().checked_sub(2)?)?;
            let previous_magnitude = previous.cents.saturating_neg();
            if previous_magnitude == latest_magnitude
                && latest_magnitude != record.amount_cents
            {
                Some(PriceChange {
                    from_cents: record.amount_cents,
                    to_cents: latest_magnitude,
                    since: latest.date,
                })
            } else {
                None
            }
        }
        SubscriptionCadence::Quarterly | SubscriptionCadence::Yearly => {
            (latest_magnitude != record.amount_cents).then_some(PriceChange {
                from_cents: record.amount_cents,
                to_cents: latest_magnitude,
                since: latest.date,
            })
        }
    }
}

fn after_cancel_charge(
    record: &SubscriptionRecord,
    charges: &[LedgerCharge],
) -> Option<LedgerCharge> {
    if record.status != SubscriptionStatus::Cancelled {
        return None;
    }
    charges
        .iter()
        .rev()
        .find(|charge| match record.ended {
            Some(ended) => charge.date > ended,
            None => true,
        })
        .cloned()
}

fn timeline_for(
    record: &SubscriptionRecord,
    charges: &[LedgerCharge],
    coverage: &CoverageIndex,
    today: NaiveDate,
) -> Vec<TimelineBucket> {
    let anchor = record.renews.or_else(|| charges.last().map(|c| c.date));
    let Some(anchor) = anchor else {
        return Vec::new();
    };
    let Some(current_n) = current_step(anchor, today, record.cadence) else {
        return Vec::new();
    };
    let relevant_accounts = relevant_accounts(charges);
    let mut buckets = Vec::new();
    for offset in 0..12 {
        let n = current_n.saturating_add(offset - 11);
        let Some(due) = add_cadence_steps(anchor, n, record.cadence) else {
            continue;
        };
        let Some(prev_n) = n.checked_sub(1) else {
            continue;
        };
        let Some(prev_due) = add_cadence_steps(anchor, prev_n, record.cadence)
        else {
            continue;
        };
        let Some(start) = prev_due.succ_opt() else {
            continue;
        };
        if start > due {
            continue;
        }
        let charged = charges
            .iter()
            .any(|charge| charge.date > prev_due && charge.date <= due);
        let elapsed = today > due;
        let spanned = coverage.spans_all(&relevant_accounts, start, due);
        let state = if charged {
            TimelineState::Charged
        } else if !elapsed {
            TimelineState::Upcoming
        } else if spanned {
            TimelineState::Missed
        } else {
            TimelineState::Uncovered
        };
        buckets.push(TimelineBucket { start, due, state });
    }
    buckets
}

fn current_step(
    anchor: NaiveDate,
    today: NaiveDate,
    cadence: SubscriptionCadence,
) -> Option<i32> {
    for n in 0..=240 {
        let due = add_cadence_steps(anchor, n, cadence)?;
        if due >= today {
            return Some(n);
        }
    }
    Some(240)
}

fn relevant_accounts(charges: &[LedgerCharge]) -> Vec<String> {
    let mut accounts = charges
        .iter()
        .map(|charge| charge.account.clone())
        .collect::<Vec<_>>();
    accounts.sort();
    accounts.dedup();
    accounts
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, NaiveDate};

    use crate::native::bank::{
        journal::{
            ImportCounts, ImportEvent, JournalSnapshot, TxnEvent, TxnOutcome,
        },
        model::BankRow,
        source::SourceKind,
        subscriptions::registry::SubscriptionAvailability,
    };

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("date")
    }

    fn ts() -> DateTime<chrono::FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z").expect("ts")
    }

    fn record(
        path: &str,
        status: SubscriptionStatus,
        amount_cents: i64,
        cadence: SubscriptionCadence,
        pricing: SubscriptionPricing,
        needles: &[&str],
    ) -> SubscriptionRecord {
        SubscriptionRecord {
            stem: path
                .rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".md")
                .to_owned(),
            path: path.to_owned(),
            link_target: path.trim_end_matches(".md").to_owned(),
            title: path.to_owned(),
            status,
            status_label: status.as_str().to_owned(),
            amount_cents,
            cadence,
            pricing,
            needles: needles
                .iter()
                .map(|needle| (*needle).to_owned())
                .collect(),
            invalid_needles: Vec::new(),
            category: None,
            url: None,
            renews: None,
            ended: None,
        }
    }

    fn registry(records: Vec<SubscriptionRecord>) -> SubscriptionRegistry {
        SubscriptionRegistry {
            availability: SubscriptionAvailability::Available,
            album: None,
            records,
            diagnostics: Vec::new(),
        }
    }

    fn row(
        account: &str,
        day: NaiveDate,
        cents: i64,
        payee: &str,
        category: Option<&str>,
        links: &[&str],
        id: Option<&str>,
        line: usize,
    ) -> BankRow {
        BankRow {
            path: format!("bank/{}.md", day.format("%Y-%m")),
            line,
            date: day,
            heading: String::new(),
            account: account.to_owned(),
            cents,
            payee: payee.to_owned(),
            category: category.map(str::to_owned),
            links: links.iter().map(|link| (*link).to_owned()).collect(),
            note: None,
            flagged: false,
            acknowledged: true,
            id: id.map(str::to_owned),
            origin: "typed",
            suggestion: None,
        }
    }

    fn txn(
        account: &str,
        id: &str,
        day: NaiveDate,
        cents: i64,
        desc: &str,
        outcome: TxnOutcome,
    ) -> TxnEvent {
        TxnEvent {
            ts: ts(),
            host: "test".to_owned(),
            path: ".bob/bank/test.jsonl".to_owned(),
            line: 1,
            account: account.to_owned(),
            key: format!("{id:0<64}"),
            id: id.to_owned(),
            posted: day,
            cents,
            desc: desc.to_owned(),
            merchant: desc.to_ascii_lowercase(),
            outcome,
            note: (outcome != TxnOutcome::BeforeStart)
                .then(|| "bank/2026-11.md".to_owned()),
        }
    }

    fn import(account: &str, start: NaiveDate, end: NaiveDate) -> ImportEvent {
        ImportEvent {
            ts: ts(),
            host: "test".to_owned(),
            path: ".bob/bank/test.jsonl".to_owned(),
            line: 1,
            account: account.to_owned(),
            source: SourceKind::Ofx,
            file_sha256: "c".repeat(64),
            start,
            end,
            balance_cents: None,
            balance_asof: None,
            counts: ImportCounts::default(),
        }
    }

    fn ledger(
        records: Vec<SubscriptionRecord>,
        txns: Vec<TxnEvent>,
        rows: Vec<BankRow>,
        imports: Vec<ImportEvent>,
        today: NaiveDate,
    ) -> LedgerSnapshot {
        let mut journal = JournalSnapshot::default();
        for event in txns {
            journal.ids.insert(event.id.clone(), event.key.clone());
            journal.transactions.insert(event.key.clone(), event);
        }
        journal.imports = imports;
        compute_ledger(&registry(records), &journal, &rows, today)
    }

    fn tunely(
        amount: i64,
        cadence: SubscriptionCadence,
        pricing: SubscriptionPricing,
        status: SubscriptionStatus,
    ) -> SubscriptionRecord {
        record(
            "billing/tunely.md",
            status,
            amount,
            cadence,
            pricing,
            &["TUNELY"],
        )
    }

    #[test]
    fn next_expected_covers_every_cadence_end_of_month_leap_and_renews() {
        let today = date(2026, 11, 15);
        let monthly = {
            let rec = tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            );
            ledger(
                vec![rec],
                vec![txn(
                    "checking",
                    "m0000001",
                    date(2026, 1, 31),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                )],
                vec![],
                vec![],
                today,
            )
        };
        assert_eq!(
            monthly.records["billing/tunely.md"].next_expected,
            Some(date(2026, 2, 28))
        );

        let quarterly = ledger(
            vec![tunely(
                3000,
                SubscriptionCadence::Quarterly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![txn(
                "checking",
                "q0000001",
                date(2026, 1, 31),
                -3000,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        assert_eq!(
            quarterly.records["billing/tunely.md"].next_expected,
            Some(date(2026, 4, 30))
        );

        let four = ledger(
            vec![tunely(
                600,
                SubscriptionCadence::FourWeekly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![txn(
                "checking",
                "f0000001",
                date(2026, 1, 6),
                -600,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        assert_eq!(
            four.records["billing/tunely.md"].next_expected,
            Some(date(2026, 2, 3))
        );

        let yearly = ledger(
            vec![tunely(
                12000,
                SubscriptionCadence::Yearly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![txn(
                "checking",
                "y0000001",
                date(2024, 2, 29),
                -12000,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        assert_eq!(
            yearly.records["billing/tunely.md"].next_expected,
            Some(date(2025, 2, 28))
        );

        let mut rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        rec.renews = Some(date(2026, 12, 1));
        let renews = ledger(
            vec![rec],
            vec![txn(
                "checking",
                "r0000001",
                date(2026, 1, 31),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        assert_eq!(
            renews.records["billing/tunely.md"].next_expected,
            Some(date(2026, 12, 1))
        );
    }

    #[test]
    fn monthly_equivalent_rounding_variable_fallback_and_cancelled_zero() {
        let today = date(2026, 11, 15);
        let fixed = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::FourWeekly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![],
            vec![],
            vec![],
            today,
        );
        assert_eq!(fixed.records["billing/tunely.md"].monthly_cents, 1082);
        assert_eq!(SubscriptionCadence::FourWeekly.as_str(), "four_weekly");
        assert_eq!(SubscriptionPricing::Fixed.as_str(), "fixed");

        let quarterly = ledger(
            vec![tunely(
                101,
                SubscriptionCadence::Quarterly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![],
            vec![],
            vec![],
            today,
        );
        assert_eq!(quarterly.records["billing/tunely.md"].monthly_cents, 34);

        let yearly = ledger(
            vec![tunely(
                6,
                SubscriptionCadence::Yearly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![],
            vec![],
            vec![],
            today,
        );
        assert_eq!(yearly.records["billing/tunely.md"].monthly_cents, 1);

        let variable_empty = ledger(
            vec![tunely(
                1500,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Variable,
                SubscriptionStatus::Active,
            )],
            vec![],
            vec![],
            vec![],
            today,
        );
        assert_eq!(
            variable_empty.records["billing/tunely.md"].monthly_cents,
            1500
        );

        let variable = ledger(
            vec![tunely(
                1500,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Variable,
                SubscriptionStatus::Active,
            )],
            vec![
                txn(
                    "checking",
                    "v0000001",
                    date(2026, 9, 1),
                    -1000,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "v0000002",
                    date(2026, 10, 1),
                    -2000,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "v0000003",
                    date(2026, 11, 1),
                    -3000,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
            ],
            vec![],
            vec![],
            today,
        );
        // Window is 2026-08-15..=2026-11-15; 1000+2000+3000 / 3 = 2000.
        assert_eq!(variable.records["billing/tunely.md"].monthly_cents, 2000);

        let cancelled = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Cancelled,
            )],
            vec![txn(
                "checking",
                "z0000001",
                date(2026, 10, 1),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        assert_eq!(cancelled.records["billing/tunely.md"].monthly_cents, 0);
    }

    #[test]
    fn refunds_net_and_never_enter_charge_facts() {
        let today = date(2026, 11, 15);
        let snapshot = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![
                txn(
                    "checking",
                    "c0000001",
                    date(2026, 9, 1),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "c0000002",
                    date(2026, 10, 1),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
            ],
            vec![row(
                "checking",
                date(2026, 10, 15),
                400,
                "Tunely credit",
                Some("refund"),
                &[],
                None,
                15,
            )],
            vec![],
            today,
        );
        let rec = &snapshot.records["billing/tunely.md"];
        assert_eq!(rec.charges.len(), 2);
        assert_eq!(rec.refunds.len(), 1);
        assert_eq!(rec.gross_cents, 1998);
        assert_eq!(rec.refund_cents, 400);
        assert_eq!(rec.net_cents, 1598);
        assert_eq!(
            rec.latest.as_ref().map(|c| c.date),
            Some(date(2026, 10, 1))
        );
        assert!(!rec.refunds.iter().any(|refund| refund.cents < 0));
    }

    #[test]
    fn price_change_needs_two_equal_monthly_charges_and_ignores_proration() {
        let today = date(2026, 11, 15);
        let confirmed = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![
                txn(
                    "checking",
                    "p0000001",
                    date(2026, 9, 1),
                    -1299,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "p0000002",
                    date(2026, 10, 1),
                    -1299,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
            ],
            vec![],
            vec![],
            today,
        );
        let change = confirmed.records["billing/tunely.md"]
            .price_change
            .clone()
            .expect("price change");
        assert_eq!(change.from_cents, 999);
        assert_eq!(change.to_cents, 1299);
        assert_eq!(change.since, date(2026, 10, 1));

        let proration = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![
                txn(
                    "checking",
                    "p0000003",
                    date(2026, 9, 1),
                    -499,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "p0000004",
                    date(2026, 10, 1),
                    -1299,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
            ],
            vec![],
            vec![],
            today,
        );
        assert_eq!(proration.records["billing/tunely.md"].price_change, None);

        let yearly = ledger(
            vec![tunely(
                12000,
                SubscriptionCadence::Yearly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![txn(
                "checking",
                "p0000005",
                date(2026, 1, 1),
                -15000,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        assert_eq!(
            yearly.records["billing/tunely.md"]
                .price_change
                .as_ref()
                .map(|change| change.to_cents),
            Some(15000)
        );

        let variable = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Variable,
                SubscriptionStatus::Active,
            )],
            vec![
                txn(
                    "checking",
                    "p0000006",
                    date(2026, 9, 1),
                    -2000,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "p0000007",
                    date(2026, 10, 1),
                    -2000,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
            ],
            vec![],
            vec![],
            today,
        );
        assert_eq!(variable.records["billing/tunely.md"].price_change, None);
    }

    #[test]
    fn still_charging_and_ended_strictness() {
        let mut rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::ToCancel,
        );
        let inside = ledger(
            vec![rec.clone()],
            vec![txn(
                "checking",
                "s0000001",
                date(2026, 10, 1),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            date(2026, 11, 15),
        );
        assert!(inside.records["billing/tunely.md"].still_charging);

        let outside = ledger(
            vec![rec.clone()],
            vec![txn(
                "checking",
                "s0000002",
                date(2026, 9, 1),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            date(2026, 11, 15),
        );
        assert!(!outside.records["billing/tunely.md"].still_charging);

        rec.status = SubscriptionStatus::Cancelled;
        rec.ended = Some(date(2026, 10, 1));
        let on_ended = ledger(
            vec![rec.clone()],
            vec![txn(
                "checking",
                "s0000003",
                date(2026, 10, 1),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            date(2026, 11, 15),
        );
        assert_eq!(on_ended.records["billing/tunely.md"].after_cancel, None);

        let after = ledger(
            vec![rec.clone()],
            vec![txn(
                "checking",
                "s0000004",
                date(2026, 10, 2),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            date(2026, 11, 15),
        );
        assert_eq!(
            after.records["billing/tunely.md"]
                .after_cancel
                .as_ref()
                .map(|charge| charge.date),
            Some(date(2026, 10, 2))
        );

        rec.ended = None;
        let no_ended = ledger(
            vec![rec],
            vec![txn(
                "checking",
                "s0000005",
                date(2026, 1, 1),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            date(2026, 11, 15),
        );
        assert!(no_ended.records["billing/tunely.md"].after_cancel.is_some());
    }

    #[test]
    fn timeline_states_coverage_gaps_and_no_evidence() {
        let today = date(2026, 11, 15);
        let rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        let empty = ledger(vec![rec.clone()], vec![], vec![], vec![], today);
        assert!(empty.records["billing/tunely.md"].timeline.is_empty());
        assert_eq!(empty.records["billing/tunely.md"].latest, None);
        assert_eq!(empty.records["billing/tunely.md"].next_expected, None);

        let snapshot = ledger(
            vec![rec],
            vec![txn(
                "checking",
                "t0000001",
                date(2026, 8, 15),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![
                import("checking", date(2026, 8, 1), date(2026, 9, 30)),
                import("checking", date(2026, 11, 1), date(2026, 11, 30)),
            ],
            today,
        );
        let timeline = &snapshot.records["billing/tunely.md"].timeline;
        assert_eq!(timeline.len(), 12);
        let current = timeline.last().expect("current");
        assert_eq!(current.due, date(2026, 11, 15));
        assert_eq!(current.state, TimelineState::Upcoming);
        assert_eq!(current.state.as_str(), "upcoming");
        assert_eq!(TimelineState::Charged.as_str(), "charged");
        assert_eq!(TimelineState::Missed.as_str(), "missed");
        assert_eq!(TimelineState::Uncovered.as_str(), "uncovered");
        let by_due = timeline
            .iter()
            .map(|bucket| (bucket.due, bucket.state))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            by_due.get(&date(2026, 8, 15)),
            Some(&TimelineState::Charged)
        );
        assert_eq!(
            by_due.get(&date(2026, 9, 15)),
            Some(&TimelineState::Missed)
        );
        assert_eq!(
            by_due.get(&date(2026, 10, 15)),
            Some(&TimelineState::Uncovered)
        );
        assert_eq!(
            by_due.get(&date(2026, 11, 15)),
            Some(&TimelineState::Upcoming)
        );
    }

    #[test]
    fn multi_account_missed_requires_coverage_on_every_observed_account() {
        let today = date(2026, 11, 15);
        let rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        let snapshot = ledger(
            vec![rec],
            vec![
                txn(
                    "checking",
                    "a0000001",
                    date(2026, 8, 15),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "savings",
                    "a0000002",
                    date(2026, 8, 20),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
            ],
            vec![],
            vec![
                import("checking", date(2026, 8, 1), date(2026, 11, 30)),
                import("savings", date(2026, 8, 1), date(2026, 8, 31)),
            ],
            today,
        );
        let by_due = snapshot.records["billing/tunely.md"]
            .timeline
            .iter()
            .map(|bucket| (bucket.due, bucket.state))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            by_due.get(&date(2026, 9, 20)),
            Some(&TimelineState::Uncovered)
        );
    }

    #[test]
    fn same_day_charges_order_deterministically_and_index_rows() {
        let today = date(2026, 11, 15);
        let rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        let day = date(2026, 10, 1);
        let snapshot = ledger(
            vec![rec],
            vec![
                txn(
                    "checking",
                    "d0000002",
                    day,
                    -500,
                    "TUNELY",
                    TxnOutcome::Written,
                ),
                txn(
                    "checking",
                    "d0000001",
                    day,
                    -400,
                    "TUNELY",
                    TxnOutcome::Written,
                ),
            ],
            vec![
                row(
                    "checking",
                    day,
                    -400,
                    "Tunely",
                    None,
                    &[],
                    Some("d0000001"),
                    4,
                ),
                row(
                    "checking",
                    day,
                    -500,
                    "Tunely",
                    None,
                    &[],
                    Some("d0000002"),
                    5,
                ),
            ],
            vec![],
            today,
        );
        let charges = &snapshot.records["billing/tunely.md"].charges;
        assert_eq!(
            charges.iter().map(|c| c.cents).collect::<Vec<_>>(),
            vec![-400, -500]
        );
        assert!(snapshot
            .by_row
            .contains_key(&("bank/2026-10.md".to_owned(), 4)));
        assert!(snapshot.by_id.contains_key("d0000001"));
    }
}

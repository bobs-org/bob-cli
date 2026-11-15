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

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;

use crate::native::bank::{
    config::BankConfig, journal::JournalSnapshot, model::BankRow,
};

use super::{
    attribution::{
        attribute, AttributedTxn, AttributionDiagnostic, AttributionSnapshot,
        AttributionSource,
    },
    cadence::{
        add_cadence_steps, current_step, div_round_half_up,
        mul_div_round_half_up, within_one_and_half_steps,
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
    pub(crate) gross_cents: Option<i64>,
    pub(crate) refund_cents: Option<i64>,
    pub(crate) net_cents: Option<i64>,
    pub(crate) latest: Option<LedgerCharge>,
    pub(crate) next_expected: Option<NaiveDate>,
    pub(crate) monthly_cents: Option<i64>,
    pub(crate) price_change: Option<PriceChange>,
    pub(crate) still_charging: bool,
    pub(crate) after_cancel: Option<LedgerCharge>,
    pub(crate) after_cancel_charges: Vec<LedgerCharge>,
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
    config: Option<&BankConfig>,
    raw_descs: &HashMap<String, String>,
) -> LedgerSnapshot {
    let coverage = CoverageIndex::from_journal(journal);
    let attributed =
        attribute(registry, journal, rows, &coverage, config, raw_descs);
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
    index_txn(
        attributed.transactions,
        &mut by_record,
        &mut by_row,
        &mut by_id,
        true,
    );
    index_txn(
        attributed.row_projections,
        &mut BTreeMap::new(),
        &mut by_row,
        &mut by_id,
        false,
    );

    let mut diagnostics = attributed.diagnostics;
    let mut records = BTreeMap::new();
    for record in &registry.records {
        let txns = by_record.remove(&record.path).unwrap_or_default();
        records.insert(
            record.path.clone(),
            record_ledger(record, &txns, &coverage, today, &mut diagnostics),
        );
    }
    LedgerSnapshot {
        records,
        by_row,
        by_id,
        coverage,
        diagnostics,
    }
}

fn index_txn(
    txns: Vec<AttributedTxn>,
    by_record: &mut BTreeMap<String, Vec<AttributedTxn>>,
    by_row: &mut BTreeMap<(String, usize), RowAttribution>,
    by_id: &mut BTreeMap<String, RowAttribution>,
    ledger_eligible: bool,
) {
    for txn in txns {
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
            by_id.entry(id).or_insert(projection);
        }
        if ledger_eligible && let Some(record_path) = txn.record_path.clone() {
            by_record.entry(record_path).or_default().push(txn);
        }
    }
}

fn record_ledger(
    record: &SubscriptionRecord,
    txns: &[AttributedTxn],
    coverage: &CoverageIndex,
    today: NaiveDate,
    diagnostics: &mut Vec<AttributionDiagnostic>,
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
    let refund_cents = sum_refunds(&refunds);
    let net_cents = match (gross_cents, refund_cents) {
        (Some(gross), Some(refund)) => gross.checked_sub(refund),
        _ => None,
    };
    let monthly_cents = monthly_equivalent(record, &charges, today);
    if gross_cents.is_none()
        || refund_cents.is_none()
        || net_cents.is_none()
        || monthly_cents.is_none()
    {
        diagnostics.push(AttributionDiagnostic {
            code: "bank_subscription_amount_overflow",
            path: record.path.clone(),
            message: format!(
                "subscription amounts for {} overflow signed 64-bit cents",
                record.path
            ),
        });
    }
    let latest = charges.last().cloned();
    let next_expected = next_expected_date(record, latest.as_ref());
    let price_change = price_change_for(record, &charges);
    let still_charging = record.status == SubscriptionStatus::ToCancel
        && latest.as_ref().is_some_and(|charge| {
            within_one_and_half_steps(charge.date, today, record.cadence)
        });
    let after_cancel_charges = after_cancel_charges(record, &charges);
    let after_cancel = after_cancel_charges.last().cloned();
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
        after_cancel_charges,
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

fn sum_magnitudes(charges: &[LedgerCharge]) -> Option<i64> {
    let mut total: i128 = 0;
    for charge in charges {
        let magnitude = i128::from(charge.cents).checked_neg()?;
        total = total.checked_add(magnitude)?;
    }
    i64::try_from(total).ok()
}

fn sum_refunds(refunds: &[LedgerCharge]) -> Option<i64> {
    let mut total: i128 = 0;
    for refund in refunds {
        total = total.checked_add(i128::from(refund.cents.max(0)))?;
    }
    i64::try_from(total).ok()
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
) -> Option<i64> {
    if record.status.is_cancelled() {
        return Some(0);
    }
    match record.pricing {
        SubscriptionPricing::Fixed => fixed_monthly(record),
        SubscriptionPricing::Variable => {
            variable_monthly(record.amount_cents, charges, today)
        }
    }
}

fn fixed_monthly(record: &SubscriptionRecord) -> Option<i64> {
    match record.cadence {
        SubscriptionCadence::Monthly => Some(record.amount_cents),
        SubscriptionCadence::FourWeekly => {
            mul_div_round_half_up(record.amount_cents, 13, 12)
        }
        SubscriptionCadence::Quarterly => {
            div_round_half_up(record.amount_cents, 3)
        }
        SubscriptionCadence::Yearly => {
            div_round_half_up(record.amount_cents, 12)
        }
    }
}

fn variable_monthly(
    amount_cents: i64,
    charges: &[LedgerCharge],
    today: NaiveDate,
) -> Option<i64> {
    let Some(start) =
        add_cadence_steps(today, -3, SubscriptionCadence::Monthly)
    else {
        return Some(amount_cents);
    };
    let mut total: i128 = 0;
    let mut any = false;
    for charge in charges {
        if charge.date >= start && charge.date <= today {
            any = true;
            let magnitude = i128::from(charge.cents).checked_neg()?;
            total = total.checked_add(magnitude)?;
        }
    }
    if !any {
        return Some(amount_cents);
    }
    div_round_half_up(i64::try_from(total).ok()?, 3)
}

fn price_change_for(
    record: &SubscriptionRecord,
    charges: &[LedgerCharge],
) -> Option<PriceChange> {
    if record.pricing != SubscriptionPricing::Fixed {
        return None;
    }
    let latest = charges.last()?;
    let latest_magnitude = latest.cents.checked_neg()?;
    match record.cadence {
        SubscriptionCadence::Monthly | SubscriptionCadence::FourWeekly => {
            let previous = charges.get(charges.len().checked_sub(2)?)?;
            let previous_magnitude = previous.cents.checked_neg()?;
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

fn after_cancel_charges(
    record: &SubscriptionRecord,
    charges: &[LedgerCharge],
) -> Vec<LedgerCharge> {
    if record.status != SubscriptionStatus::Cancelled {
        return Vec::new();
    }
    charges
        .iter()
        .filter(|charge| match record.ended {
            Some(ended) => charge.date > ended,
            None => true,
        })
        .cloned()
        .collect()
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
        let Some(n) = current_n.checked_add(offset as i32 - 11) else {
            continue;
        };
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
            subscription: None,
            bank_recurring: false,
            subscription_candidate: false,
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
        compute_ledger(
            &registry(records),
            &journal,
            &rows,
            today,
            None,
            &HashMap::new(),
        )
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
        assert_eq!(
            fixed.records["billing/tunely.md"].monthly_cents,
            Some(1082)
        );
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
        assert_eq!(
            quarterly.records["billing/tunely.md"].monthly_cents,
            Some(34)
        );

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
        assert_eq!(yearly.records["billing/tunely.md"].monthly_cents, Some(1));

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
            Some(1500)
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
        assert_eq!(
            variable.records["billing/tunely.md"].monthly_cents,
            Some(2000)
        );

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
        assert_eq!(
            cancelled.records["billing/tunely.md"].monthly_cents,
            Some(0)
        );
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
        assert_eq!(rec.gross_cents, Some(1998));
        assert_eq!(rec.refund_cents, Some(400));
        assert_eq!(rec.net_cents, Some(1598));
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

    #[test]
    fn orphan_and_covered_rows_project_without_entering_totals() {
        let today = date(2026, 11, 15);
        let rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        let orphan = ledger(
            vec![rec.clone()],
            vec![],
            vec![row(
                "checking",
                date(2026, 11, 2),
                -999,
                "Tunely",
                None,
                &["[[billing/tunely]]"],
                Some("orphan01"),
                3,
            )],
            vec![],
            today,
        );
        let facts = &orphan.records["billing/tunely.md"];
        assert!(facts.charges.is_empty());
        assert_eq!(facts.gross_cents, Some(0));
        assert_eq!(facts.net_cents, Some(0));
        assert_eq!(facts.latest, None);
        assert_eq!(facts.price_change, None);
        assert!(facts.timeline.is_empty());
        assert_eq!(
            orphan
                .by_row
                .get(&("bank/2026-11.md".to_owned(), 3))
                .and_then(|attribution| attribution.record_path.as_deref()),
            Some("billing/tunely.md")
        );

        let covered = ledger(
            vec![rec],
            vec![],
            vec![row(
                "checking",
                date(2026, 9, 10),
                -999,
                "Edited payee",
                None,
                &["[[billing/tunely]]"],
                None,
                8,
            )],
            vec![import("checking", date(2026, 9, 1), date(2026, 9, 15))],
            today,
        );
        assert!(covered.records["billing/tunely.md"].charges.is_empty());
        assert_eq!(
            covered
                .by_row
                .get(&("bank/2026-09.md".to_owned(), 8))
                .and_then(|attribution| attribution.record_path.as_deref()),
            Some("billing/tunely.md")
        );
    }

    #[test]
    fn overflow_and_i64_min_suppress_money_facts() {
        let today = date(2026, 11, 15);
        let huge = tunely(
            1_000_000_000_000_000_000,
            SubscriptionCadence::FourWeekly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        let representable = ledger(vec![huge], vec![], vec![], vec![], today);
        assert_eq!(
            representable.records["billing/tunely.md"].monthly_cents,
            Some(1_083_333_333_333_333_333)
        );

        let overflowing = tunely(
            i64::MAX,
            SubscriptionCadence::FourWeekly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        let overflow = ledger(vec![overflowing], vec![], vec![], vec![], today);
        assert_eq!(overflow.records["billing/tunely.md"].monthly_cents, None);
        assert!(overflow.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "bank_subscription_amount_overflow"
        }));

        let min_charge = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![txn(
                "checking",
                "min00001",
                date(2026, 10, 1),
                i64::MIN,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        assert_eq!(min_charge.records["billing/tunely.md"].gross_cents, None);
        assert_eq!(min_charge.records["billing/tunely.md"].net_cents, None);
        assert!(min_charge.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "bank_subscription_amount_overflow"
        }));

        let refund_overflow = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![],
            vec![
                row(
                    "checking",
                    date(2026, 10, 15),
                    i64::MAX,
                    "Tunely credit",
                    Some("refund"),
                    &["[[billing/tunely]]"],
                    None,
                    15,
                ),
                row(
                    "checking",
                    date(2026, 10, 16),
                    i64::MAX,
                    "Tunely credit",
                    Some("refund"),
                    &["[[billing/tunely]]"],
                    None,
                    16,
                ),
            ],
            vec![],
            today,
        );
        assert_eq!(
            refund_overflow.records["billing/tunely.md"].refund_cents,
            None
        );
    }

    #[test]
    fn future_renews_and_old_anchors_keep_today_in_the_window() {
        let today = date(2026, 11, 15);
        let mut rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Active,
        );
        rec.renews = Some(date(2028, 1, 31));
        let future = ledger(vec![rec], vec![], vec![], vec![], today);
        let timeline = &future.records["billing/tunely.md"].timeline;
        assert_eq!(timeline.len(), 12);
        assert_eq!(
            timeline.last().map(|bucket| bucket.due),
            Some(date(2026, 11, 30))
        );
        assert_eq!(
            timeline.last().map(|bucket| bucket.state),
            Some(TimelineState::Upcoming)
        );
        assert!(timeline
            .iter()
            .all(|bucket| bucket.due <= date(2026, 11, 30)));

        let old = ledger(
            vec![tunely(
                999,
                SubscriptionCadence::Monthly,
                SubscriptionPricing::Fixed,
                SubscriptionStatus::Active,
            )],
            vec![txn(
                "checking",
                "old00001",
                date(2000, 1, 15),
                -999,
                "TUNELY",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
            today,
        );
        let old_timeline = &old.records["billing/tunely.md"].timeline;
        assert_eq!(old_timeline.len(), 12);
        assert_eq!(
            old_timeline.last().map(|bucket| bucket.due),
            Some(date(2026, 11, 15))
        );
    }

    #[test]
    fn after_cancel_charges_keep_every_ended_rule_hit() {
        let mut rec = tunely(
            999,
            SubscriptionCadence::Monthly,
            SubscriptionPricing::Fixed,
            SubscriptionStatus::Cancelled,
        );
        rec.ended = Some(date(2026, 9, 1));
        let snapshot = ledger(
            vec![rec],
            vec![
                txn(
                    "checking",
                    "ac000001",
                    date(2026, 8, 15),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "ac000002",
                    date(2026, 10, 2),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
                txn(
                    "checking",
                    "ac000003",
                    date(2026, 11, 2),
                    -999,
                    "TUNELY",
                    TxnOutcome::BeforeStart,
                ),
            ],
            vec![],
            vec![],
            date(2026, 11, 15),
        );
        let facts = &snapshot.records["billing/tunely.md"];
        assert_eq!(
            facts
                .after_cancel_charges
                .iter()
                .map(|charge| charge.date)
                .collect::<Vec<_>>(),
            vec![date(2026, 10, 2), date(2026, 11, 2)]
        );
        assert_eq!(
            facts.after_cancel.as_ref().map(|charge| charge.date),
            Some(date(2026, 11, 2))
        );
    }
}

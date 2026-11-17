//! Historical posted-balance resolver over bank journal evidence.
//!
//! Pure over config, journal, today, and timezone. Vault rows, categories,
//! and transfer filters never enter the arithmetic.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate};

use super::{
    config::{Account, AccountKind, BankConfig},
    grammar::render_amount,
    journal::{
        import_local_date_in, timestamp_local_date, ImportEvent,
        JournalSnapshot, TxnEvent,
    },
    report::{Period, PeriodKind},
    source::SourceKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BalanceState {
    None,
    Unverified,
    AwaitingImport,
    Gap,
    SoFar,
    Settled,
    AsOf,
}

impl BalanceState {
    #[allow(dead_code)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unverified => "unverified",
            Self::AwaitingImport => "awaiting_import",
            Self::Gap => "gap",
            Self::SoFar => "so_far",
            Self::Settled => "settled",
            Self::AsOf => "as_of",
        }
    }

    fn is_number(self) -> bool {
        matches!(self, Self::SoFar | Self::Settled | Self::AsOf)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BalanceReason {
    Future,
    PreCoverage,
    JournalUnreadable,
    JournalContradiction,
    CsvOverlap,
    NoAnchor,
    AnchorMismatch,
}

impl BalanceReason {
    #[allow(dead_code)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Future => "future",
            Self::PreCoverage => "pre_coverage",
            Self::JournalUnreadable => "journal_unreadable",
            Self::JournalContradiction => "journal_contradiction",
            Self::CsvOverlap => "csv_overlap",
            Self::NoAnchor => "no_anchor",
            Self::AnchorMismatch => "anchor_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenBalance {
    pub(crate) date: NaiveDate,
    pub(crate) cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoverageSpan {
    pub(crate) since: NaiveDate,
    pub(crate) through: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnchorSummary {
    pub(crate) count: u32,
    pub(crate) agree: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeriodBalance {
    pub(crate) alias: String,
    pub(crate) label: String,
    pub(crate) kind: AccountKind,
    pub(crate) hero: bool,
    pub(crate) state: BalanceState,
    pub(crate) reason: Option<BalanceReason>,
    pub(crate) cents: Option<i64>,
    pub(crate) as_of: Option<NaiveDate>,
    pub(crate) wanted: NaiveDate,
    pub(crate) stale: bool,
    pub(crate) open: Option<OpenBalance>,
    pub(crate) change_cents: Option<i64>,
    pub(crate) coverage: Option<CoverageSpan>,
    pub(crate) anchors: AnchorSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BalanceFinding {
    pub(crate) code: &'static str,
    pub(crate) severity: &'static str,
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) day: Option<NaiveDate>,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct EffectiveAnchor {
    as_of: NaiveDate,
    cents: i64,
}

#[derive(Debug, Clone)]
struct Span {
    start: NaiveDate,
    end: NaiveDate,
    reason: Option<BalanceReason>,
    unverified_days: BTreeSet<NaiveDate>,
    effective_anchors: Vec<EffectiveAnchor>,
    observation_count: u32,
    agree: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct AccountHistory {
    pub(crate) alias: String,
    pub(crate) label: String,
    pub(crate) kind: AccountKind,
    pub(crate) hero: bool,
    pub(crate) stale_after: u32,
    pub(crate) health: Option<BalanceReason>,
    pub(crate) findings: Vec<BalanceFinding>,
    spans: Vec<Span>,
    covered: BTreeSet<NaiveDate>,
    final_days: BTreeSet<NaiveDate>,
    counted_by_day: BTreeMap<NaiveDate, i64>,
}

#[derive(Clone, Copy)]
struct SourceFlags {
    csv: bool,
    ofx: bool,
}

impl SourceFlags {
    fn classify(self) -> TxnClass {
        match (self.csv, self.ofx) {
            (true, true) => TxnClass::Ambiguous,
            (true, false) => TxnClass::CsvOnly,
            (false, true) | (false, false) => TxnClass::Counted,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TxnClass {
    Counted,
    CsvOnly,
    Ambiguous,
}

struct Observation {
    cents: i64,
    ts: DateTime<FixedOffset>,
    provisional: bool,
    import_start: NaiveDate,
    import_end: NaiveDate,
}

pub(crate) fn histories(
    config: &BankConfig,
    journal: &JournalSnapshot,
    timezone: Option<chrono_tz::Tz>,
) -> Vec<AccountHistory> {
    config
        .accounts
        .iter()
        .filter(|account| account.kind.importable())
        .map(|account| history_for(account, config, journal, timezone))
        .collect()
}

pub(crate) fn history_for(
    account: &Account,
    config: &BankConfig,
    journal: &JournalSnapshot,
    timezone: Option<chrono_tz::Tz>,
) -> AccountHistory {
    let hero = config
        .accounts
        .iter()
        .find(|candidate| candidate.kind == AccountKind::Checking)
        .is_some_and(|hero| hero.alias == account.alias);
    let health = health_reason(account, journal);
    let mut covered = BTreeSet::new();
    let mut final_days = BTreeSet::new();
    let account_imports = journal
        .imports
        .iter()
        .filter(|event| event.account == account.alias)
        .collect::<Vec<_>>();
    for event in account_imports.iter().copied() {
        if event.source != SourceKind::Ofx {
            continue;
        }
        let imported = import_local_date_in(event, timezone);
        each_day(event.start, event.end, |day| {
            covered.insert(day);
            if imported > day {
                final_days.insert(day);
            }
        });
    }
    let spans_bounds = merge_spans(&covered);
    let batch_sources = batch_sources(&account_imports);
    let mut counted_by_day = BTreeMap::new();
    let mut ambiguous = Vec::new();
    for event in journal.transactions.values() {
        if event.account != account.alias {
            continue;
        }
        match classify_txn(event, &batch_sources) {
            TxnClass::Counted => {
                *counted_by_day.entry(event.posted).or_insert(0) += event.cents;
            }
            TxnClass::CsvOnly => {}
            TxnClass::Ambiguous => ambiguous.push(event),
        }
    }

    let mut findings = Vec::new();
    let mut spans = Vec::new();
    for (start, end) in spans_bounds {
        let mut span = Span {
            start,
            end,
            reason: None,
            unverified_days: BTreeSet::new(),
            effective_anchors: Vec::new(),
            observation_count: 0,
            agree: true,
        };
        let mixed_batch = ambiguous
            .iter()
            .copied()
            .find(|event| event.posted >= start && event.posted <= end);
        if let Some(event) = mixed_batch {
            span.reason = Some(BalanceReason::CsvOverlap);
            let batch_day = timestamp_local_date(event.ts, timezone);
            findings.push(csv_overlap_finding(
                &account.alias,
                batch_day,
                start,
                end,
            ));
        }
        fill_anchors(
            &mut span,
            &account_imports,
            timezone,
            &counted_by_day,
            &account.alias,
            &mut findings,
        );
        if span.reason.is_none() && span.effective_anchors.is_empty() {
            span.reason = Some(BalanceReason::NoAnchor);
        }
        spans.push(span);
    }

    AccountHistory {
        alias: account.alias.clone(),
        label: account.label.clone(),
        kind: account.kind,
        hero,
        stale_after: config.stale_after,
        health,
        findings,
        spans,
        covered,
        final_days,
        counted_by_day,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn resolve(
    history: &AccountHistory,
    period: &Period,
    today: NaiveDate,
) -> PeriodBalance {
    let wanted = if period.so_far { today } else { period.end };
    let empty_anchors = AnchorSummary {
        count: 0,
        agree: true,
    };
    if period.start > today {
        return PeriodBalance {
            alias: history.alias.clone(),
            label: history.label.clone(),
            kind: history.kind,
            hero: history.hero,
            state: BalanceState::None,
            reason: Some(BalanceReason::Future),
            cents: None,
            as_of: None,
            wanted,
            stale: false,
            open: None,
            change_cents: None,
            coverage: None,
            anchors: empty_anchors,
        };
    }
    if let Some(reason) = history.health {
        return unverified_balance(history, wanted, None, reason);
    }
    let as_of = if period.so_far {
        latest_on_or_before(&history.covered, wanted)
    } else {
        latest_on_or_before(&history.final_days, wanted)
    };
    let Some(as_of) = as_of else {
        return PeriodBalance {
            alias: history.alias.clone(),
            label: history.label.clone(),
            kind: history.kind,
            hero: history.hero,
            state: BalanceState::None,
            reason: Some(BalanceReason::PreCoverage),
            cents: None,
            as_of: None,
            wanted,
            stale: false,
            open: None,
            change_cents: None,
            coverage: None,
            anchors: empty_anchors,
        };
    };
    if let Some(reason) = unverified_at(history, as_of) {
        return unverified_balance(history, wanted, Some(as_of), reason);
    }
    if !period.so_far && as_of < period.start {
        let resumes = history.covered.iter().any(|day| *day > wanted);
        let state = if resumes {
            BalanceState::Gap
        } else {
            BalanceState::AwaitingImport
        };
        return PeriodBalance {
            alias: history.alias.clone(),
            label: history.label.clone(),
            kind: history.kind,
            hero: history.hero,
            state,
            reason: None,
            cents: None,
            as_of: Some(as_of),
            wanted,
            stale: false,
            open: None,
            change_cents: None,
            coverage: coverage_holding(history, as_of),
            anchors: anchors_holding(history, as_of),
        };
    }
    let state = if period.so_far {
        BalanceState::SoFar
    } else if as_of == wanted {
        BalanceState::Settled
    } else {
        BalanceState::AsOf
    };
    let cents = balance_on(history, as_of);
    let stale = state == BalanceState::SoFar
        && today.signed_duration_since(as_of)
            > Duration::days(i64::from(history.stale_after));
    let (open, change_cents) =
        open_and_change(history, period, as_of, cents, state);
    PeriodBalance {
        alias: history.alias.clone(),
        label: history.label.clone(),
        kind: history.kind,
        hero: history.hero,
        state,
        reason: None,
        cents,
        as_of: Some(as_of),
        wanted,
        stale,
        open,
        change_cents,
        coverage: coverage_holding(history, as_of),
        anchors: anchors_holding(history, as_of),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn balance_on(
    history: &AccountHistory,
    day: NaiveDate,
) -> Option<i64> {
    let span = span_for_balance(history, day)?;
    if span.effective_anchors.is_empty() {
        return None;
    }
    if let Some(anchor) = span
        .effective_anchors
        .iter()
        .find(|anchor| anchor.as_of >= day)
    {
        Some(anchor.cents - posted_between(history, day, anchor.as_of))
    } else {
        let last = span.effective_anchors.last()?;
        Some(last.cents + posted_between(history, last.as_of, day))
    }
}

fn health_reason(
    account: &Account,
    journal: &JournalSnapshot,
) -> Option<BalanceReason> {
    if !journal.io_errors.is_empty() || !journal.malformed.is_empty() {
        return Some(BalanceReason::JournalUnreadable);
    }
    let attributed = journal.contradictions.iter().any(|item| {
        match journal.transactions.get(&item.key) {
            Some(event) => event.account == account.alias,
            None => true,
        }
    });
    attributed.then_some(BalanceReason::JournalContradiction)
}

fn batch_sources<'a>(
    imports: &'a [&ImportEvent],
) -> HashMap<(&'a str, DateTime<FixedOffset>), SourceFlags> {
    let mut sources = HashMap::new();
    for event in imports {
        let flags = sources.entry((event.host.as_str(), event.ts)).or_insert(
            SourceFlags {
                csv: false,
                ofx: false,
            },
        );
        match event.source {
            SourceKind::Csv => flags.csv = true,
            SourceKind::Ofx => flags.ofx = true,
        }
    }
    sources
}

fn classify_txn(
    event: &TxnEvent,
    sources: &HashMap<(&str, DateTime<FixedOffset>), SourceFlags>,
) -> TxnClass {
    sources
        .get(&(event.host.as_str(), event.ts))
        .copied()
        .map_or(TxnClass::Counted, SourceFlags::classify)
}

fn merge_spans(covered: &BTreeSet<NaiveDate>) -> Vec<(NaiveDate, NaiveDate)> {
    let mut spans = Vec::new();
    let mut iter = covered.iter().copied();
    let Some(first) = iter.next() else {
        return spans;
    };
    let mut start = first;
    let mut prev = first;
    for day in iter {
        if prev.succ_opt() != Some(day) {
            spans.push((start, prev));
            start = day;
        }
        prev = day;
    }
    spans.push((start, prev));
    spans
}

fn fill_anchors(
    span: &mut Span,
    imports: &[&ImportEvent],
    timezone: Option<chrono_tz::Tz>,
    counted_by_day: &BTreeMap<NaiveDate, i64>,
    alias: &str,
    findings: &mut Vec<BalanceFinding>,
) {
    let mut by_date: BTreeMap<NaiveDate, Vec<Observation>> = BTreeMap::new();
    for event in imports {
        if event.source != SourceKind::Ofx {
            continue;
        }
        let Some(cents) = event.balance_cents else {
            continue;
        };
        let Some(as_of) = event.balance_asof else {
            continue;
        };
        if as_of < span.start || as_of > span.end {
            continue;
        }
        span.observation_count += 1;
        let imported = import_local_date_in(event, timezone);
        by_date.entry(as_of).or_default().push(Observation {
            cents,
            ts: event.ts,
            provisional: as_of >= imported,
            import_start: event.start,
            import_end: event.end,
        });
    }

    let mut disputes = Vec::new();
    for (as_of, observations) in &by_date {
        let candidates = observations
            .iter()
            .filter(|observation| {
                !provisional_superseded(*as_of, observation, &by_date)
            })
            .collect::<Vec<_>>();
        let Some(effective) =
            candidates.iter().copied().max_by_key(|item| item.ts)
        else {
            continue;
        };
        span.effective_anchors.push(EffectiveAnchor {
            as_of: *as_of,
            cents: effective.cents,
        });
        for earlier in candidates
            .iter()
            .copied()
            .filter(|item| item.ts < effective.ts)
        {
            if earlier.provisional {
                continue;
            }
            if earlier.cents != effective.cents {
                disputes.push((*as_of, earlier.cents, effective.cents));
                break;
            }
        }
    }
    span.effective_anchors.sort_by_key(|anchor| anchor.as_of);

    for window in span.effective_anchors.windows(2) {
        let left = window[0];
        let right = window[1];
        let reconstructed =
            right.cents - posted_in(counted_by_day, left.as_of, right.as_of);
        if reconstructed == left.cents {
            continue;
        }
        span.agree = false;
        each_day_exclusive(left.as_of, right.as_of, |day| {
            span.unverified_days.insert(day);
        });
        findings.push(mismatch_finding(
            alias,
            left.as_of,
            left.cents,
            reconstructed,
            left.as_of.succ_opt(),
            right.as_of.pred_opt(),
        ));
    }

    for (as_of, bank_cents, reconstructed) in disputes {
        span.agree = false;
        span.unverified_days.insert(as_of);
        let index = span
            .effective_anchors
            .iter()
            .position(|anchor| anchor.as_of == as_of);
        let prev = index
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| span.effective_anchors.get(index));
        let next = index
            .and_then(|index| index.checked_add(1))
            .and_then(|index| span.effective_anchors.get(index));
        let (hide_from, hide_to) = match (prev, next) {
            (Some(left), Some(right)) => {
                each_day_exclusive(left.as_of, right.as_of, |day| {
                    span.unverified_days.insert(day);
                });
                (left.as_of.succ_opt(), right.as_of.pred_opt())
            }
            (Some(left), None) => {
                each_day_exclusive(left.as_of, as_of, |day| {
                    span.unverified_days.insert(day);
                });
                (left.as_of.succ_opt(), Some(as_of))
            }
            (None, Some(right)) => {
                each_day_exclusive(as_of, right.as_of, |day| {
                    span.unverified_days.insert(day);
                });
                (Some(as_of), right.as_of.pred_opt())
            }
            (None, None) => (Some(as_of), Some(as_of)),
        };
        findings.push(mismatch_finding(
            alias,
            as_of,
            bank_cents,
            reconstructed,
            hide_from,
            hide_to,
        ));
    }
}

/// A later final OFX anchor retires an intraday snapshot only when that import
/// itself covered the snapshot date. A later balance elsewhere in the span is
/// not enough to erase the snapshot's evidence.
fn provisional_superseded(
    as_of: NaiveDate,
    provisional: &Observation,
    by_date: &BTreeMap<NaiveDate, Vec<Observation>>,
) -> bool {
    if !provisional.provisional {
        return false;
    }
    by_date
        .range((Bound::Excluded(as_of), Bound::Unbounded))
        .any(|(later_as_of, observations)| {
            observations.iter().any(|later| {
                !later.provisional
                    && later.ts > provisional.ts
                    && later.import_start <= as_of
                    && as_of <= later.import_end
                    && later.import_start <= *later_as_of
                    && *later_as_of <= later.import_end
            })
        })
}

fn posted_between(
    history: &AccountHistory,
    from_exclusive: NaiveDate,
    through_inclusive: NaiveDate,
) -> i64 {
    posted_in(&history.counted_by_day, from_exclusive, through_inclusive)
}

fn posted_in(
    counted_by_day: &BTreeMap<NaiveDate, i64>,
    from_exclusive: NaiveDate,
    through_inclusive: NaiveDate,
) -> i64 {
    counted_by_day
        .range((
            Bound::Excluded(from_exclusive),
            Bound::Included(through_inclusive),
        ))
        .map(|(_, cents)| *cents)
        .sum()
}

fn span_for_balance(history: &AccountHistory, day: NaiveDate) -> Option<&Span> {
    history.spans.iter().find(|span| {
        let open = span.start.pred_opt().unwrap_or(span.start);
        open <= day && day <= span.end
    })
}

fn span_holding(history: &AccountHistory, day: NaiveDate) -> Option<&Span> {
    history
        .spans
        .iter()
        .find(|span| day >= span.start && day <= span.end)
}

fn unverified_at(
    history: &AccountHistory,
    day: NaiveDate,
) -> Option<BalanceReason> {
    let span = span_holding(history, day)?;
    if let Some(reason) = span.reason {
        return Some(reason);
    }
    span.unverified_days
        .contains(&day)
        .then_some(BalanceReason::AnchorMismatch)
}

fn coverage_holding(
    history: &AccountHistory,
    day: NaiveDate,
) -> Option<CoverageSpan> {
    span_holding(history, day).map(|span| CoverageSpan {
        since: span.start,
        through: span.end,
    })
}

fn anchors_holding(history: &AccountHistory, day: NaiveDate) -> AnchorSummary {
    span_holding(history, day)
        .map(|span| AnchorSummary {
            count: span.observation_count,
            agree: span.agree,
        })
        .unwrap_or(AnchorSummary {
            count: 0,
            agree: true,
        })
}

fn latest_on_or_before(
    days: &BTreeSet<NaiveDate>,
    wanted: NaiveDate,
) -> Option<NaiveDate> {
    days.range(..=wanted).next_back().copied()
}

fn open_and_change(
    history: &AccountHistory,
    period: &Period,
    as_of: NaiveDate,
    cents: Option<i64>,
    state: BalanceState,
) -> (Option<OpenBalance>, Option<i64>) {
    if period.kind == PeriodKind::AllTime
        || as_of < period.start
        || !state.is_number()
    {
        return (None, None);
    }
    let Some(open_date) = period.start.pred_opt() else {
        return (None, None);
    };
    if unverified_at(history, open_date).is_some() {
        return (None, None);
    }
    if let Some(span) = span_holding(history, open_date)
        && span.reason.is_some()
    {
        return (None, None);
    }
    let Some(open_cents) = balance_on(history, open_date) else {
        return (None, None);
    };
    let open = OpenBalance {
        date: open_date,
        cents: open_cents,
    };
    let change = cents.map(|value| value - open_cents);
    (Some(open), change)
}

fn unverified_balance(
    history: &AccountHistory,
    wanted: NaiveDate,
    as_of: Option<NaiveDate>,
    reason: BalanceReason,
) -> PeriodBalance {
    PeriodBalance {
        alias: history.alias.clone(),
        label: history.label.clone(),
        kind: history.kind,
        hero: history.hero,
        state: BalanceState::Unverified,
        reason: Some(reason),
        cents: None,
        as_of,
        wanted,
        stale: false,
        open: None,
        change_cents: None,
        coverage: as_of.and_then(|day| coverage_holding(history, day)),
        anchors: as_of.map(|day| anchors_holding(history, day)).unwrap_or(
            AnchorSummary {
                count: 0,
                agree: true,
            },
        ),
    }
}

fn csv_overlap_finding(
    alias: &str,
    batch_day: NaiveDate,
    start: NaiveDate,
    end: NaiveDate,
) -> BalanceFinding {
    BalanceFinding {
        code: "bank_balance_csv_overlap",
        severity: "info",
        path: "bank.md".to_owned(),
        line: None,
        day: Some(batch_day),
        message: format!(
            "{alias} mixed CSV and QFX import on {batch_day} hides {start}–{end}"
        ),
        hint: Some(
            "Import CSV and QFX files in separate runs.".to_owned(),
        ),
    }
}

fn mismatch_finding(
    alias: &str,
    anchor_date: NaiveDate,
    bank_cents: i64,
    reconstructed: i64,
    hide_from: Option<NaiveDate>,
    hide_to: Option<NaiveDate>,
) -> BalanceFinding {
    let hidden = match (hide_from, hide_to) {
        (Some(from), Some(to)) if from <= to => {
            if from == to {
                format!("hides {from}")
            } else {
                format!("hides {from}–{to}")
            }
        }
        _ => "hides no days".to_owned(),
    };
    BalanceFinding {
        code: "bank_balance_anchor_mismatch",
        severity: "warning",
        path: "bank.md".to_owned(),
        line: None,
        day: Some(anchor_date),
        message: format!(
            "{alias} bank balance on {anchor_date} is {}; reconstructed {} {hidden}",
            render_amount(bank_cents),
            render_amount(reconstructed)
        ),
        hint: Some(
            "Usually a bank correction or a re-keyed transaction.".to_owned(),
        ),
    }
}

fn each_day(
    start: NaiveDate,
    end: NaiveDate,
    mut visit: impl FnMut(NaiveDate),
) {
    if start > end {
        return;
    }
    let mut day = start;
    loop {
        visit(day);
        if day >= end {
            break;
        }
        let Some(next) = day.succ_opt() else {
            break;
        };
        day = next;
    }
}

fn each_day_exclusive(
    start: NaiveDate,
    end: NaiveDate,
    visit: impl FnMut(NaiveDate),
) {
    let Some(first) = start.succ_opt() else {
        return;
    };
    if first >= end {
        return;
    }
    let Some(last) = end.pred_opt() else {
        return;
    };
    each_day(first, last, visit);
}

#[cfg(test)]
#[path = "balance_tests.rs"]
mod tests;

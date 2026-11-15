//! Deduplicated journal-and-row attribution for subscriptions.
//!
//! Every folded journal transaction is counted once, including
//! `before_start`. A unique current bank row supplies authored links and
//! category; the journal supplies the masked descriptor and identity. ID-less
//! typed rows join only when they sit outside that account's merged import
//! windows. A unique explicit subscription link wins over every pattern and
//! every inference exclusion. Pattern inference never applies to zero
//! amounts, non-refund inflows, `transfer`/`income`/`cash` rows, or Wells
//! Fargo built-in transfers, cash withdrawals, or fees.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use crate::native::bank::{
    journal::{JournalSnapshot, TxnEvent, TxnOutcome},
    model::BankRow,
    naming::classify_builtin,
};

use super::{
    coverage::CoverageIndex,
    matching::resolve_match,
    registry::{SubscriptionRecord, SubscriptionRegistry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttributionSource {
    Link,
    Match,
}

impl AttributionSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::Match => "match",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttributionDiagnostic {
    pub(crate) code: &'static str,
    pub(crate) path: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttributedTxn {
    pub(crate) date: NaiveDate,
    pub(crate) cents: i64,
    pub(crate) account: String,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) id: Option<String>,
    pub(crate) journal_only: bool,
    pub(crate) outcome: Option<TxnOutcome>,
    pub(crate) record_path: Option<String>,
    pub(crate) source: Option<AttributionSource>,
    pub(crate) needle: Option<String>,
    pub(crate) tied_paths: Vec<String>,
    pub(crate) category: Option<String>,
    pub(crate) is_refund: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AttributionSnapshot {
    pub(crate) transactions: Vec<AttributedTxn>,
    pub(crate) diagnostics: Vec<AttributionDiagnostic>,
}

struct UniverseTxn {
    date: NaiveDate,
    cents: i64,
    account: String,
    path: Option<String>,
    line: Option<usize>,
    id: Option<String>,
    journal_only: bool,
    outcome: Option<TxnOutcome>,
    desc: String,
    category: Option<String>,
    links: Vec<String>,
}

pub(crate) fn attribute(
    registry: &SubscriptionRegistry,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    coverage: &CoverageIndex,
) -> AttributionSnapshot {
    let universe = collect_universe(journal, rows, coverage);
    let mut diagnostics = universe_diagnostics(rows);
    let records = &registry.records;
    let mut transactions = universe
        .into_iter()
        .map(|txn| resolve_txn(txn, records, &mut diagnostics))
        .collect::<Vec<_>>();
    transactions.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then(left.account.cmp(&right.account))
            .then(left.journal_only.cmp(&right.journal_only))
            .then(left.path.cmp(&right.path))
            .then(left.line.cmp(&right.line))
            .then(left.id.cmp(&right.id))
            .then(left.cents.cmp(&right.cents))
    });
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.code.cmp(right.code))
            .then_with(|| left.message.cmp(&right.message))
    });
    AttributionSnapshot {
        transactions,
        diagnostics,
    }
}

fn collect_universe(
    journal: &JournalSnapshot,
    rows: &[BankRow],
    coverage: &CoverageIndex,
) -> Vec<UniverseTxn> {
    let mut by_id = BTreeMap::<&str, Vec<&BankRow>>::new();
    for row in rows {
        if let Some(id) = row.id.as_deref() {
            by_id.entry(id).or_default().push(row);
        }
    }
    let mut consumed = BTreeSet::<(&str, usize)>::new();
    let mut universe = Vec::new();
    for event in journal.transactions.values() {
        match by_id.get(event.id.as_str()).map(Vec::as_slice) {
            Some([row]) => {
                consumed.insert((row.path.as_str(), row.line));
                universe.push(joined_txn(event, row));
            }
            Some(rows) if rows.len() > 1 => {
                for row in rows {
                    consumed.insert((row.path.as_str(), row.line));
                }
                universe.push(journal_only_txn(event));
            }
            Some(_) | None => universe.push(journal_only_txn(event)),
        }
    }
    let journal_ids = journal
        .transactions
        .values()
        .map(|event| event.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut orphan_ids = BTreeMap::<&str, Vec<&BankRow>>::new();
    for row in rows {
        if consumed.contains(&(row.path.as_str(), row.line)) {
            continue;
        }
        match row.id.as_deref() {
            Some(id) if journal_ids.contains(id) => {}
            Some(id) => orphan_ids.entry(id).or_default().push(row),
            None => {
                if !coverage.contains(&row.account, row.date) {
                    universe.push(typed_txn(row));
                }
            }
        }
    }
    for rows_for_id in orphan_ids.values() {
        if let [row] = rows_for_id.as_slice() {
            universe.push(typed_txn(row));
        }
    }
    universe
}

fn universe_diagnostics(rows: &[BankRow]) -> Vec<AttributionDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut id_rows = BTreeMap::<&str, Vec<&BankRow>>::new();
    for row in rows {
        if let Some(id) = row.id.as_deref() {
            id_rows.entry(id).or_default().push(row);
        }
    }
    for (id, matches) in id_rows {
        if matches.len() > 1 {
            let path = matches
                .first()
                .map(|row| row.path.clone())
                .unwrap_or_default();
            diagnostics.push(AttributionDiagnostic {
                code: "bank_subscription_row_id_ambiguous",
                path,
                message: format!(
                    "row id {id} appears on more than one bank row"
                ),
            });
        }
    }
    diagnostics
}

fn joined_txn(event: &TxnEvent, row: &BankRow) -> UniverseTxn {
    UniverseTxn {
        date: event.posted,
        cents: event.cents,
        account: event.account.clone(),
        path: Some(row.path.clone()),
        line: Some(row.line),
        id: Some(event.id.clone()),
        journal_only: false,
        outcome: Some(event.outcome),
        desc: event.desc.clone(),
        category: row.category.clone(),
        links: row.links.clone(),
    }
}

fn journal_only_txn(event: &TxnEvent) -> UniverseTxn {
    UniverseTxn {
        date: event.posted,
        cents: event.cents,
        account: event.account.clone(),
        path: None,
        line: None,
        id: Some(event.id.clone()),
        journal_only: true,
        outcome: Some(event.outcome),
        desc: event.desc.clone(),
        category: None,
        links: Vec::new(),
    }
}

fn typed_txn(row: &BankRow) -> UniverseTxn {
    UniverseTxn {
        date: row.date,
        cents: row.cents,
        account: row.account.clone(),
        path: Some(row.path.clone()),
        line: Some(row.line),
        id: row.id.clone(),
        journal_only: false,
        outcome: None,
        desc: row.payee.clone(),
        category: row.category.clone(),
        links: row.links.clone(),
    }
}

fn resolve_txn(
    txn: UniverseTxn,
    records: &[SubscriptionRecord],
    diagnostics: &mut Vec<AttributionDiagnostic>,
) -> AttributedTxn {
    let is_refund = txn.category.as_deref() == Some("refund");
    let explicit =
        resolve_explicit_links(&txn.links, records, &txn, diagnostics);
    let (record_path, source, needle, tied_paths) = match explicit {
        ExplicitResolution::Unique(path) => {
            (Some(path), Some(AttributionSource::Link), None, Vec::new())
        }
        ExplicitResolution::Ambiguous(tied) => (None, None, None, tied),
        ExplicitResolution::None => {
            if pattern_eligible(&txn, is_refund) {
                let resolved = resolve_match(records, &txn.desc);
                if let Some(path) = resolved.record_path {
                    (
                        Some(path),
                        Some(AttributionSource::Match),
                        resolved.needle,
                        Vec::new(),
                    )
                } else {
                    (None, None, None, resolved.tied_paths)
                }
            } else {
                (None, None, None, Vec::new())
            }
        }
    };
    AttributedTxn {
        date: txn.date,
        cents: txn.cents,
        account: txn.account,
        path: txn.path,
        line: txn.line,
        id: txn.id,
        journal_only: txn.journal_only,
        outcome: txn.outcome,
        record_path,
        source,
        needle,
        tied_paths,
        category: txn.category,
        is_refund,
    }
}

enum ExplicitResolution {
    None,
    Unique(String),
    Ambiguous(Vec<String>),
}

fn resolve_explicit_links(
    links: &[String],
    records: &[SubscriptionRecord],
    txn: &UniverseTxn,
    diagnostics: &mut Vec<AttributionDiagnostic>,
) -> ExplicitResolution {
    let mut matched = Vec::new();
    for link in links {
        let Some(target) = wikilink_note_target(link) else {
            continue;
        };
        if let Some(record) = matching_record(records, &target)
            && !matched.iter().any(|path| path == &record.path)
        {
            matched.push(record.path.clone());
        }
    }
    matched.sort();
    match matched.as_slice() {
        [] => ExplicitResolution::None,
        [path] => ExplicitResolution::Unique(path.clone()),
        _ => {
            diagnostics.push(AttributionDiagnostic {
                code: "bank_subscription_link_ambiguous",
                path: txn.path.clone().unwrap_or_default(),
                message: format!(
                    "conflicting subscription links {}",
                    matched.join(", ")
                ),
            });
            ExplicitResolution::Ambiguous(matched)
        }
    }
}

fn matching_record<'a>(
    records: &'a [SubscriptionRecord],
    target: &str,
) -> Option<&'a SubscriptionRecord> {
    let normalized = normalize_note_path(target);
    records.iter().find(|record| {
        normalize_note_path(&record.path) == normalized
            || normalize_note_path(&record.link_target) == normalized
    })
}

fn wikilink_note_target(link: &str) -> Option<String> {
    let inner = link.trim().strip_prefix("[[")?.strip_suffix("]]")?.trim();
    if inner.is_empty() || inner.contains("[[") || inner.contains("]]") {
        return None;
    }
    let before_alias =
        inner.split_once('|').map_or(inner, |(target, _)| target);
    let target = before_alias
        .split_once('#')
        .map_or(before_alias, |(target, _)| target)
        .trim();
    (!target.is_empty()).then(|| target.to_owned())
}

fn normalize_note_path(target: &str) -> String {
    let trimmed = target.trim().trim_start_matches("./");
    if trimmed.ends_with(".md") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.md")
    }
}

fn pattern_eligible(txn: &UniverseTxn, is_refund: bool) -> bool {
    if txn.cents == 0 {
        return false;
    }
    if is_refund {
        return true;
    }
    if txn.cents > 0 {
        return false;
    }
    if let Some("transfer" | "income" | "cash") = txn.category.as_deref() {
        return false;
    }
    if let Some(builtin) = classify_builtin(&txn.desc, None)
        && let Some("transfer" | "cash" | "fees") = builtin.category
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Datelike, NaiveDate};

    use crate::native::bank::{
        journal::{
            ImportCounts, ImportEvent, JournalSnapshot, TxnEvent, TxnOutcome,
        },
        model::BankRow,
        source::SourceKind,
        subscriptions::registry::{
            SubscriptionCadence, SubscriptionPricing, SubscriptionStatus,
        },
    };

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("date")
    }

    fn ts() -> DateTime<chrono::FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z").expect("ts")
    }

    fn record(path: &str, needles: &[&str]) -> SubscriptionRecord {
        record_status(path, SubscriptionStatus::Active, needles)
    }

    fn record_status(
        path: &str,
        status: SubscriptionStatus,
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
            amount_cents: 999,
            cadence: SubscriptionCadence::Monthly,
            pricing: SubscriptionPricing::Fixed,
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
            availability:
                crate::native::bank::subscriptions::registry::SubscriptionAvailability::Available,
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
    ) -> BankRow {
        BankRow {
            path: format!("bank/{}.md", day.format("%Y-%m")),
            line: day.day() as usize,
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
        let key = format!("{id:0<64}");
        TxnEvent {
            ts: ts(),
            host: "test".to_owned(),
            path: ".bob/bank/test.jsonl".to_owned(),
            line: 1,
            account: account.to_owned(),
            key,
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
            file_sha256: "b".repeat(64),
            start,
            end,
            balance_cents: None,
            balance_asof: None,
            counts: ImportCounts::default(),
        }
    }

    fn snapshot(
        records: Vec<SubscriptionRecord>,
        txns: Vec<TxnEvent>,
        rows: Vec<BankRow>,
        imports: Vec<ImportEvent>,
    ) -> AttributionSnapshot {
        let mut journal = JournalSnapshot::default();
        for event in txns {
            journal.ids.insert(event.id.clone(), event.key.clone());
            journal.transactions.insert(event.key.clone(), event);
        }
        journal.imports = imports;
        let coverage = CoverageIndex::from_journal(&journal);
        attribute(&registry(records), &journal, &rows, &coverage)
    }

    fn attributed_path(snapshot: &AttributionSnapshot) -> Vec<Option<&str>> {
        snapshot
            .transactions
            .iter()
            .map(|txn| txn.record_path.as_deref())
            .collect()
    }

    #[test]
    fn explicit_link_beats_match_and_accepts_renamed_album_paths() {
        let tunely = record("billing/tunely.md", &["CLOUDBOX"]);
        let cloudbox = record("billing/cloudbox.md", &["CLOUDBOX"]);
        let day = date(2026, 6, 2);
        let snapshot = snapshot(
            vec![tunely, cloudbox],
            vec![txn(
                "checking",
                "aaaaaaa1",
                day,
                -999,
                "CLOUDBOX PREMIUM",
                TxnOutcome::Written,
            )],
            vec![row(
                "checking",
                day,
                -999,
                "Cloudbox",
                None,
                &["[[billing/tunely.md|🔁]]"],
                Some("aaaaaaa1"),
            )],
            vec![],
        );
        assert_eq!(snapshot.transactions.len(), 1);
        let txn = &snapshot.transactions[0];
        assert_eq!(txn.record_path.as_deref(), Some("billing/tunely.md"));
        assert_eq!(txn.source, Some(AttributionSource::Link));
        assert_eq!(txn.source.map(AttributionSource::as_str), Some("link"));
        assert_eq!(txn.needle, None);
        assert!(!txn.journal_only);
    }

    #[test]
    fn alias_and_md_optional_links_resolve_to_the_same_record() {
        let tunely = record("billing/tunely.md", &["TUNELY"]);
        let day = date(2026, 6, 3);
        for link in [
            "[[billing/tunely]]",
            "[[billing/tunely.md]]",
            "[[billing/tunely|Tunely]]",
            "[[billing/tunely.md|🔁]]",
        ] {
            let snapshot = snapshot(
                vec![tunely.clone()],
                vec![],
                vec![row("checking", day, -999, "Tunely", None, &[link], None)],
                vec![],
            );
            assert_eq!(
                snapshot.transactions[0].record_path.as_deref(),
                Some("billing/tunely.md"),
                "{link}"
            );
        }
    }

    #[test]
    fn unrelated_or_unresolved_links_do_not_create_phantoms() {
        let tunely = record("billing/tunely.md", &["TUNELY"]);
        let day = date(2026, 6, 4);
        let snapshot = snapshot(
            vec![tunely],
            vec![],
            vec![row(
                "checking",
                day,
                -999,
                "Tunely Music",
                None,
                &["[[eat/pizza|🍽️]]", "[[billing/missing]]"],
                None,
            )],
            vec![],
        );
        assert_eq!(
            snapshot.transactions[0].record_path.as_deref(),
            Some("billing/tunely.md")
        );
        assert_eq!(
            snapshot.transactions[0].source,
            Some(AttributionSource::Match)
        );
    }

    #[test]
    fn conflicting_links_and_duplicate_ids_are_not_attributed() {
        let tunely = record("billing/tunely.md", &["TUNELY"]);
        let cloudbox = record("billing/cloudbox.md", &["TUNELY"]);
        let day = date(2026, 6, 5);
        let conflict = snapshot(
            vec![tunely.clone(), cloudbox.clone()],
            vec![],
            vec![row(
                "checking",
                day,
                -999,
                "Tunely",
                None,
                &["[[billing/tunely]]", "[[billing/cloudbox]]"],
                None,
            )],
            vec![],
        );
        assert_eq!(conflict.transactions[0].record_path, None);
        assert_eq!(
            conflict.transactions[0].tied_paths,
            ["billing/cloudbox.md", "billing/tunely.md"]
        );
        assert!(conflict.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "bank_subscription_link_ambiguous"
        }));

        let mut first = row(
            "checking",
            day,
            -999,
            "Tunely",
            None,
            &["[[billing/tunely]]"],
            Some("dup00001"),
        );
        first.line = 2;
        let mut second = row(
            "checking",
            day,
            -888,
            "Other",
            None,
            &["[[billing/cloudbox]]"],
            Some("dup00001"),
        );
        second.line = 3;
        second.path = "bank/2026-06-b.md".to_owned();
        let duplicates = snapshot(
            vec![tunely, cloudbox],
            vec![txn(
                "checking",
                "dup00001",
                day,
                -999,
                "TUNELY MUSIC",
                TxnOutcome::Written,
            )],
            vec![first, second],
            vec![],
        );
        assert_eq!(duplicates.transactions.len(), 1);
        assert!(duplicates.transactions[0].journal_only);
        assert_eq!(duplicates.transactions[0].record_path, None);
        assert!(duplicates.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "bank_subscription_row_id_ambiguous"
        }));
    }

    #[test]
    fn match_ties_preserve_identities_without_a_winner() {
        let left = record("billing/alpha.md", &["CLOUDBOX PLAN"]);
        let right = record("billing/zeta.md", &["CLOUDBOX PLAN"]);
        let day = date(2026, 6, 6);
        let snapshot = snapshot(
            vec![left, right],
            vec![txn(
                "checking",
                "tie00001",
                day,
                -1999,
                "CLOUDBOX PLAN",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
        );
        let txn = &snapshot.transactions[0];
        assert_eq!(txn.record_path, None);
        assert_eq!(txn.tied_paths, ["billing/alpha.md", "billing/zeta.md"]);
        assert!(txn.journal_only);
        assert_eq!(txn.outcome, Some(TxnOutcome::BeforeStart));
    }

    #[test]
    fn inference_exclusions_cover_zero_inflow_transfer_cash_and_fees() {
        let tunely = record("billing/tunely.md", &["TUNELY"]);
        let day = date(2026, 7, 1);
        let cases = [
            row("checking", day, 0, "Tunely", None, &[], None),
            row("checking", day, 999, "Tunely", None, &[], None),
            row("checking", day, -999, "Tunely", Some("transfer"), &[], None),
            row("checking", day, -999, "Tunely", Some("income"), &[], None),
            row("checking", day, -999, "Tunely", Some("cash"), &[], None),
        ];
        for row in cases {
            let snapshot =
                snapshot(vec![tunely.clone()], vec![], vec![row], vec![]);
            assert_eq!(attributed_path(&snapshot), [None]);
        }
        let builtins = [
            ("ONLINE TRANSFER TO SAVINGS", -5000),
            ("ATM WITHDRAWAL", -20000),
            ("ATM FEE", -300),
        ];
        for (desc, cents) in builtins {
            let snapshot = snapshot(
                vec![tunely.clone()],
                vec![txn(
                    "checking",
                    "bbbbbbb1",
                    day,
                    cents,
                    desc,
                    TxnOutcome::BeforeStart,
                )],
                vec![],
                vec![],
            );
            assert_eq!(attributed_path(&snapshot), [None], "{desc}");
        }
        let linked_transfer = row(
            "checking",
            day,
            -999,
            "Tunely",
            Some("transfer"),
            &["[[billing/tunely]]"],
            None,
        );
        let snapshot =
            snapshot(vec![tunely], vec![], vec![linked_transfer], vec![]);
        assert_eq!(
            snapshot.transactions[0].record_path.as_deref(),
            Some("billing/tunely.md")
        );
        assert_eq!(
            snapshot.transactions[0].source,
            Some(AttributionSource::Link)
        );
    }

    #[test]
    fn refunds_attribute_and_journal_only_history_dedupes_with_rows() {
        let tunely = record("billing/tunely.md", &["TUNELY"]);
        let day = date(2026, 8, 1);
        let refund = snapshot(
            vec![tunely.clone()],
            vec![],
            vec![row(
                "checking",
                day,
                999,
                "Tunely credit",
                Some("refund"),
                &[],
                None,
            )],
            vec![],
        );
        assert_eq!(
            refund.transactions[0].record_path.as_deref(),
            Some("billing/tunely.md")
        );
        assert!(refund.transactions[0].is_refund);

        let event = txn(
            "checking",
            "ccccccc1",
            day,
            -999,
            "TUNELY MUSIC",
            TxnOutcome::BeforeStart,
        );
        let matching_row = row(
            "checking",
            day,
            -1099,
            "Edited payee",
            Some("media"),
            &[],
            Some("ccccccc1"),
        );
        let joined = snapshot(
            vec![tunely.clone()],
            vec![event],
            vec![matching_row],
            vec![],
        );
        assert_eq!(joined.transactions.len(), 1);
        assert!(!joined.transactions[0].journal_only);
        assert_eq!(joined.transactions[0].cents, -999);
        assert_eq!(joined.transactions[0].category.as_deref(), Some("media"));

        let missing = snapshot(
            vec![tunely],
            vec![txn(
                "checking",
                "ddddddd1",
                day,
                -999,
                "TUNELY MUSIC",
                TxnOutcome::BeforeStart,
            )],
            vec![],
            vec![],
        );
        assert!(missing.transactions[0].journal_only);
        assert_eq!(
            missing.transactions[0].record_path.as_deref(),
            Some("billing/tunely.md")
        );
    }

    #[test]
    fn covered_typed_rows_are_excluded_and_uncovered_typed_rows_remain() {
        let tunely = record("billing/tunely.md", &["TUNELY"]);
        let covered_day = date(2026, 9, 10);
        let gap_day = date(2026, 9, 20);
        let after_day = date(2026, 11, 5);
        let snapshot = snapshot(
            vec![tunely],
            vec![],
            vec![
                row("checking", covered_day, -999, "Tunely", None, &[], None),
                row("checking", gap_day, -999, "Tunely", None, &[], None),
                row("savings", covered_day, -999, "Tunely", None, &[], None),
                row("checking", after_day, -999, "Tunely", None, &[], None),
            ],
            vec![import("checking", date(2026, 9, 1), date(2026, 9, 15))],
        );
        let dates = snapshot
            .transactions
            .iter()
            .map(|txn| txn.date)
            .collect::<Vec<_>>();
        assert_eq!(dates, [covered_day, gap_day, after_day]);
        assert_eq!(snapshot.transactions[0].account, "savings");
        assert!(snapshot.transactions.iter().all(|txn| !txn.journal_only));
        assert!(
            snapshot
                .transactions
                .iter()
                .all(|txn| txn.record_path.as_deref()
                    == Some("billing/tunely.md"))
        );
    }

    #[test]
    fn orphan_id_rows_are_row_only_and_not_extra_journal_entries() {
        let tunely = record("billing/tunely.md", &["TUNELY"]);
        let day = date(2026, 11, 2);
        let snapshot = snapshot(
            vec![tunely],
            vec![],
            vec![row(
                "checking",
                day,
                -999,
                "Tunely",
                None,
                &["[[billing/tunely]]"],
                Some("orphan01"),
            )],
            vec![],
        );
        assert_eq!(snapshot.transactions.len(), 1);
        assert!(!snapshot.transactions[0].journal_only);
        assert_eq!(snapshot.transactions[0].id.as_deref(), Some("orphan01"));
        assert_eq!(
            snapshot.transactions[0].source,
            Some(AttributionSource::Link)
        );
    }
}

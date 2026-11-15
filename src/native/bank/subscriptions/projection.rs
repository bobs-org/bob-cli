//! Shared subscription row enrichment for list, report, and import.
//!
//! One command loads a single subscription registry and one computed ledger
//! snapshot, reused by its consumers. Raw row collection stays usable by
//! existing tests; this entry point consumes the loaded registry, config,
//! journal, all rows, and frozen today, computes facts before list/report
//! filters, and attaches the same typed row projection to `BankRow` for
//! `row_json`. Import enriches its projected after-state with the same
//! entry point, including the batch's new journal evidence and before-start
//! history.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use super::{
    ledger::{compute_ledger, LedgerSnapshot},
    registry::{SubscriptionAvailability, SubscriptionRegistry},
};
use crate::native::bank::{
    config::BankConfig,
    journal::JournalSnapshot,
    model::{BankRow, RowSubscription, SubscriptionPriceChange},
    naming::classify_builtin,
    suggest::{self, has_read_time_rule_match, SuggestQuery},
};

/// Bounded warning emitted when the subscription album cannot be read.
/// Read output carries it once in JSON and once for human output; `bank
/// check` carries the same code as a finding. Never one warning per row,
/// and never confused with a genuinely missing album.
pub(crate) const SUBSCRIPTION_ALBUM_INVALID_CODE: &str =
    "bank_subscription_album_invalid";

pub(crate) fn album_invalid_warning(
    registry: &SubscriptionRegistry,
) -> Option<String> {
    if registry.availability == SubscriptionAvailability::Invalid {
        Some(
            "subscription records are unavailable; run `bob albums check`"
                .to_owned(),
        )
    } else {
        None
    }
}

/// Enrich every row in place with subscription facts, recurring flags,
/// candidates, and subscription-tier suggestions. A missing or invalid
/// album forces the entire projection to null/false and leaves existing
/// suggestions untouched.
pub(crate) fn enrich_rows(
    rows: &mut [BankRow],
    registry: &SubscriptionRegistry,
    journal: &JournalSnapshot,
    config: Option<&BankConfig>,
    today: NaiveDate,
) {
    enrich_rows_with_raw(
        rows,
        registry,
        journal,
        config,
        today,
        &HashMap::new(),
    );
}

/// Import after-state enrichment. `raw_descs` maps new row IDs to their
/// uncleaned source descriptions so `bank_recurring` and candidate checks
/// see the `RECURRING PAYMENT` marker the cleaner strips from payees.
/// Because valid needles cannot contain masked digit runs, import and read
/// projections agree on matching.
pub(crate) fn enrich_rows_with_raw(
    rows: &mut [BankRow],
    registry: &SubscriptionRegistry,
    journal: &JournalSnapshot,
    config: Option<&BankConfig>,
    today: NaiveDate,
    raw_descs: &HashMap<String, String>,
) {
    if registry.availability != SubscriptionAvailability::Available {
        for row in rows.iter_mut() {
            row.subscription = None;
            row.bank_recurring = false;
            row.subscription_candidate = false;
        }
        return;
    }
    let Some(config) = config else {
        for row in rows.iter_mut() {
            row.subscription = None;
            row.bank_recurring = false;
            row.subscription_candidate = false;
        }
        return;
    };
    let ledger =
        compute_ledger(registry, journal, rows, today, Some(config), raw_descs);
    let index = suggest::learned_index(rows, journal, config);
    let unique_ids = unique_id_set(rows);
    // Cache journal descriptions for recurring and rule checks.
    let mut journal_descs: HashMap<String, String> = HashMap::new();
    for (id, key) in &journal.ids {
        if let Some(event) = journal.transactions.get(key) {
            journal_descs.insert(id.clone(), event.desc.clone());
        }
    }
    for row in rows.iter_mut() {
        let desc_for_read = read_desc(row, &journal_descs, raw_descs);
        let recurring = contains_recurring_marker(&desc_for_read);
        row.bank_recurring = recurring;
        let attribution = row_attribution(row, &ledger, &unique_ids);
        if let Some(attribution) = &attribution
            && let Some(record_path) = &attribution.record_path
            && let Some(record) = registry
                .records
                .iter()
                .find(|record| &record.path == record_path)
        {
            let source = attribution
                .source
                .map(|source| source.as_str().to_owned())
                .unwrap_or_else(|| "match".to_owned());
            let drift = drift_cents(row, record.amount_cents, record.pricing);
            let ledger_record = ledger.records.get(record_path);
            let price_change = ledger_record
                .and_then(|ledger| ledger.price_change.clone())
                .map(|change| SubscriptionPriceChange {
                    from_cents: change.from_cents,
                    to_cents: change.to_cents,
                    since: change.since,
                });
            let state = row_state(
                row,
                record.pricing,
                drift,
                ledger_record.map(|ledger| {
                    (
                        ledger.after_cancel_charges.clone(),
                        ledger.latest.clone(),
                        ledger.still_charging,
                    )
                }),
            );
            row.subscription = Some(RowSubscription {
                stem: record.stem.clone(),
                path: record.path.clone(),
                title: record.title.clone(),
                status: record.status.as_str().to_owned(),
                status_label: record.status_label.clone(),
                cadence: record.cadence.as_str().to_owned(),
                pricing: record.pricing.as_str().to_owned(),
                amount_cents: record.amount_cents,
                source,
                needle: attribution.needle.clone(),
                drift_cents: drift,
                price_change,
                state,
            });
            row.subscription_candidate = false;
            // Subscription tier for uncategorized rows: rule first,
            // subscription second, then learned/bank. Authored categories
            // keep their existing suggestion.
            if row.category.is_none()
                && let Some(category) = &record.category
            {
                let unique_id: Option<&str> =
                    row.id.as_deref().filter(|id| unique_ids.contains(*id));
                // Recompute the query the original suggestion used.
                let identities =
                    suggest::row_identities(row, journal, unique_id);
                let desc = unique_id
                    .and_then(|id| journal_descs.get(id).map(String::as_str));
                let query = SuggestQuery {
                    desc,
                    payee: Some(row.payee.as_str()),
                    trntype: None,
                    identities: &identities,
                    skip: None,
                    read_time: true,
                };
                // Only override when the current suggestion is not rule:
                // rule already won, subscription must not displace it.
                let is_rule =
                    row.suggestion.as_ref().is_some_and(|suggestion| {
                        suggestion.source == suggest::SuggestionSource::Rule
                    });
                if !is_rule {
                    let _ = &index;
                    if config
                        .category(category)
                        .is_some_and(|entry| !entry.hidden)
                    {
                        // Preserve a learned/bank suggestion only when the
                        // subscription category cannot be offered; otherwise
                        // the subscription tier wins.
                        row.suggestion = suggest::suggest_with_subscription(
                            config,
                            &query,
                            &index,
                            Some(category),
                        );
                    }
                }
            }
        } else {
            row.subscription = None;
            // Candidate only for an eligible outflow with the marker, no
            // attribution, and no matching bank rule (including a rule with
            // only payee or link). Uses read-time masking semantics.
            let eligible = is_candidate_eligible(row, &desc_for_read);
            let rule_match = has_read_time_rule_match(config, &desc_for_read);
            row.subscription_candidate = eligible && recurring && !rule_match;
        }
    }
}

fn unique_id_set(rows: &[BankRow]) -> HashSet<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for row in rows {
        if let Some(id) = row.id.as_deref() {
            *counts.entry(id).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|item| item.1 == 1)
        .map(|(id, _)| id.to_owned())
        .collect()
}

fn row_attribution(
    row: &BankRow,
    ledger: &LedgerSnapshot,
    unique_ids: &HashSet<String>,
) -> Option<super::ledger::RowAttribution> {
    if let Some(attribution) = ledger.by_row.get(&(row.path.clone(), row.line))
    {
        return Some(attribution.clone());
    }
    if let Some(id) = row.id.as_deref()
        && unique_ids.contains(id)
        && let Some(attribution) = ledger.by_id.get(id)
    {
        return Some(attribution.clone());
    }
    None
}

fn read_desc(
    row: &BankRow,
    journal_descs: &HashMap<String, String>,
    raw_descs: &HashMap<String, String>,
) -> String {
    if let Some(id) = row.id.as_deref() {
        if let Some(raw) = raw_descs.get(id) {
            return raw.clone();
        }
        if let Some(desc) = journal_descs.get(id) {
            return desc.clone();
        }
    }
    row.payee.clone()
}

fn contains_recurring_marker(desc: &str) -> bool {
    desc.to_uppercase().contains("RECURRING PAYMENT")
}

fn drift_cents(
    row: &BankRow,
    amount_cents: i64,
    pricing: super::registry::SubscriptionPricing,
) -> Option<i64> {
    if pricing != super::registry::SubscriptionPricing::Fixed {
        return None;
    }
    if row.cents >= 0 {
        return None;
    }
    if row.category.as_deref() == Some("refund") {
        return None;
    }
    Some(row.cents.saturating_neg().saturating_sub(amount_cents))
}

#[allow(clippy::type_complexity)]
fn row_state(
    row: &BankRow,
    pricing: super::registry::SubscriptionPricing,
    drift: Option<i64>,
    ledger: Option<(
        Vec<super::ledger::LedgerCharge>,
        Option<super::ledger::LedgerCharge>,
        bool,
    )>,
) -> String {
    let (after_cancel_charges, latest, still_charging) =
        ledger.unwrap_or((Vec::new(), None, false));
    if row.category.as_deref() == Some("refund") {
        return "ok".to_owned();
    }
    if after_cancel_charges
        .iter()
        .any(|charge| charges_match(row, charge))
    {
        return "after_cancel".to_owned();
    }
    if still_charging
        && let Some(latest) = latest
        && charges_match(row, &latest)
    {
        return "to_cancel".to_owned();
    }
    if pricing == super::registry::SubscriptionPricing::Variable
        && row.cents < 0
    {
        return "usage".to_owned();
    }
    if drift.is_some_and(|drift| drift != 0) {
        return "drift".to_owned();
    }
    "ok".to_owned()
}

fn charges_match(row: &BankRow, charge: &super::ledger::LedgerCharge) -> bool {
    if let (Some(path), Some(line)) = (&charge.path, charge.line) {
        return &row.path == path && row.line == line;
    }
    if let (Some(row_id), Some(charge_id)) = (&row.id, &charge.id) {
        return row_id == charge_id;
    }
    false
}

fn is_candidate_eligible(row: &BankRow, desc: &str) -> bool {
    if row.cents >= 0 {
        return false;
    }
    if row.subscription.is_some() {
        return false;
    }
    if let Some("transfer" | "income" | "cash") = row.category.as_deref() {
        return false;
    }
    if let Some(builtin) = classify_builtin(desc, None)
        && let Some("transfer" | "cash" | "fees") = builtin.category
    {
        return false;
    }
    true
}

// Declared-category lookup for the outer import phase lives in
// `suggest::is_declared_category`: hidden declared keys are allowed for
// subscription fill-down.

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, NaiveDate};

    use crate::native::bank::{
        config::{parse_bank_config, ConfigParse},
        journal::{
            ImportCounts, ImportEvent, JournalSnapshot, TxnEvent, TxnOutcome,
        },
        source::SourceKind,
        subscriptions::registry::{
            SubscriptionCadence, SubscriptionPricing, SubscriptionRecord,
            SubscriptionStatus,
        },
    };

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("date")
    }

    fn today() -> NaiveDate {
        date(2026, 11, 15)
    }

    fn ts() -> DateTime<chrono::FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z").expect("ts")
    }

    fn config(text: &str) -> BankConfig {
        let body = format!(
            "---\nbank:\n  accounts:\n    checking:\n      kind: checking\n      start: 2026-09-01\n{text}---\n"
        );
        match parse_bank_config(&body) {
            ConfigParse::Ok(config) => config,
            other => panic!("expected ok, got {other:?}"),
        }
    }

    fn base_config() -> BankConfig {
        config("  categories:\n    streaming: Streaming\n    gaming: Gaming\n")
    }

    fn record(
        path: &str,
        status: SubscriptionStatus,
        amount_cents: i64,
        needles: &[&str],
        category: Option<&str>,
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
            cadence: SubscriptionCadence::Monthly,
            pricing: SubscriptionPricing::Fixed,
            needles: needles
                .iter()
                .map(|needle| (*needle).to_owned())
                .collect(),
            invalid_needles: Vec::new(),
            category: category.map(str::to_owned),
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
        day: NaiveDate,
        cents: i64,
        payee: &str,
        category: Option<&str>,
        id: Option<&str>,
        line: usize,
    ) -> BankRow {
        BankRow {
            path: format!("bank/{}.md", day.format("%Y-%m")),
            line,
            date: day,
            heading: String::new(),
            account: "checking".to_owned(),
            cents,
            payee: payee.to_owned(),
            category: category.map(str::to_owned),
            links: Vec::new(),
            note: None,
            flagged: false,
            acknowledged: false,
            id: id.map(str::to_owned),
            origin: "typed",
            suggestion: None,
            subscription: None,
            bank_recurring: false,
            subscription_candidate: false,
        }
    }

    fn txn(
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
            account: "checking".to_owned(),
            key: key.clone(),
            id: id.to_owned(),
            posted: day,
            cents,
            desc: desc.to_owned(),
            merchant: desc.to_ascii_lowercase(),
            outcome,
            note: Some("bank/2026-11.md".to_owned()),
        }
    }

    fn journal(
        txns: Vec<TxnEvent>,
        imports: Vec<ImportEvent>,
    ) -> JournalSnapshot {
        let mut snapshot = JournalSnapshot::default();
        for event in txns {
            snapshot.ids.insert(event.id.clone(), event.key.clone());
            snapshot.transactions.insert(event.key.clone(), event);
        }
        snapshot.imports = imports;
        snapshot
    }

    fn import_window(start: NaiveDate, end: NaiveDate) -> ImportEvent {
        ImportEvent {
            ts: ts(),
            host: "test".to_owned(),
            path: ".bob/bank/test.jsonl".to_owned(),
            line: 1,
            account: "checking".to_owned(),
            source: SourceKind::Ofx,
            file_sha256: "c".repeat(64),
            start,
            end,
            balance_cents: None,
            balance_asof: None,
            counts: ImportCounts::default(),
        }
    }

    #[test]
    fn missing_and_invalid_albums_force_null_false_projection() {
        let config = base_config();
        let mut rows =
            vec![row(date(2026, 11, 10), -999, "Tunely", None, None, 1)];
        let journal = JournalSnapshot::default();
        let missing = SubscriptionRegistry::missing();
        enrich_rows(&mut rows, &missing, &journal, Some(&config), today());
        assert_eq!(rows[0].subscription, None);
        assert!(!rows[0].bank_recurring);
        assert!(!rows[0].subscription_candidate);
        assert!(album_invalid_warning(&missing).is_none());

        let invalid = SubscriptionRegistry::missing_invalid();
        let mut rows =
            vec![row(date(2026, 11, 10), -999, "Tunely", None, None, 1)];
        enrich_rows(&mut rows, &invalid, &journal, Some(&config), today());
        assert_eq!(rows[0].subscription, None);
        assert!(!rows[0].bank_recurring);
        assert!(!rows[0].subscription_candidate);
        let warning = album_invalid_warning(&invalid).expect("warning");
        assert!(warning.contains("bob albums check"));
    }

    #[test]
    fn matched_row_carries_drift_ok_and_subscription_suggestion() {
        let config = base_config();
        let registry = registry(vec![record(
            "subs/tunely.md",
            SubscriptionStatus::Active,
            999,
            &["TUNELY"],
            Some("streaming"),
        )]);
        let event = txn(
            "aaaaaaaa",
            date(2026, 11, 10),
            -999,
            "TUNELY PREMIUM",
            TxnOutcome::Written,
        );
        let journal = journal(
            vec![event],
            vec![import_window(date(2026, 11, 1), date(2026, 11, 15))],
        );
        let mut rows = vec![BankRow {
            path: "bank/2026-11.md".to_owned(),
            line: 3,
            date: date(2026, 11, 10),
            heading: String::new(),
            account: "checking".to_owned(),
            cents: -999,
            payee: "Tunely".to_owned(),
            category: None,
            links: Vec::new(),
            note: None,
            flagged: false,
            acknowledged: false,
            id: Some("aaaaaaaa".to_owned()),
            origin: "imported",
            suggestion: None,
            subscription: None,
            bank_recurring: false,
            subscription_candidate: false,
        }];
        enrich_rows(&mut rows, &registry, &journal, Some(&config), today());
        let projection = rows[0].subscription.clone().expect("projection");
        assert_eq!(projection.stem, "tunely");
        assert_eq!(projection.path, "subs/tunely.md");
        assert_eq!(projection.source, "match");
        assert_eq!(projection.needle.as_deref(), Some("TUNELY"));
        assert_eq!(projection.drift_cents, Some(0));
        assert_eq!(projection.state, "ok");
        assert!(!rows[0].bank_recurring);
        assert!(!rows[0].subscription_candidate);
        let suggestion = rows[0].suggestion.clone().expect("suggestion");
        assert_eq!(suggestion.category, "streaming");
        assert_eq!(
            suggestion.source,
            crate::native::bank::suggest::SuggestionSource::Subscription
        );
        let json = rows[0].subscription.as_ref().expect("json").to_json();
        assert_eq!(json["stem"], "tunely");
        assert_eq!(json["source"], "match");
        assert_eq!(json["drift_cents"], 0);
        assert_eq!(json["state"], "ok");
    }

    #[test]
    fn rule_beats_subscription_but_projection_remains() {
        let config = config(
            "  categories:\n    streaming: Streaming\n    gaming: Gaming\n  rules:\n    - match: TUNELY\n      cat: gaming\n",
        );
        let registry = registry(vec![record(
            "subs/tunely.md",
            SubscriptionStatus::Active,
            999,
            &["TUNELY"],
            Some("streaming"),
        )]);
        let event = txn(
            "bbbbbbbb",
            date(2026, 11, 10),
            -999,
            "TUNELY PREMIUM",
            TxnOutcome::Written,
        );
        let journal = journal(
            vec![event],
            vec![import_window(date(2026, 11, 1), date(2026, 11, 15))],
        );
        let mut rows = vec![BankRow {
            path: "bank/2026-11.md".to_owned(),
            line: 3,
            date: date(2026, 11, 10),
            heading: String::new(),
            account: "checking".to_owned(),
            cents: -999,
            payee: "Tunely".to_owned(),
            category: None,
            links: Vec::new(),
            note: None,
            flagged: false,
            acknowledged: false,
            id: Some("bbbbbbbb".to_owned()),
            origin: "imported",
            suggestion: None,
            subscription: None,
            bank_recurring: false,
            subscription_candidate: false,
        }];
        enrich_rows(&mut rows, &registry, &journal, Some(&config), today());
        assert!(rows[0].subscription.is_some());
        let suggestion = rows[0].suggestion.clone().expect("suggestion");
        assert_eq!(suggestion.category, "gaming");
        assert_eq!(
            suggestion.source,
            crate::native::bank::suggest::SuggestionSource::Rule
        );
    }

    #[test]
    fn hidden_and_unknown_categories_are_not_suggested() {
        let config = config(
            "  categories:\n    streaming: Streaming\n    hidden_cat: { label: Hidden, hidden: true }\n",
        );
        for category in [Some("hidden_cat"), Some("nope"), None] {
            let registry = registry(vec![record(
                "subs/tunely.md",
                SubscriptionStatus::Active,
                999,
                &["TUNELY"],
                category,
            )]);
            let event = txn(
                "cccccccc",
                date(2026, 11, 10),
                -999,
                "TUNELY",
                TxnOutcome::Written,
            );
            let journal = journal(
                vec![event],
                vec![import_window(date(2026, 11, 1), date(2026, 11, 15))],
            );
            let mut rows = vec![BankRow {
                path: "bank/2026-11.md".to_owned(),
                line: 3,
                date: date(2026, 11, 10),
                heading: String::new(),
                account: "checking".to_owned(),
                cents: -999,
                payee: "Tunely".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("cccccccc".to_owned()),
                origin: "imported",
                suggestion: None,
                subscription: None,
                bank_recurring: false,
                subscription_candidate: false,
            }];
            enrich_rows(&mut rows, &registry, &journal, Some(&config), today());
            assert!(rows[0].subscription.is_some(), "{category:?}");
            let suggest_hidden = rows[0].suggestion.clone();
            assert!(
                suggest_hidden.is_none()
                    || suggest_hidden.is_some_and(|suggestion| {
                        suggestion.source
                            != crate::native::bank::suggest::SuggestionSource::Subscription
                    }),
                "{category:?}"
            );
        }
    }

    #[test]
    fn recurring_marker_drives_candidate_and_rules_suppress_it() {
        let plain = base_config();
        let with_rule = config(
            "  categories:\n    streaming: Streaming\n  rules:\n    - match: CLOUDBOX\n      payee: Cloudbox\n",
        );
        let registry = registry(vec![record(
            "subs/tunely.md",
            SubscriptionStatus::Active,
            999,
            &["TUNELY"],
            Some("streaming"),
        )]);
        let event = txn(
            "dddddddd",
            date(2026, 11, 10),
            -1999,
            "CLOUDBOX RECURRING PAYMENT",
            TxnOutcome::Written,
        );
        let journal = journal(
            vec![event],
            vec![import_window(date(2026, 11, 1), date(2026, 11, 15))],
        );
        let mut rows = vec![BankRow {
            path: "bank/2026-11.md".to_owned(),
            line: 5,
            date: date(2026, 11, 10),
            heading: String::new(),
            account: "checking".to_owned(),
            cents: -1999,
            payee: "Cloudbox".to_owned(),
            category: None,
            links: Vec::new(),
            note: None,
            flagged: false,
            acknowledged: false,
            id: Some("dddddddd".to_owned()),
            origin: "imported",
            suggestion: None,
            subscription: None,
            bank_recurring: false,
            subscription_candidate: false,
        }];
        enrich_rows(&mut rows, &registry, &journal, Some(&plain), today());
        assert!(rows[0].bank_recurring);
        assert!(rows[0].subscription_candidate);

        let mut ruled = rows.clone();
        for row in ruled.iter_mut() {
            row.subscription = None;
            row.bank_recurring = false;
            row.subscription_candidate = false;
            row.suggestion = None;
        }
        enrich_rows(&mut ruled, &registry, &journal, Some(&with_rule), today());
        assert!(ruled[0].bank_recurring);
        assert!(!ruled[0].subscription_candidate);
    }

    #[test]
    fn price_change_uses_history_outside_the_selected_month() {
        let config = base_config();
        let registry = registry(vec![record(
            "subs/tunely.md",
            SubscriptionStatus::Active,
            999,
            &["TUNELY"],
            Some("streaming"),
        )]);
        let first = txn(
            "eeeeeeee",
            date(2026, 9, 10),
            -1099,
            "TUNELY",
            TxnOutcome::Written,
        );
        let second = txn(
            "ffffffff",
            date(2026, 10, 10),
            -1099,
            "TUNELY",
            TxnOutcome::Written,
        );
        let journal = journal(
            vec![first, second],
            vec![import_window(date(2026, 9, 1), date(2026, 11, 15))],
        );
        let mut rows = vec![
            BankRow {
                path: "bank/2026-09.md".to_owned(),
                line: 3,
                date: date(2026, 9, 10),
                heading: String::new(),
                account: "checking".to_owned(),
                cents: -1099,
                payee: "Tunely".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("eeeeeeee".to_owned()),
                origin: "imported",
                suggestion: None,
                subscription: None,
                bank_recurring: false,
                subscription_candidate: false,
            },
            BankRow {
                path: "bank/2026-10.md".to_owned(),
                line: 3,
                date: date(2026, 10, 10),
                heading: String::new(),
                account: "checking".to_owned(),
                cents: -1099,
                payee: "Tunely".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("ffffffff".to_owned()),
                origin: "imported",
                suggestion: None,
                subscription: None,
                bank_recurring: false,
                subscription_candidate: false,
            },
        ];
        enrich_rows(&mut rows, &registry, &journal, Some(&config), today());
        for row in &rows {
            let projection = row.subscription.clone().expect("projection");
            assert_eq!(projection.drift_cents, Some(100));
            let change = projection.price_change.clone().expect("change");
            assert_eq!(change.from_cents, 999);
            assert_eq!(change.to_cents, 1099);
        }
    }

    #[test]
    fn import_and_read_projections_agree_across_masked_digits() {
        let config = base_config();
        let registry = registry(vec![record(
            "subs/tunely.md",
            SubscriptionStatus::Active,
            999,
            &["TUNELY"],
            Some("streaming"),
        )]);
        let journal = JournalSnapshot::default();
        let mut read_rows =
            vec![row(date(2026, 11, 10), -999, "Tunely", None, None, 1)];
        enrich_rows(
            &mut read_rows,
            &registry,
            &journal,
            Some(&config),
            today(),
        );
        let mut import_rows = vec![row(
            date(2026, 11, 10),
            -999,
            "Tunely",
            None,
            Some("abc123"),
            1,
        )];
        let mut raw = std::collections::HashMap::new();
        raw.insert(
            "abc123".to_owned(),
            "TUNELY PREMIUM 1234 RECURRING PAYMENT".to_owned(),
        );
        enrich_rows_with_raw(
            &mut import_rows,
            &registry,
            &journal,
            Some(&config),
            today(),
            &raw,
        );
        assert_eq!(
            read_rows[0].subscription.as_ref().map(|s| &s.path),
            import_rows[0].subscription.as_ref().map(|s| &s.path)
        );
        assert!(import_rows[0].bank_recurring);
        assert!(!read_rows[0].bank_recurring);
    }

    #[test]
    fn variable_and_refund_rows_have_null_drift_with_usage_state() {
        let config = base_config();
        let mut variable = record(
            "subs/cloudbox.md",
            SubscriptionStatus::Active,
            1999,
            &["CLOUDBOX"],
            Some("streaming"),
        );
        variable.pricing = SubscriptionPricing::Variable;
        variable.cadence = SubscriptionCadence::Monthly;
        let registry = registry(vec![variable]);
        let event = txn(
            "99999999",
            date(2026, 11, 10),
            -2500,
            "CLOUDBOX",
            TxnOutcome::Written,
        );
        let journal = journal(
            vec![event],
            vec![import_window(date(2026, 11, 1), date(2026, 11, 15))],
        );
        let mut rows = vec![BankRow {
            path: "bank/2026-11.md".to_owned(),
            line: 7,
            date: date(2026, 11, 10),
            heading: String::new(),
            account: "checking".to_owned(),
            cents: -2500,
            payee: "Cloudbox".to_owned(),
            category: None,
            links: Vec::new(),
            note: None,
            flagged: false,
            acknowledged: false,
            id: Some("99999999".to_owned()),
            origin: "imported",
            suggestion: None,
            subscription: None,
            bank_recurring: false,
            subscription_candidate: false,
        }];
        enrich_rows(&mut rows, &registry, &journal, Some(&config), today());
        let projection = rows[0].subscription.clone().expect("projection");
        assert_eq!(projection.drift_cents, None);
        assert_eq!(projection.state, "usage");
        assert_eq!(projection.price_change, None);
    }

    #[test]
    fn after_cancel_marks_every_ended_charge_and_to_cancel_only_latest() {
        let config = base_config();
        let mut cancelled = record(
            "subs/tunely.md",
            SubscriptionStatus::Cancelled,
            999,
            &["TUNELY"],
            Some("streaming"),
        );
        cancelled.ended = Some(date(2026, 9, 1));
        let ended = registry(vec![cancelled]);
        let first = txn(
            "aaaaaaaa",
            date(2026, 10, 2),
            -999,
            "TUNELY",
            TxnOutcome::Written,
        );
        let second = txn(
            "bbbbbbbb",
            date(2026, 11, 2),
            -999,
            "TUNELY",
            TxnOutcome::Written,
        );
        let ended_journal = journal(
            vec![first, second],
            vec![import_window(date(2026, 10, 1), date(2026, 11, 15))],
        );
        let mut rows = vec![
            BankRow {
                path: "bank/2026-10.md".to_owned(),
                line: 3,
                date: date(2026, 10, 2),
                heading: String::new(),
                account: "checking".to_owned(),
                cents: -999,
                payee: "Tunely".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("aaaaaaaa".to_owned()),
                origin: "imported",
                suggestion: None,
                subscription: None,
                bank_recurring: false,
                subscription_candidate: false,
            },
            BankRow {
                path: "bank/2026-11.md".to_owned(),
                line: 3,
                date: date(2026, 11, 2),
                heading: String::new(),
                account: "checking".to_owned(),
                cents: -999,
                payee: "Tunely".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("bbbbbbbb".to_owned()),
                origin: "imported",
                suggestion: None,
                subscription: None,
                bank_recurring: false,
                subscription_candidate: false,
            },
        ];
        enrich_rows(&mut rows, &ended, &ended_journal, Some(&config), today());
        assert_eq!(
            rows[0]
                .subscription
                .as_ref()
                .map(|item| item.state.as_str()),
            Some("after_cancel")
        );
        assert_eq!(
            rows[1]
                .subscription
                .as_ref()
                .map(|item| item.state.as_str()),
            Some("after_cancel")
        );

        let to_cancel = record(
            "subs/cloudbox.md",
            SubscriptionStatus::ToCancel,
            1999,
            &["CLOUDBOX"],
            Some("streaming"),
        );
        let cancelling = registry(vec![to_cancel]);
        let older = txn(
            "cccccccc",
            date(2026, 10, 10),
            -1999,
            "CLOUDBOX",
            TxnOutcome::Written,
        );
        let latest = txn(
            "dddddddd",
            date(2026, 11, 10),
            -1999,
            "CLOUDBOX",
            TxnOutcome::Written,
        );
        let cancelling_journal = journal(
            vec![older, latest],
            vec![import_window(date(2026, 10, 1), date(2026, 11, 15))],
        );
        let mut rows = vec![
            BankRow {
                path: "bank/2026-10.md".to_owned(),
                line: 5,
                date: date(2026, 10, 10),
                heading: String::new(),
                account: "checking".to_owned(),
                cents: -1999,
                payee: "Cloudbox".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("cccccccc".to_owned()),
                origin: "imported",
                suggestion: None,
                subscription: None,
                bank_recurring: false,
                subscription_candidate: false,
            },
            BankRow {
                path: "bank/2026-11.md".to_owned(),
                line: 5,
                date: date(2026, 11, 10),
                heading: String::new(),
                account: "checking".to_owned(),
                cents: -1999,
                payee: "Cloudbox".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("dddddddd".to_owned()),
                origin: "imported",
                suggestion: None,
                subscription: None,
                bank_recurring: false,
                subscription_candidate: false,
            },
        ];
        enrich_rows(
            &mut rows,
            &cancelling,
            &cancelling_journal,
            Some(&config),
            today(),
        );
        assert_eq!(
            rows[0]
                .subscription
                .as_ref()
                .map(|item| item.state.as_str()),
            Some("ok")
        );
        assert_eq!(
            rows[1]
                .subscription
                .as_ref()
                .map(|item| item.state.as_str()),
            Some("to_cancel")
        );
    }
}

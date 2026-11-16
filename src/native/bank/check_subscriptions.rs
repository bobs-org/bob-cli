//! Subscription findings for `bob bank check`.
//!
//! Record findings project the shared subscription ledger. Candidate groups
//! use masked journal descriptions and deliberately propose edits without
//! writing the vault.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    check::{Finding, Severity},
    config::{suggest_name, BankConfig},
    grammar::render_amount,
    identity::merchant_key,
    journal::{JournalSnapshot, TxnEvent},
    model::BankRow,
    naming::{classify_builtin, clean_description, sanitize_payee, title_case},
    subscriptions::{
        ledger::{compute_ledger, LedgerSnapshot, RecordLedger},
        registry::{
            SubscriptionAvailability, SubscriptionCadence, SubscriptionRecord,
            SubscriptionRegistry, SubscriptionStatus,
        },
    },
    suggest::has_read_time_rule_match,
};

const CANDIDATE_LIMIT: usize = 10;

pub(crate) fn findings(
    config: Option<&BankConfig>,
    registry: &SubscriptionRegistry,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    today: NaiveDate,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    if registry.availability == SubscriptionAvailability::Invalid {
        let path = registry
            .album
            .as_ref()
            .map(|album| album.path.clone())
            .unwrap_or_default();
        findings.push(make_finding(
            super::subscriptions::projection::SUBSCRIPTION_ALBUM_INVALID_CODE,
            Severity::Warning,
            path,
            None,
            "subscription records are unavailable; run `bob albums check`",
            Some("run `bob albums check` for details".to_owned()),
            None,
        ));
    }

    let ledger = (registry.availability == SubscriptionAvailability::Available)
        .then(|| {
            compute_ledger(
                registry,
                journal,
                rows,
                today,
                config,
                &HashMap::new(),
            )
        });
    if let Some(ledger) = &ledger {
        append_record_findings(
            config,
            registry,
            journal,
            rows,
            ledger,
            &mut findings,
        );
    }
    append_candidate_findings(
        config,
        journal,
        rows,
        ledger.as_ref(),
        &mut findings,
    );
    findings
}

fn append_record_findings(
    config: Option<&BankConfig>,
    registry: &SubscriptionRegistry,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    ledger: &LedgerSnapshot,
    findings: &mut Vec<Finding>,
) {
    let by_path = registry
        .records
        .iter()
        .map(|record| (record.path.as_str(), record))
        .collect::<HashMap<_, _>>();

    for record in &registry.records {
        for needle in &record.invalid_needles {
            findings.push(make_finding(
                "bank_subscription_match_invalid",
                Severity::Warning,
                &record.path,
                None,
                format!(
                    "{}: match needle {:?} {} and was ignored",
                    record.title,
                    needle.text,
                    needle.reason.as_str()
                ),
                Some("Use a specific, digit-free statement phrase.".to_owned()),
                None,
            ));
        }
        if let Some(category) = &record.category
            && config.is_some_and(|config| config.category(category).is_none())
        {
            let keys = config
                .into_iter()
                .flat_map(|config| config.categories.iter())
                .map(|category| category.key.clone())
                .collect::<Vec<_>>();
            let hint = suggest_name(category, &keys).or_else(|| {
                Some("Choose a declared bank.categories key.".to_owned())
            });
            findings.push(make_finding(
                "bank_subscription_category_unknown",
                Severity::Warning,
                &record.path,
                None,
                format!(
                    "{} uses undeclared bank category {category:?}",
                    record.title
                ),
                hint,
                None,
            ));
        }
    }

    for diagnostic in &registry.diagnostics {
        if diagnostic.field.as_deref() == Some("amount")
            && diagnostic.code == "bank_subscription_record_invalid"
        {
            findings.push(make_finding(
                "bank_subscription_amount_invalid",
                Severity::Warning,
                &diagnostic.path,
                None,
                format!(
                    "subscription amount is invalid: {}",
                    diagnostic.message
                ),
                Some(
                    "Use a nonnegative amount with no fractional cents."
                        .to_owned(),
                ),
                None,
            ));
        } else if diagnostic.code == "bank_subscription_match_invalid"
            && !diagnostic.message.starts_with("match needle")
        {
            findings.push(make_finding(
                "bank_subscription_match_invalid",
                Severity::Warning,
                &diagnostic.path,
                None,
                format!(
                    "invalid subscription match entry: {}",
                    diagnostic.message
                ),
                Some("Use a specific, digit-free statement phrase.".to_owned()),
                None,
            ));
        }
    }

    for record in &registry.records {
        let Some(record_ledger) = ledger.records.get(&record.path) else {
            continue;
        };
        append_ledger_findings(record, record_ledger, findings);
    }
    append_overlaps(registry, journal, rows, ledger, &by_path, findings);
}

fn append_ledger_findings(
    record: &SubscriptionRecord,
    ledger: &RecordLedger,
    findings: &mut Vec<Finding>,
) {
    if let Some(charge) = &ledger.after_cancel {
        let mut hint_parts = Vec::new();
        if let Some(url) = &record.url {
            hint_parts.push(format!("Manage: {url}"));
        }
        if record.ended.is_none() {
            hint_parts.push(format!(
                "Set the cancel date with `bob records update subscription {} -s ended=YYYY-MM-DD`.",
                shell_quote(&record.stem)
            ));
        }
        let hint = (!hint_parts.is_empty()).then(|| hint_parts.join(" · "));
        findings.push(make_finding(
            "bank_subscription_cancelled_charge",
            Severity::Warning,
            &record.path,
            charge.line,
            format!(
                "{} was charged {} on {} after cancellation",
                record.title,
                money_magnitude(charge.cents),
                day_label(charge.date)
            ),
            hint,
            Some(charge.date),
        ));
    }
    if ledger.still_charging
        && let Some(charge) = &ledger.latest
    {
        let hint = record.url.as_ref().map(|url| format!("Manage: {url}"));
        findings.push(make_finding(
            "bank_subscription_to_cancel_charged",
            Severity::Info,
            &record.path,
            charge.line,
            format!(
                "{} is still paying {} (latest charge {})",
                record.title,
                money_magnitude(charge.cents),
                day_label(charge.date)
            ),
            hint,
            Some(charge.date),
        ));
    }
    if let Some(change) = &ledger.price_change {
        let new_amount = decimal_amount(change.to_cents);
        let note = format!(
            "{}: price changed to {}",
            day_label(change.since),
            money_magnitude(-change.to_cents)
        );
        let hint = format!(
            "`bob records update subscription {} -s amount={} -n {}`",
            shell_quote(&record.stem),
            new_amount,
            shell_quote(&note)
        );
        findings.push(make_finding(
            "bank_subscription_price",
            Severity::Info,
            &record.path,
            ledger.latest.as_ref().and_then(|charge| charge.line),
            format!(
                "{} price changed from {} to {} on {}",
                record.title,
                money_magnitude(-change.from_cents),
                money_magnitude(-change.to_cents),
                day_label(change.since)
            ),
            Some(hint),
            Some(change.since),
        ));
    }
}

fn append_overlaps(
    registry: &SubscriptionRegistry,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    ledger: &LedgerSnapshot,
    by_path: &HashMap<&str, &SubscriptionRecord>,
    findings: &mut Vec<Finding>,
) {
    let mut needles = BTreeMap::<String, BTreeMap<String, String>>::new();
    for record in registry
        .records
        .iter()
        .filter(|record| record.status != SubscriptionStatus::Cancelled)
    {
        for needle in &record.needles {
            needles
                .entry(needle.to_lowercase())
                .or_default()
                .entry(record.path.clone())
                .or_insert_with(|| record.title.clone());
        }
    }
    for (folded, records) in
        needles.into_iter().filter(|(_, records)| records.len() > 1)
    {
        let record_paths = records.keys().cloned().collect::<Vec<_>>();
        let names = records.values().cloned().collect::<Vec<_>>();
        let path = record_paths.first().cloned().unwrap_or_default();
        let display_needle = registry
            .records
            .iter()
            .find(|record| record_paths.contains(&record.path))
            .and_then(|record| {
                record
                    .needles
                    .iter()
                    .find(|needle| needle.to_lowercase() == folded)
            })
            .cloned()
            .unwrap_or(folded);
        findings.push(make_finding(
            "bank_subscription_overlap",
            Severity::Info,
            path,
            None,
            format!(
                "live subscriptions {} share the statement needle {:?}",
                names.join(", "),
                display_needle
            ),
            Some("Make the Shows-up-as needles distinct, or leave the shared phrase matchless.".to_owned()),
            None,
        ));
    }

    let rows_by_location = rows
        .iter()
        .map(|row| ((row.path.as_str(), row.line), row))
        .collect::<HashMap<_, _>>();
    let mut seen_ids = HashSet::new();
    for ((path, line), attribution) in &ledger.by_row {
        if attribution.tied_paths.is_empty() {
            continue;
        }
        if let Some(row) = rows_by_location.get(&(path.as_str(), *line)) {
            if let Some(id) = row.id.as_deref() {
                seen_ids.insert(id.to_owned());
            }
            append_tied_finding(
                &attribution.tied_paths,
                by_path,
                row.path.clone(),
                Some(row.line),
                Some(row.date),
                findings,
            );
        }
    }
    for event in journal.transactions.values() {
        if seen_ids.contains(&event.id) {
            continue;
        }
        if let Some(attribution) = ledger.by_id.get(&event.id)
            && !attribution.tied_paths.is_empty()
        {
            append_tied_finding(
                &attribution.tied_paths,
                by_path,
                event.note.as_deref().unwrap_or(".bob/bank"),
                None,
                Some(event.posted),
                findings,
            );
        }
    }
}

fn append_tied_finding(
    paths: &[String],
    by_path: &HashMap<&str, &SubscriptionRecord>,
    path: impl Into<String>,
    line: Option<usize>,
    day: Option<NaiveDate>,
    findings: &mut Vec<Finding>,
) {
    let names = paths
        .iter()
        .map(|path| {
            by_path
                .get(path.as_str())
                .map(|record| record.title.clone())
                .unwrap_or_else(|| path.clone())
        })
        .collect::<Vec<_>>();
    findings.push(make_finding(
        "bank_subscription_overlap",
        Severity::Info,
        path,
        line,
        format!("a bank transaction matched tied subscriptions: {}", names.join(", ")),
        Some("Make the Shows-up-as needles distinct, or add an explicit link to the intended record.".to_owned()),
        day,
    ));
}

#[derive(Clone, Copy)]
struct CandidateTxn<'a> {
    event: &'a TxnEvent,
    row: Option<&'a BankRow>,
}

#[derive(Debug)]
struct Candidate {
    title: String,
    amount_cents: i64,
    cadence: SubscriptionCadence,
    needle: Option<String>,
    category: Option<String>,
    annualized_cents: i128,
    latest: NaiveDate,
    merchant: String,
}

fn append_candidate_findings(
    config: Option<&BankConfig>,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    ledger: Option<&LedgerSnapshot>,
    findings: &mut Vec<Finding>,
) {
    let mut candidates = find_candidates(config, journal, rows, ledger);
    candidates.sort_by(|left, right| {
        right
            .annualized_cents
            .cmp(&left.annualized_cents)
            .then(left.merchant.cmp(&right.merchant))
    });
    let omitted = candidates.len().saturating_sub(CANDIDATE_LIMIT);
    for candidate in candidates.into_iter().take(CANDIDATE_LIMIT) {
        let proposal = json!({
            "title": candidate.title,
            "amount_cents": candidate.amount_cents,
            "cadence": candidate.cadence.as_str(),
            "match": candidate.needle.as_ref().map(|needle| vec![needle]),
            "category": candidate.category,
        });
        let command = candidate_command(&proposal);
        let hint = if let Some(needle) = candidate.needle.as_deref() {
            let rule = dismissal_rule(
                needle,
                &candidate.title,
                candidate.category.as_deref(),
            );
            format!(
                "Create it with `{command}`, or add this bank.rules entry: `{rule}`."
            )
        } else {
            format!("Create it with `{command}`.")
        };
        findings.push(Finding {
            code: "bank_subscription_candidate",
            severity: Severity::Info,
            path: ".bob/bank".to_owned(),
            line: None,
            message: format!(
                "possible subscription {}: {} · {} · latest {}",
                candidate.title,
                money_magnitude(-candidate.amount_cents),
                cadence_label(candidate.cadence),
                day_label(candidate.latest)
            ),
            hint: Some(hint),
            day: None,
            key_path: None,
            proposal: Some(proposal),
        });
    }
    if omitted > 0 {
        findings.push(make_finding(
            "bank_subscription_candidate",
            Severity::Info,
            ".bob/bank",
            None,
            format!("… and {omitted} more subscription candidates"),
            Some("Review the highest annualized candidates first.".to_owned()),
            None,
        ));
    }
}

fn find_candidates(
    config: Option<&BankConfig>,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    ledger: Option<&LedgerSnapshot>,
) -> Vec<Candidate> {
    let mut rows_by_id = HashMap::<&str, Vec<&BankRow>>::new();
    for row in rows {
        if let Some(id) = row.id.as_deref() {
            rows_by_id.entry(id).or_default().push(row);
        }
    }
    let unique_rows = rows_by_id
        .into_iter()
        .filter_map(|(id, rows)| (rows.len() == 1).then_some((id, rows[0])))
        .collect::<HashMap<_, _>>();

    let mut groups = BTreeMap::<String, Vec<CandidateTxn<'_>>>::new();
    for event in journal.transactions.values() {
        if event.cents >= 0 {
            continue;
        }
        if ledger
            .and_then(|ledger| ledger.by_id.get(&event.id))
            .is_some_and(|attribution| attribution.record_path.is_some())
        {
            continue;
        }
        let row = unique_rows.get(event.id.as_str()).copied();
        if row.is_some_and(|row| {
            matches!(
                row.category.as_deref(),
                Some("transfer" | "income" | "cash" | "refund")
            )
        }) {
            continue;
        }
        if classify_builtin(&event.desc, None).is_some_and(|builtin| {
            matches!(builtin.category, Some("transfer" | "cash" | "fees"))
        }) {
            continue;
        }
        if config
            .is_some_and(|config| has_read_time_rule_match(config, &event.desc))
        {
            continue;
        }
        let merchant = merchant_key(&event.desc);
        if merchant.is_empty() {
            continue;
        }
        groups
            .entry(merchant)
            .or_default()
            .push(CandidateTxn { event, row });
    }

    groups
        .into_iter()
        .filter_map(|(merchant, mut transactions)| {
            transactions.sort_by(|left, right| {
                left.event
                    .posted
                    .cmp(&right.event.posted)
                    .then(left.event.account.cmp(&right.event.account))
                    .then(left.event.id.cmp(&right.event.id))
            });
            let has_marker = transactions.iter().any(|txn| {
                txn.event
                    .desc
                    .to_ascii_uppercase()
                    .contains("RECURRING PAYMENT")
            });
            let detected = detect_cadence(&transactions);
            if !has_marker && detected.is_none() {
                return None;
            }
            let cadence = detected.unwrap_or(SubscriptionCadence::Monthly);
            let latest = transactions.last()?;
            let amount_cents = latest.event.cents.saturating_neg();
            let cleaned = clean_description(&latest.event.desc);
            let title = sanitize_payee(&title_case(&cleaned));
            let needle = candidate_needle(&title);
            let category = latest.row.and_then(|row| {
                row.category.clone().or_else(|| {
                    row.suggestion
                        .as_ref()
                        .map(|suggestion| suggestion.category.clone())
                })
            });
            let annualized_cents = i128::from(amount_cents)
                .saturating_mul(cadence_per_year(cadence));
            Some(Candidate {
                title,
                amount_cents,
                cadence,
                needle,
                category,
                annualized_cents,
                latest: latest.event.posted,
                merchant,
            })
        })
        .collect()
}

fn detect_cadence(
    transactions: &[CandidateTxn<'_>],
) -> Option<SubscriptionCadence> {
    let mut best: Option<(usize, NaiveDate, SubscriptionCadence)> = None;
    for (minimum, low, high, cadence) in [
        (3, 26, 33, SubscriptionCadence::FourWeekly),
        (3, 85, 95, SubscriptionCadence::Quarterly),
        (2, 340, 390, SubscriptionCadence::Yearly),
    ] {
        for start in 0..transactions.len() {
            for end in start + minimum..=transactions.len() {
                let window = &transactions[start..end];
                let intervals = window
                    .windows(2)
                    .map(|pair| {
                        pair[1]
                            .event
                            .posted
                            .signed_duration_since(pair[0].event.posted)
                            .num_days()
                    })
                    .collect::<Vec<_>>();
                if intervals.iter().any(|days| *days < low || *days > high) {
                    break;
                }
                if !stable_amounts(window) {
                    continue;
                }
                let cadence = if cadence == SubscriptionCadence::FourWeekly {
                    let elapsed = i128::from(
                        window
                            .last()?
                            .event
                            .posted
                            .signed_duration_since(window.first()?.event.posted)
                            .num_days(),
                    );
                    let intervals = i128::try_from(window.len() - 1).ok()?;
                    if elapsed * 2 < intervals * 59 {
                        SubscriptionCadence::FourWeekly
                    } else {
                        SubscriptionCadence::Monthly
                    }
                } else {
                    cadence
                };
                let candidate =
                    (window.len(), window.last()?.event.posted, cadence);
                if best.as_ref().is_none_or(|current| {
                    candidate.0 > current.0
                        || (candidate.0 == current.0 && candidate.1 > current.1)
                }) {
                    best = Some(candidate);
                }
            }
        }
    }
    best.map(|(_, _, cadence)| cadence)
}

fn stable_amounts(transactions: &[CandidateTxn<'_>]) -> bool {
    if transactions.is_empty() {
        return false;
    }
    let mut amounts = transactions
        .iter()
        .map(|txn| txn.event.cents.saturating_neg().max(0))
        .collect::<Vec<_>>();
    amounts.sort_unstable();
    let median = if amounts.len() % 2 == 0 {
        let upper = amounts[amounts.len() / 2];
        let lower = amounts[amounts.len() / 2 - 1];
        lower.saturating_add(upper) / 2
    } else {
        amounts[amounts.len() / 2]
    };
    let spread = amounts
        .last()
        .copied()
        .unwrap_or_default()
        .saturating_sub(amounts.first().copied().unwrap_or_default());
    i128::from(spread).saturating_mul(4) <= i128::from(median)
}

fn candidate_needle(title: &str) -> Option<String> {
    if super::subscriptions::registry::validate_needle(title).is_none() {
        return Some(title.to_owned());
    }
    let mut words = title.split_whitespace().collect::<Vec<_>>();
    words.sort_by_key(|word| std::cmp::Reverse(word.chars().count()));
    words
        .into_iter()
        .find(|word| {
            super::subscriptions::registry::validate_needle(word).is_none()
        })
        .map(str::to_owned)
}

fn cadence_per_year(cadence: SubscriptionCadence) -> i128 {
    match cadence {
        SubscriptionCadence::Monthly => 12,
        SubscriptionCadence::FourWeekly => 13,
        SubscriptionCadence::Quarterly => 4,
        SubscriptionCadence::Yearly => 1,
    }
}

fn cadence_label(cadence: SubscriptionCadence) -> &'static str {
    match cadence {
        SubscriptionCadence::Monthly => "monthly",
        SubscriptionCadence::FourWeekly => "every 4 weeks",
        SubscriptionCadence::Quarterly => "quarterly",
        SubscriptionCadence::Yearly => "yearly",
    }
}

fn dismissal_rule(needle: &str, title: &str, category: Option<&str>) -> String {
    let mut rule = format!(
        "- {{ match: {}, payee: {}",
        yaml_quote(needle),
        yaml_quote(title)
    );
    if let Some(category) = category {
        rule.push_str(&format!(", cat: {}", yaml_quote(category)));
    }
    rule.push_str(" }");
    rule
}

fn candidate_command(proposal: &Value) -> String {
    let title = proposal["title"].as_str().unwrap_or("Subscription");
    let amount = proposal["amount_cents"].as_i64().unwrap_or_default();
    let cadence = proposal["cadence"].as_str().unwrap_or("monthly");
    let mut command = format!(
        "bob records new subscription {} -s status=to_decide -s amount={} -s cadence={}",
        shell_quote(title),
        decimal_amount(amount),
        cadence
    );
    if let Some(needle) = proposal["match"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(Value::as_str)
    {
        command.push_str(&format!(
            " -s {}",
            shell_quote(&format!("match={needle}"))
        ));
    }
    if let Some(category) = proposal["category"].as_str() {
        command.push_str(&format!(
            " -s {}",
            shell_quote(&format!("category={category}"))
        ));
    }
    command
}

fn decimal_amount(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents.unsigned_abs() % 100)
}

fn money_magnitude(signed_cents: i64) -> String {
    render_amount(signed_cents.saturating_abs().saturating_neg())
}

fn day_label(day: NaiveDate) -> String {
    day.format("%b %-d").to_string()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn yaml_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn make_finding(
    code: &'static str,
    severity: Severity,
    path: impl Into<String>,
    line: Option<usize>,
    message: impl Into<String>,
    hint: Option<String>,
    day: Option<NaiveDate>,
) -> Finding {
    Finding {
        code,
        severity,
        path: path.into(),
        line,
        message: message.into(),
        hint,
        day,
        key_path: None,
        proposal: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, NaiveDate};

    use crate::native::bank::{
        config::{parse_bank_config, ConfigParse},
        journal::TxnOutcome,
        subscriptions::registry::{
            InvalidNeedle, NeedleInvalidReason, SubscriptionAlbum,
            SubscriptionPricing,
        },
    };

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn ts() -> DateTime<chrono::FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z").unwrap()
    }

    fn config(extra: &str) -> BankConfig {
        let rules = if extra.trim().is_empty() {
            "[]".to_owned()
        } else {
            format!("\n{extra}")
        };
        let text = format!(
            "---\nbank:\n  accounts:\n    checking: {{ kind: checking, start: 2026-01-01 }}\n  categories:\n    bills: Bills\n    pharmacy: Pharmacy\n  rules: {rules}\n---\n"
        );
        match parse_bank_config(&text) {
            ConfigParse::Ok(config) => config,
            other => panic!("expected valid config, got {other:?}"),
        }
    }

    fn registry(records: Vec<SubscriptionRecord>) -> SubscriptionRegistry {
        SubscriptionRegistry {
            availability: SubscriptionAvailability::Available,
            album: Some(SubscriptionAlbum {
                stem: "subscriptions".to_owned(),
                path: "subscriptions.md".to_owned(),
                title: "Subscriptions".to_owned(),
                directory: "subscriptions".to_owned(),
                emoji: Some("🔁".to_owned()),
            }),
            records,
            diagnostics: Vec::new(),
        }
    }

    fn record(
        stem: &str,
        status: SubscriptionStatus,
        amount_cents: i64,
        needles: &[&str],
    ) -> SubscriptionRecord {
        SubscriptionRecord {
            stem: stem.to_owned(),
            path: format!("subscriptions/{stem}.md"),
            link_target: format!("subscriptions/{stem}"),
            title: stem.to_owned(),
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
            category: None,
            url: None,
            renews: None,
            ended: None,
        }
    }

    fn txn(id: &str, day: NaiveDate, cents: i64, desc: &str) -> TxnEvent {
        let key = format!("{id:0<64}");
        TxnEvent {
            ts: ts(),
            host: "test".to_owned(),
            path: ".bob/bank/test.jsonl".to_owned(),
            line: 1,
            account: "checking".to_owned(),
            key,
            id: id.to_owned(),
            posted: day,
            cents,
            desc: desc.to_owned(),
            merchant: merchant_key(desc),
            outcome: TxnOutcome::BeforeStart,
            note: None,
        }
    }

    fn journal(events: Vec<TxnEvent>) -> JournalSnapshot {
        let mut journal = JournalSnapshot::default();
        for event in events {
            journal.ids.insert(event.id.clone(), event.key.clone());
            journal.transactions.insert(event.key.clone(), event);
        }
        journal
    }

    fn candidate_rows(journal: &JournalSnapshot) -> Vec<Candidate> {
        find_candidates(None, journal, &[], None)
    }

    fn event(
        id: &str,
        year: i32,
        month: u32,
        day: u32,
        cents: i64,
        desc: &str,
    ) -> TxnEvent {
        txn(
            id,
            NaiveDate::from_ymd_opt(year, month, day).unwrap(),
            cents,
            desc,
        )
    }

    fn bank_row(id: &str, day: NaiveDate, cents: i64) -> BankRow {
        BankRow {
            path: "bank/2026-11.md".to_owned(),
            line: 12,
            date: day,
            heading: "## Thu Nov 12".to_owned(),
            account: "checking".to_owned(),
            cents,
            payee: "Tunely Premium".to_owned(),
            category: Some("bills".to_owned()),
            links: Vec::new(),
            note: None,
            flagged: false,
            acknowledged: true,
            id: Some(id.to_owned()),
            origin: "imported",
            suggestion: None,
            subscription: None,
            bank_recurring: true,
            subscription_candidate: true,
        }
    }

    #[test]
    fn candidate_requires_marker_or_recurrence_and_excludes_false_positives() {
        let coffee = [
            event("a0000001", 2026, 11, 1, -500, "COFFEE SHOP"),
            event("a0000002", 2026, 11, 2, -500, "COFFEE SHOP"),
            event("a0000003", 2026, 11, 3, -500, "COFFEE SHOP"),
        ];
        let atm = [
            event("b0000001", 2026, 7, 1, -300, "ATM FEE"),
            event("b0000002", 2026, 7, 29, -300, "ATM FEE"),
            event("b0000003", 2026, 8, 26, -300, "ATM FEE"),
        ];
        let pharmacy = [
            event("c0000001", 2026, 1, 1, -2500, "PHARMACY"),
            event("c0000002", 2026, 3, 8, -2500, "PHARMACY"),
            event("c0000003", 2026, 5, 22, -2500, "PHARMACY"),
        ];
        let barber = [
            event("d0000001", 2026, 1, 1, -4000, "BARBER CLUB"),
            event("d0000002", 2026, 3, 22, -4000, "BARBER CLUB"),
            event("d0000003", 2026, 6, 10, -4000, "BARBER CLUB"),
        ];
        let obligation = [event(
            "e0000001",
            2026,
            11,
            1,
            -1900,
            "GYM RECURRING PAYMENT 11/01/26",
        )];
        let mut all = Vec::new();
        all.extend(coffee);
        all.extend(atm);
        all.extend(pharmacy);
        all.extend(barber);
        all.extend(obligation);
        let journal = journal(all);
        let cfg = config("    - { match: GYM, payee: Gym, cat: bills }\n");
        let candidates = find_candidates(Some(&cfg), &journal, &[], None);
        assert!(
            candidates.is_empty(),
            "unexpected candidates: {candidates:?}"
        );
    }

    #[test]
    fn candidate_finds_four_weekly_marker_first_charge_and_descriptor_change() {
        let four_weekly = [
            event("f0000001", 2026, 7, 1, -999, "TUNELY BILL"),
            event("f0000002", 2026, 7, 29, -999, "TUNELY BILL"),
            event("f0000003", 2026, 8, 26, -999, "TUNELY BILL"),
        ];
        let marker_only = [event(
            "f0000004",
            2026,
            11,
            12,
            -1299,
            "RECURRING PAYMENT 11/12/26 CLOUD BOX",
        )];
        let changed_descriptor = [
            event("f0000005", 2026, 9, 1, -1999, "STREAMLINE PLUS"),
            event("f0000006", 2026, 10, 1, -1999, "STREAMLINE PLUS"),
            event(
                "f0000007",
                2026,
                11,
                1,
                -1999,
                "STREAMLINE PREMIUM RECURRING PAYMENT 11/01/26",
            ),
        ];
        let monthly = [
            event("f0000008", 2026, 1, 15, -1499, "NEWSLETTER PLAN"),
            event("f0000009", 2026, 2, 15, -1499, "NEWSLETTER PLAN"),
            event("f0000010", 2026, 3, 15, -1499, "NEWSLETTER PLAN"),
        ];
        let weekend_shifted_monthly = [
            event("f0000016", 2026, 7, 15, -2499, "WEEKEND BILLING PLAN"),
            event("f0000017", 2026, 8, 17, -2499, "WEEKEND BILLING PLAN"),
            event("f0000018", 2026, 9, 15, -2499, "WEEKEND BILLING PLAN"),
        ];
        let quarterly = [
            event("f0000011", 2026, 1, 1, -2500, "QUARTERLY SERVICE"),
            event("f0000012", 2026, 4, 1, -2500, "QUARTERLY SERVICE"),
            event("f0000013", 2026, 7, 1, -2500, "QUARTERLY SERVICE"),
        ];
        let yearly = [
            event("f0000014", 2025, 11, 15, -12000, "ANNUAL SERVICE"),
            event("f0000015", 2026, 11, 15, -12000, "ANNUAL SERVICE"),
        ];
        let journal = journal(
            four_weekly
                .into_iter()
                .chain(marker_only)
                .chain(changed_descriptor)
                .chain(monthly)
                .chain(weekend_shifted_monthly)
                .chain(quarterly)
                .chain(yearly)
                .collect(),
        );
        let candidates = candidate_rows(&journal);
        assert_eq!(candidates.len(), 7);
        let tunely = candidates
            .iter()
            .find(|item| item.title == "Tunely Bill")
            .unwrap();
        assert_eq!(tunely.cadence, SubscriptionCadence::FourWeekly);
        let newsletter = candidates
            .iter()
            .find(|item| item.title == "Newsletter Plan")
            .unwrap();
        assert_eq!(newsletter.cadence, SubscriptionCadence::Monthly);
        let weekend = candidates
            .iter()
            .find(|item| item.title == "Weekend Billing Plan")
            .unwrap();
        assert_eq!(weekend.cadence, SubscriptionCadence::Monthly);
        let quarterly = candidates
            .iter()
            .find(|item| item.title == "Quarterly Service")
            .unwrap();
        assert_eq!(quarterly.cadence, SubscriptionCadence::Quarterly);
        let annual = candidates
            .iter()
            .find(|item| item.title == "Annual Service")
            .unwrap();
        assert_eq!(annual.cadence, SubscriptionCadence::Yearly);
        let cloud = candidates
            .iter()
            .find(|item| item.title == "Cloud Box")
            .unwrap();
        assert_eq!(cloud.cadence, SubscriptionCadence::Monthly);
        assert!(candidates
            .iter()
            .any(|item| item.title == "Streamline Premium"));
        assert!(candidates.iter().all(|item| {
            item.needle.as_deref().is_some_and(|needle| {
                super::super::subscriptions::registry::validate_needle(needle)
                    .is_none()
            })
        }));
    }

    #[test]
    fn subscription_findings_project_ledger_and_registry_diagnostics() {
        let mut to_cancel = record(
            "Daily Ledger",
            SubscriptionStatus::ToCancel,
            600,
            &["DAILY LEDGER"],
        );
        to_cancel.url = Some("https://manage.example.test".to_owned());
        let mut cancelled = record(
            "Cloudbox",
            SubscriptionStatus::Cancelled,
            999,
            &["CLOUDBOX"],
        );
        cancelled.ended = Some(date(5));
        let mut priced =
            record("Tunely", SubscriptionStatus::Active, 999, &["TUNELY"]);
        priced.category = Some("unknown".to_owned());
        priced.invalid_needles.push(InvalidNeedle {
            text: "PURCHASE".to_owned(),
            reason: NeedleInvalidReason::NoisePhrase,
        });
        let alpha =
            record("Alpha", SubscriptionStatus::Active, 999, &["SHARED BILL"]);
        let beta =
            record("Beta", SubscriptionStatus::ToDecide, 999, &["SHARED BILL"]);
        let mut registry =
            registry(vec![to_cancel, cancelled, priced, alpha, beta]);
        registry.diagnostics.push(
            super::super::subscriptions::registry::SubscriptionDiagnostic {
                code: "bank_subscription_record_invalid",
                path: "subscriptions/bad.md".to_owned(),
                field: Some("amount".to_owned()),
                message: "amount has a fractional cent and was rejected"
                    .to_owned(),
            },
        );
        let journal = journal(vec![
            txn("a1000001", date(10), -600, "DAILY LEDGER"),
            txn("a1000002", date(8), -1299, "CLOUDBOX"),
            txn("a1000003", date(4), -1099, "TUNELY"),
            txn("a1000004", date(11), -1099, "TUNELY"),
            txn("a1000005", date(9), -500, "SHARED BILL"),
        ]);
        let cfg = config("");
        let ledger = compute_ledger(
            &registry,
            &journal,
            &[],
            date(12),
            Some(&cfg),
            &HashMap::new(),
        );
        let mut findings = Vec::new();
        append_record_findings(
            Some(&cfg),
            &registry,
            &journal,
            &[],
            &ledger,
            &mut findings,
        );
        let codes = findings
            .iter()
            .map(|finding| finding.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"bank_subscription_to_cancel_charged"));
        assert!(codes.contains(&"bank_subscription_cancelled_charge"));
        assert!(codes.contains(&"bank_subscription_price"));
        assert!(codes.contains(&"bank_subscription_match_invalid"));
        assert!(codes.contains(&"bank_subscription_category_unknown"));
        assert!(codes.contains(&"bank_subscription_amount_invalid"));
        assert!(codes.contains(&"bank_subscription_overlap"));
    }

    #[test]
    fn cancelled_charge_hint_keeps_manage_url_and_teaches_ended() {
        let mut cancelled = record(
            "Cloudbox",
            SubscriptionStatus::Cancelled,
            999,
            &["CLOUDBOX"],
        );
        cancelled.url = Some("https://example.invalid/manage".to_owned());
        let reg = registry(vec![cancelled.clone()]);
        let journal =
            journal(vec![txn("c1000001", date(11), -999, "CLOUDBOX")]);
        let ledger = compute_ledger(
            &reg,
            &journal,
            &[],
            date(12),
            None,
            &HashMap::new(),
        );
        let mut findings = Vec::new();
        append_ledger_findings(
            &cancelled,
            &ledger.records[&cancelled.path],
            &mut findings,
        );
        let hint = findings[0].hint.as_deref().expect("hint");
        assert!(hint.contains("Manage: https://example.invalid/manage"));
        assert!(hint.contains("ended=YYYY-MM-DD"));

        cancelled.ended = Some(date(10));
        let reg = registry(vec![cancelled.clone()]);
        let ledger = compute_ledger(
            &reg,
            &journal,
            &[],
            date(12),
            None,
            &HashMap::new(),
        );
        let mut findings = Vec::new();
        append_ledger_findings(
            &cancelled,
            &ledger.records[&cancelled.path],
            &mut findings,
        );
        let hint = findings[0].hint.as_deref().expect("hint");
        assert_eq!(hint, "Manage: https://example.invalid/manage");
    }

    #[test]
    fn candidate_json_contains_a_paste_ready_proposal() {
        let journal = journal(vec![event(
            "a1000001",
            2026,
            11,
            12,
            -999,
            "RECURRING PAYMENT 11/12/26 TUNELY PREMIUM",
        )]);
        let rows = [bank_row("a1000001", date(12), -999)];
        let mut findings = Vec::new();
        append_candidate_findings(None, &journal, &rows, None, &mut findings);
        let proposal = findings[0].proposal.as_ref().unwrap();
        assert_eq!(proposal["title"], "Tunely Premium");
        assert_eq!(proposal["amount_cents"], 999);
        assert_eq!(proposal["cadence"], "monthly");
        assert_eq!(proposal["match"][0], "Tunely Premium");
        assert_eq!(proposal["category"], "bills");
        assert!(findings[0]
            .hint
            .as_deref()
            .unwrap()
            .contains("bob records new subscription"));
        assert!(findings[0]
            .hint
            .as_deref()
            .unwrap()
            .contains("category=bills"));
        assert!(findings[0].hint.as_deref().unwrap().contains("- { match:"));
    }

    #[test]
    fn candidates_are_ranked_capped_and_report_one_overflow_line() {
        let events = (1..=11)
            .map(|index| {
                event(
                    &format!("c{index:07x}"),
                    2026,
                    11,
                    12,
                    -(index * 100),
                    &format!("RECURRING PAYMENT 11/12/26 SERVICE {index}"),
                )
            })
            .collect();
        let journal = journal(events);
        let mut findings = Vec::new();
        append_candidate_findings(None, &journal, &[], None, &mut findings);
        assert_eq!(findings.len(), 11);
        assert_eq!(
            findings[0].proposal.as_ref().unwrap()["amount_cents"],
            1100
        );
        assert_eq!(findings[9].proposal.as_ref().unwrap()["amount_cents"], 200);
        assert!(findings[10].proposal.is_none());
        assert!(findings[10].message.contains("and 1 more"));
    }

    #[test]
    fn invalid_album_is_one_warning_and_candidate_checks_still_run() {
        let mut registry = registry(Vec::new());
        registry.availability = SubscriptionAvailability::Invalid;
        let journal = journal(vec![event(
            "a1000001",
            2026,
            11,
            12,
            -999,
            "RECURRING PAYMENT 11/12/26 TUNELY",
        )]);
        let got = findings(None, &registry, &journal, &[], date(12));
        assert_eq!(
            got.iter()
                .filter(|item| item.code == "bank_subscription_album_invalid")
                .count(),
            1
        );
        assert!(got
            .iter()
            .any(|item| item.code == "bank_subscription_candidate"));
    }
}

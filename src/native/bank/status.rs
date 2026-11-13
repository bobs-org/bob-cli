//! Per-account balance proof from the import journal and current ledger rows.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, NaiveDate};

use super::{
    config::{
        account_statuses, Account, AccountKind, AccountStatus, BankConfig,
    },
    identity::merchant_key,
    journal::{ImportEvent, JournalSnapshot, TxnOutcome},
    model::BankRow,
    source::SourceKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusFinding {
    pub(crate) code: &'static str,
    pub(crate) severity: &'static str,
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) day: Option<NaiveDate>,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

#[derive(Debug, Clone)]
struct AccountProof {
    status: &'static str,
    last_import: Option<NaiveDate>,
    balance_cents: Option<i64>,
    balance_asof: Option<NaiveDate>,
    delta_cents: Option<i64>,
    gap: Option<(NaiveDate, NaiveDate)>,
}

/// The configuration-only fallback remains useful to commands that do not
/// report balance state.
pub(crate) fn accounts(config: &BankConfig) -> Vec<AccountStatus> {
    account_statuses(config)
}

/// Compute each account's current proof from journaled bank evidence and the
/// rows still present in the monthly ledger.
pub(crate) fn accounts_with_evidence(
    config: &BankConfig,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    today: NaiveDate,
) -> (Vec<AccountStatus>, Vec<StatusFinding>) {
    let mut accounts = Vec::with_capacity(config.accounts.len());
    let mut findings = Vec::new();
    for (index, account) in config.accounts.iter().enumerate() {
        let proof = proof_for(account, config, journal, rows, today);
        accounts.push(AccountStatus {
            alias: account.alias.clone(),
            label: account.label.clone(),
            kind: account.kind,
            default: index == 0,
            start: account.start,
            status: proof.status,
            hint: status_hint(account, &proof),
            last_import: proof.last_import,
            balance_cents: proof.balance_cents,
            balance_asof: proof.balance_asof,
            delta_cents: proof.delta_cents,
        });
        append_findings(account, journal, rows, today, &proof, &mut findings);
    }
    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
            .then(left.code.cmp(right.code))
            .then(left.message.cmp(&right.message))
    });
    (accounts, findings)
}

fn proof_for(
    account: &Account,
    config: &BankConfig,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    today: NaiveDate,
) -> AccountProof {
    let last_import = journal
        .imports
        .iter()
        .filter(|event| event.account == account.alias)
        .max_by_key(|event| event.ts)
        .map(|event| match crate::native::env::configured_timezone() {
            Some(zone) => event.ts.with_timezone(&zone).date_naive(),
            None => event.ts.with_timezone(&chrono::Local).date_naive(),
        });
    if account.kind == AccountKind::Cash {
        return AccountProof {
            status: "untracked",
            last_import,
            balance_cents: None,
            balance_asof: None,
            delta_cents: None,
            gap: None,
        };
    }

    let latest_balance = latest_balance(journal, &account.alias);
    let Some((balance_event, balance_cents, balance_asof)) = latest_balance
    else {
        return AccountProof {
            status: "unverified",
            last_import,
            balance_cents: None,
            balance_asof: None,
            delta_cents: None,
            gap: None,
        };
    };
    if !journal.io_errors.is_empty() || !journal.malformed.is_empty() {
        return AccountProof {
            status: "unverified",
            last_import,
            balance_cents: Some(balance_cents),
            balance_asof: Some(balance_asof),
            delta_cents: None,
            gap: None,
        };
    }
    let Some(start) = account.start else {
        return AccountProof {
            status: "unverified",
            last_import,
            balance_cents: Some(balance_cents),
            balance_asof: Some(balance_asof),
            delta_cents: None,
            gap: None,
        };
    };

    let anchor = journal
        .imports
        .iter()
        .filter(|event| {
            event.account == account.alias
                && event.balance_cents.is_some()
                && event.balance_asof.is_some_and(|asof| asof >= start)
        })
        .min_by_key(|event| (event.balance_asof.unwrap_or(start), event.ts));
    let delta_cents = anchor.and_then(|anchor_event| {
        let anchor_balance = anchor_event.balance_cents?;
        let anchor_asof = anchor_event.balance_asof?;
        let opening = anchor_balance
            - transaction_sum(journal, &account.alias, start, anchor_asof);
        Some(
            balance_cents
                - (opening
                    + ledger_transaction_sum(
                        journal,
                        rows,
                        &account.alias,
                        start,
                        balance_asof,
                    )),
        )
    });
    let gap = coverage_gap(
        journal
            .imports
            .iter()
            .filter(|event| event.account == account.alias),
        start,
        balance_asof,
    );
    let stale = today.signed_duration_since(balance_asof)
        > Duration::days(i64::from(config.stale_after));
    let status = if stale {
        "stale"
    } else if gap.is_some() {
        "gap"
    } else if delta_cents.is_some_and(|delta| delta != 0) {
        "delta"
    } else if delta_cents.is_some() {
        "balanced"
    } else {
        "unverified"
    };
    let _ = balance_event;
    AccountProof {
        status,
        last_import,
        balance_cents: Some(balance_cents),
        balance_asof: Some(balance_asof),
        delta_cents,
        gap,
    }
}

fn latest_balance<'a>(
    journal: &'a JournalSnapshot,
    alias: &str,
) -> Option<(&'a ImportEvent, i64, NaiveDate)> {
    journal
        .imports
        .iter()
        .filter(|event| event.account == alias)
        .filter_map(|event| {
            Some((event, event.balance_cents?, event.balance_asof?))
        })
        .max_by_key(|(event, _, asof)| (*asof, event.ts))
}

fn transaction_sum(
    journal: &JournalSnapshot,
    alias: &str,
    start: NaiveDate,
    through: NaiveDate,
) -> i64 {
    journal
        .transactions
        .values()
        .filter(|event| {
            event.account == alias
                && event.outcome != TxnOutcome::BeforeStart
                && event.posted >= start
                && event.posted <= through
        })
        .map(|event| event.cents)
        .sum()
}

fn ledger_transaction_sum(
    journal: &JournalSnapshot,
    rows: &[BankRow],
    alias: &str,
    start: NaiveDate,
    through: NaiveDate,
) -> i64 {
    journal
        .transactions
        .values()
        .filter(|event| {
            event.account == alias
                && event.outcome != TxnOutcome::BeforeStart
                && event.posted >= start
                && event.posted <= through
        })
        .filter_map(|event| {
            let mut matching = rows.iter().filter(|row| {
                row.account == alias && row.id.as_deref() == Some(&event.id)
            });
            let row = matching.next()?;
            matching.next().is_none().then_some(row.cents)
        })
        .sum()
}

fn coverage_gap<'a>(
    imports: impl Iterator<Item = &'a ImportEvent>,
    start: NaiveDate,
    through: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let mut windows = imports
        .filter(|event| event.source == SourceKind::Ofx)
        .filter_map(|event| {
            let first = event.start.max(start);
            let last = event.end.min(through);
            (first <= last).then_some((first, last))
        })
        .collect::<Vec<_>>();
    windows.sort();
    let mut covered_through = start.pred_opt()?;
    for (first, last) in windows {
        if last <= covered_through {
            continue;
        }
        if first > covered_through.succ_opt()? {
            return Some((covered_through.succ_opt()?, first.pred_opt()?));
        }
        covered_through = covered_through.max(last);
        if covered_through >= through {
            return None;
        }
    }
    if covered_through < through {
        Some((covered_through.succ_opt()?, through))
    } else {
        None
    }
}

fn status_hint(account: &Account, proof: &AccountProof) -> String {
    match proof.status {
        "untracked" => "Cash accounts are never imported".to_owned(),
        "balanced" => format!(
            "Ledger balance matches through {}",
            proof.balance_asof.expect("balanced has an as-of date")
        ),
        "delta" => format!(
            "Bank balance differs by {}",
            super::grammar::render_amount(
                proof.delta_cents.expect("delta status has a delta")
            )
        ),
        "gap" => {
            let (from, to) = proof.gap.expect("gap status has a gap");
            format!("QFX coverage gap {from}–{to}")
        }
        "stale" => format!(
            "Last QFX balance is {}; import a current statement",
            proof.balance_asof.expect("stale has an as-of date")
        ),
        _ if proof.last_import.is_some() => {
            "No QFX ledger balance yet — import a QFX statement to verify"
                .to_owned()
        }
        _ => {
            let _ = account;
            "No import yet — bob bank import FILE".to_owned()
        }
    }
}

fn append_findings(
    account: &Account,
    journal: &JournalSnapshot,
    rows: &[BankRow],
    _today: NaiveDate,
    proof: &AccountProof,
    findings: &mut Vec<StatusFinding>,
) {
    if account.kind == AccountKind::Cash {
        return;
    }
    if let Some((from, to)) = proof.gap {
        findings.push(finding(
            "bank_coverage_gap",
            "warning",
            "bank.md",
            None,
            None,
            format!("{} has a QFX coverage gap {from}–{to}", account.alias),
            Some(
                "Import a statement whose coverage overlaps the gap."
                    .to_owned(),
            ),
        ));
    }
    if proof.status == "stale" {
        findings.push(finding(
            "bank_account_stale",
            "warning",
            "bank.md",
            None,
            proof.balance_asof,
            format!(
                "{} last reported a ledger balance on {}",
                account.alias,
                proof.balance_asof.expect("stale has an as-of date")
            ),
            Some("Import a current QFX statement.".to_owned()),
        ));
    }
    if let Some(delta) = proof.delta_cents
        && delta != 0
    {
        findings.push(finding(
            "bank_balance_delta",
            "error",
            "bank.md",
            None,
            proof.balance_asof,
            format!(
                "{} QFX balance differs from the reconstructed ledger by {}",
                account.alias,
                super::grammar::render_amount(delta)
            ),
            Some("Review missing or edited rows; bob never writes an adjusting row.".to_owned()),
        ));
    }

    let account_rows = rows
        .iter()
        .filter(|row| row.account == account.alias)
        .collect::<Vec<_>>();
    let mut by_id: HashMap<&str, Vec<&BankRow>> = HashMap::new();
    for row in &account_rows {
        if let Some(id) = row.id.as_deref() {
            by_id.entry(id).or_default().push(row);
        }
    }
    let transactions = journal
        .transactions
        .values()
        .filter(|event| {
            event.account == account.alias
                && event.outcome != TxnOutcome::BeforeStart
        })
        .collect::<Vec<_>>();
    for event in &transactions {
        match by_id.get(event.id.as_str()).map(Vec::as_slice) {
            None | Some([]) => findings.push(finding(
                "bank_row_missing",
                "error",
                event.note.as_deref().unwrap_or("bank.md"),
                None,
                Some(event.posted),
                format!(
                    "{} transaction from {} is missing",
                    account.alias, event.posted
                ),
                Some("Run `bob bank import -F` to restore journaled rows deliberately.".to_owned()),
            )),
            Some(found) => {
                if found.len() != 1 {
                    continue;
                }
                let row = found[0];
                if row.cents != event.cents {
                    findings.push(finding(
                        "bank_amount_edited",
                        "warning",
                        &row.path,
                        Some(row.line),
                        Some(row.date),
                        format!(
                            "{} row amount {} differs from the bank's {}; keeping your edit",
                            account.alias,
                            super::grammar::render_amount(row.cents),
                            super::grammar::render_amount(event.cents)
                        ),
                        None,
                    ));
                }
            }
        }
    }

    let windows = journal
        .imports
        .iter()
        .filter(|event| {
            event.account == account.alias && event.source == SourceKind::Ofx
        })
        .map(|event| (event.start, event.end))
        .collect::<Vec<_>>();
    let Some(start) = account.start else {
        return;
    };
    let through = proof.balance_asof;
    let typed = account_rows
        .iter()
        .filter(|row| {
            row.id.is_none()
                && row.date >= start
                && through.is_some_and(|end| {
                    row.date <= end
                        && windows.iter().any(|(first, last)| {
                            row.date >= *first && row.date <= *last
                        })
                })
        })
        .collect::<Vec<_>>();
    let mut twin_pairs = HashSet::new();
    for row in typed {
        for event in &transactions {
            if event.cents == row.cents
                || event.merchant != merchant_key(&row.payee)
                || !(-1..=3).contains(&(row.date - event.posted).num_days())
            {
                continue;
            }
            let pair = (row.path.as_str(), row.line, event.id.as_str());
            if twin_pairs.insert(pair) {
                findings.push(finding(
                    "bank_twins",
                    "warning",
                    &row.path,
                    Some(row.line),
                    Some(row.date),
                    format!(
                        "possible twin of {} transaction on {} with a different amount",
                        account.alias, event.posted
                    ),
                    Some("Compare the typed and posted rows; nothing is merged automatically.".to_owned()),
                ));
            }
        }
        let exact_match = transactions.iter().any(|event| {
            event.cents == row.cents
                && event.merchant == merchant_key(&row.payee)
                && (-1..=3).contains(&(row.date - event.posted).num_days())
        });
        if !exact_match {
            findings.push(finding(
                "bank_row_unmatched",
                "info",
                &row.path,
                Some(row.line),
                Some(row.date),
                format!(
                    "typed {} row has no matching bank transaction yet",
                    account.alias
                ),
                Some("Check again after the next bank import.".to_owned()),
            ));
        }
    }
}

fn finding(
    code: &'static str,
    severity: &'static str,
    path: impl Into<String>,
    line: Option<usize>,
    day: Option<NaiveDate>,
    message: String,
    hint: Option<String>,
) -> StatusFinding {
    StatusFinding {
        code,
        severity,
        path: path.into(),
        line,
        day,
        message,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, FixedOffset, NaiveDate};

    use super::*;
    use crate::native::bank::journal::{ImportCounts, TxnEvent, TxnOutcome};

    fn day(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn stamp(day: u32) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(&format!(
            "2026-11-{day:02}T12:00:00-05:00"
        ))
        .unwrap()
    }

    fn config(start: u32) -> BankConfig {
        match super::super::config::parse_bank_config(&format!(
            "---\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-{start:02}\n  categories:\n    groceries: Groceries\n  stale_after: 10\n---\n"
        )) {
            super::super::config::ConfigParse::Ok(config) => config,
            other => panic!("expected valid config, got {other:?}"),
        }
    }

    fn import(
        start: u32,
        end: u32,
        balance_day: u32,
        balance: i64,
        ts: u32,
    ) -> ImportEvent {
        ImportEvent {
            ts: stamp(ts),
            host: "athena".to_owned(),
            path: ".bob/bank/athena.jsonl".to_owned(),
            line: ts,
            account: "checking".to_owned(),
            source: SourceKind::Ofx,
            file_sha256: format!("{ts:064x}"),
            start: day(start),
            end: day(end),
            balance_cents: Some(balance),
            balance_asof: Some(day(balance_day)),
            counts: ImportCounts::default(),
        }
    }

    fn txn(id: &str, posted: u32, cents: i64) -> TxnEvent {
        TxnEvent {
            ts: stamp(posted),
            host: "athena".to_owned(),
            path: ".bob/bank/athena.jsonl".to_owned(),
            line: posted,
            account: "checking".to_owned(),
            key: format!("{id:0<64}"),
            id: id.to_owned(),
            posted: day(posted),
            cents,
            desc: "SHOPRITE 1234".to_owned(),
            merchant: "shoprite".to_owned(),
            outcome: TxnOutcome::Written,
            note: Some("bank/2026-11.md".to_owned()),
        }
    }

    fn row(id: &str, posted: u32, cents: i64) -> BankRow {
        BankRow {
            path: "bank/2026-11.md".to_owned(),
            line: posted as usize,
            date: day(posted),
            heading: format!("## {}", day(posted)),
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
        }
    }

    #[test]
    fn derives_the_opening_anchor_and_detects_a_coverage_gap() {
        let config = config(1);
        let mut journal = JournalSnapshot::default();
        journal.imports =
            vec![import(1, 5, 5, 10_000, 6), import(8, 12, 12, 10_000, 12)];
        journal
            .transactions
            .insert("a".to_owned(), txn("aaaaaaaa", 3, -1_000));
        journal
            .transactions
            .insert("b".to_owned(), txn("bbbbbbbb", 10, 0));
        let (accounts, findings) = accounts_with_evidence(
            &config,
            &journal,
            &[row("aaaaaaaa", 3, -1_000), row("bbbbbbbb", 10, 0)],
            day(12),
        );
        assert_eq!(accounts[0].status, "gap");
        assert_eq!(accounts[0].balance_cents, Some(10_000));
        assert_eq!(accounts[0].delta_cents, Some(0));
        assert!(findings.iter().any(|finding| {
            finding.code == "bank_coverage_gap"
                && finding.message.contains("2026-11-06–2026-11-07")
        }));
    }

    #[test]
    fn detects_balance_delta_missing_row_and_stale_accounts() {
        let config = config(1);
        let mut journal = JournalSnapshot::default();
        journal.imports =
            vec![import(1, 5, 5, 10_000, 5), import(1, 5, 5, 10_250, 5)];
        journal
            .transactions
            .insert("a".to_owned(), txn("aaaaaaaa", 3, -1_000));
        journal
            .transactions
            .insert("b".to_owned(), txn("bbbbbbbb", 4, -500));
        let (accounts, findings) = accounts_with_evidence(
            &config,
            &journal,
            &[row("aaaaaaaa", 3, -1_000)],
            day(20),
        );
        assert_eq!(accounts[0].status, "stale");
        assert_eq!(accounts[0].delta_cents, Some(-250));
        assert!(findings
            .iter()
            .any(|finding| finding.code == "bank_balance_delta"));
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.code == "bank_row_missing")
                .count(),
            1
        );
        assert!(findings
            .iter()
            .any(|finding| finding.code == "bank_account_stale"));
    }

    #[test]
    fn csv_without_a_ledger_balance_stays_unverified() {
        let config = config(1);
        let mut journal = JournalSnapshot::default();
        let mut event = import(1, 5, 5, 0, 6);
        event.source = SourceKind::Csv;
        event.balance_cents = None;
        event.balance_asof = None;
        journal.imports.push(event);
        let (accounts, _) =
            accounts_with_evidence(&config, &journal, &[], day(12));
        assert_eq!(accounts[0].status, "unverified");
        assert_eq!(accounts[0].last_import, Some(day(6)));
        assert_eq!(accounts[0].balance_cents, None);
    }

    #[test]
    fn reports_edited_amounts_twins_and_unmatched_typed_rows() {
        let config = config(1);
        let mut journal = JournalSnapshot::default();
        journal.imports.push(import(1, 12, 12, 10_000, 12));
        let mut event = txn("aaaaaaaa", 3, -1_000);
        event.desc = "SHOPRITE".to_owned();
        journal.ids.insert(event.id.clone(), event.key.clone());
        journal.transactions.insert(event.key.clone(), event);
        let rows = vec![
            BankRow {
                path: "bank/2026-11.md".to_owned(),
                line: 3,
                date: day(3),
                heading: "## Tue Nov 3".to_owned(),
                account: "checking".to_owned(),
                cents: -900,
                payee: "Shoprite".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: false,
                id: Some("aaaaaaaa".to_owned()),
                origin: "imported",
                suggestion: None,
            },
            BankRow {
                path: "bank/2026-11.md".to_owned(),
                line: 4,
                date: day(3),
                heading: "## Tue Nov 3".to_owned(),
                account: "checking".to_owned(),
                cents: -1_200,
                payee: "Shoprite".to_owned(),
                category: None,
                links: Vec::new(),
                note: None,
                flagged: false,
                acknowledged: true,
                id: None,
                origin: "typed",
                suggestion: None,
            },
        ];
        let (_, findings) =
            accounts_with_evidence(&config, &journal, &rows, day(12));
        for code in ["bank_amount_edited", "bank_twins", "bank_row_unmatched"] {
            assert!(
                findings.iter().any(|finding| finding.code == code),
                "{code}"
            );
        }
    }

    #[test]
    fn deleted_imported_row_sets_delta_and_missing_finding() {
        let config = config(1);
        let mut journal = JournalSnapshot::default();
        journal.imports.push(import(1, 12, 12, 10_000, 12));
        let event = txn("aaaaaaaa", 3, -1_000);
        journal.ids.insert(event.id.clone(), event.key.clone());
        journal.transactions.insert(event.key.clone(), event);
        let (accounts, findings) =
            accounts_with_evidence(&config, &journal, &[], day(12));
        assert_eq!(accounts[0].status, "delta");
        assert_eq!(accounts[0].delta_cents, Some(-1_000));
        assert!(findings
            .iter()
            .any(|finding| finding.code == "bank_row_missing"));
        assert!(findings
            .iter()
            .any(|finding| finding.code == "bank_balance_delta"));
    }
}

//! Effective, read-only report for `bob bank config show`.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    config::{self, AccountStatus, BankConfig, Category, ConfigParse, Rule},
    journal::JournalSnapshot,
    model,
    month::{parse_month_note, LocatedRowKind},
    scan::{self, BankScan},
    status,
};
use crate::native::style::Styler;

const SCHEMA_VERSION: u32 = 1;
const HOTKEYS: &str = "abcdefghijklmnopqrstuvwxyz1234567890";

#[derive(Debug, Clone)]
pub(crate) struct ConfigReport {
    pub(crate) today: NaiveDate,
    pub(crate) status: &'static str,
    pub(crate) source_line: Option<usize>,
    pub(crate) partial: bool,
    pub(crate) teaching_empty: bool,
    pub(crate) config_fingerprint: Option<String>,
    pub(crate) categories: Vec<CategoryView>,
    pub(crate) accounts: Vec<AccountView>,
    pub(crate) rules: Vec<RuleView>,
    pub(crate) stale_after: Option<StaleAfterView>,
    pub(crate) free_hotkeys: Vec<char>,
    pub(crate) findings: Vec<ConfigFinding>,
    pub(crate) usage: Usage,
    pub(crate) starter: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CategoryView {
    category: Category,
    rows: usize,
    rule_references: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct AccountView {
    status: AccountStatus,
    source_line: Option<usize>,
    label_authored: bool,
    explicit_rows: usize,
    default_rows: usize,
    journal_imports: usize,
    journal_transactions: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct RuleView {
    index: usize,
    rule: Rule,
}

#[derive(Debug, Clone)]
pub(crate) struct StaleAfterView {
    days: u32,
    authored: bool,
    source_line: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Usage {
    rows: usize,
    journal_imports: usize,
    journal_transactions: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigFinding {
    code: &'static str,
    severity: &'static str,
    path: String,
    line: Option<usize>,
    key_path: Option<String>,
    message: String,
    hint: Option<String>,
}

impl ConfigReport {
    pub(crate) fn warning_messages(&self) -> Vec<String> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == "warning")
            .map(|finding| format!("{}: {}", finding.code, finding.message))
            .collect()
    }

    pub(crate) fn build(
        scan: &BankScan,
        journal: &JournalSnapshot,
        today: NaiveDate,
    ) -> Self {
        let mut report = Self {
            today,
            status: "missing",
            source_line: None,
            partial: false,
            teaching_empty: false,
            config_fingerprint: None,
            categories: Vec::new(),
            accounts: Vec::new(),
            rules: Vec::new(),
            stale_after: None,
            free_hotkeys: Vec::new(),
            findings: Vec::new(),
            usage: Usage::default(),
            starter: None,
        };

        if scan::is_teaching_empty(scan) {
            report.status = "teaching_empty";
            report.teaching_empty = true;
            report.starter = Some(starter(today));
            return report;
        }

        let Some(hub) = &scan.hub else {
            report.findings.push(ConfigFinding {
                code: "bank_hub_missing",
                severity: "error",
                path: "bank.md".to_owned(),
                line: None,
                key_path: Some("bank".to_owned()),
                message: "bank/ exists without bank.md".to_owned(),
                hint: Some("Create bank.md with a bank: mapping.".to_owned()),
            });
            report.starter = Some(starter(today));
            return report;
        };

        report.source_line = config::bank_source_line(&hub.contents);
        report.config_fingerprint = config::config_fingerprint(&hub.contents);
        let parsed = config::parse_bank_config(&hub.contents);
        let (config, _invalid) = match parsed {
            ConfigParse::MissingMapping => {
                report.findings.push(ConfigFinding {
                    code: "bank_hub_missing",
                    severity: "error",
                    path: "bank.md".to_owned(),
                    line: None,
                    key_path: Some("bank".to_owned()),
                    message: "bank.md has no bank: mapping".to_owned(),
                    hint: Some(
                        "Add the starter bank: mapping below.".to_owned(),
                    ),
                });
                report.starter = Some(starter(today));
                return report;
            }
            ConfigParse::Invalid {
                diagnostics,
                config,
            } => {
                report.status = "invalid";
                report.partial = config.is_some();
                report.findings.extend(diagnostics.into_iter().map(|item| {
                    ConfigFinding {
                        code: "bank_config_invalid",
                        severity: "error",
                        path: "bank.md".to_owned(),
                        line: item.line,
                        key_path: Some(item.key_path),
                        message: item.message,
                        hint: item.hint,
                    }
                }));
                (config, true)
            }
            ConfigParse::Ok(config) => {
                report.status = "valid";
                (Some(config), false)
            }
        };

        if let Some(config) = config {
            report.fill_config(config, scan, journal);
        }
        report
    }

    fn fill_config(
        &mut self,
        config: BankConfig,
        scan: &BankScan,
        journal: &JournalSnapshot,
    ) {
        let rows = model::collect_rows(scan, Some(&config), Some(journal));
        let (account_statuses, _) =
            status::accounts_with_evidence(&config, journal, &rows, self.today);
        let (row_count, explicit_rows, default_rows) =
            account_row_usage(scan, &config);
        self.usage = Usage {
            rows: row_count,
            journal_imports: journal.imports.len(),
            journal_transactions: journal.transactions.len(),
        };

        self.categories = config
            .categories
            .iter()
            .map(|category| CategoryView {
                category: category.clone(),
                rows: rows
                    .iter()
                    .filter(|row| {
                        row.category.as_deref() == Some(&category.key)
                    })
                    .count(),
                rule_references: config
                    .rules
                    .iter()
                    .filter(|rule| rule.cat.as_deref() == Some(&category.key))
                    .count(),
            })
            .collect();

        self.accounts = config
            .accounts
            .iter()
            .zip(account_statuses)
            .map(|(account, state)| AccountView {
                status: state,
                source_line: account.source_line,
                label_authored: account.label_authored,
                explicit_rows: *explicit_rows.get(&account.alias).unwrap_or(&0),
                default_rows: *default_rows.get(&account.alias).unwrap_or(&0),
                journal_imports: journal
                    .imports
                    .iter()
                    .filter(|event| event.account == account.alias)
                    .count(),
                journal_transactions: journal
                    .transactions
                    .values()
                    .filter(|event| event.account == account.alias)
                    .count(),
            })
            .collect();

        self.rules = config
            .rules
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, rule)| RuleView { index, rule })
            .collect();
        self.stale_after = Some(StaleAfterView {
            days: config.stale_after,
            authored: config.stale_after_authored,
            source_line: config.stale_after_line,
        });
        let claimed: std::collections::HashSet<char> = config
            .categories
            .iter()
            .filter_map(|category| category.hotkey)
            .collect();
        self.free_hotkeys = HOTKEYS
            .chars()
            .filter(|key| *key != 'q' && !claimed.contains(key))
            .collect();

        for (index, rule) in config.rules.iter().enumerate() {
            if contains_long_digit_run(&rule.match_text) {
                self.findings.push(ConfigFinding {
                    code: "bank_rule_read_time_dead",
                    severity: "warning",
                    path: "bank.md".to_owned(),
                    line: rule.source_line,
                    key_path: Some(format!("bank.rules.{index}.match")),
                    message: format!(
                        "rule match {:?} cannot match read-time hints because journal descriptions mask digit runs; imports still use it",
                        rule.match_text
                    ),
                    hint: Some(
                        "Use a digit-free match phrase if this rule should affect read-time hints.".to_owned(),
                    ),
                });
            }
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let errors = self
            .findings
            .iter()
            .filter(|finding| finding.severity == "error")
            .count();
        let warnings = self
            .findings
            .iter()
            .filter(|finding| finding.severity == "warning")
            .count();
        json!({
            "ok": errors == 0,
            "schema_version": SCHEMA_VERSION,
            "today": self.today.format("%Y-%m-%d").to_string(),
            "state": self.status,
            "status": self.status,
            "path": "bank.md",
            "line": self.source_line,
            "fingerprint": self.config_fingerprint,
            "partial": self.partial,
            "teaching_empty": self.teaching_empty,
            "config_fingerprint": self.config_fingerprint,
            "categories": self.categories.iter().map(CategoryView::to_json).collect::<Vec<_>>(),
            "accounts": self.accounts.iter().map(AccountView::to_json).collect::<Vec<_>>(),
            "rules": self.rules.iter().map(RuleView::to_json).collect::<Vec<_>>(),
            "stale_after": self.stale_after.as_ref().map(StaleAfterView::to_json),
            "free_hotkeys": self.free_hotkeys.iter().map(char::to_string).collect::<Vec<_>>(),
            "usage": {
                "rows": self.usage.rows,
                "journal": {
                    "import_events": self.usage.journal_imports,
                    "transaction_events": self.usage.journal_transactions,
                    "events": self.usage.journal_imports + self.usage.journal_transactions,
                },
            },
            "issues": {"errors": errors, "warnings": warnings},
            "findings": self.findings.iter().map(ConfigFinding::to_json).collect::<Vec<_>>(),
            "starter": self.starter,
        })
    }
}

impl CategoryView {
    fn to_json(&self) -> Value {
        let category = &self.category;
        let mut value = category.to_json();
        let object = value.as_object_mut().expect("category JSON object");
        object.insert("source_line".to_owned(), json!(category.source_line));
        object.insert("line".to_owned(), json!(category.source_line));
        object.insert(
            "hotkey_source".to_owned(),
            json!(category.hotkey_source.as_str()),
        );
        object.insert("rows".to_owned(), json!(self.rows));
        object.insert("rule_refs".to_owned(), json!(self.rule_references));
        object.insert("declared".to_owned(), json!(category.declared));
        object.insert(
            "provenance".to_owned(),
            json!({
                "entry": if category.declared { "declared" } else { "builtin" },
                "label": if category.label_authored { "authored" } else { "default" },
                "hotkey": category.hotkey_source.as_str(),
            }),
        );
        object.insert(
            "usage".to_owned(),
            json!({"rows": self.rows, "rule_references": self.rule_references}),
        );
        value
    }
}

impl AccountView {
    fn to_json(&self) -> Value {
        let mut value = self.status.to_json();
        let object = value.as_object_mut().expect("account JSON object");
        object.insert("source_line".to_owned(), json!(self.source_line));
        object.insert("line".to_owned(), json!(self.source_line));
        object.insert("label_explicit".to_owned(), json!(self.label_authored));
        object.insert(
            "rows".to_owned(),
            json!(self.explicit_rows + self.default_rows),
        );
        object.insert(
            "journal_events".to_owned(),
            json!(self.journal_imports + self.journal_transactions),
        );
        object.insert(
            "provenance".to_owned(),
            json!({"label": if self.label_authored { "authored" } else { "default" }}),
        );
        object.insert(
            "usage".to_owned(),
            json!({
                "rows": self.explicit_rows + self.default_rows,
                "explicit_rows": self.explicit_rows,
                "default_rows": self.default_rows,
                "journal_events": {
                    "imports": self.journal_imports,
                    "transactions": self.journal_transactions,
                },
            }),
        );
        value
    }
}

impl RuleView {
    fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "match": self.rule.match_text,
            "payee": self.rule.payee,
            "cat": self.rule.cat,
            "link": self.rule.link,
            "source_line": self.rule.source_line,
        })
    }
}

impl StaleAfterView {
    fn to_json(&self) -> Value {
        json!({
            "days": self.days,
            "explicit": self.authored,
            "line": self.source_line,
            "source_line": self.source_line,
        })
    }
}

impl ConfigFinding {
    fn to_json(&self) -> Value {
        json!({
            "code": self.code,
            "severity": self.severity,
            "path": self.path,
            "line": self.line,
            "key_path": self.key_path,
            "message": self.message,
            "hint": self.hint,
        })
    }
}

pub(crate) fn print_human(report: &ConfigReport) {
    let styler = Styler::detect();
    println!("🏦 Bank setup · {}", report.status.replace('_', " "));
    if report.teaching_empty {
        println!("No bank hub yet. Create bank.md with this starter mapping:");
        if let Some(starter) = &report.starter {
            println!("\n{starter}");
        }
        return;
    }
    if report.status == "missing" {
        println!("No usable bank: mapping. Add one to bank.md:");
        if let Some(starter) = &report.starter {
            println!("\n{starter}");
        }
    }
    if let Some(fingerprint) = &report.config_fingerprint {
        println!("Fingerprint  {}", styler.dim(fingerprint));
    }

    if !report.accounts.is_empty() {
        println!("\nAccounts");
        for account in &report.accounts {
            let state = match account.status.status {
                "balanced" => "✓ balanced",
                "delta" => "✗ delta",
                "gap" => "⚠ coverage gap",
                "stale" => "⚠ stale",
                "unverified" => "· unverified",
                "untracked" => "· untracked",
                other => other,
            };
            let default = if account.status.default {
                " · default"
            } else {
                ""
            };
            let line = account
                .source_line
                .map(|line| format!(" · bank.md:{line}"))
                .unwrap_or_default();
            println!(
                "  {} · {}{} · {} rows ({} explicit, {} default){}",
                account.status.label,
                account.status.alias,
                default,
                account.explicit_rows + account.default_rows,
                account.explicit_rows,
                account.default_rows,
                line,
            );
            println!("    {state}");
        }
    }

    if !report.categories.is_empty() {
        println!("\nCategories");
        let mut groups: Vec<(String, Vec<&CategoryView>)> = Vec::new();
        for category in &report.categories {
            let group = category
                .category
                .group
                .clone()
                .unwrap_or_else(|| "Ungrouped".to_owned());
            if let Some((_, items)) =
                groups.iter_mut().find(|(name, _)| *name == group)
            {
                items.push(category);
            } else {
                groups.push((group, vec![category]));
            }
        }
        for (group, categories) in groups {
            println!("  {}", styler.dim(&group));
            for item in categories {
                let category = &item.category;
                let keycap = category
                    .hotkey
                    .map(|key| key.to_ascii_uppercase().to_string())
                    .unwrap_or_else(|| "·".to_owned());
                let hidden = if category.hidden { " · hidden" } else { "" };
                let line = category
                    .source_line
                    .map(|line| format!(" · bank.md:{line}"))
                    .unwrap_or_default();
                println!(
                    "    [{keycap}] {} {} · {} rows · {}{}{}",
                    category.emoji,
                    category.label,
                    item.rows,
                    category.hotkey_source.as_str().replace('_', " "),
                    hidden,
                    line,
                );
            }
        }
    }
    if !report.free_hotkeys.is_empty() {
        let free = report
            .free_hotkeys
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        println!("\nFree keys  {free}  (q reserved)");
    }

    println!("\nRules");
    if report.rules.is_empty() {
        println!("  No rules");
    } else {
        for rule in &report.rules {
            let line = rule
                .rule
                .source_line
                .map(|line| format!(" · bank.md:{line}"))
                .unwrap_or_default();
            let category = rule
                .rule
                .cat
                .as_deref()
                .map(|cat| format!(" → {cat}"))
                .unwrap_or_default();
            println!(
                "  {}. {}{}{}",
                rule.index + 1,
                rule.rule.match_text,
                category,
                line
            );
        }
    }
    if let Some(stale_after) = &report.stale_after {
        let source = if stale_after.authored {
            "explicit"
        } else {
            "default"
        };
        let line = stale_after
            .source_line
            .map(|line| format!(" · bank.md:{line}"))
            .unwrap_or_default();
        println!("\nStale after  {} days ({source}){line}", stale_after.days);
    }

    if report.findings.is_empty() {
        if report.status == "valid" {
            println!(
                "\n{}",
                styler.green("✓ bank: is valid — bob uses it as shown")
            );
        }
    } else {
        println!("\nChecks");
        for finding in &report.findings {
            let line = finding
                .line
                .map(|line| format!("{line}"))
                .unwrap_or_else(|| "?".to_owned());
            let key_path = finding
                .key_path
                .as_deref()
                .map(|path| format!(" {path}:"))
                .unwrap_or_default();
            let icon = if finding.severity == "error" {
                "✗"
            } else {
                "⚠"
            };
            let rendered = format!(
                "{icon} {}:{line}{key_path} {}",
                finding.path, finding.message
            );
            if finding.severity == "error" {
                println!("  {}", styler.red(&rendered));
            } else {
                println!("  {}", styler.yellow(&rendered));
            }
            if let Some(hint) = &finding.hint {
                println!("    {}", styler.dim(hint));
            }
        }
    }
}

fn account_row_usage(
    scan: &BankScan,
    config: &BankConfig,
) -> (usize, HashMap<String, usize>, HashMap<String, usize>) {
    let default_alias = config
        .default_account()
        .map(|account| account.alias.as_str());
    let mut explicit = HashMap::new();
    let mut defaults = HashMap::new();
    let mut rows = 0;
    for month in &scan.months {
        let note = parse_month_note(&month.path, &month.contents, month.month);
        for day in &note.days {
            for row in &day.rows {
                let LocatedRowKind::Parsed(parsed) = &row.kind else {
                    continue;
                };
                rows += 1;
                if let Some(alias) = parsed.account.as_deref() {
                    *explicit.entry(alias.to_owned()).or_insert(0) += 1;
                } else if let Some(alias) = default_alias {
                    *defaults.entry(alias.to_owned()).or_insert(0) += 1;
                }
            }
        }
    }
    (rows, explicit, defaults)
}

fn contains_long_digit_run(text: &str) -> bool {
    let mut run = 0;
    for character in text.chars() {
        if character.is_ascii_digit() {
            run += 1;
            if run >= 4 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn starter(today: NaiveDate) -> String {
    format!(
        "bank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: {}",
        today.format("%Y-%m-%d")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::bank::{
        grammar::YearMonth,
        journal::{
            ImportCounts, ImportEvent, JournalSnapshot, TxnEvent, TxnOutcome,
        },
        scan::{BankScan, HubFile, MonthFile},
        source::SourceKind,
    };

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()
    }

    fn scan(hub: &str, month: Option<&str>) -> BankScan {
        BankScan {
            hub: Some(HubFile {
                path: "bank.md".to_owned(),
                contents: hub.to_owned(),
            }),
            bank_dir_exists: month.is_some(),
            months: month
                .map(|contents| {
                    vec![MonthFile {
                        path: "bank/2026-11.md".to_owned(),
                        month: YearMonth {
                            year: 2026,
                            month: 11,
                        },
                        contents: contents.to_owned(),
                    }]
                })
                .unwrap_or_default(),
            month_entries: Vec::new(),
            io_errors: Vec::new(),
        }
    }

    const HUB: &str = "---\nbank:\n  accounts:\n    checking:\n      kind: checking\n      start: 2026-11-01\n  categories:\n    coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }\n    hidden: { label: Hidden, hotkey: h, hidden: true }\n  rules:\n    - match: VENMO\n      cat: coffee\n    - { match: CARD 1234, cat: coffee }\n---\n";

    #[test]
    fn report_carries_every_effective_field_provenance_usage_and_finding() {
        let month = "---\nmonth: 2026-11\n---\n\n# November 2026\n\n## Thu Nov 12\n\n- [ ] $5.00 Cafe [cat:: coffee]\n- [x] $3.00 Store [acct:: checking] [cat:: coffee]\n";
        let report = ConfigReport::build(
            &scan(HUB, Some(month)),
            &JournalSnapshot::default(),
            today(),
        );
        let json = report.to_json();
        assert_eq!(json["state"], "valid");
        assert_eq!(json["status"], "valid");
        assert_eq!(json["path"], "bank.md");
        assert_eq!(json["line"], 2);
        assert_eq!(json["partial"], false);
        assert!(json["config_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(json["fingerprint"], json["config_fingerprint"]);
        assert_eq!(json["categories"].as_array().unwrap().len(), 6);
        assert_eq!(json["categories"][0]["provenance"]["hotkey"], "explicit");
        assert_eq!(json["categories"][0]["hotkey_source"], "explicit");
        assert_eq!(
            json["categories"][0]["line"],
            json["categories"][0]["source_line"]
        );
        assert_eq!(json["categories"][2]["provenance"]["entry"], "builtin");
        assert_eq!(json["categories"][0]["usage"]["rows"], 2);
        assert_eq!(json["categories"][0]["rows"], 2);
        assert_eq!(json["categories"][0]["usage"]["rule_references"], 2);
        assert_eq!(json["accounts"][0]["usage"]["explicit_rows"], 1);
        assert_eq!(json["accounts"][0]["usage"]["default_rows"], 1);
        assert_eq!(json["accounts"][0]["label_explicit"], false);
        assert_eq!(json["accounts"][0]["rows"], 2);
        assert_eq!(json["accounts"][0]["journal_events"], 0);
        assert_eq!(json["rules"].as_array().unwrap().len(), 2);
        assert_eq!(json["stale_after"]["days"], 10);
        assert_eq!(json["stale_after"]["explicit"], false);
        assert_eq!(json["findings"][0]["code"], "bank_rule_read_time_dead");
        assert_eq!(json["findings"][0]["line"], 13);
        assert_eq!(json["findings"][0]["key_path"], "bank.rules.1.match");
        assert!(json["free_hotkeys"]
            .as_array()
            .unwrap()
            .iter()
            .all(|key| key != "q"));
    }

    #[test]
    fn report_hotkey_provenance_covers_all_sources() {
        let hub = concat!(
            "---\nbank:\n  accounts:\n    checking: { label: Primary, kind: checking, start: 2026-11-01 }\n",
            "  categories:\n",
            "    items: { label: Items, hotkey: i }\n",
            "    hidden: { label: Hidden, hotkey: h, hidden: true }\n",
            "    private: { label: Private, hidden: true }\n",
            "    income: { label: Pay, group: In }\n---\n",
        );
        let report = ConfigReport::build(
            &scan(hub, None),
            &JournalSnapshot::default(),
            today(),
        )
        .to_json();
        let categories = report["categories"].as_array().unwrap();
        let category = |key: &str| {
            categories
                .iter()
                .find(|item| item["key"] == key)
                .unwrap_or_else(|| panic!("missing category {key}"))
        };

        assert_eq!(category("items")["hotkey_source"], "explicit");
        assert_eq!(category("items")["provenance"]["label"], "authored");
        assert_eq!(category("hidden")["hotkey_source"], "explicit");
        assert_eq!(category("hidden")["hidden"], true);
        assert_eq!(category("private")["hotkey_source"], "none");
        assert_eq!(category("income")["hotkey_source"], "derived");
        assert_eq!(category("income")["hotkey"], "n");
        assert_eq!(category("refund")["hotkey_source"], "builtin_default");
        assert_eq!(category("refund")["provenance"]["entry"], "builtin");
        assert_eq!(category("refund")["provenance"]["label"], "default");
        assert_eq!(report["accounts"][0]["label_explicit"], true);
        assert_eq!(report["accounts"][0]["provenance"]["label"], "authored");
    }

    #[test]
    fn live_config_shape_reports_the_expected_free_hotkeys() {
        let unavailable = "qbjlmuwxyz90irta"
            .chars()
            .collect::<std::collections::HashSet<_>>();
        let explicit = HOTKEYS
            .chars()
            .filter(|key| !unavailable.contains(key))
            .collect::<Vec<_>>();
        assert_eq!(explicit.len(), 20);
        let mut hub = String::from(
            "---\nbank:\n  accounts:\n    checking: { kind: checking, start: 2026-11-01 }\n  categories:\n",
        );
        for (index, key) in explicit.iter().enumerate() {
            hub.push_str(&format!(
                "    cat{index}: {{ label: Category {index}, hotkey: {key} }}\n"
            ));
        }
        hub.push_str("    hidden: { label: Hidden, hidden: true }\n---\n");

        let report = ConfigReport::build(
            &scan(&hub, None),
            &JournalSnapshot::default(),
            today(),
        )
        .to_json();
        assert_eq!(report["categories"].as_array().unwrap().len(), 25);
        assert_eq!(
            report["free_hotkeys"],
            json!(["b", "j", "l", "m", "u", "w", "x", "y", "z", "9", "0"])
        );
    }

    #[test]
    fn report_counts_journal_events_by_account() {
        let stamp =
            chrono::DateTime::parse_from_rfc3339("2026-11-12T12:00:00-05:00")
                .unwrap();
        let posted = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        let journal = JournalSnapshot {
            imports: vec![ImportEvent {
                ts: stamp,
                host: "athena".to_owned(),
                path: "bank/2026-11.md".to_owned(),
                line: 1,
                account: "checking".to_owned(),
                source: SourceKind::Ofx,
                file_sha256: "abc".to_owned(),
                start: posted,
                end: posted,
                balance_cents: None,
                balance_asof: None,
                counts: ImportCounts::default(),
            }],
            transactions: [(
                "key".to_owned(),
                TxnEvent {
                    ts: stamp,
                    host: "athena".to_owned(),
                    path: "bank/2026-11.md".to_owned(),
                    line: 2,
                    account: "checking".to_owned(),
                    key: "key".to_owned(),
                    id: "abcd1234".to_owned(),
                    posted,
                    cents: -500,
                    desc: "Cafe".to_owned(),
                    merchant: "cafe".to_owned(),
                    outcome: TxnOutcome::Written,
                    note: None,
                },
            )]
            .into_iter()
            .collect(),
            ..JournalSnapshot::default()
        };
        let json =
            ConfigReport::build(&scan(HUB, None), &journal, today()).to_json();
        assert_eq!(json["usage"]["journal"]["import_events"], 1);
        assert_eq!(json["usage"]["journal"]["transaction_events"], 1);
        assert_eq!(
            json["accounts"][0]["usage"]["journal_events"]["imports"],
            1
        );
        assert_eq!(
            json["accounts"][0]["usage"]["journal_events"]["transactions"],
            1
        );
    }

    #[test]
    fn invalid_partial_parse_and_missing_mapping_keep_guidance() {
        let invalid = HUB.replace("cat: coffee", "cat: absent");
        let report = ConfigReport::build(
            &scan(&invalid, None),
            &JournalSnapshot::default(),
            today(),
        );
        let json = report.to_json();
        assert_eq!(json["status"], "invalid");
        assert_eq!(json["state"], "invalid");
        assert_eq!(json["partial"], true);
        assert!(!json["categories"].as_array().unwrap().is_empty());
        assert!(!json["accounts"].as_array().unwrap().is_empty());
        assert_eq!(json["rules"].as_array().unwrap().len(), 2);
        assert!(json["free_hotkeys"]
            .as_array()
            .unwrap()
            .iter()
            .all(|key| key != "q"));
        assert_eq!(json["findings"][0]["key_path"], "bank.rules.0.cat");
        assert_eq!(json["findings"][0]["line"], 12);

        let missing = ConfigReport::build(
            &scan("---\nparent: cash\n---\n", None),
            &JournalSnapshot::default(),
            today(),
        )
        .to_json();
        assert_eq!(missing["status"], "missing");
        assert_eq!(missing["state"], "missing");
        assert_eq!(missing["partial"], false);
        assert!(missing["starter"].as_str().unwrap().contains("checking:"));
        assert!(missing["rules"].as_array().unwrap().is_empty());
    }

    #[test]
    fn teaching_empty_report_is_explicit() {
        let scan = BankScan {
            hub: None,
            bank_dir_exists: false,
            months: Vec::new(),
            month_entries: Vec::new(),
            io_errors: Vec::new(),
        };
        let json =
            ConfigReport::build(&scan, &JournalSnapshot::default(), today())
                .to_json();
        assert_eq!(json["ok"], true);
        assert_eq!(json["state"], "teaching_empty");
        assert_eq!(json["teaching_empty"], true);
        assert!(json["starter"].as_str().unwrap().contains("2026-11-15"));
    }
}

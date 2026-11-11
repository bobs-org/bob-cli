//! `bank:` configuration parser with key-path diagnostics.

use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDate;
use serde_yaml::{Mapping, Value};

use crate::native::albums::{key_line_map, output::split_icon};
use crate::native::projects::{
    parse_frontmatter, trim_yaml_scalar, Frontmatter,
};

use super::grammar::YearMonth;

const BANK_KEYS: &[&str] = &["accounts", "categories", "rules", "stale_after"];
const ACCOUNT_KEYS: &[&str] = &["label", "kind", "start"];
const RULE_KEYS: &[&str] = &["match", "payee", "cat", "link"];
const DEFAULT_STALE_AFTER: u32 = 10;

pub(crate) const BUILTIN_CATEGORIES: &[(&str, &str, &str)] = &[
    ("income", "💵", "Income"),
    ("refund", "↩️", "Refund"),
    ("transfer", "↔️", "Transfer"),
    ("cash", "🏧", "Cash out"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountKind {
    Checking,
    Savings,
    Credit,
    Cash,
}

impl AccountKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Savings => "savings",
            Self::Credit => "credit",
            Self::Cash => "cash",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "checking" => Some(Self::Checking),
            "savings" => Some(Self::Savings),
            "credit" => Some(Self::Credit),
            "cash" => Some(Self::Cash),
            _ => None,
        }
    }

    pub(crate) fn importable(self) -> bool {
        !matches!(self, Self::Cash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    pub(crate) alias: String,
    pub(crate) label: String,
    pub(crate) kind: AccountKind,
    pub(crate) start: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Category {
    pub(crate) key: String,
    pub(crate) emoji: String,
    pub(crate) label: String,
    pub(crate) builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) match_text: String,
    pub(crate) payee: Option<String>,
    pub(crate) cat: Option<String>,
    pub(crate) link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BankConfig {
    pub(crate) accounts: Vec<Account>,
    pub(crate) categories: Vec<Category>,
    pub(crate) rules: Vec<Rule>,
    pub(crate) stale_after: u32,
}

impl BankConfig {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn default_account(&self) -> Option<&Account> {
        self.accounts.first()
    }

    pub(crate) fn account(&self, alias: &str) -> Option<&Account> {
        self.accounts.iter().find(|account| account.alias == alias)
    }

    pub(crate) fn category(&self, key: &str) -> Option<&Category> {
        self.categories.iter().find(|category| category.key == key)
    }

    pub(crate) fn is_builtin_meaning(key: &str) -> bool {
        BUILTIN_CATEGORIES.iter().any(|(name, _, _)| *name == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigDiagnostic {
    pub(crate) line: Option<usize>,
    pub(crate) key_path: String,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigParse {
    MissingMapping,
    Invalid {
        diagnostics: Vec<ConfigDiagnostic>,
        config: Option<BankConfig>,
    },
    Ok(BankConfig),
}

pub(crate) fn parse_bank_config(contents: &str) -> ConfigParse {
    let Some(frontmatter) = parse_frontmatter(contents) else {
        return ConfigParse::MissingMapping;
    };
    parse_bank_config_from_frontmatter(&frontmatter)
}

pub(crate) fn parse_bank_config_from_frontmatter(
    frontmatter: &Frontmatter<'_>,
) -> ConfigParse {
    let line_by_path = key_line_map(frontmatter);
    let Some(section) = nested_section(frontmatter, "bank") else {
        return ConfigParse::MissingMapping;
    };
    match serde_yaml::from_str::<Value>(&section.yaml) {
        Ok(value) => match mapping_get_from_value(&value, "bank") {
            Some(bank) => {
                parse_bank_value(bank, &line_by_path, section.line_number)
            }
            None => ConfigParse::MissingMapping,
        },
        Err(error) => {
            let line = error
                .location()
                .map(|location| section.line_number + location.line() - 1)
                .unwrap_or(section.line_number);
            ConfigParse::Invalid {
                diagnostics: vec![ConfigDiagnostic {
                    line: Some(line),
                    key_path: "bank".to_owned(),
                    message: format!("could not parse bank YAML: {error}"),
                    hint: None,
                }],
                config: None,
            }
        }
    }
}

fn mapping_get_from_value<'a>(
    value: &'a Value,
    key: &str,
) -> Option<&'a Value> {
    value
        .as_mapping()
        .and_then(|mapping| mapping_get(mapping, key))
}

fn parse_bank_value(
    value: &Value,
    line_by_path: &HashMap<String, usize>,
    bank_line: usize,
) -> ConfigParse {
    let Some(mapping) = value.as_mapping() else {
        return ConfigParse::Invalid {
            diagnostics: vec![ConfigDiagnostic {
                line: Some(bank_line),
                key_path: "bank".to_owned(),
                message: "bank: must be a mapping".to_owned(),
                hint: None,
            }],
            config: None,
        };
    };
    let mut diagnostics = Vec::new();
    for key in mapping_keys(mapping) {
        if !BANK_KEYS.contains(&key.as_str()) {
            diagnostics.push(unknown_key(
                "bank",
                &key,
                BANK_KEYS,
                line_by_path,
            ));
        }
    }

    let accounts = parse_accounts(mapping, line_by_path, &mut diagnostics);
    let categories = parse_categories(mapping, line_by_path, &mut diagnostics);
    let rules =
        parse_rules(mapping, line_by_path, &categories, &mut diagnostics);
    let stale_after =
        parse_stale_after(mapping, line_by_path, &mut diagnostics);

    let config = (!accounts.is_empty()).then_some(BankConfig {
        accounts,
        categories,
        rules,
        stale_after,
    });
    if diagnostics.is_empty() {
        match config {
            Some(config) => ConfigParse::Ok(config),
            None => ConfigParse::Invalid {
                diagnostics: vec![ConfigDiagnostic {
                    line: line_by_path
                        .get("bank.accounts")
                        .copied()
                        .or_else(|| line_by_path.get("bank").copied()),
                    key_path: "bank.accounts".to_owned(),
                    message: "bank.accounts is required and must not be empty"
                        .to_owned(),
                    hint: None,
                }],
                config: None,
            },
        }
    } else {
        ConfigParse::Invalid {
            diagnostics,
            config,
        }
    }
}

fn parse_accounts(
    mapping: &Mapping,
    line_by_path: &HashMap<String, usize>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Vec<Account> {
    let Some(value) = mapping_get(mapping, "accounts") else {
        diagnostics.push(ConfigDiagnostic {
            line: line_by_path.get("bank").copied(),
            key_path: "bank.accounts".to_owned(),
            message: "bank.accounts is required and must not be empty"
                .to_owned(),
            hint: None,
        });
        return Vec::new();
    };
    let Some(accounts) = value.as_mapping() else {
        diagnostics.push(ConfigDiagnostic {
            line: line_by_path.get("bank.accounts").copied(),
            key_path: "bank.accounts".to_owned(),
            message: "bank.accounts must be a mapping".to_owned(),
            hint: None,
        });
        return Vec::new();
    };
    if accounts.is_empty() {
        diagnostics.push(ConfigDiagnostic {
            line: line_by_path.get("bank.accounts").copied(),
            key_path: "bank.accounts".to_owned(),
            message: "bank.accounts is required and must not be empty"
                .to_owned(),
            hint: None,
        });
        return Vec::new();
    }
    let mut parsed = Vec::new();
    for (key, account_value) in accounts {
        let Some(alias) = key.as_str() else {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get("bank.accounts").copied(),
                key_path: "bank.accounts".to_owned(),
                message: "account aliases must be strings".to_owned(),
                hint: None,
            });
            continue;
        };
        let path = format!("bank.accounts.{alias}");
        if !is_alias(alias) {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get(&path).copied(),
                key_path: path.clone(),
                message: format!(
                    "account alias {alias:?} must match [a-z][a-z0-9_]*"
                ),
                hint: None,
            });
        }
        parsed.extend(parse_account(
            alias,
            account_value,
            &path,
            line_by_path,
            diagnostics,
        ));
    }
    parsed
}

fn parse_account(
    alias: &str,
    value: &Value,
    path: &str,
    line_by_path: &HashMap<String, usize>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<Account> {
    let Some(mapping) = value.as_mapping() else {
        diagnostics.push(ConfigDiagnostic {
            line: line_by_path.get(path).copied(),
            key_path: path.to_owned(),
            message: format!("{path} must be a mapping"),
            hint: None,
        });
        return None;
    };
    for key in mapping_keys(mapping) {
        if !ACCOUNT_KEYS.contains(&key.as_str()) {
            diagnostics.push(unknown_key(
                path,
                &key,
                ACCOUNT_KEYS,
                line_by_path,
            ));
        }
    }
    let label = optional_string(mapping, "label")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| humanize_alias(alias));
    let kind_path = format!("{path}.kind");
    let kind = match required_string(mapping, "kind") {
        Some(raw) => match AccountKind::parse(&raw) {
            Some(kind) => Some(kind),
            None => {
                diagnostics.push(ConfigDiagnostic {
                    line: line_by_path.get(&kind_path).copied(),
                    key_path: kind_path,
                    message: "expected checking, savings, credit, or cash"
                        .to_owned(),
                    hint: None,
                });
                None
            }
        },
        None => {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get(&kind_path).copied(),
                key_path: kind_path,
                message: "kind is required".to_owned(),
                hint: None,
            });
            None
        }
    }?;
    let start_path = format!("{path}.start");
    let start = match optional_string(mapping, "start") {
        Some(raw) => match NaiveDate::parse_from_str(&raw, "%Y-%m-%d") {
            Ok(date) => Some(date),
            Err(_) => {
                diagnostics.push(ConfigDiagnostic {
                    line: line_by_path.get(&start_path).copied(),
                    key_path: start_path.clone(),
                    message: "start must be YYYY-MM-DD".to_owned(),
                    hint: None,
                });
                None
            }
        },
        None => None,
    };
    if kind != AccountKind::Cash && start.is_none() {
        diagnostics.push(ConfigDiagnostic {
            line: line_by_path.get(&start_path).copied(),
            key_path: start_path,
            message: "start is required for every kind except cash".to_owned(),
            hint: None,
        });
        return None;
    }
    Some(Account {
        alias: alias.to_owned(),
        label,
        kind,
        start,
    })
}

fn parse_categories(
    mapping: &Mapping,
    line_by_path: &HashMap<String, usize>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Vec<Category> {
    let mut declared = Vec::new();
    let mut declared_keys = Vec::new();
    if let Some(value) = mapping_get(mapping, "categories") {
        let Some(categories) = value.as_mapping() else {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get("bank.categories").copied(),
                key_path: "bank.categories".to_owned(),
                message: "bank.categories must be a mapping".to_owned(),
                hint: None,
            });
            return with_builtins(Vec::new());
        };
        for (key, label_value) in categories {
            let Some(name) = key.as_str() else {
                diagnostics.push(ConfigDiagnostic {
                    line: line_by_path.get("bank.categories").copied(),
                    key_path: "bank.categories".to_owned(),
                    message: "category keys must be strings".to_owned(),
                    hint: None,
                });
                continue;
            };
            let path = format!("bank.categories.{name}");
            if !is_alias(name) {
                diagnostics.push(ConfigDiagnostic {
                    line: line_by_path.get(&path).copied(),
                    key_path: path.clone(),
                    message: format!(
                        "category key {name:?} must match [a-z][a-z0-9_]*"
                    ),
                    hint: None,
                });
            }
            let Some(label) = label_value.as_str() else {
                diagnostics.push(ConfigDiagnostic {
                    line: line_by_path.get(&path).copied(),
                    key_path: path,
                    message: "category labels must be strings".to_owned(),
                    hint: None,
                });
                continue;
            };
            let trimmed = label.trim();
            if trimmed.is_empty() {
                diagnostics.push(ConfigDiagnostic {
                    line: line_by_path.get(&path).copied(),
                    key_path: path,
                    message: "category labels must not be empty".to_owned(),
                    hint: None,
                });
                continue;
            }
            let (emoji, rest) = split_icon(trimmed);
            let builtin = BankConfig::is_builtin_meaning(name);
            declared.push(Category {
                key: name.to_owned(),
                emoji: emoji.unwrap_or("").to_owned(),
                label: if rest.is_empty() {
                    trimmed.to_owned()
                } else {
                    rest.to_owned()
                },
                builtin,
            });
            declared_keys.push(name.to_owned());
        }
    }
    let mut categories = declared;
    for (key, emoji, label) in BUILTIN_CATEGORIES {
        if !declared_keys.iter().any(|name| name == key) {
            categories.push(Category {
                key: (*key).to_owned(),
                emoji: (*emoji).to_owned(),
                label: (*label).to_owned(),
                builtin: true,
            });
        }
    }
    categories
}

fn with_builtins(mut declared: Vec<Category>) -> Vec<Category> {
    let keys: Vec<String> = declared
        .iter()
        .map(|category| category.key.clone())
        .collect();
    for (key, emoji, label) in BUILTIN_CATEGORIES {
        if !keys.iter().any(|name| name == key) {
            declared.push(Category {
                key: (*key).to_owned(),
                emoji: (*emoji).to_owned(),
                label: (*label).to_owned(),
                builtin: true,
            });
        }
    }
    declared
}

fn parse_rules(
    mapping: &Mapping,
    line_by_path: &HashMap<String, usize>,
    categories: &[Category],
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Vec<Rule> {
    let Some(value) = mapping_get(mapping, "rules") else {
        return Vec::new();
    };
    let Some(rules) = value.as_sequence() else {
        diagnostics.push(ConfigDiagnostic {
            line: line_by_path.get("bank.rules").copied(),
            key_path: "bank.rules".to_owned(),
            message: "bank.rules must be a list".to_owned(),
            hint: None,
        });
        return Vec::new();
    };
    let mut parsed = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        let path = format!("bank.rules.{index}");
        let Some(rule_map) = rule.as_mapping() else {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get("bank.rules").copied(),
                key_path: path,
                message: "each rule must be a mapping".to_owned(),
                hint: None,
            });
            continue;
        };
        for key in mapping_keys(rule_map) {
            if !RULE_KEYS.contains(&key.as_str()) {
                diagnostics.push(unknown_key(
                    &path,
                    &key,
                    RULE_KEYS,
                    line_by_path,
                ));
            }
        }
        let Some(match_text) = required_string(rule_map, "match")
            .filter(|value| !value.is_empty())
        else {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get(&format!("{path}.match")).copied(),
                key_path: format!("{path}.match"),
                message: "match is required".to_owned(),
                hint: None,
            });
            continue;
        };
        let cat = optional_string(rule_map, "cat");
        if let Some(cat) = cat.as_deref()
            && !categories.iter().any(|category| category.key == cat)
        {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get(&format!("{path}.cat")).copied(),
                key_path: format!("{path}.cat"),
                message: format!("unknown category {cat:?}"),
                hint: nearest(&[cat], &category_keys(categories)),
            });
        }
        parsed.push(Rule {
            match_text,
            payee: optional_string(rule_map, "payee"),
            cat,
            link: optional_string(rule_map, "link"),
        });
    }
    parsed
}

fn category_keys(categories: &[Category]) -> Vec<String> {
    categories
        .iter()
        .map(|category| category.key.clone())
        .collect()
}

fn parse_stale_after(
    mapping: &Mapping,
    line_by_path: &HashMap<String, usize>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> u32 {
    let Some(value) = mapping_get(mapping, "stale_after") else {
        return DEFAULT_STALE_AFTER;
    };
    let path = "bank.stale_after";
    if let Some(number) = value.as_u64() {
        if number > u32::MAX as u64 {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get(path).copied(),
                key_path: path.to_owned(),
                message: "stale_after must be a number of days".to_owned(),
                hint: None,
            });
            return DEFAULT_STALE_AFTER;
        }
        return number as u32;
    }
    if let Some(number) = value.as_i64() {
        if number < 0 {
            diagnostics.push(ConfigDiagnostic {
                line: line_by_path.get(path).copied(),
                key_path: path.to_owned(),
                message: "stale_after must be a non-negative number of days"
                    .to_owned(),
                hint: None,
            });
            return DEFAULT_STALE_AFTER;
        }
        return number as u32;
    }
    diagnostics.push(ConfigDiagnostic {
        line: line_by_path.get(path).copied(),
        key_path: path.to_owned(),
        message: "stale_after must be a number of days".to_owned(),
        hint: None,
    });
    DEFAULT_STALE_AFTER
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountStatus {
    pub(crate) alias: String,
    pub(crate) label: String,
    pub(crate) kind: AccountKind,
    pub(crate) default: bool,
    pub(crate) start: Option<NaiveDate>,
    pub(crate) status: &'static str,
    pub(crate) hint: String,
}

pub(crate) fn account_statuses(config: &BankConfig) -> Vec<AccountStatus> {
    config
        .accounts
        .iter()
        .enumerate()
        .map(|(index, account)| {
            let (status, hint) = if account.kind.importable() {
                (
                    "unverified",
                    "No import yet — bob bank import FILE".to_owned(),
                )
            } else {
                ("untracked", "Cash accounts are never imported".to_owned())
            };
            AccountStatus {
                alias: account.alias.clone(),
                label: account.label.clone(),
                kind: account.kind,
                default: index == 0,
                start: account.start,
                status,
                hint,
            }
        })
        .collect()
}

pub(crate) fn month_from_filename(path: &Path) -> Option<YearMonth> {
    let stem = path.file_stem()?.to_str()?;
    YearMonth::parse(stem)
}

struct NestedSection {
    yaml: String,
    line_number: usize,
}

fn nested_section<'a>(
    frontmatter: &Frontmatter<'a>,
    key: &str,
) -> Option<NestedSection> {
    let start = frontmatter.lines.iter().position(|line| {
        !line.starts_with([' ', '\t'])
            && line
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(':'))
    })?;
    let mut end = start + 1;
    while end < frontmatter.lines.len() {
        let line = frontmatter.lines[end];
        if !line.starts_with([' ', '\t'])
            && !line.trim().is_empty()
            && !line.trim_start().starts_with('#')
        {
            break;
        }
        end += 1;
    }
    Some(NestedSection {
        yaml: frontmatter.lines[start..end].join("\n"),
        line_number: start + 2,
    })
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

fn mapping_keys(mapping: &Mapping) -> Vec<String> {
    mapping
        .keys()
        .filter_map(|key| key.as_str().map(ToOwned::to_owned))
        .collect()
}

fn required_string(mapping: &Mapping, key: &str) -> Option<String> {
    optional_string(mapping, key).filter(|value| !value.is_empty())
}

fn optional_string(mapping: &Mapping, key: &str) -> Option<String> {
    let value = mapping_get(mapping, key)?;
    let raw = value.as_str()?.trim();
    let trimmed = trim_yaml_scalar(raw).trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn is_alias(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn humanize_alias(alias: &str) -> String {
    alias
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn unknown_key(
    parent: &str,
    key: &str,
    allowed: &[&str],
    line_by_path: &HashMap<String, usize>,
) -> ConfigDiagnostic {
    let path = format!("{parent}.{key}");
    ConfigDiagnostic {
        line: line_by_path
            .get(&path)
            .copied()
            .or_else(|| line_by_path.get(parent).copied()),
        key_path: path,
        message: format!("unknown key {key:?}"),
        hint: nearest(
            &[key],
            &allowed.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        ),
    }
}

pub(crate) fn suggest_name(value: &str, allowed: &[String]) -> Option<String> {
    nearest(&[value], allowed)
}

fn nearest(query: &[&str], allowed: &[String]) -> Option<String> {
    let needle = query.first()?.to_lowercase();
    let mut candidates: Vec<(usize, String)> = allowed
        .iter()
        .map(|candidate| {
            (
                edit_distance(&needle, &candidate.to_lowercase()),
                candidate.clone(),
            )
        })
        .filter(|(distance, _)| *distance <= 2)
        .collect();
    candidates.sort();
    let names: Vec<String> = candidates
        .into_iter()
        .take(3)
        .map(|(_, name)| name)
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(format!("did you mean {}?", names.join(", ")))
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_byte) in left.bytes().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_byte) in right.bytes().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(
                    previous[right_index]
                        + usize::from(left_byte != right_byte),
                );
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ConfigParse {
        parse_bank_config(text)
    }

    fn valid_hub() -> &'static str {
        "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-12\n  categories:\n    groceries: 🛒 Groceries\n  rules:\n    - match: VENMO\n      cat: transfer\n  stale_after: 10\n---\n# Bank\n"
    }

    #[test]
    fn parses_checking_account_and_builtins() {
        let ConfigParse::Ok(config) = parse(valid_hub()) else {
            panic!("expected ok");
        };
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].alias, "checking");
        assert_eq!(config.default_account().unwrap().alias, "checking");
        assert_eq!(config.accounts[0].kind, AccountKind::Checking);
        assert!(config.category("groceries").is_some());
        assert!(config.category("income").is_some());
        assert!(config.category("transfer").unwrap().builtin);
        assert_eq!(config.stale_after, 10);
        assert_eq!(config.rules[0].match_text, "VENMO");
        assert_eq!(account_statuses(&config)[0].status, "unverified");
    }

    #[test]
    fn unknown_keys_name_file_line_and_path() {
        let text = "---\nbank:\n  accountz:\n    checking:\n      kind: checking\n      start: 2026-11-12\n---\n";
        let ConfigParse::Invalid { diagnostics, .. } = parse(text) else {
            panic!("expected invalid");
        };
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.key_path.contains("accountz"))
            .unwrap();
        assert_eq!(diagnostic.line, Some(3));
        assert!(
            diagnostic
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("accounts")),
            "{diagnostic:?}"
        );
    }

    #[test]
    fn kind_error_uses_key_path() {
        let text = "---\nbank:\n  accounts:\n    checking:\n      kind: debit\n      start: 2026-11-12\n---\n";
        let ConfigParse::Invalid { diagnostics, .. } = parse(text) else {
            panic!("expected invalid");
        };
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.key_path, "bank.accounts.checking.kind");
        assert_eq!(diagnostic.line, Some(5));
        assert!(diagnostic
            .message
            .contains("checking, savings, credit, or cash"));
    }

    #[test]
    fn missing_bank_mapping() {
        assert!(matches!(
            parse("---\nparent: \"[[cash]]\"\n---\n"),
            ConfigParse::MissingMapping
        ));
    }
}

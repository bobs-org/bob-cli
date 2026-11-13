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
const CATEGORY_KEYS: &[&str] = &["label", "hotkey", "group", "hidden"];
const RULE_KEYS: &[&str] = &["match", "payee", "cat", "link"];
const DEFAULT_STALE_AFTER: u32 = 10;
const RESERVED_HOTKEY: char = 'q';
const BUILTIN_GROUP: &str = "Money moves";

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
    pub(crate) hotkey: Option<char>,
    pub(crate) group: Option<String>,
    pub(crate) hidden: bool,
}

impl Category {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "key": self.key,
            "emoji": self.emoji,
            "label": self.label,
            "builtin": self.builtin,
            "hotkey": self.hotkey.map(|hotkey| hotkey.to_string()),
            "group": self.group,
            "hidden": self.hidden,
        })
    }
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
    let line_by_path = bank_line_map(frontmatter);
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
    let mut explicit = Vec::new();
    let mut claimed_explicit: HashMap<char, String> = HashMap::new();
    if let Some(value) = mapping_get(mapping, "categories") {
        let Some(categories) = value.as_mapping() else {
            diagnostics.push(ConfigDiagnostic {
                line: line_for(line_by_path, "bank.categories"),
                key_path: "bank.categories".to_owned(),
                message: "bank.categories must be a mapping".to_owned(),
                hint: None,
            });
            return finalize_categories(with_builtins(Vec::new()), &[]);
        };
        for (key, category_value) in categories {
            let Some(name) = key.as_str() else {
                diagnostics.push(ConfigDiagnostic {
                    line: line_for(line_by_path, "bank.categories"),
                    key_path: "bank.categories".to_owned(),
                    message: "category keys must be strings".to_owned(),
                    hint: None,
                });
                continue;
            };
            let path = format!("bank.categories.{name}");
            if !is_alias(name) {
                diagnostics.push(ConfigDiagnostic {
                    line: line_for(line_by_path, &path),
                    key_path: path.clone(),
                    message: format!(
                        "category key {name:?} must match [a-z][a-z0-9_]*"
                    ),
                    hint: None,
                });
            }
            let Some((category, hotkey)) = parse_category_value(
                name,
                category_value,
                &path,
                line_by_path,
                diagnostics,
            ) else {
                continue;
            };
            if let Some(hotkey) = hotkey {
                if let Some(first) = claimed_explicit.get(&hotkey) {
                    diagnostics.push(ConfigDiagnostic {
                        line: line_for(line_by_path, &format!("{path}.hotkey")),
                        key_path: format!("{path}.hotkey"),
                        message: format!(
                            "hotkey {hotkey:?} already claimed by {first}"
                        ),
                        hint: None,
                    });
                } else {
                    claimed_explicit.insert(hotkey, path.clone());
                }
            }
            declared.push(category);
            explicit.push(hotkey);
        }
    }
    let declared_keys: Vec<String> = declared
        .iter()
        .map(|category| category.key.clone())
        .collect();
    for (key, emoji, label) in BUILTIN_CATEGORIES {
        if !declared_keys.iter().any(|name| name == key) {
            declared.push(empty_builtin(key, emoji, label));
            explicit.push(None);
        }
    }
    finalize_categories(declared, &explicit)
}

fn parse_category_value(
    name: &str,
    value: &Value,
    path: &str,
    line_by_path: &HashMap<String, usize>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<(Category, Option<char>)> {
    if let Some(mapping) = value.as_mapping() {
        return parse_category_mapping(
            name,
            mapping,
            path,
            line_by_path,
            diagnostics,
        );
    }
    let Some(label) = value.as_str() else {
        diagnostics.push(ConfigDiagnostic {
            line: line_for(line_by_path, path),
            key_path: path.to_owned(),
            message: "category values must be a label string or a mapping"
                .to_owned(),
            hint: None,
        });
        return None;
    };
    let trimmed = label.trim();
    if trimmed.is_empty() {
        diagnostics.push(ConfigDiagnostic {
            line: line_for(line_by_path, path),
            key_path: path.to_owned(),
            message: "category labels must not be empty".to_owned(),
            hint: None,
        });
        return None;
    }
    Some((category_from_label(name, trimmed), None))
}

fn parse_category_mapping(
    name: &str,
    mapping: &Mapping,
    path: &str,
    line_by_path: &HashMap<String, usize>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<(Category, Option<char>)> {
    for key in mapping_keys(mapping) {
        if !CATEGORY_KEYS.contains(&key.as_str()) {
            diagnostics.push(unknown_key(
                path,
                &key,
                CATEGORY_KEYS,
                line_by_path,
            ));
        }
    }
    let label_path = format!("{path}.label");
    let Some(label) = required_string(mapping, "label") else {
        diagnostics.push(ConfigDiagnostic {
            line: line_for(line_by_path, &label_path),
            key_path: label_path,
            message: "label is required".to_owned(),
            hint: None,
        });
        return None;
    };
    let mut category = category_from_label(name, &label);
    let hotkey = match mapping_get(mapping, "hotkey") {
        Some(value) => parse_hotkey_value(
            value,
            &format!("{path}.hotkey"),
            line_by_path,
            diagnostics,
        ),
        None => None,
    };
    if let Some(value) = mapping_get(mapping, "group") {
        match value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            Some(group) => {
                category.group = Some(trim_yaml_scalar(group).to_owned());
            }
            None => diagnostics.push(ConfigDiagnostic {
                line: line_for(line_by_path, &format!("{path}.group")),
                key_path: format!("{path}.group"),
                message: "group must be non-empty text".to_owned(),
                hint: None,
            }),
        }
    }
    if let Some(value) = mapping_get(mapping, "hidden") {
        match value.as_bool() {
            Some(hidden) => category.hidden = hidden,
            None => diagnostics.push(ConfigDiagnostic {
                line: line_for(line_by_path, &format!("{path}.hidden")),
                key_path: format!("{path}.hidden"),
                message: "hidden must be a boolean".to_owned(),
                hint: None,
            }),
        }
    }
    Some((category, hotkey))
}

fn category_from_label(name: &str, label: &str) -> Category {
    let (emoji, rest) = split_icon(label);
    Category {
        key: name.to_owned(),
        emoji: emoji.unwrap_or("").to_owned(),
        label: if rest.is_empty() {
            label.to_owned()
        } else {
            rest.to_owned()
        },
        builtin: BankConfig::is_builtin_meaning(name),
        hotkey: None,
        group: None,
        hidden: false,
    }
}

fn empty_builtin(key: &str, emoji: &str, label: &str) -> Category {
    Category {
        key: key.to_owned(),
        emoji: emoji.to_owned(),
        label: label.to_owned(),
        builtin: true,
        hotkey: None,
        group: None,
        hidden: false,
    }
}

fn with_builtins(mut declared: Vec<Category>) -> Vec<Category> {
    let keys: Vec<String> = declared
        .iter()
        .map(|category| category.key.clone())
        .collect();
    for (key, emoji, label) in BUILTIN_CATEGORIES {
        if !keys.iter().any(|name| name == key) {
            declared.push(empty_builtin(key, emoji, label));
        }
    }
    declared
}

fn finalize_categories(
    mut categories: Vec<Category>,
    explicit: &[Option<char>],
) -> Vec<Category> {
    for category in &mut categories {
        if category.builtin && category.group.is_none() {
            category.group = Some(BUILTIN_GROUP.to_owned());
        }
    }
    assign_hotkeys(&mut categories, explicit);
    categories
}

fn assign_hotkeys(categories: &mut [Category], explicit: &[Option<char>]) {
    let mut claimed = std::collections::HashSet::new();
    for hotkey in explicit.iter().flatten() {
        claimed.insert(*hotkey);
    }
    for (category, hotkey) in categories.iter_mut().zip(explicit.iter()) {
        if let Some(hotkey) = hotkey {
            category.hotkey = Some(*hotkey);
        }
    }
    for category in categories.iter_mut() {
        if category.hotkey.is_some() || category.hidden {
            continue;
        }
        let Some(default) = builtin_default_hotkey(&category.key) else {
            continue;
        };
        if claimed.insert(default) {
            category.hotkey = Some(default);
        }
    }
    for category in categories.iter_mut() {
        if category.hotkey.is_some() || category.hidden {
            continue;
        }
        if let Some(hotkey) =
            derive_hotkey(&category.key, &category.label, &claimed)
        {
            claimed.insert(hotkey);
            category.hotkey = Some(hotkey);
        }
    }
}

fn builtin_default_hotkey(key: &str) -> Option<char> {
    match key {
        "income" => Some('i'),
        "refund" => Some('r'),
        "transfer" => Some('t'),
        "cash" => Some('a'),
        _ => None,
    }
}

fn derive_hotkey(
    key: &str,
    label: &str,
    claimed: &std::collections::HashSet<char>,
) -> Option<char> {
    let mut candidates = Vec::new();
    candidates.extend(key.chars().filter_map(normalize_hotkey_char));
    candidates.extend(label.chars().filter_map(|ch| {
        ch.is_ascii_alphabetic()
            .then(|| ch.to_ascii_lowercase())
            .filter(|letter| is_hotkey_char(*letter))
    }));
    candidates.extend(['1', '2', '3', '4', '5', '6', '7', '8', '9', '0']);
    candidates
        .into_iter()
        .find(|hotkey| is_hotkey_char(*hotkey) && !claimed.contains(hotkey))
}

fn normalize_hotkey_char(ch: char) -> Option<char> {
    let letter = if ch.is_ascii_uppercase() {
        ch.to_ascii_lowercase()
    } else {
        ch
    };
    is_hotkey_char(letter).then_some(letter)
}

fn is_hotkey_char(ch: char) -> bool {
    ch != RESERVED_HOTKEY && (ch.is_ascii_lowercase() || ch.is_ascii_digit())
}

fn parse_hotkey_value(
    value: &Value,
    path: &str,
    line_by_path: &HashMap<String, usize>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<char> {
    let parsed = if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        let mut chars = trimmed.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => Some(ch),
            _ => None,
        }
    } else if let Some(number) = value.as_u64() {
        (number <= 9).then(|| char::from_digit(number as u32, 10).expect("0-9"))
    } else if let Some(number) = value.as_i64() {
        (0..=9)
            .contains(&number)
            .then(|| char::from_digit(number as u32, 10).expect("0-9"))
    } else {
        None
    };
    let Some(hotkey) = parsed else {
        diagnostics.push(ConfigDiagnostic {
            line: line_for(line_by_path, path),
            key_path: path.to_owned(),
            message: "hotkey must be one character in [a-z0-9]".to_owned(),
            hint: None,
        });
        return None;
    };
    if hotkey == RESERVED_HOTKEY {
        diagnostics.push(ConfigDiagnostic {
            line: line_for(line_by_path, path),
            key_path: path.to_owned(),
            message: "hotkey \"q\" is reserved".to_owned(),
            hint: None,
        });
        return None;
    }
    if hotkey.is_ascii_uppercase() || !is_hotkey_char(hotkey) {
        diagnostics.push(ConfigDiagnostic {
            line: line_for(line_by_path, path),
            key_path: path.to_owned(),
            message: "hotkey must be one character in [a-z0-9]".to_owned(),
            hint: None,
        });
        return None;
    }
    Some(hotkey)
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
    pub(crate) last_import: Option<NaiveDate>,
    pub(crate) balance_cents: Option<i64>,
    pub(crate) balance_asof: Option<NaiveDate>,
    pub(crate) delta_cents: Option<i64>,
}

impl AccountStatus {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "alias": self.alias,
            "label": self.label,
            "kind": self.kind.as_str(),
            "default": self.default,
            "start": self.start.map(|date| date.format("%Y-%m-%d").to_string()),
            "status": self.status,
            "last_import": self.last_import.map(|date| date.format("%Y-%m-%d").to_string()),
            "balance_cents": self.balance_cents,
            "balance_asof": self.balance_asof.map(|date| date.format("%Y-%m-%d").to_string()),
            "delta_cents": self.delta_cents,
            "hint": self.hint,
        })
    }
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
                last_import: None,
                balance_cents: None,
                balance_asof: None,
                delta_cents: None,
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

fn bank_line_map(frontmatter: &Frontmatter<'_>) -> HashMap<String, usize> {
    let mut map = key_line_map(frontmatter);
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (index, line) in frontmatter.lines.iter().enumerate() {
        let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(['#', '-']) {
            continue;
        }
        let Some((raw_key, _)) = trimmed.split_once(':') else {
            continue;
        };
        let key = raw_key.trim().trim_matches(['\'', '"']);
        if key.is_empty() {
            continue;
        }
        while stack
            .last()
            .is_some_and(|(indent, _)| *indent >= indentation)
        {
            stack.pop();
        }
        let mut path = stack
            .iter()
            .map(|(_, key)| key.as_str())
            .collect::<Vec<_>>();
        path.push(key);
        let path_str = path.join(".");
        if line.contains('{') {
            map.entry(path_str).or_insert(index + 2);
            continue;
        }
        if !line.contains('}') {
            stack.push((indentation, key.to_owned()));
        }
    }
    map
}

fn line_for(
    line_by_path: &HashMap<String, usize>,
    path: &str,
) -> Option<usize> {
    let mut current = path;
    loop {
        if let Some(&line) = line_by_path.get(current) {
            return Some(line);
        }
        match current.rsplit_once('.') {
            Some((parent, _)) => current = parent,
            None => return None,
        }
    }
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

pub(crate) fn is_alias(value: &str) -> bool {
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
        line: line_for(line_by_path, &path),
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

    fn accounts() -> &'static str {
        "---\nbank:\n  accounts:\n    checking:\n      kind: checking\n      start: 2026-11-12\n"
    }

    fn parse_ok(categories: &str) -> BankConfig {
        let text = format!("{}  categories:\n{categories}---\n", accounts());
        match parse(&text) {
            ConfigParse::Ok(config) => config,
            other => panic!("expected ok, got {other:?}"),
        }
    }

    fn parse_invalid(categories: &str) -> Vec<ConfigDiagnostic> {
        let text = format!("{}  categories:\n{categories}---\n", accounts());
        match parse(&text) {
            ConfigParse::Invalid { diagnostics, .. } => diagnostics,
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn mapping_and_scalar_forms_assign_hotkeys_and_groups() {
        let config = parse_ok(
            "    coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }\n    kids: { label: 🧸 Kids, hidden: true }\n    transit: 🚆 Transit\n",
        );
        let coffee = config.category("coffee").unwrap();
        assert_eq!(coffee.hotkey, Some('c'));
        assert_eq!(coffee.group.as_deref(), Some("Everyday"));
        assert!(!coffee.hidden);
        let kids = config.category("kids").unwrap();
        assert!(kids.hidden);
        assert_eq!(kids.hotkey, None);
        let transit = config.category("transit").unwrap();
        assert_eq!(transit.hotkey, Some('n'));
        assert_eq!(transit.label, "Transit");
        let income = config.category("income").unwrap();
        assert_eq!(income.hotkey, Some('i'));
        assert_eq!(income.group.as_deref(), Some("Money moves"));
        let transfer = config.category("transfer").unwrap();
        assert_eq!(transfer.hotkey, Some('t'));
        let cash = config.category("cash").unwrap();
        assert_eq!(cash.hotkey, Some('a'));
        assert_eq!(
            config
                .categories
                .iter()
                .map(|category| category.key.as_str())
                .collect::<Vec<_>>(),
            [
                "coffee", "kids", "transit", "income", "refund", "transfer",
                "cash"
            ]
        );
    }

    #[test]
    fn yaml_integer_hotkeys_and_block_form_lines() {
        let text = format!(
            "{}  categories:\n    home:\n      label: Housing\n      hotkey: 1\n      group: Bills\n---\n",
            accounts()
        );
        let ConfigParse::Ok(config) = parse(&text) else {
            panic!("expected ok");
        };
        assert_eq!(config.category("home").unwrap().hotkey, Some('1'));
        assert_eq!(
            config.category("home").unwrap().group.as_deref(),
            Some("Bills")
        );
    }

    #[test]
    fn reserved_and_duplicate_hotkeys_name_lines() {
        let flow = parse_invalid("    coffee: { label: Coffee, hotkey: q }\n");
        let reserved = flow
            .iter()
            .find(|item| item.key_path == "bank.categories.coffee.hotkey")
            .unwrap();
        assert!(reserved.message.contains("reserved"), "{reserved:?}");
        assert_eq!(reserved.line, Some(8));

        let text = format!(
            "{}  categories:\n    coffee:\n      label: Coffee\n      hotkey: c\n    convenience:\n      label: Convenience\n      hotkey: c\n---\n",
            accounts()
        );
        let ConfigParse::Invalid { diagnostics, .. } = parse(&text) else {
            panic!("expected invalid");
        };
        let duplicate = diagnostics
            .iter()
            .find(|item| item.key_path == "bank.categories.convenience.hotkey")
            .unwrap();
        assert!(
            duplicate.message.contains("already claimed"),
            "{duplicate:?}"
        );
        assert_eq!(duplicate.line, Some(13));
    }

    #[test]
    fn mapping_errors_cover_label_hidden_group_and_unknown_keys() {
        let missing = parse_invalid("    coffee: { hotkey: c }\n");
        assert!(missing.iter().any(|item| {
            item.key_path == "bank.categories.coffee.label"
                && item.message.contains("required")
        }));

        let hidden =
            parse_invalid("    coffee: { label: Coffee, hidden: maybe }\n");
        assert!(hidden.iter().any(|item| {
            item.key_path == "bank.categories.coffee.hidden"
                && item.message.contains("boolean")
        }));

        let group = parse_invalid("    coffee: { label: Coffee, group: '' }\n");
        assert!(group.iter().any(|item| {
            item.key_path == "bank.categories.coffee.group"
                && item.message.contains("non-empty")
        }));

        let unknown =
            parse_invalid("    coffee: { label: Coffee, lable: extra }\n");
        let diagnostic = unknown
            .iter()
            .find(|item| item.key_path == "bank.categories.coffee.lable")
            .unwrap();
        assert!(
            diagnostic
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("label")),
            "{diagnostic:?}"
        );
    }

    #[test]
    fn hidden_explicit_hotkey_is_kept_and_blocks_others() {
        let config = parse_ok(
            "    old: { label: Old, hotkey: o, hidden: true }\n    other: 📦 Other\n",
        );
        let old = config.category("old").unwrap();
        assert!(old.hidden);
        assert_eq!(old.hotkey, Some('o'));
        assert_eq!(config.category("other").unwrap().hotkey, Some('h'));
    }

    #[test]
    fn builtin_override_and_taken_default_stay_stable() {
        let config = parse_ok(
            "    items: { label: Items, hotkey: i }\n    income: { label: 💵 Pay, group: In }\n",
        );
        let items = config.category("items").unwrap();
        assert_eq!(items.hotkey, Some('i'));
        let income = config.category("income").unwrap();
        assert_eq!(income.label, "Pay");
        assert_eq!(income.group.as_deref(), Some("In"));
        assert_eq!(income.hotkey, Some('n'));
        let refund = config.category("refund").unwrap();
        assert_eq!(refund.hotkey, Some('r'));
    }
}

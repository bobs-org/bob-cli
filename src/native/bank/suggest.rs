//! Read-time and import-time category suggestions.
//!
//! One function serves `list`/`report`/`import` row JSON and import's
//! category tier. Tiers, highest first: rule, learned, bank built-in.

use std::collections::HashMap;

use chrono::NaiveDate;

use super::{
    config::BankConfig,
    identity::merchant_key,
    journal::{JournalSnapshot, TxnEvent},
    model::BankRow,
    naming::classify_builtin,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuggestionSource {
    Rule,
    Learned,
    Bank,
}

impl SuggestionSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Learned => "learned",
            Self::Bank => "bank",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CategorySuggestion {
    pub(crate) category: String,
    pub(crate) source: SuggestionSource,
}

impl CategorySuggestion {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "category": self.category,
            "source": self.source.as_str(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SkipRow<'a> {
    pub(crate) path: &'a str,
    pub(crate) line: usize,
    pub(crate) id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LearnedEntry {
    date: NaiveDate,
    path: String,
    line: usize,
    category: String,
    id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LearnedIndex {
    by_identity: HashMap<String, Vec<LearnedEntry>>,
}

#[derive(Debug, Clone)]
pub(crate) struct SuggestQuery<'a> {
    pub(crate) desc: Option<&'a str>,
    pub(crate) payee: Option<&'a str>,
    pub(crate) trntype: Option<&'a str>,
    pub(crate) identities: &'a [String],
    pub(crate) skip: Option<SkipRow<'a>>,
    pub(crate) read_time: bool,
}

pub(crate) fn suggest_category(
    config: &BankConfig,
    query: &SuggestQuery<'_>,
    index: &LearnedIndex,
) -> Option<CategorySuggestion> {
    if let Some(category) = rule_category(config, query) {
        return Some(CategorySuggestion {
            category,
            source: SuggestionSource::Rule,
        });
    }
    if let Some(category) = index.lookup(query.identities, query.skip) {
        return Some(CategorySuggestion {
            category,
            source: SuggestionSource::Learned,
        });
    }
    let desc = query.desc?;
    let trntype = if query.read_time { None } else { query.trntype };
    classify_builtin(desc, trntype)
        .and_then(|item| item.category.map(str::to_owned))
        .filter(|category| is_offerable(config, category))
        .map(|category| CategorySuggestion {
            category,
            source: SuggestionSource::Bank,
        })
}

pub(crate) fn learned_index(
    rows: &[BankRow],
    journal: &JournalSnapshot,
    config: &BankConfig,
) -> LearnedIndex {
    let mut index = LearnedIndex::default();
    for row in rows {
        if !row.acknowledged {
            continue;
        }
        let Some(category) = row.category.as_deref() else {
            continue;
        };
        if !is_offerable(config, category) {
            continue;
        }
        let unique_id = unique_row_id(rows, row.id.as_deref());
        for identity in row_identities(row, journal, unique_id) {
            if identity.is_empty() {
                continue;
            }
            index
                .by_identity
                .entry(identity)
                .or_default()
                .push(LearnedEntry {
                    date: row.date,
                    path: row.path.clone(),
                    line: row.line,
                    category: category.to_owned(),
                    id: row.id.clone(),
                });
        }
    }
    for entries in index.by_identity.values_mut() {
        entries.sort_by(|left, right| {
            (right.date, right.path.as_str(), right.line).cmp(&(
                left.date,
                left.path.as_str(),
                left.line,
            ))
        });
    }
    index
}

pub(crate) fn attach_suggestions(
    rows: &mut [BankRow],
    config: &BankConfig,
    journal: &JournalSnapshot,
) {
    let index = learned_index(rows, journal, config);
    let computed: Vec<Option<CategorySuggestion>> = rows
        .iter()
        .map(|row| {
            if row.suggestion.is_some() {
                return row.suggestion.clone();
            }
            let unique_id = unique_row_id(rows, row.id.as_deref());
            let identities = row_identities(row, journal, unique_id);
            let desc = unique_id.and_then(|id| {
                journal_event(journal, id).map(|event| event.desc.as_str())
            });
            suggest_category(
                config,
                &SuggestQuery {
                    desc,
                    payee: Some(row.payee.as_str()),
                    trntype: None,
                    identities: &identities,
                    skip: Some(SkipRow {
                        path: row.path.as_str(),
                        line: row.line,
                        id: row.id.as_deref(),
                    }),
                    read_time: true,
                },
                &index,
            )
        })
        .collect();
    for (row, suggestion) in rows.iter_mut().zip(computed) {
        if row.suggestion.is_none() {
            row.suggestion = suggestion;
        }
    }
}

pub(crate) fn row_identities(
    row: &BankRow,
    journal: &JournalSnapshot,
    unique_id: Option<&str>,
) -> Vec<String> {
    identities(
        unique_id.and_then(|id| {
            journal_event(journal, id).map(|event| event.desc.as_str())
        }),
        Some(row.payee.as_str()),
    )
}

pub(crate) fn identities(
    journal_desc: Option<&str>,
    payee: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(desc) = journal_desc {
        push_identity(&mut out, merchant_key(desc));
    }
    if let Some(payee) = payee {
        push_identity(&mut out, merchant_key(payee));
    }
    out
}

fn push_identity(out: &mut Vec<String>, identity: String) {
    if identity.is_empty() || out.iter().any(|existing| existing == &identity) {
        return;
    }
    out.push(identity);
}

#[cfg(test)]
impl LearnedIndex {
    pub(crate) fn with_teacher(identity: &str, category: &str) -> Self {
        let mut index = Self::default();
        index.by_identity.insert(
            identity.to_owned(),
            vec![LearnedEntry {
                date: NaiveDate::from_ymd_opt(2026, 11, 12).expect("date"),
                path: "bank/2026-11.md".to_owned(),
                line: 1,
                category: category.to_owned(),
                id: Some("aaaaaaaa".to_owned()),
            }],
        );
        index
    }
}

impl LearnedIndex {
    fn lookup(
        &self,
        identities: &[String],
        skip: Option<SkipRow<'_>>,
    ) -> Option<String> {
        let mut best: Option<&LearnedEntry> = None;
        for identity in identities {
            let Some(entries) = self.by_identity.get(identity) else {
                continue;
            };
            for entry in entries {
                if skip.is_some_and(|skip| skip.matches(entry)) {
                    continue;
                }
                best = Some(match best {
                    None => entry,
                    Some(current)
                        if (entry.date, entry.path.as_str(), entry.line)
                            > (
                                current.date,
                                current.path.as_str(),
                                current.line,
                            ) =>
                    {
                        entry
                    }
                    Some(current) => current,
                });
                break;
            }
        }
        best.map(|entry| entry.category.clone())
    }
}

impl SkipRow<'_> {
    fn matches(self, entry: &LearnedEntry) -> bool {
        (self.path == entry.path && self.line == entry.line)
            || (self.id.is_some()
                && entry.id.as_deref() == self.id
                && self.id.is_some())
    }
}

fn rule_category(
    config: &BankConfig,
    query: &SuggestQuery<'_>,
) -> Option<String> {
    let haystack = match query.desc {
        Some(desc) if !desc.is_empty() => desc,
        _ => query.payee?,
    };
    for rule in &config.rules {
        let Some(cat) = rule.cat.as_deref() else {
            continue;
        };
        if query.read_time && has_long_digit_run(&rule.match_text) {
            continue;
        }
        if !contains_folded(haystack, &rule.match_text) {
            continue;
        }
        if is_offerable(config, cat) {
            return Some(cat.to_owned());
        }
    }
    None
}

fn is_offerable(config: &BankConfig, key: &str) -> bool {
    config
        .category(key)
        .is_some_and(|category| !category.hidden)
}

fn has_long_digit_run(text: &str) -> bool {
    let mut run = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
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

fn contains_folded(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn journal_event<'a>(
    journal: &'a JournalSnapshot,
    id: &str,
) -> Option<&'a TxnEvent> {
    let key = journal.ids.get(id)?;
    journal.transactions.get(key)
}

fn unique_row_id<'a>(
    rows: &'a [BankRow],
    id: Option<&'a str>,
) -> Option<&'a str> {
    let id = id?;
    let matches = rows
        .iter()
        .filter(|row| row.id.as_deref() == Some(id))
        .count();
    (matches == 1).then_some(id)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate};

    use super::*;
    use crate::native::bank::{
        config::{parse_bank_config, ConfigParse},
        journal::TxnOutcome,
    };

    fn config(extra: &str) -> BankConfig {
        let text = format!(
            "---\nbank:\n  accounts:\n    checking:\n      kind: checking\n      start: 2026-11-01\n{extra}---\n"
        );
        match parse_bank_config(&text) {
            ConfigParse::Ok(config) => config,
            other => panic!("expected ok, got {other:?}"),
        }
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 12).expect("date")
    }

    fn row(
        line: usize,
        payee: &str,
        category: Option<&str>,
        acknowledged: bool,
        id: Option<&str>,
    ) -> BankRow {
        BankRow {
            path: "bank/2026-11.md".to_owned(),
            line,
            date: day(),
            heading: String::new(),
            account: "checking".to_owned(),
            cents: -400,
            payee: payee.to_owned(),
            category: category.map(str::to_owned),
            links: Vec::new(),
            note: None,
            flagged: false,
            acknowledged,
            id: id.map(str::to_owned),
            origin: "typed",
            suggestion: None,
        }
    }

    fn journal_with(id: &str, desc: &str) -> JournalSnapshot {
        let mut journal = JournalSnapshot::default();
        let key = format!("{id}-key");
        journal.ids.insert(id.to_owned(), key.clone());
        journal.transactions.insert(
            key.clone(),
            TxnEvent {
                ts: DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z")
                    .expect("ts"),
                host: "test".to_owned(),
                path: ".bob/bank/test.jsonl".to_owned(),
                line: 1,
                account: "checking".to_owned(),
                key,
                id: id.to_owned(),
                posted: day(),
                cents: -400,
                desc: desc.to_owned(),
                merchant: merchant_key(desc),
                outcome: TxnOutcome::Written,
                note: Some("bank/2026-11.md".to_owned()),
            },
        );
        journal
    }

    fn suggested(
        config: &BankConfig,
        desc: Option<&str>,
        payee: Option<&str>,
        trntype: Option<&str>,
        index: &LearnedIndex,
        read_time: bool,
    ) -> Option<CategorySuggestion> {
        let identities = identities(desc, payee);
        suggest_category(
            config,
            &SuggestQuery {
                desc,
                payee,
                trntype,
                identities: &identities,
                skip: None,
                read_time,
            },
            index,
        )
    }

    #[test]
    fn rule_beats_learned_beats_bank_and_hidden_is_dropped() {
        let config = config(
            "  categories:\n    coffee: ☕ Coffee\n    dining: 🍽️ Dining\n    kids: { label: 🧸 Kids, hidden: true }\n  rules:\n    - match: COFFEE\n      cat: coffee\n    - match: TOY\n      cat: kids\n",
        );
        let teacher =
            row(1, "Cafe Nero", Some("dining"), true, Some("aaaaaaaa"));
        let index =
            learned_index(&[teacher], &JournalSnapshot::default(), &config);
        let rule = suggested(
            &config,
            Some("COFFEE SHOP"),
            Some("Cafe Nero"),
            None,
            &index,
            true,
        );
        assert_eq!(
            rule,
            Some(CategorySuggestion {
                category: "coffee".to_owned(),
                source: SuggestionSource::Rule,
            })
        );
        let learned =
            suggested(&config, None, Some("Cafe Nero"), None, &index, true);
        assert_eq!(
            learned,
            Some(CategorySuggestion {
                category: "dining".to_owned(),
                source: SuggestionSource::Learned,
            })
        );
        let bank = suggested(
            &config,
            Some("ATM WITHDRAWAL"),
            Some("ATM withdrawal"),
            None,
            &LearnedIndex::default(),
            true,
        );
        assert_eq!(
            bank,
            Some(CategorySuggestion {
                category: "cash".to_owned(),
                source: SuggestionSource::Bank,
            })
        );
        let hidden = suggested(
            &config,
            Some("TOY STORE"),
            Some("Toy Store"),
            None,
            &LearnedIndex::default(),
            true,
        );
        assert_eq!(hidden, None);
    }

    #[test]
    fn four_digit_rules_match_only_at_import_time() {
        let config = config(
            "  categories:\n    dining: 🍽️ Dining\n  rules:\n    - match: \"9999\"\n      cat: dining\n",
        );
        let import = suggested(
            &config,
            Some("STORE 9999"),
            None,
            None,
            &LearnedIndex::default(),
            false,
        );
        assert_eq!(
            import.as_ref().map(|item| item.source),
            Some(SuggestionSource::Rule)
        );
        let read = suggested(
            &config,
            Some("STORE 9999"),
            None,
            None,
            &LearnedIndex::default(),
            true,
        );
        assert_eq!(read, None);
    }

    #[test]
    fn waiting_rows_never_teach_and_skip_self_uses_the_prior_teacher() {
        let config = config("  categories:\n    dining: 🍽️ Dining\n    groceries: 🛒 Groceries\n");
        let waiting =
            row(1, "Cafe Nero", Some("dining"), false, Some("aaaaaaaa"));
        let student = row(2, "Cafe Nero", None, false, None);
        let mut rows = vec![waiting, student];
        attach_suggestions(&mut rows, &config, &JournalSnapshot::default());
        assert_eq!(rows[1].suggestion, None);

        let older = row(1, "Cafe Nero", Some("dining"), true, Some("aaaaaaaa"));
        let looking =
            row(2, "Cafe Nero", Some("groceries"), true, Some("bbbbbbbb"));
        let mut rows = vec![older, looking];
        attach_suggestions(&mut rows, &config, &JournalSnapshot::default());
        assert_eq!(
            rows[1].suggestion,
            Some(CategorySuggestion {
                category: "dining".to_owned(),
                source: SuggestionSource::Learned,
            })
        );
    }

    #[test]
    fn typed_row_learns_from_a_renamed_imported_payee() {
        let config = config("  categories:\n    dining: 🍽️ Dining\n");
        let teacher =
            row(1, "Cafe Nero", Some("dining"), true, Some("aaaaaaaa"));
        let student = row(2, "Cafe Nero", None, false, None);
        let journal = journal_with("aaaaaaaa", "TOTALLY DIFFERENT MERCHANT");
        let mut rows = vec![teacher, student];
        attach_suggestions(&mut rows, &config, &journal);
        assert_eq!(
            rows[1].suggestion,
            Some(CategorySuggestion {
                category: "dining".to_owned(),
                source: SuggestionSource::Learned,
            })
        );
    }
}

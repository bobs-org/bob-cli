//! Relation label vocabulary: key, fast catalog scan, preferences
//! fold, and ranking (`docs/task-relations.md` §16).
//!
//! One row per label key counts every accepted LINKS entry with that
//! label: open, closed, `done/`, daily notes, nested projects, and
//! references. Hiding removes a label from suggestions only; LINKS
//! text, mirrors, chips, counts, and typed use are unchanged.

use std::{collections::BTreeMap, io, path::Path};

use serde::Serialize;

use crate::native::task_relations::label::{
    collapse_label_text, inverse_phrase, relation_label_hide_form,
    relation_label_quoted_form,
};

mod journal;
mod scan;

pub(crate) use journal::Prefs;
pub(crate) use scan::{invalidate as invalidate_scan, load_scan};

pub(crate) use cli::run_list;
pub(crate) use rename::run_rename;
pub(crate) use write::{run_hide, run_unhide};
pub(crate) mod cli;
pub(crate) mod rename;
pub(crate) mod reverse;
#[cfg(test)]
mod tests;
pub(crate) mod write;

pub(crate) const MAX_LABEL_WARNINGS: usize = 8;

/// The one key function: writer normalization (trim, collapse
/// whitespace runs, strip one trailing `:`) then Unicode
/// `to_lowercase`. It feeds the catalog, the typed-row match, the
/// journal, rename, and, through vectors, the JS mirror. No NFC or
/// other new equivalence.
pub(crate) fn relation_label_key(label: &str) -> String {
    collapse_label_text(label).to_lowercase()
}

/// One spelling of a key with its use count, so case drift is visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub(crate) struct LabelSpelling {
    pub(crate) name: String,
    pub(crate) count: u64,
}

/// One catalog row: every accepted LINKS entry with one key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub(crate) struct LabelRecord {
    pub(crate) key: String,
    /// Most-used spelling; ties go to the first seen.
    pub(crate) name: String,
    pub(crate) count: u64,
    pub(crate) spellings: Vec<LabelSpelling>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) inverse: Option<String>,
}

impl LabelRecord {
    pub(crate) fn replacement(&self) -> String {
        relation_label_quoted_form(&self.name)
    }

    pub(crate) fn hide_replacement(&self) -> String {
        relation_label_hide_form(&self.name)
    }
}

/// The relation label catalog for one vault: the fast LINKS scan plus
/// the shared preference-journal fold.
#[derive(Debug, Clone)]
pub(crate) struct Catalog {
    records: BTreeMap<String, LabelRecord>,
    prefs: Prefs,
    warnings: Vec<String>,
}

impl Catalog {
    pub(crate) fn load(vault: &Path) -> io::Result<Self> {
        let (records, mut warnings) = load_scan(vault)?;
        let (prefs, mut journal_warnings) = journal::load_prefs(vault)?;
        warnings.append(&mut journal_warnings);
        let mut catalog = Self {
            records,
            prefs,
            warnings,
        };
        catalog.add_journal_only_keys();
        Ok(catalog)
    }

    /// A hidden key with zero uses stays listable (count 0) from the
    /// journal's stored name, so it can always be found and unhidden.
    fn add_journal_only_keys(&mut self) {
        for (key, name) in self.prefs.names() {
            if self.records.contains_key(key) {
                continue;
            }
            self.records.insert(
                key.clone(),
                LabelRecord {
                    key: key.clone(),
                    name: name.clone(),
                    count: 0,
                    spellings: Vec::new(),
                    inverse: inverse_phrase(name),
                },
            );
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&LabelRecord> {
        self.records.get(key)
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = &LabelRecord> {
        self.records.values()
    }

    pub(crate) fn known_count(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn links_total(&self) -> u64 {
        self.records.values().map(|record| record.count).sum()
    }

    pub(crate) fn hidden_count(&self) -> usize {
        self.records
            .keys()
            .filter(|key| self.prefs.hidden(key))
            .count()
    }

    pub(crate) fn is_hidden(&self, key: &str) -> bool {
        self.prefs.hidden(key)
    }

    pub(crate) fn has_explicit_state(&self, key: &str) -> bool {
        self.prefs.has_explicit_state(key)
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Rows for a query with a hidden filter, in bob's documented
    /// rank: prefix matches, then substring matches on the
    /// lowercased, whitespace-collapsed label, each in catalog order
    /// (count desc, lowercase name, name).
    pub(crate) fn rank_for_query(
        &self,
        query: &str,
        hidden: HiddenFilter,
    ) -> Vec<&LabelRecord> {
        let query = collapse_label_text(query).to_lowercase();
        let mut ordered: Vec<&LabelRecord> = self
            .records()
            .filter(|record| match hidden {
                HiddenFilter::Exclude => !self.is_hidden(&record.key),
                HiddenFilter::Only => self.is_hidden(&record.key),
                HiddenFilter::Include => true,
            })
            .collect();
        ordered.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.key.cmp(&right.key))
                .then_with(|| left.name.cmp(&right.name))
        });
        if query.is_empty() {
            return ordered;
        }
        let (prefix, rest): (Vec<_>, Vec<_>) = ordered
            .into_iter()
            .filter(|record| {
                collapse_label_text(&record.name)
                    .to_lowercase()
                    .contains(&query)
            })
            .partition(|record| {
                collapse_label_text(&record.name)
                    .to_lowercase()
                    .starts_with(&query)
            });
        prefix.into_iter().chain(rest).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HiddenFilter {
    Exclude,
    Only,
    Include,
}

/// Count one accepted label spelling toward its key row. The display
/// spelling is the most-used; ties go to the first seen. `seen` holds
/// one `key\0spelling` marker per spelling in encounter order.
pub(crate) fn count_label(
    rows: &mut BTreeMap<String, LabelRecord>,
    seen: &mut Vec<String>,
    label: &str,
) {
    let key = relation_label_key(label);
    if key.is_empty() {
        return;
    }
    let marker = format!("{key}\0{label}");
    if !seen.contains(&marker) {
        seen.push(marker);
    }
    if let Some(record) = rows.get_mut(&key) {
        record.count = record.count.saturating_add(1);
        match record
            .spellings
            .iter_mut()
            .find(|spelling| spelling.name == label)
        {
            Some(spelling) => {
                spelling.count = spelling.count.saturating_add(1);
            }
            None => {
                record.spellings.push(LabelSpelling {
                    name: label.to_string(),
                    count: 1,
                });
            }
        }
        let best = record
            .spellings
            .iter()
            .max_by(|left, right| {
                left.count.cmp(&right.count).then_with(|| {
                    seen_position(seen, &key, &right.name)
                        .cmp(&seen_position(seen, &key, &left.name))
                })
            })
            .map(|spelling| spelling.name.clone());
        if let Some(best) = best {
            record.name = best;
        }
        return;
    }
    rows.insert(
        key.clone(),
        LabelRecord {
            key,
            name: label.to_string(),
            count: 1,
            spellings: vec![LabelSpelling {
                name: label.to_string(),
                count: 1,
            }],
            inverse: inverse_phrase(label),
        },
    );
}

fn seen_position(seen: &[String], key: &str, name: &str) -> usize {
    let marker = format!("{key}\0{name}");
    seen.iter()
        .position(|entry| entry == &marker)
        .unwrap_or(usize::MAX)
}

/// Catalog rows in list order (count desc, lowercase name, name),
/// before any query rank.
pub(crate) fn sort_spellings(record: &mut LabelRecord) {
    record.spellings.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
}

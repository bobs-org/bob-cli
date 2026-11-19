//! Vault-derived history for the four task log kinds.
//!
//! `bob logs list` is a read-only report. The scan keeps only each managed
//! marker's direct entry summaries; no phrase store is written to the vault.

use std::{collections::BTreeMap, io, path::Path, time::Duration};

use serde::{Deserialize, Serialize};

mod classify;
mod cli;
mod parse;
mod rank;
mod scan;

#[cfg(test)]
mod tests;

pub(crate) use cli::{build_cli, run};

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const HALF_LIFE_DAYS: f64 = 7.0;

/// Canonical public order used by the CLI, JSON output, and clients.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogKind {
    Schedule,
    Work,
    Rotten,
    Cancel,
}

impl LogKind {
    pub(crate) const ALL: [Self; 4] =
        [Self::Schedule, Self::Work, Self::Rotten, Self::Cancel];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Work => "work",
            Self::Rotten => "rotten",
            Self::Cancel => "cancel",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Schedule => "Schedule Log",
            Self::Work => "Work Log",
            Self::Rotten => "Rotten Log",
            Self::Cancel => "Cancel Log",
        }
    }

    pub(crate) fn emoji(self) -> &'static str {
        match self {
            Self::Schedule => "🗓️",
            Self::Work => "🛠️",
            Self::Rotten => "🥀",
            Self::Cancel => "❌",
        }
    }

    pub(crate) fn recency_precision(self) -> &'static str {
        match self {
            Self::Work => "minute",
            Self::Schedule => "day-approx",
            Self::Rotten | Self::Cancel => "day",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Phrase {
    pub(crate) text: String,
    pub(crate) key: String,
    pub(crate) uses: u64,
    pub(crate) last_used: String,
    pub(crate) score: f64,
    pub(crate) recency_rank: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Excluded {
    pub(crate) machine: usize,
    pub(crate) blank: usize,
    pub(crate) too_long: usize,
    pub(crate) copies: usize,
    pub(crate) unparsed: usize,
}

impl Excluded {
    fn add_assign(&mut self, other: &Self) {
        self.machine = self.machine.saturating_add(other.machine);
        self.blank = self.blank.saturating_add(other.blank);
        self.too_long = self.too_long.saturating_add(other.too_long);
        self.copies = self.copies.saturating_add(other.copies);
        self.unparsed = self.unparsed.saturating_add(other.unparsed);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LogCatalog {
    pub(crate) phrases: BTreeMap<LogKind, Vec<Phrase>>,
    pub(crate) excluded: BTreeMap<LogKind, Excluded>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
    #[serde(default)]
    pub(crate) scanned_unix: u64,
}

impl LogCatalog {
    pub(crate) fn phrases_for(&self, kind: LogKind) -> &[Phrase] {
        self.phrases
            .get(&kind)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub(crate) fn excluded_for(&self, kind: LogKind) -> Excluded {
        self.excluded.get(&kind).cloned().unwrap_or_default()
    }
}

/// Rank one kind's catalog for other native Bob surfaces.
pub(crate) fn rank_phrases<'a>(
    catalog: &'a LogCatalog,
    kind: LogKind,
    query: Option<&str>,
) -> Vec<RankedPhrase<'a>> {
    rank::rank(catalog.phrases_for(kind), query)
}

/// Load a catalog. A max age is only used by keystroke-heavy callers such as
/// capture-complete; `None` always performs a stat walk before returning.
pub(crate) fn load(
    vault: &Path,
    max_age: Option<Duration>,
) -> io::Result<LogCatalog> {
    scan::load(vault, max_age)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MatchKind {
    Exact,
    Prefix,
    Word,
    Substring,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RankedPhrase<'a> {
    pub(crate) phrase: &'a Phrase,
    pub(crate) matched: Option<MatchKind>,
}

#[derive(Debug, Clone, Serialize)]
struct ListPhrase {
    text: String,
    key: String,
    uses: u64,
    last_used: String,
    score: f64,
    recency_rank: usize,
    #[serde(rename = "match")]
    matched: Option<MatchKind>,
}

#[derive(Debug, Clone, Serialize)]
struct ListLog {
    kind: LogKind,
    label: &'static str,
    emoji: &'static str,
    recency_precision: &'static str,
    phrases_total: usize,
    truncated: bool,
    excluded: Excluded,
    phrases: Vec<ListPhrase>,
}

#[derive(Debug, Clone, Serialize)]
struct ListReport {
    schema_version: u32,
    ok: bool,
    query: Option<String>,
    half_life_days: f64,
    logs: Vec<ListLog>,
    warnings: Vec<String>,
}

impl ListLog {
    fn new(
        kind: LogKind,
        ranked: Vec<RankedPhrase<'_>>,
        total: usize,
        catalog: &LogCatalog,
        limit: usize,
    ) -> Self {
        let phrases_total = total;
        let truncated = ranked.len() > limit;
        let phrases = ranked
            .into_iter()
            .take(limit)
            .map(|row| ListPhrase {
                text: row.phrase.text.clone(),
                key: row.phrase.key.clone(),
                uses: row.phrase.uses,
                last_used: row.phrase.last_used.clone(),
                score: row.phrase.score,
                recency_rank: row.phrase.recency_rank,
                matched: row.matched,
            })
            .collect();
        Self {
            kind,
            label: kind.label(),
            emoji: kind.emoji(),
            recency_precision: kind.recency_precision(),
            phrases_total,
            truncated,
            excluded: catalog.excluded_for(kind),
            phrases,
        }
    }
}

fn build_report(
    catalog: &LogCatalog,
    kinds: &[LogKind],
    query: Option<&str>,
    limit: usize,
) -> ListReport {
    let logs = kinds
        .iter()
        .copied()
        .map(|kind| {
            let ranked = rank::rank(catalog.phrases_for(kind), query);
            ListLog::new(
                kind,
                ranked,
                catalog.phrases_for(kind).len(),
                catalog,
                limit,
            )
        })
        .collect();
    ListReport {
        schema_version: SCHEMA_VERSION,
        ok: true,
        query: query.map(str::to_string),
        half_life_days: HALF_LIFE_DAYS,
        logs,
        warnings: catalog.warnings.clone(),
    }
}

fn aggregate_excluded(catalog: &LogCatalog, kinds: &[LogKind]) -> Excluded {
    let mut total = Excluded::default();
    for kind in kinds {
        let item = catalog.excluded.get(kind).cloned().unwrap_or_default();
        total.add_assign(&item);
    }
    total
}

fn parse_kind(value: &str) -> Option<LogKind> {
    LogKind::from_str(value)
}

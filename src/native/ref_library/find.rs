//! Batch identity lookup for `bob ref find`.
//!
//! One query resolves through the shared [`resolve_query`] index lookup:
//! exact matches (identity, path, source PDF, id, stem) give the
//! `in_library` verdict, queued intake PDFs give `in_intake` (with `-i`),
//! title and slug candidates at or above `--min-score` give `possible`,
//! and anything else is `not_found`. Queries run in order and duplicates
//! are kept.
use serde::Serialize;

use super::{
    classify_query, doi_keys, normalize_doi_query, parse_arxiv_query,
    path_matches, resolve_query, stored_identity, url_query_keys, MatchKind,
    QueryKind, RefIndex, RefRow,
};
use crate::native::highlights_ref::IntakeRecord;

/// The verdict for one query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Verdict {
    InLibrary,
    InIntake,
    Possible,
    NotFound,
}

/// One exact match for a query.
#[derive(Debug, Clone, Serialize)]
pub(super) struct FindMatch {
    pub match_kind: MatchKind,
    pub matched_key: Option<String>,
    #[serde(rename = "ref")]
    pub row: RefRow,
}

/// One title or slug candidate for a query.
#[derive(Debug, Clone, Serialize)]
pub(super) struct FindCandidate {
    pub match_kind: MatchKind,
    pub score: u32,
    #[serde(rename = "ref")]
    pub row: RefRow,
}

/// One queued intake PDF that recorded a query's identity key.
#[derive(Debug, Clone, Serialize)]
pub(super) struct FindIntake {
    pub path: String,
    pub source_url: String,
    pub title: Option<String>,
    pub status: Option<String>,
}

/// Everything `bob ref find` reports for one query.
#[derive(Debug, Clone, Serialize)]
pub(super) struct FindResult {
    pub query: String,
    pub query_kind: &'static str,
    pub keys: Vec<String>,
    pub verdict: Verdict,
    pub reading_state: Option<String>,
    pub matches: Vec<FindMatch>,
    pub candidates: Vec<FindCandidate>,
    pub intake: Vec<FindIntake>,
}

/// Counts over one batch of queries.
#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct FindSummary {
    pub queries: usize,
    pub in_library: usize,
    pub finished: usize,
    pub in_intake: usize,
    pub possible: usize,
    pub not_found: usize,
}

/// Resolve every query against the index, in order.
pub(super) fn resolve_find(
    index: &RefIndex,
    queries: &[String],
    min_score: u64,
    intake: Option<&[IntakeRecord]>,
) -> (Vec<FindResult>, FindSummary) {
    let mut results = Vec::with_capacity(queries.len());
    let mut summary = FindSummary {
        queries: queries.len(),
        ..FindSummary::default()
    };
    for query in queries {
        let result = resolve_one(index, query, min_score, intake);
        match result.verdict {
            Verdict::InLibrary => {
                summary.in_library += 1;
                if result.reading_state.as_deref() == Some("finished") {
                    summary.finished += 1;
                }
            }
            Verdict::InIntake => summary.in_intake += 1,
            Verdict::Possible => summary.possible += 1,
            Verdict::NotFound => summary.not_found += 1,
        }
        results.push(result);
    }
    (results, summary)
}

fn resolve_one(
    index: &RefIndex,
    query: &str,
    min_score: u64,
    intake: Option<&[IntakeRecord]>,
) -> FindResult {
    let kind = classify_query(query);
    let keys = query_keys(query, kind);
    let hits = resolve_query(&index.rows, &index.bob_dir, query);
    let mut exact = Vec::new();
    let mut candidates = Vec::new();
    for hit in &hits {
        let row = &index.rows[hit.row];
        match hit.kind {
            MatchKind::Identity
            | MatchKind::Path
            | MatchKind::SourcePdf
            | MatchKind::Id
            | MatchKind::Stem => exact.push(FindMatch {
                match_kind: hit.kind,
                matched_key: matched_key(
                    &index.bob_dir,
                    row,
                    hit.kind,
                    query,
                    &keys,
                ),
                row: row.clone(),
            }),
            MatchKind::TitleExact | MatchKind::Title | MatchKind::SlugTitle => {
                if hit.score.is_some_and(|score| u64::from(score) >= min_score)
                {
                    candidates.push(FindCandidate {
                        match_kind: hit.kind,
                        score: hit.score.unwrap_or(0),
                        row: row.clone(),
                    });
                }
            }
        }
    }
    let intake_hits = intake
        .map(|records| match_intake(records, &index.bob_dir, &keys))
        .unwrap_or_default();
    let (verdict, reading_state) = if exact.is_empty() {
        if intake_hits.is_empty() {
            if candidates.is_empty() {
                (Verdict::NotFound, None)
            } else {
                (Verdict::Possible, None)
            }
        } else {
            (Verdict::InIntake, None)
        }
    } else {
        (Verdict::InLibrary, Some(exact[0].row.reading_state.clone()))
    };
    FindResult {
        query: query.to_string(),
        query_kind: query_kind_name(kind),
        keys,
        verdict,
        reading_state,
        matches: exact,
        candidates,
        intake: intake_hits,
    }
}

/// The identity keys one query looks up. Path, name, and title queries
/// carry no keys, so they never match intake records either.
fn query_keys(query: &str, kind: QueryKind) -> Vec<String> {
    match kind {
        QueryKind::Url => url_query_keys(query),
        QueryKind::Arxiv => parse_arxiv_query(query)
            .map(|id| vec![format!("arxiv:{id}")])
            .unwrap_or_default(),
        QueryKind::Doi => normalize_doi_query(query)
            .map(|doi| doi_keys(&doi))
            .unwrap_or_default(),
        QueryKind::Path | QueryKind::Name | QueryKind::Title => Vec::new(),
    }
}

fn query_kind_name(kind: QueryKind) -> &'static str {
    match kind {
        QueryKind::Url => "url",
        QueryKind::Arxiv => "arxiv",
        QueryKind::Doi => "doi",
        QueryKind::Path => "path",
        QueryKind::Name => "name",
        QueryKind::Title => "title",
    }
}

/// The stored value one exact hit matched: the query key for identity
/// hits, the note path or source PDF for path hits, the stem for stem
/// hits, and the frontmatter id for id hits.
fn matched_key(
    bob_dir: &std::path::Path,
    row: &RefRow,
    kind: MatchKind,
    query: &str,
    keys: &[String],
) -> Option<String> {
    match kind {
        MatchKind::Identity => keys
            .iter()
            .find(|key| row.identity.keys.contains(key))
            .cloned(),
        MatchKind::Path => {
            if path_matches(bob_dir, query, &row.path) {
                Some(row.path.clone())
            } else {
                row.source_pdf.clone()
            }
        }
        MatchKind::SourcePdf => row.source_pdf.clone(),
        MatchKind::Id => row.id.clone(),
        MatchKind::Stem => row.path.rsplit('/').next().map(|stem| {
            stem.strip_suffix(".md")
                .or_else(|| stem.strip_suffix(".MD"))
                .unwrap_or(stem)
                .to_string()
        }),
        MatchKind::TitleExact | MatchKind::Title | MatchKind::SlugTitle => None,
    }
}

/// Queued intake PDFs whose marker identity shares a query key, in path
/// order. Only key-carrying queries (URL, arXiv, DOI) can hit intake.
fn match_intake(
    records: &[IntakeRecord],
    bob_dir: &std::path::Path,
    keys: &[String],
) -> Vec<FindIntake> {
    if keys.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for record in records {
        let stored = stored_identity(std::slice::from_ref(&record.source_url));
        if keys.iter().any(|key| stored.keys.contains(key)) {
            hits.push(FindIntake {
                path: display_path(bob_dir, &record.path),
                source_url: record.source_url.clone(),
                title: record.title.clone(),
                status: record.status.clone(),
            });
        }
    }
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    hits
}

/// Vault-relative intake paths (`xlib/…`) when under the vault.
fn display_path(bob_dir: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(bob_dir)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

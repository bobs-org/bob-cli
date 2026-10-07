//! Exact resolution, title scoring, the slug fallback, and ordering.
//!
//! Title scoring normalizes by casefolding, turning every non-alphanumeric
//! run into a space, and dropping the plan stopwords; the score is
//! `round(100 * 2|Q∩T| / (|Q|+|T|))` over token sets, raised to at least 90
//! on containment when the shorter side has 3 or more tokens.
use std::collections::BTreeSet;

use super::identity::{
    classify_query, doi_keys, normalize_doi_query, parse_arxiv_query,
    url_query_keys, QueryKind,
};
use super::row::RefRow;
use super::status::reading_state_rank;

/// How one row matched a query.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatchKind {
    Identity,
    Path,
    SourcePdf,
    Id,
    Stem,
    TitleExact,
    Title,
    SlugTitle,
}

/// One resolved row: its index into [`RefIndex`](super::RefIndex), how it
/// matched, and the title score for candidate kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScoredHit {
    pub row: usize,
    pub kind: MatchKind,
    pub score: Option<u32>,
}

/// Resolve one query against the index: exact matches first in primary-match
/// order, then up to 5 title/slug candidates by score.
pub(crate) fn resolve_query(
    rows: &[RefRow],
    bob_dir: &std::path::Path,
    query: &str,
) -> Vec<ScoredHit> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    match classify_query(trimmed) {
        QueryKind::Url => resolve_url(rows, trimmed),
        QueryKind::Arxiv => {
            let id = parse_arxiv_query(trimmed).expect("classified arxiv");
            let key = format!("arxiv:{id}");
            exact_hits(rows, MatchKind::Identity, |row| {
                row.identity.keys.iter().any(|k| k == &key)
            })
        }
        QueryKind::Doi => {
            let doi = normalize_doi_query(trimmed).expect("classified doi");
            // Bare and `doi:` queries derive the same keys as stored
            // keys: the `doi:` key plus an `arxiv:` key for arXiv DOIs.
            let keys = doi_keys(&doi);
            exact_hits(rows, MatchKind::Identity, |row| {
                row.identity.keys.iter().any(|key| keys.contains(key))
            })
        }
        QueryKind::Path => resolve_path(rows, bob_dir, trimmed),
        QueryKind::Name => resolve_name(rows, trimmed),
        QueryKind::Title => title_candidates(rows, trimmed, MatchKind::Title),
    }
}

/// Primary-match ordering: non-superseded first, then the most-advanced
/// reading state, then path.
pub(crate) fn primary_rank(row: &RefRow) -> (bool, u8, &str) {
    (
        row.superseded_by.is_some(),
        reading_state_rank(&row.reading_state),
        row.path.as_str(),
    )
}

fn sort_primary(hits: &mut [ScoredHit], rows: &[RefRow]) {
    hits.sort_by(|a, b| {
        primary_rank(&rows[a.row]).cmp(&primary_rank(&rows[b.row]))
    });
}

fn exact_hits(
    rows: &[RefRow],
    kind: MatchKind,
    matches: impl Fn(&RefRow) -> bool,
) -> Vec<ScoredHit> {
    let mut hits = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches(row))
        .map(|(index, _)| ScoredHit {
            row: index,
            kind,
            score: None,
        })
        .collect::<Vec<_>>();
    sort_primary(&mut hits, rows);
    hits
}

fn resolve_url(rows: &[RefRow], query: &str) -> Vec<ScoredHit> {
    let keys = url_query_keys(query);
    let mut hits = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row.identity.keys.iter().any(|key| keys.contains(key))
        })
        .map(|(index, _)| ScoredHit {
            row: index,
            kind: MatchKind::Identity,
            score: None,
        })
        .collect::<Vec<_>>();
    sort_primary(&mut hits, rows);
    if !hits.is_empty() {
        return hits;
    }
    // Slug fallback: the words of the last non-empty path segment, without
    // extension and split on `-`/`_`, when there are at least 2 words.
    match slug_words(query) {
        Some(words) => {
            title_candidates(rows, &words.join(" "), MatchKind::SlugTitle)
        }
        None => Vec::new(),
    }
}

fn resolve_path(
    rows: &[RefRow],
    bob_dir: &std::path::Path,
    query: &str,
) -> Vec<ScoredHit> {
    let mut hits = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if path_matches(bob_dir, query, &row.path) {
            hits.push(ScoredHit {
                row: index,
                kind: MatchKind::Path,
                score: None,
            });
        } else if let Some(source_pdf) = row.source_pdf.as_deref()
            && path_matches(bob_dir, query, source_pdf)
        {
            hits.push(ScoredHit {
                row: index,
                kind: MatchKind::SourcePdf,
                score: None,
            });
        }
    }
    sort_primary(&mut hits, rows);
    hits
}

/// A path query matches the note path (vault-relative, `.md` optional, or
/// absolute inside the vault) and, separately, `source_pdf`.
pub(crate) fn path_matches(
    bob_dir: &std::path::Path,
    query: &str,
    target: &str,
) -> bool {
    let query = query.trim();
    if query == target {
        return true;
    }
    let query_path = std::path::Path::new(query);
    if query_path.is_absolute()
        && let Ok(rel) = query_path.strip_prefix(bob_dir)
        && let Some(rel) = rel.to_str()
    {
        let rel = rel.replace('\\', "/");
        if rel == target || with_md(&rel) == target {
            return true;
        }
    }
    with_md(query) == target || strip_md(query) == strip_md(target)
}

fn with_md(path: &str) -> String {
    if path.to_lowercase().ends_with(".md") {
        path.to_string()
    } else {
        format!("{path}.md")
    }
}

fn strip_md(path: &str) -> &str {
    path.strip_suffix(".md")
        .or_else(|| path.strip_suffix(".MD"))
        .unwrap_or(path)
}

fn resolve_name(rows: &[RefRow], query: &str) -> Vec<ScoredHit> {
    let folded = query.to_lowercase();
    let mut hits = Vec::new();
    let mut matched = vec![false; rows.len()];
    for (index, row) in rows.iter().enumerate() {
        let stem = row.path.rsplit('/').next().unwrap_or(&row.path);
        let stem = strip_md(stem);
        if stem.eq_ignore_ascii_case(&folded) {
            hits.push(ScoredHit {
                row: index,
                kind: MatchKind::Stem,
                score: None,
            });
            matched[index] = true;
        } else if row
            .id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(&folded))
        {
            hits.push(ScoredHit {
                row: index,
                kind: MatchKind::Id,
                score: None,
            });
            matched[index] = true;
        }
    }
    sort_primary(&mut hits, rows);
    // Name queries also title-score; exact hits never repeat as candidates.
    hits.extend(
        title_candidates(rows, query, MatchKind::Title)
            .into_iter()
            .filter(|hit| !matched[hit.row]),
    );
    hits
}

/// Up to 5 title candidates with a positive score, ordered by score.
fn title_candidates(
    rows: &[RefRow],
    query: &str,
    kind: MatchKind,
) -> Vec<ScoredHit> {
    let mut scored = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let score = title_score(query, &row.title);
            let exact =
                normalize_title_text(query) == normalize_title_text(&row.title);
            let kind = if exact && kind == MatchKind::Title {
                MatchKind::TitleExact
            } else {
                kind
            };
            (index, score, exact, kind)
        })
        .filter(|(_, score, _, _)| *score > 0)
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            primary_rank(&rows[a.0]).cmp(&primary_rank(&rows[b.0]))
        })
    });
    scored
        .into_iter()
        .take(5)
        .map(|(index, score, _, kind)| ScoredHit {
            row: index,
            kind,
            score: Some(score),
        })
        .collect()
}

/// Score a query against a title over normalized token sets.
pub(crate) fn title_score(query: &str, title: &str) -> u32 {
    let (query_text, query_tokens) = normalize_title(query);
    let (title_text, title_tokens) = normalize_title(title);
    if query_tokens.is_empty() || title_tokens.is_empty() {
        return 0;
    }
    if query_text == title_text {
        return 100;
    }
    let shared = query_tokens.intersection(&title_tokens).count();
    if shared == 0 {
        return 0;
    }
    let mut score = (100.0 * 2.0 * shared as f64
        / (query_tokens.len() + title_tokens.len()) as f64)
        .round() as u32;
    let (shorter, longer) = if query_tokens.len() <= title_tokens.len() {
        (&query_text, &title_text)
    } else {
        (&title_text, &query_text)
    };
    if shorter.split_whitespace().count() >= 3 && longer.contains(shorter) {
        score = score.max(90);
    }
    score.min(100)
}

/// Normalize to `(joined text, token set)`: casefold, non-alphanumeric runs
/// to one space, stopwords dropped. The joined text keeps first-occurrence
/// order (for the containment rule); the set feeds the Dice score.
fn normalize_title(text: &str) -> (String, BTreeSet<String>) {
    let mut folded = String::with_capacity(text.len());
    for c in text.to_lowercase().chars() {
        if c.is_alphanumeric() {
            folded.push(c);
        } else if !folded.ends_with(' ') {
            folded.push(' ');
        }
    }
    let mut tokens = BTreeSet::new();
    let mut ordered = Vec::new();
    for token in folded.split_whitespace().filter(|t| !is_stopword(t)) {
        if tokens.insert(token.to_string()) {
            ordered.push(token);
        }
    }
    (ordered.join(" "), tokens)
}

fn normalize_title_text(text: &str) -> String {
    normalize_title(text).0
}

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "for"
            | "in"
            | "is"
            | "of"
            | "on"
            | "the"
            | "to"
            | "with"
    )
}

/// The slug words of a URL's last non-empty path segment, without extension
/// and split on `-`/`_`, when there are at least 2 words.
fn slug_words(raw: &str) -> Option<Vec<String>> {
    let url = url::Url::parse(raw.trim()).ok()?;
    let segment = url
        .path_segments()?
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find(|part| !part.is_empty())?;
    let stem = segment
        .rsplit_once('.')
        .map(|(head, _)| head)
        .unwrap_or(segment);
    let words = stem
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    (words.len() >= 2).then_some(words)
}

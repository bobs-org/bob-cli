//! Bars, short-query rules, hold, ordering, and limits.

use chrono::Datelike;

use super::corpus::Candidate;
use super::score::{round_score, round_sort_score};
use super::tokenize::{self, Token};

pub(super) const POSSIBLE_DUPLICATE: f64 = 0.70;
pub(super) const SIMILAR: f64 = 0.48;
pub(super) const CLOSED_DUPLICATE: f64 = 0.70;
pub(super) const HOLD_MARGIN: f64 = 0.10;
pub(super) const SHORT_QUERY_TERMS: usize = 3;
pub(super) const SHORT_QUERY_BOOST: f64 = 0.10;
pub(super) const MIN_TERMS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Band {
    SameTitle,
    PossibleDuplicate,
    Similar,
    ClosedDuplicate,
    Hold,
}

impl Band {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::SameTitle => "same_title",
            Self::PossibleDuplicate => "possible_duplicate",
            Self::Similar => "similar",
            Self::ClosedDuplicate => "closed_duplicate",
            Self::Hold => "hold",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::SameTitle => 0,
            Self::PossibleDuplicate => 1,
            Self::Similar => 2,
            Self::ClosedDuplicate => 3,
            Self::Hold => 4,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RankedMatch<'a> {
    pub(super) candidate: &'a Candidate,
    pub(super) band: Band,
    pub(super) score: f64,
    pub(super) matched_ranges: Vec<(usize, usize)>,
}

pub(super) fn rank<'a>(
    query_title: &str,
    query_tokens: &[Token],
    candidates: &'a [Candidate],
    scores: &[f64],
    preferred_route: Option<&str>,
    limit: usize,
) -> Vec<RankedMatch<'a>> {
    let terms = tokenize::unique_term_count(query_tokens);
    let short = terms < SHORT_QUERY_TERMS;
    let boost = if short { SHORT_QUERY_BOOST } else { 0.0 };
    let possible_bar = POSSIBLE_DUPLICATE + boost;
    let similar_bar = SIMILAR + boost;
    let closed_bar = CLOSED_DUPLICATE + boost;
    let query_normalized = tokenize::normalize_title(query_title);
    let query_numbers = tokenize::numbers(query_tokens);

    let mut classified = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let score = scores.get(index).copied().unwrap_or(0.0);
        let same_title = tokenize::normalize_title(&candidate.title)
            == query_normalized
            && !query_normalized.is_empty();
        // Only a score at a duplicate bar needs the number comparison.
        let numbers_differ = score >= possible_bar.min(closed_bar)
            && tokenize::numbers(&candidate.tokens) != query_numbers;
        let Some(band) = classify(
            candidate,
            Evidence {
                score,
                same_title,
                numbers_differ,
                terms,
            },
            possible_bar,
            similar_bar,
            closed_bar,
        ) else {
            continue;
        };
        classified.push(RankedMatch {
            candidate,
            band,
            score: round_score(score),
            matched_ranges: matched_ranges(query_tokens, &candidate.tokens),
        });
    }

    let has_open_duplicate = classified.iter().any(|ranked| {
        ranked.candidate.open
            && matches!(ranked.band, Band::SameTitle | Band::PossibleDuplicate)
    });
    if has_open_duplicate {
        classified.retain(|ranked| ranked.band != Band::ClosedDuplicate);
    }

    classified.sort_by(|left, right| {
        left.band
            .rank()
            .cmp(&right.band.rank())
            .then_with(|| {
                round_sort_score(right.score).cmp(&round_sort_score(left.score))
            })
            .then_with(|| right.candidate.open.cmp(&left.candidate.open))
            .then_with(|| {
                status_rank(left.candidate.status_symbol)
                    .cmp(&status_rank(right.candidate.status_symbol))
            })
            .then_with(|| {
                route_rank(left.candidate, preferred_route)
                    .cmp(&route_rank(right.candidate, preferred_route))
            })
            .then_with(|| {
                recency(right.candidate).cmp(&recency(left.candidate))
            })
            .then_with(|| {
                left.candidate.note_path.cmp(&right.candidate.note_path)
            })
            .then_with(|| left.candidate.line.cmp(&right.candidate.line))
    });

    let mut kept_closed = false;
    classified.retain(|ranked| {
        if ranked.band != Band::ClosedDuplicate {
            return true;
        }
        if kept_closed {
            return false;
        }
        kept_closed = true;
        true
    });

    let mut shown = Vec::new();
    let mut holds = Vec::new();
    for ranked in classified {
        if ranked.band == Band::Hold {
            if holds.len() < limit {
                holds.push(ranked);
            }
        } else if shown.len() < limit {
            shown.push(ranked);
        }
    }
    shown.extend(holds);
    shown
}

/// What one candidate has in common with the query title.
struct Evidence {
    score: f64,
    same_title: bool,
    /// Titles naming different numbers ("Part 1" / "Part 2", "2025" /
    /// "2026") describe different instances, so they can be similar but
    /// never a possible or closed duplicate.
    numbers_differ: bool,
    terms: usize,
}

fn classify(
    candidate: &Candidate,
    evidence: Evidence,
    possible_bar: f64,
    similar_bar: f64,
    closed_bar: f64,
) -> Option<Band> {
    let Evidence {
        score,
        same_title,
        numbers_differ,
        terms,
    } = evidence;
    if same_title {
        return Some(if candidate.open {
            Band::SameTitle
        } else {
            Band::ClosedDuplicate
        });
    }
    if terms < MIN_TERMS {
        return None;
    }
    if candidate.open {
        if score >= possible_bar && !numbers_differ {
            Some(Band::PossibleDuplicate)
        } else if score >= similar_bar {
            Some(Band::Similar)
        } else if score >= similar_bar - HOLD_MARGIN {
            Some(Band::Hold)
        } else {
            None
        }
    } else if score >= closed_bar && !numbers_differ {
        Some(Band::ClosedDuplicate)
    } else {
        None
    }
}

fn matched_ranges(query: &[Token], candidate: &[Token]) -> Vec<(usize, usize)> {
    let query_terms: Vec<&str> =
        query.iter().map(|token| token.term.as_str()).collect();
    let mut ranges: Vec<(usize, usize)> = candidate
        .iter()
        .filter(|token| query_terms.iter().any(|term| *term == token.term))
        .map(|token| (token.start, token.end))
        .collect();
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

fn status_rank(symbol: char) -> u8 {
    match symbol {
        '/' => 0,
        '*' => 1,
        ' ' => 2,
        '?' => 3,
        _ => 4,
    }
}

fn route_rank(candidate: &Candidate, preferred: Option<&str>) -> u8 {
    match preferred {
        Some(route) if candidate.route == route => 0,
        _ => 1,
    }
}

fn recency(candidate: &Candidate) -> i32 {
    let date = candidate
        .closed
        .as_ref()
        .map(|closed| closed.date)
        .or(candidate.created);
    match date {
        Some(date) => {
            date.year() * 10_000 + date.month() as i32 * 100 + date.day() as i32
        }
        None => 0,
    }
}

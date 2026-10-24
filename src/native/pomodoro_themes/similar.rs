use chrono::NaiveDate;

use crate::native::{
    note_tasks::bounded_levenshtein, plan_budget::normalize_component,
};

const MIN_QUERY_CHARS: usize = 3;
const MIN_LEVENSHTEIN_CHARS: usize = 5;
const MAX_SIMILAR: usize = 2;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SimilarTheme<'a> {
    pub(crate) key: &'a str,
    pub(crate) name: &'a str,
    pub(crate) last_used: NaiveDate,
    pub(crate) hidden: bool,
}

/// Near-duplicate heads-up (PT7): at most two visible known names, most
/// recent first. The query's own key never qualifies.
pub(crate) fn similar_to(
    query: &str,
    themes: &[SimilarTheme<'_>],
) -> Vec<String> {
    let query_key = normalize_component(query);
    if query_key.chars().count() < MIN_QUERY_CHARS {
        return Vec::new();
    }
    let query_compact = compact_key(&query_key);
    let mut matches: Vec<SimilarTheme<'_>> = themes
        .iter()
        .copied()
        .filter(|theme| {
            !theme.hidden
                && theme.key != query_key
                && is_similar(&query_key, &query_compact, theme.key)
        })
        .collect();
    matches.sort_by(|left, right| {
        right
            .last_used
            .cmp(&left.last_used)
            .then_with(|| left.name.cmp(right.name))
            .then_with(|| left.key.cmp(right.key))
    });
    matches
        .into_iter()
        .take(MAX_SIMILAR)
        .map(|theme| theme.name.to_string())
        .collect()
}

fn is_similar(query_key: &str, query_compact: &str, other_key: &str) -> bool {
    if compact_key(other_key) == query_compact {
        return true;
    }
    if is_plural_pair(query_key, other_key) {
        return true;
    }
    query_key.chars().count() >= MIN_LEVENSHTEIN_CHARS
        && other_key.chars().count() >= MIN_LEVENSHTEIN_CHARS
        && bounded_levenshtein(query_key, other_key, 1).is_some()
}

fn compact_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn is_plural_pair(left: &str, right: &str) -> bool {
    is_plural_of(left, right) || is_plural_of(right, left)
}

fn is_plural_of(plural: &str, singular: &str) -> bool {
    plural.strip_suffix("es") == Some(singular)
        || plural.strip_suffix('s') == Some(singular)
}

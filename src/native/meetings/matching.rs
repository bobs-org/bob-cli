//! Automatic meeting matching: score Event Task candidates against
//! pasted notes text, with evidence for every ranked candidate.
//!
//! A candidate is _strong_ when at least 75% of its title's content
//! words, and at least two, appear in the notes. Auto-select happens only
//! when exactly one candidate is strong and the runner-up scores under
//! 50%: a wrong link is worse than asking.

use super::model::MeetingRow;

/// Lowercased alphanumeric runs minus this stopword list count as content
/// words. Short glue words plus the call/sync vocabulary that appears in
/// nearly every meeting title.
pub(crate) const STOPWORDS: &[&str] = &[
    "a", "an", "and", "call", "for", "in", "of", "on", "sync", "the", "to",
    "w", "with",
];

/// One scored candidate, in rank order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScoredCandidate {
    /// Index into the caller's candidate slice.
    pub row_index: usize,
    /// Fraction of the title's content words found in the notes.
    pub score: f64,
    /// Human evidence, for example `title words in notes (3/4): …`.
    pub evidence: String,
}

/// The auto-match verdict.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AutoMatch {
    /// Exactly one strong candidate with a weak runner-up.
    Selected {
        row_index: usize,
        score: f64,
        evidence: String,
    },
    /// No winner: the ranked candidates for a picker or a dry run.
    Unresolved { ranked: Vec<ScoredCandidate> },
}

/// Split text into lowercased alphanumeric runs.
fn word_runs(text: &str) -> Vec<String> {
    text.split(|char: char| !char.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase())
        .collect()
}

/// The title's content words in order, deduplicated.
pub(crate) fn content_words(title: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    word_runs(title)
        .into_iter()
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
        .filter(|word| seen.insert(word.clone()))
        .collect()
}

/// Score every candidate against the notes and auto-select at most one.
pub(crate) fn auto_match(
    notes_text: &str,
    candidates: &[MeetingRow],
) -> AutoMatch {
    let notes: std::collections::BTreeSet<String> =
        word_runs(notes_text).into_iter().collect();
    let mut ranked: Vec<ScoredCandidate> = candidates
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            let words = content_words(&row.title);
            let matched: Vec<String> = words
                .iter()
                .filter(|word| notes.contains(*word))
                .cloned()
                .collect();
            let score = if words.is_empty() {
                0.0
            } else {
                matched.len() as f64 / words.len() as f64
            };
            let evidence = if matched.is_empty() {
                format!("no title words in notes (0/{})", words.len())
            } else {
                format!(
                    "title words in notes ({}/{}): {}",
                    matched.len(),
                    words.len(),
                    matched.join(", ")
                )
            };
            ScoredCandidate {
                row_index,
                score,
                evidence,
            }
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let left_row = &candidates[left.row_index];
                let right_row = &candidates[right.row_index];
                (right_row.date, right_row.starts_at)
                    .cmp(&(left_row.date, left_row.starts_at))
            })
    });
    let strong: Vec<&ScoredCandidate> = ranked
        .iter()
        .filter(|candidate| {
            let total =
                content_words(&candidates[candidate.row_index].title).len();
            total >= 2 && candidate.score >= 0.75
        })
        .collect();
    let runner_up = ranked.get(1).map_or(0.0, |candidate| candidate.score);
    if let [only] = strong.as_slice()
        && runner_up < 0.5
    {
        return AutoMatch::Selected {
            row_index: only.row_index,
            score: only.score,
            evidence: only.evidence.clone(),
        };
    }
    AutoMatch::Unresolved { ranked }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopwords_leave_real_content_words() {
        assert_eq!(
            content_words("Weekly Sync with Acme Corp"),
            vec!["weekly", "acme", "corp"]
        );
        assert_eq!(content_words("and w the"), Vec::<String>::new());
    }
}

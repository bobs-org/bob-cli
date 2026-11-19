use unicode_normalization::UnicodeNormalization;

use super::{MatchKind, Phrase, RankedPhrase, HALF_LIFE_DAYS};

/// Writer/client normalization: NFC, Unicode lowercase, collapsed whitespace.
pub(crate) fn normalize(text: &str) -> String {
    let nfc: String = text.nfc().collect();
    let lowered = nfc.chars().flat_map(char::to_lowercase).collect::<String>();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Literal four-tier rank. The newest matching row is pinned within its tier;
/// the rest use the bob-provided frecency score and recency rank.
pub(crate) fn rank<'a>(
    phrases: &'a [Phrase],
    query: Option<&str>,
) -> Vec<RankedPhrase<'a>> {
    let Some(query) = query else {
        return sort_all(phrases)
            .into_iter()
            .map(|phrase| RankedPhrase {
                phrase,
                matched: None,
            })
            .collect();
    };
    let query = normalize(query);
    if query.is_empty() {
        return sort_all(phrases)
            .into_iter()
            .map(|phrase| RankedPhrase {
                phrase,
                matched: None,
            })
            .collect();
    }
    let mut tiers: [Vec<&Phrase>; 4] = std::array::from_fn(|_| Vec::new());
    for phrase in phrases {
        if phrase.key == query {
            tiers[0].push(phrase);
        } else if phrase.key.starts_with(&query) {
            tiers[1].push(phrase);
        } else if has_word_prefix(&phrase.key, &query) {
            tiers[2].push(phrase);
        } else if phrase.key.contains(&query) {
            tiers[3].push(phrase);
        }
    }
    let mut result = Vec::new();
    for (index, rows) in tiers.into_iter().enumerate() {
        let kind = match index {
            0 => MatchKind::Exact,
            1 => MatchKind::Prefix,
            2 => MatchKind::Word,
            _ => MatchKind::Substring,
        };
        for phrase in sort_tier(rows) {
            result.push(RankedPhrase {
                phrase,
                matched: Some(kind),
            });
        }
    }
    result
}

fn has_word_prefix(text: &str, query: &str) -> bool {
    text.match_indices(query).any(|(byte_index, _)| {
        if byte_index == 0 {
            return true;
        }
        let before = text[..byte_index].chars().next_back();
        let at = text[byte_index..].chars().next();
        before.is_some_and(|ch| !ch.is_alphanumeric())
            && at.is_some_and(char::is_alphanumeric)
    })
}

fn sort_all(phrases: &[Phrase]) -> Vec<&Phrase> {
    sort_tier(phrases.iter().collect())
}

fn sort_tier(mut phrases: Vec<&Phrase>) -> Vec<&Phrase> {
    let newest = phrases.iter().map(|phrase| phrase.recency_rank).min();
    phrases.sort_by(|left, right| {
        let left_pinned = Some(left.recency_rank) == newest;
        let right_pinned = Some(right.recency_rank) == newest;
        right_pinned
            .cmp(&left_pinned)
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| left.recency_rank.cmp(&right.recency_rank))
            .then_with(|| left.key.cmp(&right.key))
    });
    phrases
}

/// Score one use at the scanner's reference date.
pub(crate) fn use_score(age_days: f64) -> f64 {
    0.5_f64.powf(age_days.max(0.0) / HALF_LIFE_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_nfc_case_and_whitespace() {
        assert_eq!(normalize("  CAFÉ\t\nÉcole  "), "café école");
        assert_eq!(normalize("cafe\u{301}"), "café");
        assert_eq!(normalize("🎲 東京"), "🎲 東京");
    }

    #[test]
    fn seven_day_half_life_halves_one_use() {
        assert!((use_score(7.0) - 0.5).abs() < f64::EPSILON);
        assert_eq!(use_score(0.0), 1.0);
    }

    #[test]
    fn uses_exact_prefix_word_and_substring_tiers_with_newest_pin() {
        let phrases = vec![
            phrase("xagent", 0, 1.0),
            phrase("agent", 3, 0.1),
            phrase("the agent", 1, 10.0),
            phrase("new agent today", 2, 0.2),
            phrase("agentic", 5, 0.1),
            phrase("other", 4, 1.0),
        ];
        let ranked = rank(&phrases, Some("agent"));
        assert_eq!(
            ranked
                .iter()
                .map(|r| r.phrase.key.as_str())
                .collect::<Vec<_>>(),
            ["agent", "agentic", "the agent", "new agent today", "xagent"]
        );
        assert_eq!(
            ranked.iter().map(|r| r.matched).collect::<Vec<_>>(),
            [
                Some(MatchKind::Exact),
                Some(MatchKind::Prefix),
                Some(MatchKind::Word),
                Some(MatchKind::Word),
                Some(MatchKind::Substring)
            ]
        );
    }

    fn phrase(key: &str, recency_rank: usize, score: f64) -> Phrase {
        Phrase {
            text: key.into(),
            key: key.into(),
            uses: 1,
            last_used: "2026-11-19".into(),
            score,
            recency_rank,
        }
    }
}

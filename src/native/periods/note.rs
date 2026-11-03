//! Period-note anatomy parser and review-state machine.
//!
//! The contract lives in `docs/periods.md` §3. Sections are the bodies
//! under exact `## <Name>` headings up to the next `## ` heading, the
//! `> [!agent]` callout, `## Archive`, or EOF, with blank lines trimmed.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::calendar::PeriodId;

/// Review state, computed by first match: reflected, drafted, edited,
/// then empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewState {
    Reflected,
    Drafted,
    Edited,
    Empty,
}

impl ReviewState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reflected => "reflected",
            Self::Drafted => "drafted",
            Self::Edited => "edited",
            Self::Empty => "empty",
        }
    }

    /// The one-character mark the list shows for a state.
    #[allow(dead_code)]
    pub(crate) fn mark(self) -> &'static str {
        match self {
            Self::Reflected => "✓",
            Self::Drafted => "·",
            Self::Edited => "·",
            Self::Empty => "–",
        }
    }
}

/// A parsed period note: its sections, frontmatter keys, review state,
/// Headline, Reflection lead, and instruction version.
///
/// Later phases (report, migrate) reuse every field; core reads the
/// review state and Headline through `list`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct ParsedNote {
    pub(crate) sections: BTreeMap<String, String>,
    pub(crate) frontmatter: BTreeMap<String, String>,
    pub(crate) review_state: ReviewState,
    pub(crate) headline: Option<String>,
    pub(crate) reflection_lead: Option<String>,
    pub(crate) instructions_version: Option<u32>,
}

/// Parse `contents` into sections and frontmatter-derived state.
pub(crate) fn parse(contents: &str) -> ParsedNote {
    let frontmatter = parse_frontmatter(contents);
    let sections = parse_sections(contents);
    let reflection = sections.get("Reflection").cloned().unwrap_or_default();
    let review = sections.get("Review").cloned().unwrap_or_default();
    let review_state = if !reflection.trim().is_empty() {
        ReviewState::Reflected
    } else if review.trim().is_empty() {
        ReviewState::Empty
    } else {
        let digest = review_digest(&review);
        match frontmatter.get("review_digest") {
            Some(stored) if stored == &digest => ReviewState::Drafted,
            _ => ReviewState::Edited,
        }
    };
    let headline = parse_headline(&review);
    let reflection_lead = parse_reflection_lead(&reflection);
    let instructions_version = parse_instructions_version(contents);
    ParsedNote {
        sections,
        frontmatter,
        review_state,
        headline,
        reflection_lead,
        instructions_version,
    }
}

/// SHA-256 digest of a Review body: `sha256:` plus the first 16 hex
/// characters over the body after normalizing line endings to `\n`,
/// stripping trailing whitespace from each line, and trimming.
pub(crate) fn review_digest(review_body: &str) -> String {
    let normalized = review_body.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<String> = normalized
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();
    let trimmed = lines.join("\n").trim().to_string();
    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
    let hex_digest = hex::encode(hasher.finalize());
    format!("sha256:{}", &hex_digest[..16])
}

/// First `## Review` line matching `**Headline:** <text>`, else none.
pub(crate) fn parse_headline(review_body: &str) -> Option<String> {
    for line in review_body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("**Headline:**") {
            let headline = rest.trim().to_string();
            if !headline.is_empty() {
                return Some(headline);
            }
        }
    }
    None
}

/// First sentence of `## Reflection`: ends at `.`, `?`, or `!`
/// followed by whitespace, or at the end of the line. Capped at 40
/// words with `…`.
pub(crate) fn parse_reflection_lead(reflection_body: &str) -> Option<String> {
    let trimmed = reflection_body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first_line = trimmed.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return None;
    }
    let mut end = first_line.len();
    for (index, window) in first_line.char_indices() {
        if matches!(window, '.' | '?' | '!') {
            let next = index + window.len_utf8();
            if next >= first_line.len()
                || first_line[next..].starts_with(char::is_whitespace)
            {
                end = next;
                break;
            }
        }
    }
    let mut sentence = first_line[..end].trim().to_string();
    let words: Vec<&str> = sentence.split_whitespace().collect();
    if words.len() > 40 {
        sentence = format!("{}…", words[..40].join(" "));
    }
    if sentence.is_empty() {
        None
    } else {
        Some(sentence)
    }
}

/// Instructions version from the callout title
/// `Agent instructions · <kind> v<N>`.
pub(crate) fn parse_instructions_version(contents: &str) -> Option<u32> {
    for line in contents.lines() {
        let Some(marker) = line.find("Agent instructions ·") else {
            continue;
        };
        let tail = &line[marker..];
        // The version is the `v<N>` right after the kind word; later
        // `v` letters in the heading prose (Nov, Review, have) do not
        // count.
        for (index, _) in tail.match_indices('v') {
            let before = &tail[..index];
            if !before.ends_with(' ') {
                continue;
            }
            let digits: String = tail[index + 1..]
                .chars()
                .take_while(|ch| ch.is_numeric())
                .collect();
            if !digits.is_empty()
                && let Ok(version) = digits.parse::<u32>()
            {
                return Some(version);
            }
        }
    }
    None
}

/// Required headings and fences for `id` are present.
///
/// The migrate phase reports this as `unscaffolded`; core keeps the
/// check beside the parser it guards.
#[allow(dead_code)]
pub(crate) fn structure_ok(id: PeriodId, contents: &str) -> bool {
    let sections = parse_sections(contents);
    let has_strip = contents.contains("```bob-stats-strip");
    let has_parts = contents.contains("```bob-period-parts");
    match id {
        PeriodId::Week { .. } => {
            sections.contains_key("Intentions")
                && sections.contains_key("Days")
                && sections.contains_key("Review")
                && sections.contains_key("Reflection")
                && has_strip
                && has_parts
        }
        PeriodId::Quarter { .. } => {
            sections.contains_key("Outcomes")
                && sections.contains_key("Weeks")
                && sections.contains_key("Review")
                && sections.contains_key("Reflection")
                && has_strip
                && has_parts
        }
        PeriodId::Year(_) => {
            sections.contains_key("Theme")
                && sections.contains_key("Outcomes")
                && sections.contains_key("Quarters")
                && sections.contains_key("Review")
                && sections.contains_key("Reflection")
                && has_strip
                && has_parts
        }
        PeriodId::AllTime => {
            sections.contains_key("Years")
                && sections.contains_key("Notes")
                && has_strip
                && has_parts
        }
    }
}

fn parse_frontmatter(contents: &str) -> BTreeMap<String, String> {
    let mut frontmatter = BTreeMap::new();
    let mut lines = contents.lines();
    if lines.next().map(str::trim) != Some("---") {
        return frontmatter;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        // Multi-line list bodies (aliases, tags) are skipped: only
        // scalar `key: value` lines are indexed.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let mut value = value.trim().to_string();
            // Strip one layer of surrounding quotes.
            if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value = value[1..value.len() - 1].to_string();
            }
            if !key.is_empty() {
                frontmatter.insert(key, value);
            }
        }
    }
    frontmatter
}

fn parse_sections(contents: &str) -> BTreeMap<String, String> {
    let mut sections = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut body: Vec<String> = Vec::new();
    // Skip the frontmatter block.
    let mut lines = contents.lines().peekable();
    if lines.peek().map(|line| line.trim()) == Some("---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }
    for line in lines {
        if line.starts_with("> [!agent]") {
            break;
        }
        if line.trim() == "## Archive" || line.starts_with("## Archive ") {
            break;
        }
        if let Some(name) = line.strip_prefix("## ") {
            if let Some(previous) = current.take() {
                sections.insert(previous, trim_body(&body));
            }
            // Only exact `## <Name>` headings open sections: a name
            // with extra words still counts under its full name.
            let name = name.trim().to_string();
            if !name.is_empty() && !name.contains("  ") {
                current = Some(name);
            } else if !name.is_empty() {
                current = Some(name);
            }
            body = Vec::new();
            continue;
        }
        if current.is_some() {
            body.push(line.to_string());
        }
    }
    if let Some(previous) = current {
        sections.insert(previous, trim_body(&body));
    }
    sections
}

fn trim_body(lines: &[String]) -> String {
    let joined = lines.join("\n");
    joined.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drafted_note() -> String {
        let review = "**Headline:** A calm week\n\nDid the work.\n";
        let digest = review_digest(review);
        format!(
            "---\nparent: \"[[2026/2026-Q4]]\"\ntype: \"[[day]]\"\nid: 2026-W45\nreview_digest: {digest}\n---\n\n# 2026-W45\n\n## Review\n\n{review}\n## Reflection\n"
        )
    }

    #[test]
    fn empty_reflected_drafted_and_edited_states() {
        let empty = parse("# 2026-W45\n\n## Review\n\n## Reflection\n");
        assert_eq!(empty.review_state, ReviewState::Empty);

        let reflected = parse("# 2026-W45\n\n## Review\n\nOld.\n\n## Reflection\n\nIt meant a lot.\n");
        assert_eq!(reflected.review_state, ReviewState::Reflected);

        let drafted = parse(&drafted_note());
        assert_eq!(drafted.review_state, ReviewState::Drafted);
        assert_eq!(drafted.headline.as_deref(), Some("A calm week"));

        let edited =
            parse("# 2026-W45\n\n## Review\n\n**Headline:** Changed\n");
        assert_eq!(edited.review_state, ReviewState::Edited);
    }

    #[test]
    fn headline_and_reflection_lead_rules() {
        assert_eq!(
            parse_headline("**Headline:** One two three\n\nBody\n"),
            Some("One two three".to_string())
        );
        assert_eq!(parse_headline("No headline here\n"), None);
        assert_eq!(
            parse_reflection_lead("It was good. Then more."),
            Some("It was good.".to_string())
        );
        assert_eq!(parse_reflection_lead("   "), None);
    }

    #[test]
    fn digest_is_stable_over_trailing_whitespace() {
        let first = review_digest("Line one  \nLine two\n");
        let second = review_digest("Line one\nLine two  \n\n");
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));
        assert_eq!(first.len(), "sha256:".len() + 16);
    }
}

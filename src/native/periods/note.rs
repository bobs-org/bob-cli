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

/// Keep a bob rewrite from reading as Bryan's edit.
///
/// When a vault rewrite (`bob task archive` moving links to `done/`, a
/// type or relation-label rename, …) changes the `## Review` of a note
/// whose Review was still `drafted` in `before`, restamp its
/// `review_digest` in `after` so the state stays `drafted`. `edited` is
/// the only automatic sign that Bryan corrected a draft, and it also
/// blocks redrafting without `-F`. Every other note, and every note
/// whose Review is unchanged or not `drafted`, returns `after` as is.
pub(crate) fn carry_review_digest(before: &str, after: &str) -> String {
    // Cheap pre-filter: only period notes stamped by `draft` qualify.
    if before == after || !before.contains("review_digest:") {
        return after.to_string();
    }
    if parse(before).review_state != ReviewState::Drafted {
        return after.to_string();
    }
    let parsed_after = parse(after);
    if parsed_after.review_state != ReviewState::Edited {
        return after.to_string();
    }
    let review = parsed_after
        .sections
        .get("Review")
        .cloned()
        .unwrap_or_default();
    let digest = review_digest(&review);
    let mut lines = after.split_inclusive('\n');
    let mut out = String::with_capacity(after.len());
    let Some(first) = lines.next() else {
        return after.to_string();
    };
    if first.trim() != "---" {
        return after.to_string();
    }
    out.push_str(first);
    let mut stamped = false;
    for line in lines.by_ref() {
        let trimmed = line.trim();
        if trimmed == "---" {
            out.push_str(line);
            break;
        }
        if !stamped
            && line
                .split_once(':')
                .is_some_and(|(key, _)| key.trim() == "review_digest")
        {
            let ending = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(&format!("review_digest: {digest}{ending}"));
            stamped = true;
            continue;
        }
        out.push_str(line);
    }
    if !stamped {
        return after.to_string();
    }
    out.extend(lines);
    out
}

/// The Headline: the whole paragraph that opens with the first
/// `**Headline:** <text>` line, soft-wrapped lines joined with single
/// spaces, else none. The paragraph ends at a blank line, a list item,
/// or the next bold `**Part:**` label, so a hard-wrapped Headline still
/// reaches parent notes as one sentence.
pub(crate) fn parse_headline(review_body: &str) -> Option<String> {
    let mut lines = review_body.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim().strip_prefix("**Headline:**") else {
            continue;
        };
        let mut words: Vec<&str> = rest.split_whitespace().collect();
        for next in lines.by_ref() {
            if ends_headline_paragraph(next) {
                break;
            }
            words.extend(next.split_whitespace());
        }
        if words.is_empty() {
            continue;
        }
        return Some(words.join(" "));
    }
    None
}

/// The opening `**Headline:**` paragraph of `text` with its soft-wrapped
/// continuation lines joined onto one physical line; every other byte is
/// kept. `None` when `text` does not open with a Headline.
pub(crate) fn unwrap_headline(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let rest = first.trim().strip_prefix("**Headline:**")?;
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    let mut tail: Vec<&str> = Vec::new();
    let mut in_paragraph = true;
    for line in lines {
        if in_paragraph && !ends_headline_paragraph(line) {
            words.extend(line.split_whitespace());
            continue;
        }
        in_paragraph = false;
        tail.push(line);
    }
    let mut out = format!("**Headline:** {}", words.join(" "));
    for line in tail {
        out.push('\n');
        out.push_str(line);
    }
    Some(out)
}

/// Whether `line` ends the Headline paragraph: a blank line, a list
/// item, or another bold `**Part:**` label.
fn ends_headline_paragraph(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return true;
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && trimmed[digits..].starts_with(". ") {
        return true;
    }
    trimmed.starts_with("**")
        && trimmed[2..]
            .find("**")
            .is_some_and(|end| trimmed[2..2 + end].ends_with(':'))
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

/// True when a period note holds nothing `ensure` must preserve: it is
/// empty or whitespace-only, or it is only a closed frontmatter block
/// with a blank body. The second shape is what clicking a dangling
/// period link leaves behind, because the vault's Templater folder
/// template stubs every new note with `parent:` and `created:`
/// frontmatter. Any body text, or an unclosed frontmatter block, counts
/// as content.
pub(crate) fn is_unfilled(contents: &str) -> bool {
    if contents.trim().is_empty() {
        return true;
    }
    let mut lines = contents.lines();
    if lines.next().map(str::trim) != Some("---") {
        return false;
    }
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
    }
    closed && lines.all(|line| line.trim().is_empty())
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

    #[test]
    fn unfilled_notes_are_blank_or_frontmatter_only_stubs() {
        assert!(is_unfilled(""));
        assert!(is_unfilled("  \n\t\n"));
        let stub = "---\nparent: \"[[2026/2026-W46|2026/2026-W46]]\"\ncreated: 2026-11-03T16:30:00-05:00\n---\n";
        assert!(is_unfilled(stub), "a Templater stub is fillable");
        assert!(is_unfilled(&format!("{stub}\n  \n")));
        assert!(!is_unfilled(&format!("{stub}Bryan's words\n")));
        assert!(
            !is_unfilled("---\nparent: x\n"),
            "unclosed frontmatter is content"
        );
        assert!(!is_unfilled("Bryan's words\n"));
    }

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
        // A hard-wrapped Headline reaches parents whole.
        assert_eq!(
            parse_headline(
                "**Headline:** A 24h 45m week, put 45% into bob, led by WIP/Next\nauto-decay and templates.\n\n**Shipped:**\n"
            ),
            Some(
                "A 24h 45m week, put 45% into bob, led by WIP/Next auto-decay and templates."
                    .to_string()
            )
        );
        // The paragraph stops at a list item or the next bold label.
        assert_eq!(
            parse_headline("**Headline:** Shipped X.\n**Shipped:**\n- [[a]]\n"),
            Some("Shipped X.".to_string())
        );
        assert_eq!(
            parse_headline("**Headline:** Shipped X.\n- [[a]]\n"),
            Some("Shipped X.".to_string())
        );
        assert_eq!(
            unwrap_headline(
                "**Headline:** One\ntwo  three\n\n**Shipped:**\n- x"
            ),
            Some(
                "**Headline:** One two three\n\n**Shipped:**\n- x".to_string()
            )
        );
        assert_eq!(unwrap_headline("Body first\n**Headline:** x"), None);
        assert_eq!(
            parse_reflection_lead("It was good. Then more."),
            Some("It was good.".to_string())
        );
        assert_eq!(parse_reflection_lead("   "), None);
    }

    #[test]
    fn bob_rewrites_keep_a_drafted_review_drafted() {
        let before = drafted_note()
            .replace("Did the work.", "Shipped [[bob#^decay|decay]].");
        // Re-stamp the fixture so `before` is genuinely drafted.
        let review =
            "**Headline:** A calm week\n\nShipped [[bob#^decay|decay]].";
        let before = before.replace(
            &review_digest("**Headline:** A calm week\n\nDid the work.\n"),
            &review_digest(review),
        );
        assert_eq!(parse(&before).review_state, ReviewState::Drafted);

        // An archive moves the link to `done/`: still drafted afterwards.
        let after = before.replace("[[bob#^decay", "[[done/bob_done#^decay");
        assert_eq!(parse(&after).review_state, ReviewState::Edited);
        let carried = carry_review_digest(&before, &after);
        assert_eq!(parse(&carried).review_state, ReviewState::Drafted);
        assert!(carried.contains("[[done/bob_done#^decay|decay]]"));
        assert_eq!(
            carried.lines().count(),
            after.lines().count(),
            "only the digest line changes"
        );

        // An edited Review stays edited; other notes pass through.
        let edited = after.clone();
        assert_eq!(carry_review_digest(&edited, &edited), edited);
        let moved_edit = edited.replace("A calm", "A busy");
        assert_eq!(carry_review_digest(&edited, &moved_edit), moved_edit);
        let plain = "# Note\n\n[[bob#^decay]]\n";
        assert_eq!(
            carry_review_digest(plain, "# Note\n\n[[done/bob_done#^decay]]\n"),
            "# Note\n\n[[done/bob_done#^decay]]\n"
        );
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

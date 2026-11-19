//! Period-note anatomy parser and review-state machine.
//!
//! The contract lives in `docs/periods.md` §3. Sections are the bodies
//! under exact `## <Name>` headings up to the next `## ` heading, the
//! `> [!agent]` callout, `## Archive`, or EOF, with blank lines trimmed.

use std::collections::BTreeMap;

use super::calendar::PeriodId;
use crate::native::daily_headline::ends_headline_paragraph;
use crate::native::intentions::{
    self,
    fields::{FieldProvenance, FrontmatterState},
    state,
    write::{self, OwnedPatch, Patch, WriteError},
    NoteSubject,
};

pub(crate) use crate::native::intentions::ReviewState;

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
    parse_for_period(
        contents,
        PeriodId::Week {
            year: 1970,
            week: 1,
        },
    )
}

pub(crate) fn parse_for_period(contents: &str, id: PeriodId) -> ParsedNote {
    let frontmatter = parse_frontmatter(contents);
    let sections = parse_sections(contents);
    let fields = intentions::read(contents, NoteSubject::Period(id));
    let review = sections.get("Review").cloned().unwrap_or_default();
    let review_state = intentions::evaluate(&fields, !review.trim().is_empty());
    let headline = fields.summary.value;
    let reflection = fields.reflection.value.unwrap_or_default();
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
    intentions::digest(review_body)
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
pub(crate) fn carry_review_digest(
    before: &str,
    after: &str,
) -> Result<String, WriteError> {
    if before == after {
        return Ok(after.to_owned());
    }
    let subject = NoteSubject::Period(PeriodId::Week {
        year: 1970,
        week: 1,
    });
    let before_fields = intentions::read(before, subject);
    let review_before = before_fields.review_body.as_str();
    let before_digest = &before_fields.review_digest;
    if before_fields.frontmatter_state != FrontmatterState::Valid
        || !before_digest.present
        || !before_digest.valid
        || before_digest.value.as_deref() != Some(&review_digest(review_before))
        || state::evaluate(&before_fields, !review_before.trim().is_empty())
            != ReviewState::Drafted
    {
        return Ok(after.to_owned());
    }
    let after_fields = intentions::read(after, subject);
    if after_fields.frontmatter_state != FrontmatterState::Valid
        || review_before == after_fields.review_body
        || !after_fields.review_digest.present
        || !after_fields.review_digest.valid
        || after_fields.review_digest.value != before_digest.value
    {
        return Ok(after.to_owned());
    }
    write::apply(
        after,
        &OwnedPatch {
            review_digest: Patch::Set(review_digest(&after_fields.review_body)),
            ..OwnedPatch::default()
        },
    )
}

/// Carry both agent digests through a mechanical rewrite. Eligibility is
/// always read from `before`; staged edits cannot turn an edited or reflected
/// note back into an owned draft.
pub(crate) fn carry_rewrite_digests(
    before: &str,
    after: &str,
    subject: Option<NoteSubject>,
) -> Result<String, WriteError> {
    let after_review = carry_review_digest(before, after)?;
    let Some(subject) = subject else {
        return Ok(after_review);
    };
    carry_summary_digest(before, &after_review, subject)
}

fn carry_summary_digest(
    before: &str,
    after: &str,
    subject: NoteSubject,
) -> Result<String, WriteError> {
    if before == after
        || matches!(subject, NoteSubject::Period(PeriodId::AllTime))
    {
        return Ok(after.to_owned());
    }
    let before_fields = intentions::read(before, subject);
    let Some(summary_before) = before_fields.summary.value.as_deref() else {
        return Ok(after.to_owned());
    };
    if before_fields.frontmatter_state != FrontmatterState::Valid
        || before_fields.summary.provenance != FieldProvenance::Frontmatter
        || !before_fields.summary.valid
        || !before_fields.summary_digest.present
        || !before_fields.summary_digest.valid
        || before_fields.summary_digest.value.as_deref()
            != Some(&state::digest(summary_before))
        || state::evaluate(
            &before_fields,
            matches!(subject, NoteSubject::Period(_))
                && !before_fields.review_body.trim().is_empty(),
        ) != ReviewState::Drafted
    {
        return Ok(after.to_owned());
    }

    let after_fields = intentions::read(after, subject);
    let Some(summary_after) = after_fields.summary.value.as_deref() else {
        return Ok(after.to_owned());
    };
    if after_fields.frontmatter_state != FrontmatterState::Valid
        || after_fields.summary.provenance != FieldProvenance::Frontmatter
        || !after_fields.summary.valid
        || summary_before == summary_after
        || after_fields.summary_digest.value
            != before_fields.summary_digest.value
    {
        return Ok(after.to_owned());
    }

    write::apply(
        after,
        &OwnedPatch {
            summary_digest: Patch::Set(state::digest(summary_after)),
            ..OwnedPatch::default()
        },
    )
}

/// The Headline: the whole paragraph that opens with the first
/// `**Headline:** <text>` line, soft-wrapped lines joined with single
/// spaces, else none. The paragraph ends at a blank line, a list item,
/// or the next bold `**Part:**` label, so a hard-wrapped Headline still
/// reaches parent notes as one sentence.
#[cfg(test)]
pub(crate) fn parse_headline(review_body: &str) -> Option<String> {
    crate::native::intentions::legacy::opening_headline(review_body)
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
/// check beside the parser it guards. After the intentions migration
/// the old Intentions/Outcomes/Theme/Reflection sections are gone, so
/// both the old and the new anatomy pass.
#[allow(dead_code)]
pub(crate) fn structure_ok(id: PeriodId, contents: &str) -> bool {
    let sections = parse_sections(contents);
    let has_strip = contents.contains("```bob-stats-strip");
    let has_parts = contents.contains("```bob-period-parts");
    match id {
        PeriodId::Week { .. } => {
            sections.contains_key("Days")
                && sections.contains_key("Review")
                && has_strip
                && has_parts
        }
        PeriodId::Quarter { .. } => {
            sections.contains_key("Weeks")
                && sections.contains_key("Review")
                && has_strip
                && has_parts
        }
        PeriodId::Year(_) => {
            sections.contains_key("Quarters")
                && sections.contains_key("Review")
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
        for marker in ["+ unrelated list", "1) unrelated list"] {
            assert_eq!(
                parse_headline(&format!(
                    "**Headline:** Shipped X.\n{marker}\n"
                )),
                Some("Shipped X.".to_string()),
                "{marker}"
            );
            assert_eq!(
                unwrap_headline(&format!(
                    "**Headline:** Shipped X.\n{marker}\n"
                )),
                Some(format!("**Headline:** Shipped X.\n{marker}")),
                "{marker}"
            );
        }
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
        let carried = carry_review_digest(&before, &after).unwrap();
        assert_eq!(parse(&carried).review_state, ReviewState::Drafted);
        assert!(carried.contains("[[done/bob_done#^decay|decay]]"));
        assert_eq!(
            carried.lines().count(),
            after.lines().count(),
            "only the digest line changes"
        );

        // An edited Review stays edited; other notes pass through.
        let edited = after.clone();
        assert_eq!(carry_review_digest(&edited, &edited).unwrap(), edited);
        let moved_edit = edited.replace("A calm", "A busy");
        assert_eq!(
            carry_review_digest(&edited, &moved_edit).unwrap(),
            moved_edit
        );
        let plain = "# Note\n\n[[bob#^decay]]\n";
        assert_eq!(
            carry_review_digest(plain, "# Note\n\n[[done/bob_done#^decay]]\n"),
            Ok("# Note\n\n[[done/bob_done#^decay]]\n".to_owned())
        );
    }

    #[test]
    fn canonical_summary_carry_uses_original_provenance_and_verified_writer() {
        let day = NoteSubject::Day(
            chrono::NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(),
        );
        let summary = "Shipped [[work#^block]].";
        let before = format!(
            "---\nsummary: {summary:?}\nsummary_digest: {}\nsummary_drafted: 2026-11-19\ncustom: keep # exact\n---\n# 2026-11-18\n",
            intentions::digest(summary)
        );
        let after = before.replace("[[work#^block]]", "[[done/work#^block]]");
        let carried =
            carry_rewrite_digests(&before, &after, Some(day)).unwrap();
        let fields = intentions::read(&carried, day);
        assert_eq!(
            fields.summary_digest.value.as_deref(),
            Some(intentions::digest("Shipped [[done/work#^block]].").as_str())
        );
        assert_eq!(fields.summary_drafted.value.as_deref(), Some("2026-11-19"));
        assert!(carried.contains("custom: keep # exact\n"));
        assert_eq!(
            state::summary_state(&fields),
            crate::native::intentions::state::SummaryState::Drafted
        );

        let period = NoteSubject::Period(PeriodId::Week {
            year: 2026,
            week: 47,
        });
        let summary = "Shipped [[work#^block]].";
        let review = "**Headline:** A useful week\n\n[[work#^block]] shipped.";
        let before = format!(
            "---\nsummary: {summary:?}\nsummary_digest: {}\nreview_digest: {}\nsummary_drafted: 2026-11-19\n---\n# 2026-W47\n\n## Review\n\n{review}\n",
            intentions::digest(summary),
            review_digest(review)
        );
        let after = before.replace("[[work#^block]]", "[[done/work#^block]]");
        let carried =
            carry_rewrite_digests(&before, &after, Some(period)).unwrap();
        let fields = intentions::read(&carried, period);
        assert_eq!(
            fields.summary_digest.value.as_deref(),
            Some(intentions::digest("Shipped [[done/work#^block]].").as_str())
        );
        assert_eq!(
            fields.review_digest.value.as_deref(),
            Some(review_digest(
                "**Headline:** A useful week\n\n[[done/work#^block]] shipped."
            )
            .as_str())
        );
        assert_eq!(
            state::evaluate(&fields, true),
            ReviewState::Drafted,
            "both agent parts stay drafted after one transaction"
        );
    }

    #[test]
    fn summary_carry_skips_edited_reflected_legacy_and_unchanged_parts() {
        let day = NoteSubject::Day(
            chrono::NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(),
        );
        let summary = "Shipped [[work#^block]].";
        let drafted = format!(
            "---\nsummary: {summary:?}\nsummary_digest: {}\nsummary_drafted: 2026-11-19\n---\n",
            intentions::digest(summary)
        );
        let edited = drafted.replace(
            "summary_digest: ",
            "summary_digest: sha256:0000000000000000 # was ",
        );
        let edited_after =
            edited.replace("[[work#^block]]", "[[done/work#^block]]");
        assert_eq!(
            carry_rewrite_digests(&edited, &edited_after, Some(day)).unwrap(),
            edited_after
        );

        let reflected =
            drafted.replace("---\n", "reflection: Meaningful.\n---\n");
        let reflected_after =
            reflected.replace("[[work#^block]]", "[[done/work#^block]]");
        assert_eq!(
            carry_rewrite_digests(&reflected, &reflected_after, Some(day))
                .unwrap(),
            reflected_after
        );

        let period = NoteSubject::Period(PeriodId::Week {
            year: 2026,
            week: 47,
        });
        let legacy_review = format!(
            "---\nreview_digest: {}\n---\n# 2026-W47\n\n## Review\n\n**Headline:** A useful week\n\n[[work#^block]] shipped.\n",
            review_digest("**Headline:** A useful week\n\n[[work#^block]] shipped.\n")
        );
        let legacy_after =
            legacy_review.replace("[[work#^block]]", "[[done/work#^block]]");
        let carried =
            carry_rewrite_digests(&legacy_review, &legacy_after, Some(period))
                .unwrap();
        assert_eq!(
            intentions::read(&carried, period).summary.provenance,
            FieldProvenance::LegacyReview
        );
        assert_eq!(
            parse_for_period(
                &carried,
                match period {
                    NoteSubject::Period(id) => id,
                    _ => unreachable!(),
                }
            )
            .review_state,
            ReviewState::Drafted
        );

        let all_time = NoteSubject::Period(PeriodId::AllTime);
        let all_time_review = "[[work#^block]] remains recorded.";
        let all_time_before = format!(
            "---\nreview_digest: {}\n---\n## Review\n\n{all_time_review}\n",
            review_digest(all_time_review)
        );
        let all_time_after =
            all_time_before.replace("[[work#^block]]", "[[done/work#^block]]");
        let all_time_carried = carry_rewrite_digests(
            &all_time_before,
            &all_time_after,
            Some(all_time),
        )
        .unwrap();
        assert!(all_time_carried.contains(&review_digest(
            "[[done/work#^block]] remains recorded.\n"
        )));
        assert!(!all_time_carried.contains("summary_digest:"));
        assert_eq!(
            carry_rewrite_digests(&drafted, &drafted, Some(day)).unwrap(),
            drafted,
            "unchanged parts retain exact bytes"
        );
    }

    #[test]
    fn quoted_review_digest_keys_are_carried_without_touching_other_yaml() {
        let review = "**Headline:** A useful week\n\n[[work#^block]] shipped.";
        let before = format!(
            "---\n\"review_digest\": {}\ncustom: keep\n---\n# 2026-W47\n\n## Review\n\n{review}\n",
            review_digest(review)
        );
        let after = before.replace("[[work#^block]]", "[[done/work#^block]]");
        let carried = carry_review_digest(&before, &after).unwrap();
        assert!(carried.contains(&format!(
            "\"review_digest\": {}\n",
            review_digest(
                "**Headline:** A useful week\n\n[[done/work#^block]] shipped."
            )
        )));
        assert!(carried.contains("custom: keep\n"));
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

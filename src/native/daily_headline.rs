//! Daily Headline: one Bryan-owned `**Headline:**` line in a daily note
//! preamble. bob reads it and never writes it.

use super::markdown;

/// The first non-empty Daily Headline in a note's preamble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DailyHeadline {
    /// Joined remainder, Markdown kept, whitespace collapsed to single
    /// spaces.
    pub(crate) text: String,
    /// Screen text: wiki-links, Markdown links, and emphasis stripped.
    pub(crate) display: String,
}

/// Parse the first non-empty `**Headline:**` / `**Headline**:` paragraph
/// in the preamble (after YAML frontmatter, before the first `## `
/// heading, skipping fenced blocks). An empty slot is not a Headline.
#[cfg(test)]
pub(crate) fn parse(contents: &str) -> Option<DailyHeadline> {
    crate::native::intentions::legacy::parse_daily_headline(contents)
}

/// Whether `line` ends a Headline paragraph: a blank line, a list item,
/// a heading, a fence, or another bold `**Label:**` / `**Label**:`.
pub(crate) fn ends_headline_paragraph(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    let bytes = trimmed.as_bytes();
    if bytes
        .first()
        .is_some_and(|byte| matches!(*byte, b'-' | b'+' | b'*'))
        && bytes
            .get(1)
            .is_some_and(|byte| matches!(*byte, b' ' | b'\t'))
    {
        return true;
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let marker_and_separator = &trimmed.as_bytes()[digits..];
        if marker_and_separator
            .first()
            .is_some_and(|byte| matches!(*byte, b'.' | b')'))
            && marker_and_separator
                .get(1)
                .is_some_and(|byte| matches!(*byte, b' ' | b'\t'))
        {
            return true;
        }
    }
    if markdown::atx_heading(line).is_some() {
        return true;
    }
    if markdown::fence_marker(line).is_some() {
        return true;
    }
    is_bold_label(trimmed)
}

/// Display text for one `[[...]]` span: the alias, else the last path
/// segment without any `#section` or `#^block` suffix.
pub(crate) fn wikilink_display(span: &str) -> String {
    let inner = span
        .strip_prefix("[[")
        .unwrap_or(span)
        .strip_suffix("]]")
        .unwrap_or(span);
    if let Some((_, alias)) = inner.split_once('|') {
        return alias.trim().to_string();
    }
    let target = inner.split('#').next().unwrap_or(inner);
    target
        .rsplit('/')
        .next()
        .unwrap_or(target)
        .trim()
        .to_string()
}

/// Replace every `[[...]]` span with its display text and drop stray
/// brackets, so a quote carrying links never nests inside an alias.
pub(crate) fn flatten_wikilinks(text: &str) -> String {
    replace_wikilinks(text).replace(['[', ']'], "")
}

fn replace_wikilinks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let Some(relative_end) = rest[start + 2..].find("]]") else {
            break;
        };
        let end = start + 2 + relative_end + 2;
        out.push_str(&rest[..start]);
        out.push_str(&wikilink_display(&rest[start..end]));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn is_bold_label(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("**") else {
        return false;
    };
    let Some(end) = rest.find("**") else {
        return false;
    };
    let label = &rest[..end];
    let after = &rest[end + 2..];
    label.ends_with(':') || after.starts_with(':')
}

fn flatten_display(text: &str) -> String {
    let without_wiki = replace_wikilinks(text);
    let without_links = replace_markdown_links(&without_wiki);
    let stripped = without_links
        .replace("**", "")
        .replace("__", "")
        .replace("~~", "")
        .replace("==", "")
        .replace('`', "");
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Shared display projection for one Markdown intention/summary string.
pub(crate) fn display_text(text: &str) -> String {
    flatten_display(text)
}

fn replace_markdown_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative_start) = text[cursor..].find('[') {
        let start = cursor + relative_start;
        if is_escaped(text.as_bytes(), start) {
            out.push_str(&text[cursor..start + 1]);
            cursor = start + 1;
            continue;
        }
        out.push_str(&text[cursor..start]);
        let Some((label_end, link_end)) = markdown_link_span(text, start)
        else {
            // Leave a malformed link and the remainder readable. In
            // particular, do not flatten a nested-looking link inside its
            // unfinished destination.
            out.push_str(&text[start..]);
            return out;
        };
        out.push_str(&text[start + 1..label_end]);
        cursor = link_end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Find a complete inline link beginning at `[`. Labels may contain nested
/// brackets; destinations may contain balanced parentheses and backslash
/// escapes. Returning `None` leaves the original bytes untouched.
fn markdown_link_span(text: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut cursor = start.checked_add(1)?;
    let mut brackets = 0usize;
    let label_end = loop {
        let byte = *bytes.get(cursor)?;
        if byte == b'\\' {
            cursor = (cursor + 2).min(bytes.len());
            continue;
        }
        match byte {
            b'[' => brackets += 1,
            b']' if brackets == 0 => break cursor,
            b']' => brackets -= 1,
            _ => {}
        }
        cursor += 1;
    };
    let open = label_end + 1;
    if bytes.get(open) != Some(&b'(') {
        return None;
    }

    cursor = open + 1;
    let mut parentheses = 1usize;
    while let Some(&byte) = bytes.get(cursor) {
        if byte == b'\\' {
            cursor = (cursor + 2).min(bytes.len());
            continue;
        }
        match byte {
            b'(' => parentheses += 1,
            b')' => {
                parentheses -= 1;
                if parentheses == 0 {
                    return Some((label_end, cursor + 1));
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn is_escaped(bytes: &[u8], index: usize) -> bool {
    let mut preceding = index;
    while preceding > 0 && bytes[preceding - 1] == b'\\' {
        preceding -= 1;
    }
    (index - preceding) % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(contents: &str) -> Option<(String, String)> {
        parse(contents).map(|headline| (headline.text, headline.display))
    }

    #[test]
    fn both_bold_forms_and_label_case() {
        assert_eq!(
            parsed("# Day\n\n**Headline:** Shipped it\n"),
            Some(("Shipped it".into(), "Shipped it".into()))
        );
        assert_eq!(
            parsed("# Day\n\n**Headline**: Shipped it\n"),
            Some(("Shipped it".into(), "Shipped it".into()))
        );
        assert_eq!(
            parsed("# Day\n\n**headline:** Shipped it\n"),
            Some(("Shipped it".into(), "Shipped it".into()))
        );
        assert_eq!(
            parsed("# Day\n\n**HEADLINE**: Shipped it\n"),
            Some(("Shipped it".into(), "Shipped it".into()))
        );
    }

    #[test]
    fn empty_slot_is_none() {
        assert_eq!(parsed("# Day\n\n**Headline:**\n\n- [ ] #task\n"), None);
        assert_eq!(parsed("# Day\n\n**Headline:**   \n"), None);
        assert_eq!(parsed("# Day\n\nNo headline here\n"), None);
    }

    #[test]
    fn soft_wrap_joins_and_stops_at_summary() {
        assert_eq!(
            parsed(concat!(
                "# Day\n\n",
                "**Headline:** Converge take-home submitted and the zettel\n",
                "album's first draft published\n\n",
                "**Summary:** What actually happened.\n",
            )),
            Some((
                "Converge take-home submitted and the zettel album's first draft published"
                    .into(),
                "Converge take-home submitted and the zettel album's first draft published"
                    .into()
            ))
        );
        assert_eq!(
            parsed(concat!(
                "# Day\n\n",
                "**Headline:** One sentence\n",
                "**Summary:** Later.\n",
            )),
            Some(("One sentence".into(), "One sentence".into()))
        );
    }

    #[test]
    fn first_non_empty_headline_wins() {
        assert_eq!(
            parsed(concat!(
                "# Day\n\n",
                "**Headline:**\n\n",
                "**Headline:** The real one\n\n",
                "**Headline:** Ignored extra\n",
            )),
            Some(("The real one".into(), "The real one".into()))
        );
    }

    #[test]
    fn frontmatter_fence_and_section_headlines_are_ignored() {
        assert_eq!(
            parsed(concat!(
                "---\n",
                "**Headline:** secret\n",
                "parent: \"[[2026/2026-W47]]\"\n",
                "---\n",
                "# Day\n",
            )),
            None
        );
        assert_eq!(
            parsed(concat!(
                "# Day\n\n",
                "```\n",
                "**Headline:** fenced\n",
                "```\n",
                "## Pomodoros\n",
            )),
            None
        );
        assert_eq!(
            parsed(concat!(
                "# Day\n\n",
                "## Pomodoros\n",
                "**Headline:** too late\n",
            )),
            None
        );
    }

    #[test]
    fn crlf_input() {
        assert_eq!(
            parsed("# Day\r\n\r\n**Headline:** Hello\r\n\r\n## Pomodoros\r\n"),
            Some(("Hello".into(), "Hello".into()))
        );
    }

    #[test]
    fn display_flattens_each_link_and_marker_kind() {
        let parsed = parse(concat!(
            "# Day\n\n",
            "**Headline:** Ship [[bob#^x|the alias]] and [[areas/work]] plus ",
            "[docs](https://example.com) with **bold**, __under__, ~~strike~~, ",
            "==mark==, and `code`.\n",
        ))
        .expect("headline");
        assert_eq!(
            parsed.text,
            "Ship [[bob#^x|the alias]] and [[areas/work]] plus [docs](https://example.com) with **bold**, __under__, ~~strike~~, ==mark==, and `code`."
        );
        assert_eq!(
            parsed.display,
            "Ship the alias and work plus docs with bold, under, strike, mark, and code."
        );
    }

    #[test]
    fn paragraph_ends_at_list_heading_and_fence() {
        assert_eq!(
            parsed("# Day\n\n**Headline:** Keep\n- [ ] #task gtd\n"),
            Some(("Keep".into(), "Keep".into()))
        );
        assert_eq!(
            parsed("# Day\n\n**Headline:** Keep\n## Pomodoros\n"),
            Some(("Keep".into(), "Keep".into()))
        );
        assert_eq!(
            parsed("# Day\n\n**Headline:** Keep\n```bob-plan\n```\n"),
            Some(("Keep".into(), "Keep".into()))
        );
    }

    #[test]
    fn paragraph_ends_at_all_supported_list_markers_but_keeps_prose() {
        for marker in [
            "+ unrelated list",
            "1) unrelated list",
            "2. unrelated list",
            "- unrelated list",
            "* unrelated list",
        ] {
            let input = format!("# Day\n\n**Headline:** Ship\n{marker}\n");
            assert_eq!(
                parsed(&input),
                Some(("Ship".into(), "Ship".into())),
                "{marker}"
            );
        }
        assert_eq!(
            parsed("# Day\n\n**Headline:** Ship + deliver\nand 1) be ready\n"),
            Some((
                "Ship + deliver and 1) be ready".into(),
                "Ship + deliver and 1) be ready".into(),
            ))
        );
        assert!(!ends_headline_paragraph("+attached prose"));
        assert!(!ends_headline_paragraph("1)item prose"));
    }

    #[test]
    fn markdown_link_destinations_balance_parentheses_and_escapes() {
        let parsed = parse(concat!(
            "# Day\n\n",
            "**Headline:** Read [the guide](https://example.test/a_(b)) and ",
            "[the escaped guide](https://example.test/a_\\(b\\)) today.\n",
        ))
        .expect("headline");
        assert_eq!(
            parsed.text,
            "Read [the guide](https://example.test/a_(b)) and [the escaped guide](https://example.test/a_\\(b\\)) today."
        );
        assert_eq!(
            parsed.display,
            "Read the guide and the escaped guide today."
        );
    }

    #[test]
    fn malformed_markdown_links_remain_readable() {
        assert_eq!(
            parsed("# Day\n\n**Headline:** See [broken](https://example.test/a_(b) and [open\n"),
            Some((
                "See [broken](https://example.test/a_(b) and [open".into(),
                "See [broken](https://example.test/a_(b) and [open".into(),
            ))
        );
        assert_eq!(
            parsed("# Day\n\n**Headline:** A [label] without a destination\n"),
            Some((
                "A [label] without a destination".into(),
                "A [label] without a destination".into()
            ))
        );
    }
}

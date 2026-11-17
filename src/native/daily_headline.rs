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
pub(crate) fn parse(contents: &str) -> Option<DailyHeadline> {
    let lines: Vec<&str> = contents.lines().collect();
    let start = markdown::strictly_closed_frontmatter_end(&lines)
        .map(|end| end + 1)
        .unwrap_or(0);
    let fenced = markdown::fenced_lines(&lines, start..lines.len());
    let mut index = start;
    while index < lines.len() {
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        let line = lines[index];
        if is_preamble_end(line) {
            break;
        }
        if let Some(rest) = headline_label_rest(line) {
            let mut words: Vec<&str> = rest.split_whitespace().collect();
            let mut next = index + 1;
            while next < lines.len() {
                if fenced.contains(&next)
                    || ends_headline_paragraph(lines[next])
                {
                    break;
                }
                words.extend(lines[next].split_whitespace());
                next += 1;
            }
            if !words.is_empty() {
                let text = words.join(" ");
                let display = flatten_display(&text);
                return Some(DailyHeadline { text, display });
            }
            index = next.max(index + 1);
            continue;
        }
        index += 1;
    }
    None
}

/// Whether `line` ends a Headline paragraph: a blank line, a list item,
/// a heading, a fence, or another bold `**Label:**` / `**Label**:`.
pub(crate) fn ends_headline_paragraph(line: &str) -> bool {
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
    out.replace(['[', ']'], "")
}

fn is_preamble_end(line: &str) -> bool {
    markdown::atx_heading(line).is_some_and(|(level, _)| level >= 2)
}

/// Remainder after a `**Headline:**` or `**Headline**:` label. The label
/// matches ASCII-case-insensitively.
fn headline_label_rest(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("**")?;
    let close = rest.find("**")?;
    let label = &rest[..close];
    let after = &rest[close + 2..];
    if label.eq_ignore_ascii_case("Headline:") {
        return Some(after);
    }
    if label.eq_ignore_ascii_case("Headline") {
        return after.strip_prefix(':');
    }
    None
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

fn replace_markdown_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else {
            out.push('[');
            out.push_str(after);
            return out;
        };
        let label = &after[..end];
        let next = &after[end + 1..];
        if let Some(url_rest) = next.strip_prefix('(')
            && let Some(close) = url_rest.find(')')
        {
            out.push_str(label);
            rest = &url_rest[close + 1..];
            continue;
        }
        out.push('[');
        rest = after;
    }
    out.push_str(rest);
    out
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
}

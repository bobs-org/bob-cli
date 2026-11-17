//! Shared `## Notes` empty-section rule for record notes.
//!
//! [`strip_empty_notes_sections`] removes every empty level-2 `## Notes`
//! section from a record note, preserving frontmatter bytes, BOM, line
//! endings, and every other line.

#[derive(Debug, Clone)]
struct Line {
    content: String,
    ending: String,
}

fn split_preserving_ends(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            let end = index + 1;
            let raw = &text[start..end];
            let (content, ending) = if raw.ends_with("\r\n") {
                (raw[..raw.len() - 2].to_owned(), "\r\n".to_owned())
            } else {
                (raw[..raw.len() - 1].to_owned(), "\n".to_owned())
            };
            lines.push(Line { content, ending });
            start = end;
        }
        index += 1;
    }
    if start < text.len() {
        lines.push(Line {
            content: text[start..].to_owned(),
            ending: String::new(),
        });
    } else if text.is_empty() {
        lines.push(Line {
            content: String::new(),
            ending: String::new(),
        });
    }
    lines
}

#[derive(Debug, Clone)]
struct Heading {
    line: usize,
    level: usize,
    title: String,
}

fn parse_headings(lines: &[Line]) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut fence: Option<crate::native::markdown::MarkdownFence> = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(open) = fence {
            if crate::native::markdown::closes_fence(&line.content, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = crate::native::markdown::fence_marker(&line.content)
        {
            fence = Some(open);
            continue;
        }
        if let Some((level, title)) =
            crate::native::markdown::atx_heading(&line.content)
        {
            headings.push(Heading {
                line: index,
                level,
                title: title.to_owned(),
            });
        }
    }
    headings
}

fn is_notes_heading(heading: &Heading) -> bool {
    heading.level == 2 && heading.title.trim() == "Notes"
}

/// Remove every empty `## Notes` section from a record note.
///
/// Returns `None` when nothing changes or when the note has no closed
/// frontmatter block. An empty section holds only whitespace-only lines
/// between its heading and the next level-1/2 heading (or EOF). Any other
/// content — bullets, prose, a `###` subheading, `%%` or HTML comments, a
/// fence, or a block id — keeps the section.
pub(crate) fn strip_empty_notes_sections(original: &str) -> Option<String> {
    let lines = split_preserving_ends(original);
    if lines.is_empty() {
        return None;
    }
    let first_content = lines[0]
        .content
        .strip_prefix('\u{feff}')
        .unwrap_or(lines[0].content.as_str());
    if first_content != "---" {
        return None;
    }
    let mut closing: Option<usize> = None;
    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.content == "---" {
            closing = Some(index);
            break;
        }
    }
    let closing_line = closing?;
    let body_start = closing_line + 1;
    if body_start > lines.len() {
        return None;
    }
    let body = &lines[body_start..];
    let headings = parse_headings(body);
    let notes: Vec<&Heading> = headings
        .iter()
        .filter(|heading| is_notes_heading(heading))
        .collect();
    if notes.is_empty() {
        return None;
    }
    // Empty sections in body coordinates: (heading_line, section_end).
    let mut empty: Vec<(usize, usize)> = Vec::new();
    for note in notes {
        let end = headings
            .iter()
            .filter(|heading| heading.line > note.line && heading.level <= 2)
            .map(|heading| heading.line)
            .min()
            .unwrap_or(body.len());
        let mut has_content = false;
        for line in &body[note.line + 1..end] {
            if !line.content.trim().is_empty() {
                has_content = true;
                break;
            }
        }
        if !has_content {
            empty.push((note.line, end));
        }
    }
    if empty.is_empty() {
        return None;
    }
    // Remove from the end so earlier indices stay valid. Each removal
    // deletes the heading plus the blank lines that follow it in the
    // section; blanks before the heading stay so the next heading keeps
    // its existing separation.
    let mut body_out: Vec<Line> = body.to_vec();
    // Whether the original last empty section ran to EOF determines the
    // trailing-blank cleanup below.
    let last_ran_to_eof = empty.iter().any(|(_, end)| *end == body.len());
    // Sort descending by heading line for stable removal.
    empty.sort_by_key(|(line, _)| *line);
    for (heading_line, section_end) in empty.iter().rev() {
        // Later removals sit at or after section_end, so indices below
        // section_end are unchanged and the blank run is still aligned.
        let mut remove_end = heading_line + 1;
        while remove_end < *section_end
            && remove_end < body_out.len()
            && body_out[remove_end].content.trim().is_empty()
        {
            remove_end += 1;
        }
        body_out.drain(*heading_line..remove_end);
    }
    if last_ran_to_eof {
        while body_out
            .last()
            .is_some_and(|line| line.content.trim().is_empty())
        {
            body_out.pop();
        }
    }
    let mut out = String::new();
    for line in &lines[..body_start] {
        out.push_str(&line.content);
        out.push_str(&line.ending);
    }
    for line in &body_out {
        out.push_str(&line.content);
        out.push_str(&line.ending);
    }
    if out == original {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::strip_empty_notes_sections;

    fn note(body: &str) -> String {
        format!("---\ntype: \"[[record]]\"\ntitle: \"T\"\n---\n{body}")
    }

    #[test]
    fn strips_eof_section_and_keeps_h1_newline() {
        let before = note("# T\n\n## Notes\n");
        let after = strip_empty_notes_sections(&before).expect("strips");
        assert_eq!(after, note("# T\n"));
    }

    #[test]
    fn strips_section_before_source() {
        let before =
            note("# B-Unos\n\n## Notes\n\n## Source\n\n```text\nrow\n```\n");
        let after = strip_empty_notes_sections(&before).expect("strips");
        assert_eq!(after, note("# B-Unos\n\n## Source\n\n```text\nrow\n```\n"));
    }

    #[test]
    fn strips_heading_without_final_newline() {
        let before = note("# T\n\n## Notes");
        let after = strip_empty_notes_sections(&before).expect("strips");
        assert_eq!(after, note("# T\n"));
    }

    #[test]
    fn keeps_nonempty_section() {
        let before = note("# T\n\n## Notes\n\n- a\n");
        assert_eq!(strip_empty_notes_sections(&before), None);
    }

    #[test]
    fn keeps_subheading_comment_fence_and_block_id() {
        for body in [
            "# T\n\n## Notes\n\n### Sub\n",
            "# T\n\n## Notes\n\n%% comment\n",
            "# T\n\n## Notes\n\n<!-- html -->\n",
            "# T\n\n## Notes\n\n```text\nrow\n```\n",
            "# T\n\n## Notes\n\n- a ^abc123\n",
        ] {
            let before = note(body);
            assert_eq!(
                strip_empty_notes_sections(&before),
                None,
                "keeps {body:?}"
            );
        }
    }

    #[test]
    fn removes_multiple_empty_sections() {
        let before = note(
            "# T\n\n## Notes\n\n## Notes\n\n- a\n\n## Notes\n\n## Other\n",
        );
        let after = strip_empty_notes_sections(&before).expect("strips");
        assert_eq!(after, note("# T\n\n## Notes\n\n- a\n\n## Other\n"));
    }

    #[test]
    fn ignores_fenced_notes_heading() {
        let before = note("# T\n\n```text\n## Notes\n```\n\n## Notes\n\n- a\n");
        assert_eq!(strip_empty_notes_sections(&before), None);
    }

    #[test]
    fn preserves_bom_crlf_and_refuses_without_frontmatter() {
        let before = "\u{feff}---\r\ntype: \"[[record]]\"\r\n---\r\n\r\n# T\r\n\r\n## Notes\r\n";
        let after = strip_empty_notes_sections(before).expect("strips");
        assert!(after.starts_with('\u{feff}'));
        assert!(after.contains("\r\n"));
        assert_eq!(
            after,
            "\u{feff}---\r\ntype: \"[[record]]\"\r\n---\r\n\r\n# T\r\n"
        );
        assert_eq!(strip_empty_notes_sections("# T\n\n## Notes\n"), None);
        assert_eq!(strip_empty_notes_sections("---\ntype: x\n"), None);
    }

    #[test]
    fn roundtrip_and_idempotence() {
        let bodies = [
            "# T\n",
            "# T\n\n## Notes\n\n- a\n",
            "# T\n\n## Must Order\n\n- Dragon roll\n",
            "# T\n\n## Notes\n\n- a\n\n## Must Order\n\n- Dragon roll\n",
        ];
        for body in bodies {
            let full = note(body);
            assert_eq!(
                strip_empty_notes_sections(&full),
                None,
                "new render stays clean: {body:?}"
            );
        }
        let before = note("# T\n\n## Notes\n");
        let once = strip_empty_notes_sections(&before).expect("first strip");
        assert_eq!(strip_empty_notes_sections(&once), None);
    }
}

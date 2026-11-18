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

/// Outcome of the redundant-H1 pass over one record note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct H1Strip {
    /// The rewritten note when the redundant H1 was removed.
    pub(crate) after: Option<String>,
    /// Why an H1 stays: the first H1 differs from the title, is not the
    /// first body content, or a later H1 remains after the removal.
    pub(crate) kept: Option<&'static str>,
}

/// Remove a redundant H1 from a record note.
///
/// A redundant H1 is the first non-blank body line outside fenced code, a
/// level-1 ATX heading whose text — trimmed of surrounding whitespace and
/// closing `#`s — equals the frontmatter `title` exactly. Removal deletes
/// that line and the blank lines that follow it; when the body is then
/// blank-only, the note ends right after the closing `---`. An H1 that
/// differs from the title, an H1 that is not the first body content, and
/// any later H1 are kept. Lines inside fenced code never count as headings
/// or content. Frontmatter, CRLF, and BOM bytes are preserved.
///
/// When `album_stem` is present, walk lines and nav-like `↑` lines for that
/// album are skipped when looking for the first body content, so an H1
/// directly below the walk line is still redundant and is stripped, leaving
/// `---␤␤<walk>␤␤## Notes…`. Tidy never inserts, rewrites, or removes walk
/// lines; the blank-body collapse never fires while a walk line exists.
pub(crate) fn strip_redundant_h1(original: &str, title: &str) -> H1Strip {
    strip_redundant_h1_for_album(original, title, None)
}

/// Album-aware redundant-H1 pass for tidy. See [`strip_redundant_h1`].
pub(crate) fn strip_redundant_h1_for_album(
    original: &str,
    title: &str,
    album_stem: Option<&str>,
) -> H1Strip {
    let absent = H1Strip {
        after: None,
        kept: None,
    };
    let lines = split_preserving_ends(original);
    if lines.is_empty() {
        return absent;
    }
    let first_content = lines[0]
        .content
        .strip_prefix('\u{feff}')
        .unwrap_or(lines[0].content.as_str());
    if first_content != "---" {
        return absent;
    }
    let mut closing: Option<usize> = None;
    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.content == "---" {
            closing = Some(index);
            break;
        }
    }
    let Some(closing_line) = closing else {
        return absent;
    };
    let body_start = closing_line + 1;
    let mut fence = None;
    let mut first_content_line: Option<usize> = None;
    let mut first_h1: Option<(usize, String)> = None;
    for (index, line) in lines.iter().enumerate().skip(body_start) {
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
        if line.content.trim().is_empty() {
            continue;
        }
        if let Some(stem) = album_stem
            && crate::native::albums::walk::is_walk_or_nav_line(
                &line.content,
                stem,
            )
        {
            continue;
        }
        if first_content_line.is_none() {
            first_content_line = Some(index);
        }
        if first_h1.is_none()
            && let Some((level, text)) =
                crate::native::markdown::atx_heading(&line.content)
            && level == 1
        {
            first_h1 = Some((index, text.to_owned()));
        }
    }
    let Some((h1_line, h1_text)) = first_h1 else {
        return absent;
    };
    if Some(h1_line) != first_content_line {
        return H1Strip {
            after: None,
            kept: Some("the H1 is not the first body content"),
        };
    }
    if h1_text != title {
        return H1Strip {
            after: None,
            kept: Some("the H1 differs from the frontmatter title"),
        };
    }
    let mut out: Vec<Line> = lines[..body_start].to_vec();
    let mut cursor = h1_line + 1;
    while cursor < lines.len() && lines[cursor].content.trim().is_empty() {
        cursor += 1;
    }
    out.extend_from_slice(&lines[body_start..h1_line]);
    out.extend_from_slice(&lines[cursor..]);
    // A body left blank-only ends right after the closing `---`.
    let body_blank = out[body_start..]
        .iter()
        .all(|line| line.content.trim().is_empty());
    if body_blank {
        out.truncate(body_start);
    }
    let mut after = String::new();
    for line in &out {
        after.push_str(&line.content);
        after.push_str(&line.ending);
    }
    if after == original {
        return absent;
    }
    // Any later H1 stays and counts as kept.
    let mut rest_fence = None;
    let mut later_h1 = false;
    for line in out[body_start..].iter() {
        if let Some(open) = rest_fence {
            if crate::native::markdown::closes_fence(&line.content, open) {
                rest_fence = None;
            }
            continue;
        }
        if let Some(open) = crate::native::markdown::fence_marker(&line.content)
        {
            rest_fence = Some(open);
            continue;
        }
        if crate::native::markdown::atx_heading(&line.content)
            .is_some_and(|(level, _)| level == 1)
        {
            later_h1 = true;
            break;
        }
    }
    H1Strip {
        after: Some(after),
        kept: later_h1.then_some("a later H1 is kept"),
    }
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
    use super::{
        strip_empty_notes_sections, strip_redundant_h1,
        strip_redundant_h1_for_album, H1Strip,
    };

    fn note(body: &str) -> String {
        format!("---\ntype: \"[[record]]\"\ntitle: \"T\"\n---\n{body}")
    }

    fn titled(title: &str, body: &str) -> String {
        format!("---\ntype: \"[[record]]\"\ntitle: {title:?}\n---\n{body}")
    }

    #[test]
    fn redundant_h1_removed_with_following_blanks() {
        let before = note("# T\n\n## Notes\n\n- a\n");
        let strip = strip_redundant_h1(&before, "T");
        assert_eq!(strip.kept, None);
        assert_eq!(
            strip.after.as_deref(),
            Some(note("## Notes\n\n- a\n").as_str())
        );
    }

    #[test]
    fn redundant_h1_with_closing_hashes_removed() {
        let before = note("# T ##\n\n- a\n");
        let strip = strip_redundant_h1(&before, "T");
        assert_eq!(strip.after.as_deref(), Some(note("- a\n").as_str()));
    }

    #[test]
    fn h1_only_body_ends_at_frontmatter() {
        let before = note("# T\n");
        let strip = strip_redundant_h1(&before, "T");
        assert_eq!(strip.kept, None);
        assert_eq!(
            strip.after.as_deref(),
            Some("---\ntype: \"[[record]]\"\ntitle: \"T\"\n---\n")
        );
    }

    #[test]
    fn differing_h1_is_kept() {
        let before = note("# Other\n\n- a\n");
        assert_eq!(
            strip_redundant_h1(&before, "T"),
            H1Strip {
                after: None,
                kept: Some("the H1 differs from the frontmatter title"),
            }
        );
    }

    #[test]
    fn h1_after_prose_is_kept() {
        let before = note("Intro.\n\n# T\n");
        assert_eq!(
            strip_redundant_h1(&before, "T"),
            H1Strip {
                after: None,
                kept: Some("the H1 is not the first body content"),
            }
        );
    }

    #[test]
    fn fenced_h1_is_ignored() {
        let before = note("```text\n# T\n```\n\n- a\n");
        assert_eq!(
            strip_redundant_h1(&before, "T"),
            H1Strip {
                after: None,
                kept: None,
            }
        );
    }

    #[test]
    fn later_h1_is_kept_after_removal() {
        let before = note("# T\n\n# Other\n");
        let strip = strip_redundant_h1(&before, "T");
        assert_eq!(strip.kept, Some("a later H1 is kept"));
        assert_eq!(strip.after.as_deref(), Some(note("# Other\n").as_str()));
    }

    #[test]
    fn h1_less_bodies_are_absent() {
        for body in ["", "\n", "## Notes\n\n- a\n", "- stray\n"] {
            let before = note(body);
            assert_eq!(
                strip_redundant_h1(&before, "T"),
                H1Strip {
                    after: None,
                    kept: None,
                },
                "absent for {body:?}"
            );
        }
        assert_eq!(
            strip_redundant_h1("# T\n", "T"),
            H1Strip {
                after: None,
                kept: None,
            }
        );
    }

    #[test]
    fn redundant_h1_preserves_crlf_and_bom() {
        let before =
            "\u{feff}---\r\ntype: \"[[record]]\"\r\ntitle: \"T\"\r\n---\r\n\r\n# T\r\n\r\n## Notes\r\n\r\n- a\r\n";
        let strip = strip_redundant_h1(before, "T");
        let after = strip.after.expect("removed");
        assert!(after.starts_with('\u{feff}'));
        assert!(!after.contains("# T\r\n"));
        assert!(after.contains("## Notes\r\n"));
        assert!(
            after
                .match_indices('\n')
                .all(|(index, _)| index > 0
                    && after.as_bytes()[index - 1] == b'\r'),
            "every newline stays CRLF:\n{}",
            after.replace('\r', "\\r")
        );
    }

    #[test]
    fn quoted_title_matches_its_h1() {
        let before = titled("Ma \"Pa\" Deli", "# Ma \"Pa\" Deli\n");
        let strip = strip_redundant_h1(&before, "Ma \"Pa\" Deli");
        assert!(strip.after.is_some(), "{strip:?}");
        assert!(!strip.after.unwrap().contains("# Ma"));
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

    #[test]
    fn h1_below_walk_line_is_still_redundant() {
        let walk = "[[eat|↑ Eat]]";
        let before = note(&format!("{walk}\n\n# T\n\n## Notes\n\n- a\n"));
        let strip = strip_redundant_h1_for_album(&before, "T", Some("eat"));
        assert_eq!(strip.kept, None);
        assert_eq!(
            strip.after.as_deref(),
            Some(note(&format!("{walk}\n\n## Notes\n\n- a\n")).as_str())
        );
        // Without the album stem the same H1 is kept, since the walk line
        // counts as the first body content.
        assert_eq!(
            strip_redundant_h1(&before, "T"),
            H1Strip {
                after: None,
                kept: Some("the H1 is not the first body content"),
            }
        );
    }

    #[test]
    fn prose_before_walk_keeps_h1() {
        let walk = "[[eat|↑ Eat]]";
        let before = note(&format!("Intro.\n\n{walk}\n\n# T\n"));
        assert_eq!(
            strip_redundant_h1_for_album(&before, "T", Some("eat")),
            H1Strip {
                after: None,
                kept: Some("the H1 is not the first body content"),
            }
        );
    }

    #[test]
    fn walk_line_crlf_and_bom_survive_h1_strip() {
        let walk = "[[eat|↑ Eat]]";
        let before = format!(
            "\u{feff}---\r\ntype: \"[[record]]\"\r\ntitle: \"T\"\r\n---\r\n\r\n{walk}\r\n\r\n# T\r\n\r\n## Notes\r\n"
        );
        let strip = strip_redundant_h1_for_album(&before, "T", Some("eat"));
        let after = strip.after.expect("removed");
        assert!(after.starts_with('\u{feff}'));
        assert!(after.contains(walk));
        assert!(!after.contains("# T\r\n"));
        assert!(
            after
                .match_indices('\n')
                .all(|(index, _)| index > 0
                    && after.as_bytes()[index - 1] == b'\r'),
            "every newline stays CRLF:\n{}",
            after.replace('\r', "\\r")
        );
    }
}

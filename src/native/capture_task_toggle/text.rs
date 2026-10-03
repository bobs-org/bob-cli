use crate::native::capture::{leading_spaces_or_tabs_len, LineSpan};

pub(super) fn insert_line_before(
    content: &str,
    lines: &[LineSpan<'_>],
    at_index: usize,
    new_line: &str,
) -> String {
    if at_index >= lines.len() {
        return insert_line_after(content, lines, lines.len() - 1, new_line);
    }
    let insert_offset = line_start_offset(lines, at_index);
    let ending = if line_has_crlf(content, lines, at_index) {
        "\r\n"
    } else {
        "\n"
    };
    format!(
        "{}{new_line}{ending}{}",
        &content[..insert_offset],
        &content[insert_offset..]
    )
}

pub(super) fn extract_line_range<'a>(
    contents: &'a str,
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> &'a str {
    let start = line_start_offset(lines, start_line);
    &contents[start..lines[end_line].end]
}

pub(super) fn remove_line_range(
    contents: &str,
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> String {
    let start = line_start_offset(lines, start_line);
    format!("{}{}", &contents[..start], &contents[lines[end_line].end..])
}

pub(super) fn reindent_subtree(
    subtree: &str,
    from_indent: &str,
    to_indent: &str,
) -> String {
    let mut result = String::new();
    for (line, ending) in physical_line_pieces(subtree) {
        if line.trim().is_empty() {
            result.push_str(line);
        } else if let Some(suffix) = line.strip_prefix(from_indent) {
            result.push_str(to_indent);
            result.push_str(suffix);
        } else {
            result.push_str(line);
        }
        result.push_str(ending);
    }
    result
}

pub(super) fn logical_lines(text: &str) -> Vec<String> {
    physical_line_pieces(text)
        .into_iter()
        .map(|(line, _)| line.to_string())
        .collect()
}

fn physical_line_pieces(text: &str) -> Vec<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            let ending_start = if index > start && bytes[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            lines.push((
                &text[start..ending_start],
                &text[ending_start..index + 1],
            ));
            index += 1;
            start = index;
        } else {
            index += 1;
        }
    }
    if start < text.len() {
        lines.push((&text[start..], ""));
    }
    lines
}

pub(super) fn insert_lines_after(
    content: &str,
    lines: &[LineSpan<'_>],
    after_index: usize,
    new_lines: &[String],
) -> String {
    if new_lines.is_empty() {
        return content.to_string();
    }
    let insert_offset = lines[after_index].end;
    let ending = if line_has_crlf(content, lines, after_index) {
        "\r\n"
    } else {
        "\n"
    };
    let at_eof_no_nl =
        insert_offset == content.len() && !content.ends_with('\n');
    let mut block = String::new();
    for (index, line) in new_lines.iter().enumerate() {
        let last = index + 1 == new_lines.len();
        block.push_str(line);
        if at_eof_no_nl {
            if !last {
                block.push_str(ending);
            }
        } else {
            block.push_str(ending);
        }
    }
    if at_eof_no_nl {
        format!("{content}{ending}{block}")
    } else {
        format!(
            "{}{block}{}",
            &content[..insert_offset],
            &content[insert_offset..]
        )
    }
}

// ---------------------------------------------------------------------------
// Shared line utilities.
// ---------------------------------------------------------------------------

/// The exclusive-of-nothing-past-it last line index of `parent_index`'s
/// child block: every later line that is blank or indented deeper than the
/// parent, stopping at the first nonblank line indented at or shallower
/// than the parent. Mirrors `findChildBlockEndLine`; used for both a task's
/// own child block (to bound the Schedule Log marker search) and a Schedule
/// Log marker's or Pomodoro entry's own children (both always top-level, so
/// this naturally stops at the Pomodoros section boundary too).
pub(crate) fn child_block_end_line(
    lines: &[LineSpan<'_>],
    parent_index: usize,
) -> usize {
    let parent_indent = leading_spaces_or_tabs_len(lines[parent_index].text);
    let mut end_index = parent_index;
    let mut index = parent_index + 1;
    while index < lines.len() {
        let text = lines[index].text;
        if text.trim().is_empty() {
            index += 1;
            continue;
        }
        if leading_spaces_or_tabs_len(text) > parent_indent {
            end_index = index;
            index += 1;
            continue;
        }
        break;
    }
    end_index
}

pub(super) fn line_start_offset(lines: &[LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines[index - 1].end
    }
}

pub(super) fn line_index_at_offset(
    lines: &[LineSpan<'_>],
    offset: usize,
) -> usize {
    lines.iter().take_while(|line| line.end <= offset).count()
}

/// Replace the logical content of line `index` with `new_text`, preserving
/// whatever line-ending bytes (`\n`, `\r\n`, or none at EOF) followed it.
pub(super) fn replace_line(
    contents: &str,
    lines: &[LineSpan<'_>],
    index: usize,
    new_text: &str,
) -> String {
    let start = line_start_offset(lines, index);
    let text_end = start + lines[index].text.len();
    format!("{}{new_text}{}", &contents[..start], &contents[text_end..])
}

fn line_has_crlf(content: &str, lines: &[LineSpan<'_>], index: usize) -> bool {
    let start = line_start_offset(lines, index);
    content[start..lines[index].end].ends_with("\r\n")
}

/// Insert `new_line` as a new physical line immediately after line
/// `after_index`, matching that line's `\n`/`\r\n` ending. Handles the edge
/// case where `after_index` is the last line and the file has no trailing
/// newline at all (a leading separator is needed then; every other case --
/// including `after_index` being the last line when the file *does* end
/// with a newline -- needs no special-casing, since `content[insert..]` is
/// already empty there).
pub(super) fn insert_line_after(
    content: &str,
    lines: &[LineSpan<'_>],
    after_index: usize,
    new_line: &str,
) -> String {
    let insert_offset = lines[after_index].end;
    let ending = if line_has_crlf(content, lines, after_index) {
        "\r\n"
    } else {
        "\n"
    };
    if insert_offset == content.len() && !content.ends_with('\n') {
        format!("{content}{ending}{new_line}")
    } else {
        format!(
            "{}{new_line}{ending}{}",
            &content[..insert_offset],
            &content[insert_offset..]
        )
    }
}

// ---------------------------------------------------------------------------
// Pomodoro child indentation and link insertion.
// ---------------------------------------------------------------------------

/// An existing unordered (`-`/`*`/`+`) child bullet's indentation. Unlike
/// `list_item_body`'s marker matching, ordered-list children are not
/// candidates, matching `unorderedChildIndentation`'s comment: this mirrors
/// `bob capture`'s own insertion-target rule.
fn unordered_child_indentation(line: &str) -> Option<&str> {
    let indent_len = leading_spaces_or_tabs_len(line);
    if indent_len == 0 {
        return None;
    }
    let indentation = &line[..indent_len];
    let after_indent = &line.as_bytes()[indent_len..];
    let is_bullet = matches!(after_indent.first(), Some(b'-' | b'*' | b'+'))
        && matches!(after_indent.get(1), Some(b' ' | b'\t'));
    is_bullet.then_some(indentation)
}

/// The indentation for a new sub-bullet under the selected Pomodoro entry:
/// reuse an existing direct child's indentation when the entry already has
/// one, else the section's own established child indentation anywhere else
/// in the Pomodoros section, else the canonical tab fallback. Mirrors
/// `findPomodoroChildIndentation` exactly.
pub(super) fn pomodoro_child_indentation(
    lines: &[LineSpan<'_>],
    entry_index: usize,
    entry_end_index: usize,
    section_start: usize,
    section_end: usize,
) -> String {
    for line in &lines[entry_index + 1..=entry_end_index] {
        if let Some(indentation) = unordered_child_indentation(line.text) {
            return indentation.to_string();
        }
    }
    for line in &lines[section_start..section_end] {
        if let Some(indentation) = unordered_child_indentation(line.text) {
            return indentation.to_string();
        }
    }
    "\t".to_string()
}

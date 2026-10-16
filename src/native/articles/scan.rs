//! Classify each source line into one region.

use crate::native::markdown::{
    atx_heading, closes_fence, fence_marker, html_comment_closes,
    html_comment_opens, is_blockquote_line, is_indented_code_line,
    setext_underline, strictly_closed_frontmatter_end, MarkdownFence,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Region {
    Frontmatter,
    Fence,
    UnclosedFence,
    Math,
    PercentComment,
    HtmlComment,
    HtmlBlock,
    Quote,
    ListItem,
    ListContinuation,
    FootnoteContinuation,
    IndentedCode,
    Table,
    Paragraph,
    Blank,
    AtxHeading,
    SetextUnderline,
    ThematicBreak,
}

impl Region {
    pub(crate) fn protects_body(self) -> bool {
        matches!(
            self,
            Self::Frontmatter
                | Self::Fence
                | Self::UnclosedFence
                | Self::Math
                | Self::PercentComment
                | Self::HtmlComment
        )
    }

    pub(crate) fn is_protected_layout(self) -> bool {
        matches!(
            self,
            Self::Frontmatter
                | Self::Fence
                | Self::UnclosedFence
                | Self::Math
                | Self::PercentComment
                | Self::HtmlComment
                | Self::HtmlBlock
        )
    }

    pub(crate) fn is_listy(self) -> bool {
        matches!(
            self,
            Self::ListItem
                | Self::ListContinuation
                | Self::FootnoteContinuation
        )
    }

    pub(crate) fn is_quote(self) -> bool {
        self == Self::Quote
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClassifiedLine<'a> {
    pub(crate) index: usize,
    pub(crate) content: &'a str,
    pub(crate) ending: &'a str,
    pub(crate) region: Region,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Document<'a> {
    pub(crate) bom: bool,
    pub(crate) lines: Vec<ClassifiedLine<'a>>,
    pub(crate) default_ending: &'static str,
    pub(crate) unclosed_fence: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum InlineMaskMode {
    Marker,
    Links,
}

pub(crate) fn classify(text: &str) -> Document<'_> {
    let (bom, rest) = match text.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let physical = split_physical_lines(rest);
    let default_ending = physical
        .iter()
        .find_map(|(_, ending)| match *ending {
            "\r\n" => Some("\r\n"),
            "\n" => Some("\n"),
            _ => None,
        })
        .unwrap_or("\n");
    let contents: Vec<&str> = physical.iter().map(|(c, _)| *c).collect();
    let frontmatter_end = closed_frontmatter_end(&contents);
    let mut lines = Vec::with_capacity(physical.len());
    let mut fence: Option<MarkdownFence> = None;
    let mut fence_open_index: Option<usize> = None;
    let mut math = false;
    let mut percent = false;
    let mut html_comment = false;
    let mut html_block = false;
    let mut in_list = false;
    let mut in_footnote = false;
    let mut last_paragraph: Option<(usize, usize)> = None;

    for (index, (content, ending)) in physical.iter().copied().enumerate() {
        if let Some(end) = frontmatter_end
            && index <= end
        {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Frontmatter,
            });
            continue;
        }

        if fence.is_some() {
            let closing = fence.is_some_and(|open| closes_fence(content, open));
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Fence,
            });
            if closing {
                fence = None;
                fence_open_index = None;
            }
            last_paragraph = None;
            continue;
        }

        if math {
            let closes = math_line(content).is_some();
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Math,
            });
            if closes {
                math = false;
            }
            last_paragraph = None;
            continue;
        }

        if percent {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::PercentComment,
            });
            if content.contains("%%") {
                percent = false;
            }
            last_paragraph = None;
            continue;
        }

        if html_comment {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::HtmlComment,
            });
            if html_comment_closes(content) {
                html_comment = false;
            }
            last_paragraph = None;
            continue;
        }

        if html_block {
            if is_blank(content) {
                html_block = false;
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::Blank,
                });
            } else {
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::HtmlBlock,
                });
            }
            last_paragraph = None;
            continue;
        }

        if is_blank(content) {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Blank,
            });
            last_paragraph = None;
            continue;
        }

        let at_column_0 = !starts_indented(content);
        if (in_list || in_footnote) && !at_column_0 {
            if let Some(marker) = fence_marker(content) {
                fence = Some(marker);
                fence_open_index = Some(index);
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::Fence,
                });
                last_paragraph = None;
                continue;
            }
            let region = if in_footnote {
                Region::FootnoteContinuation
            } else {
                Region::ListContinuation
            };
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region,
            });
            last_paragraph = None;
            continue;
        }
        if at_column_0 {
            in_list = false;
            in_footnote = false;
        }

        if let Some(marker) = fence_marker(content) {
            fence = Some(marker);
            fence_open_index = Some(index);
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Fence,
            });
            last_paragraph = None;
            continue;
        }

        match math_line(content) {
            Some(MathLine::OneLine) => {
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::Math,
                });
                last_paragraph = None;
                continue;
            }
            Some(MathLine::Open) => {
                math = true;
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::Math,
                });
                last_paragraph = None;
                continue;
            }
            None => {}
        }

        match percent_line(content) {
            Some(CommentLine::OneLine) => {
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::PercentComment,
                });
                last_paragraph = None;
                continue;
            }
            Some(CommentLine::Open) => {
                percent = true;
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::PercentComment,
                });
                last_paragraph = None;
                continue;
            }
            None => {}
        }

        if html_comment_opens(content) {
            html_comment = !html_comment_closes(content);
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::HtmlComment,
            });
            last_paragraph = None;
            continue;
        }

        if html_block_opens(content) {
            html_block = true;
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::HtmlBlock,
            });
            last_paragraph = None;
            continue;
        }

        if is_blockquote_line(content) {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Quote,
            });
            last_paragraph = None;
            continue;
        }

        if let Some(level) = setext_underline(content) {
            let after_list_quote_table = lines.iter().rev().find(|line| {
                line.region != Region::Blank
                    && line.region != Region::Frontmatter
            });
            let forbidden = after_list_quote_table.is_some_and(|line| {
                line.region.is_listy()
                    || line.region.is_quote()
                    || line.region == Region::Table
            });
            let continues_list = last_paragraph.is_some_and(|(start, _)| {
                start > 0 && lines[start - 1].region.is_listy()
            });
            if let Some((start, count)) = last_paragraph
                && !forbidden
                && !continues_list
            {
                let _ = (level, start, count);
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::SetextUnderline,
                });
                last_paragraph = None;
                continue;
            }
            if is_thematic_break(content) {
                lines.push(ClassifiedLine {
                    index,
                    content,
                    ending,
                    region: Region::ThematicBreak,
                });
                last_paragraph = None;
                continue;
            }
        }

        if is_thematic_break(content) {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::ThematicBreak,
            });
            last_paragraph = None;
            continue;
        }

        if is_list_item(content) {
            in_list = true;
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::ListItem,
            });
            last_paragraph = None;
            continue;
        }

        if is_footnote_def(content) {
            in_footnote = true;
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Paragraph,
            });
            last_paragraph = None;
            continue;
        }

        if atx_heading(content).is_some() {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::AtxHeading,
            });
            last_paragraph = None;
            continue;
        }

        if table_row(content) {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::Table,
            });
            last_paragraph = None;
            continue;
        }

        if is_indented_code_line(content) && !in_list && !in_footnote {
            lines.push(ClassifiedLine {
                index,
                content,
                ending,
                region: Region::IndentedCode,
            });
            last_paragraph = None;
            continue;
        }

        match last_paragraph {
            Some((start, count)) => {
                last_paragraph = Some((start, count + 1));
            }
            None => last_paragraph = Some((index, 1)),
        }
        lines.push(ClassifiedLine {
            index,
            content,
            ending,
            region: Region::Paragraph,
        });
    }

    let unclosed_fence = if fence.is_some() {
        fence_open_index
    } else {
        None
    };
    if let Some(open) = unclosed_fence {
        for line in lines.iter_mut().skip(open + 1) {
            line.region = Region::UnclosedFence;
        }
    }

    Document {
        bom,
        lines,
        default_ending,
        unclosed_fence,
    }
}

pub(crate) fn split_physical_lines(text: &str) -> Vec<(&str, &str)> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find('\n') {
            Some(index) => {
                if index > 0 && rest.as_bytes()[index - 1] == b'\r' {
                    lines.push((&rest[..index - 1], "\r\n"));
                } else {
                    lines.push((&rest[..index], "\n"));
                }
                rest = &rest[index + 1..];
            }
            None => {
                lines.push((rest, ""));
                break;
            }
        }
    }
    lines
}

fn closed_frontmatter_end(lines: &[&str]) -> Option<usize> {
    strictly_closed_frontmatter_end(lines)
}

/// Byte positions covered by inline code, math, or comments. Shared by
/// article link parsing and marker scans so those rules protect the same text.
pub(crate) fn inline_mask(line: &str, mode: InlineMaskMode) -> Vec<bool> {
    let bytes = line.as_bytes();
    let mut masked = vec![false; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        let end = if bytes[i..].starts_with(b"<!--") {
            find_inline_bytes(&bytes[i + 4..], b"-->")
                .map(|relative| i + 4 + relative + 3)
        } else if bytes[i..].starts_with(b"%%") {
            find_inline_bytes(&bytes[i + 2..], b"%%")
                .map(|relative| i + 2 + relative + 2)
        } else if bytes[i] == b'`' {
            let ticks =
                bytes[i..].iter().take_while(|byte| **byte == b'`').count();
            find_inline_bytes(&bytes[i + ticks..], &vec![b'`'; ticks])
                .map(|relative| i + ticks + relative + ticks)
        } else if bytes[i] == b'$'
            && (matches!(mode, InlineMaskMode::Links) || !is_escaped(bytes, i))
        {
            let ticks = if bytes.get(i + 1) == Some(&b'$') {
                2
            } else {
                1
            };
            match mode {
                InlineMaskMode::Links => {
                    find_inline_bytes(&bytes[i + ticks..], &vec![b'$'; ticks])
                        .map(|relative| i + ticks + relative + ticks)
                }
                InlineMaskMode::Marker => {
                    find_math_close(bytes, i + ticks, ticks, true)
                        .map(|close| close + ticks)
                }
            }
        } else {
            None
        };
        if let Some(end) = end {
            masked[i..end].fill(true);
            i = end;
        } else {
            i += 1;
        }
    }
    masked
}

pub(crate) fn is_escaped(bytes: &[u8], index: usize) -> bool {
    let mut start = index;
    while start > 0 && bytes[start - 1] == b'\\' {
        start -= 1;
    }
    (index - start) % 2 == 1
}

fn find_inline_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_math_close(
    bytes: &[u8],
    mut index: usize,
    ticks: usize,
    escape_delimiters: bool,
) -> Option<usize> {
    while index < bytes.len() {
        if bytes[index] == b'$'
            && (!escape_delimiters || !is_escaped(bytes, index))
        {
            let closing = bytes[index..]
                .iter()
                .take_while(|byte| **byte == b'$')
                .count();
            if (ticks == 1 && closing == 1) || (ticks == 2 && closing >= 2) {
                return Some(index);
            }
            index += closing.max(1);
        } else {
            index += 1;
        }
    }
    None
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn starts_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

enum MathLine {
    Open,
    OneLine,
}

fn math_line(line: &str) -> Option<MathLine> {
    let rest = strip_atx_indent(line)?;
    let rest = rest.trim_end();
    if !rest.starts_with("$$") {
        return None;
    }
    let after = &rest[2..];
    if after.contains("$$") {
        Some(MathLine::OneLine)
    } else {
        Some(MathLine::Open)
    }
}

enum CommentLine {
    Open,
    OneLine,
}

fn percent_line(line: &str) -> Option<CommentLine> {
    let rest = line.trim_start();
    let after = rest.strip_prefix("%%")?;
    if after.contains("%%") {
        Some(CommentLine::OneLine)
    } else {
        Some(CommentLine::Open)
    }
}

fn html_block_opens(line: &str) -> bool {
    let Some(rest) = strip_atx_indent(line) else {
        return false;
    };
    let rest = rest.trim_start();
    let mut chars = rest.chars();
    if chars.next() != Some('<') {
        return false;
    }
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
}

fn is_thematic_break(line: &str) -> bool {
    let Some(rest) = strip_atx_indent(line) else {
        return false;
    };
    let trimmed = rest.trim_end();
    let mut chars = trimmed.chars().filter(|c| !c.is_whitespace());
    let Some(marker) = chars.next() else {
        return false;
    };
    if !matches!(marker, '-' | '*' | '_') {
        return false;
    }
    let mut count = 1;
    for character in chars {
        if character != marker {
            return false;
        }
        count += 1;
    }
    count >= 3
}

fn is_list_item(line: &str) -> bool {
    if is_thematic_break(line) {
        return false;
    }
    let Some(rest) = strip_atx_indent(line) else {
        return false;
    };
    let bytes = rest.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if matches!(bytes[0], b'-' | b'*' | b'+') {
        return bytes.len() == 1 || bytes[1].is_ascii_whitespace();
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if !(1..=9).contains(&digits) {
        return false;
    }
    match bytes.get(digits) {
        Some(b'.' | b')') => {
            bytes.len() == digits + 1
                || bytes
                    .get(digits + 1)
                    .is_some_and(|b| b.is_ascii_whitespace())
        }
        _ => false,
    }
}

fn is_footnote_def(line: &str) -> bool {
    let Some(rest) = strip_atx_indent(line) else {
        return false;
    };
    let Some(after) = rest.strip_prefix("[^") else {
        return false;
    };
    let Some(end) = after.find("]:") else {
        return false;
    };
    !after[..end].is_empty()
}

fn table_row(line: &str) -> bool {
    let Some(rest) = strip_atx_indent(line) else {
        return false;
    };
    rest.starts_with('|')
}

fn strip_atx_indent(line: &str) -> Option<&str> {
    let spaces = line.bytes().take_while(|b| *b == b' ').count();
    if spaces > 3 {
        return None;
    }
    Some(&line[spaces..])
}

pub(crate) fn setext_span(
    lines: &[ClassifiedLine<'_>],
    underline: usize,
) -> Option<(usize, usize)> {
    if lines.get(underline)?.region != Region::SetextUnderline {
        return None;
    }
    let mut start = underline;
    while start > 0 && lines[start - 1].region == Region::Paragraph {
        start -= 1;
    }
    if start == underline {
        return None;
    }
    Some((start, underline - start))
}

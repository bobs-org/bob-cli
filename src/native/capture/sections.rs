//! Markdown section/task/bullet insertion and line spans.
use super::*;

#[cfg(test)]
pub(super) fn parse_capture_text(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<ParsedCaptureText, CaptureError> {
    parse_capture_text_with_clip_control(
        raw_text,
        forced_route,
        forced_section,
        true,
    )
}

/// Run the shared capture grammar and re-wrap its message as this command's
/// usage error, which owns the exit code `bob capture` reports.
#[cfg(test)]
pub(super) fn parse_capture_text_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
) -> Result<ParsedCaptureText, CaptureError> {
    capture_language::parse_capture_text_with_clip_control(
        raw_text,
        forced_route,
        forced_section,
        parse_clip_markers,
    )
    .map_err(CaptureError::usage)
}

pub(super) fn parse_capture_draft_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
) -> Result<capture_language::ParsedCaptureDraft, CaptureError> {
    capture_language::parse_capture_draft_with_clip_control(
        raw_text,
        forced_route,
        forced_section,
        parse_clip_markers,
    )
    .map_err(CaptureError::usage)
}

#[cfg(test)]
pub(super) fn extract_trailing_schedule(tokens: &mut Vec<&str>) -> Option<u64> {
    capture_language::extract_terminal_markers(tokens, false)
        .0
        .scheduled_offset
}

pub(crate) fn insert_task_line(
    contents: &str,
    task_line: &str,
) -> (String, Placement) {
    let lines = line_spans(contents);
    if let Some(section) = tasks_section(&lines) {
        let index = last_task_block_insert_index_in_range(
            &lines,
            section.start_line,
            section.end_line,
        )
        .unwrap_or(section.heading_end);
        let addition = if index == section.heading_end {
            empty_section_insertion_text(contents, index, task_line)
        } else {
            insertion_text(contents, index, task_line)
        };
        return (insert_at(contents, index, &addition), Placement::Inserted);
    }

    let Some(index) =
        last_task_block_insert_index_in_range(&lines, 0, lines.len())
    else {
        let addition = insertion_text(contents, contents.len(), task_line);
        return (
            insert_at(contents, contents.len(), &addition),
            Placement::Appended,
        );
    };

    let addition = insertion_text(contents, index, task_line);
    (insert_at(contents, index, &addition), Placement::Inserted)
}

pub(super) fn insert_at(
    contents: &str,
    index: usize,
    addition: &str,
) -> String {
    let mut updated = String::with_capacity(contents.len() + addition.len());
    updated.push_str(&contents[..index]);
    updated.push_str(addition);
    updated.push_str(&contents[index..]);
    updated
}

pub(super) fn insertion_text(
    contents: &str,
    index: usize,
    line: &str,
) -> String {
    let ending = document_line_ending(contents);
    let line = line.replace('\n', ending);
    let needs_leading_newline = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading_newline {
        format!("{ending}{line}{ending}")
    } else {
        format!("{line}{ending}")
    }
}

pub(super) fn empty_section_insertion_text(
    contents: &str,
    index: usize,
    line: &str,
) -> String {
    let ending = document_line_ending(contents);
    let line = line.replace('\n', ending);
    if index > 0 && contents[..index].ends_with('\n') {
        format!("{ending}{line}{ending}")
    } else {
        format!("{ending}{ending}{line}{ending}")
    }
}

pub(super) fn document_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

pub(super) fn last_task_block_insert_index_in_range(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<usize> {
    let mut last_index = None;
    for (index, line) in lines[start_line..end_line].iter().enumerate() {
        if is_top_level_task_line(line.text) {
            last_index = Some(task_block_end(lines, start_line + index));
        }
    }
    last_index
}

pub(super) fn insert_bullet_line(
    contents: &str,
    bullet_line: &str,
    section_prefix: Option<&str>,
    exact: bool,
) -> (String, Placement) {
    let lines = line_spans(contents);
    let headings = markdown_headings(&lines);
    let section =
        target_bullet_section(&lines, &headings, section_prefix, exact);

    if let Some(index) = last_bullet_block_insert_index_in_range(
        &lines,
        section.start_line,
        section.end_line,
    ) {
        let addition = insertion_text(contents, index, bullet_line);
        return (insert_at(contents, index, &addition), Placement::Inserted);
    }

    match section.heading_end {
        Some(heading_end) => {
            let addition = empty_section_insertion_text(
                contents,
                heading_end,
                bullet_line,
            );
            (
                insert_at(contents, heading_end, &addition),
                Placement::Inserted,
            )
        }
        None => {
            let index = section.insertion_start;
            let addition = insertion_text(contents, index, bullet_line);
            let placement = if index >= contents.len() {
                Placement::Appended
            } else {
                Placement::Inserted
            };
            (insert_at(contents, index, &addition), placement)
        }
    }
}

/// A Markdown section the bullet capture can target.
///
/// `heading_end` is the byte offset just past the heading line, or `None` for
/// the zeroth (pre-heading) section. `start_line`/`end_line` bound the section
/// body for bullet scanning, and `insertion_start` is where an empty zeroth
/// section receives its first bullet.
#[derive(Debug, Clone, Copy)]
pub(super) struct MarkdownSection {
    pub(super) heading_end: Option<usize>,
    pub(super) start_line: usize,
    pub(super) end_line: usize,
    pub(super) insertion_start: usize,
}

pub(super) fn target_bullet_section(
    lines: &[LineSpan<'_>],
    headings: &[MarkdownHeading<'_>],
    section_prefix: Option<&str>,
    exact: bool,
) -> MarkdownSection {
    let matches = |heading: &MarkdownHeading<'_>| {
        heading.title != "Tasks"
            && heading_matches_bullet_selector(
                heading.title,
                section_prefix,
                exact,
            )
    };
    // Prefer the first matching non-H1 heading, falling back to the first
    // matching H1 heading only when no non-H1 heading matches.
    let target = headings
        .iter()
        .position(|heading| heading.level != 1 && matches(heading))
        .or_else(|| {
            headings
                .iter()
                .position(|heading| heading.level == 1 && matches(heading))
        });

    match target {
        Some(pos) => {
            let heading_index = headings[pos].line_index;
            let heading_end = lines[heading_index].end;
            let end_line = headings
                .get(pos + 1)
                .map(|heading| heading.line_index)
                .unwrap_or(lines.len());
            MarkdownSection {
                heading_end: Some(heading_end),
                start_line: heading_index + 1,
                end_line,
                insertion_start: heading_end,
            }
        }
        None => {
            let (start_line, insertion_start) = match frontmatter_span(lines) {
                Some((line_after, byte_end)) => (line_after, byte_end),
                None => (0, 0),
            };
            let end_line = headings
                .first()
                .map(|heading| heading.line_index)
                .unwrap_or(lines.len());
            MarkdownSection {
                heading_end: None,
                start_line,
                end_line,
                insertion_start,
            }
        }
    }
}

/// Whether `title` matches a bullet capture's section selector. A bare marker
/// (no selector) matches every heading; otherwise exact selectors compare the
/// whole title case insensitively, and prefix selectors compare against the
/// start of `title` case insensitively.
pub(super) fn heading_matches_bullet_selector(
    title: &str,
    section_prefix: Option<&str>,
    exact: bool,
) -> bool {
    match section_prefix {
        None => true,
        Some(selector) => {
            let title = title.to_lowercase();
            let selector = selector.to_lowercase();
            if exact {
                title == selector
            } else {
                title.starts_with(&selector)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SectionHeading {
    pub(crate) title: String,
    pub(crate) level: usize,
}

pub(crate) fn non_tasks_section_headings(
    contents: &str,
) -> Vec<SectionHeading> {
    let lines = line_spans(contents);
    markdown_headings(&lines)
        .into_iter()
        .filter(|heading| heading.title != "Tasks")
        .map(|heading| SectionHeading {
            title: heading.title.to_string(),
            level: heading.level,
        })
        .collect()
}

pub(super) fn last_bullet_block_insert_index_in_range(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<usize> {
    let mut last_index = None;
    for (offset, line) in lines[start_line..end_line].iter().enumerate() {
        if is_top_level_bullet_line(line.text) {
            last_index = Some(task_block_end(lines, start_line + offset));
        }
    }
    last_index
}

pub(super) fn is_top_level_bullet_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- ") else {
        return false;
    };
    !is_checkbox_marker(rest)
}

pub(super) fn is_checkbox_marker(after_dash: &str) -> bool {
    let mut chars = after_dash.chars();
    chars.next() == Some('[')
        && chars.next().is_some()
        && chars.next() == Some(']')
}

/// An ATX heading discovered while scanning a note.
///
/// `line_index` is the heading's line, `level` is its ATX level (number of
/// leading `#`), and `title` is the stripped heading text.
#[derive(Debug, Clone, Copy)]
pub(super) struct MarkdownHeading<'a> {
    pub(super) line_index: usize,
    pub(super) level: usize,
    pub(super) title: &'a str,
}

/// Collect every ATX heading, skipping YAML frontmatter and fenced code blocks.
pub(super) fn markdown_headings<'a>(
    lines: &[LineSpan<'a>],
) -> Vec<MarkdownHeading<'a>> {
    let mut headings = Vec::new();
    let mut in_frontmatter = false;
    let mut fence = None;

    for (index, line) in lines.iter().enumerate() {
        if index == 0 && line.text.trim() == "---" {
            in_frontmatter = true;
            continue;
        }

        if in_frontmatter {
            if line.text.trim() == "---" {
                in_frontmatter = false;
            }
            continue;
        }

        if let Some(open_fence) = fence {
            if closes_fence(line.text, open_fence) {
                fence = None;
            }
            continue;
        }

        if let Some(open_fence) = fence_marker(line.text) {
            fence = Some(open_fence);
            continue;
        }

        if let Some((level, title)) = markdown::atx_heading(line.text) {
            headings.push(MarkdownHeading {
                line_index: index,
                level,
                title,
            });
        }
    }

    headings
}

/// Byte span of YAML frontmatter as `(line_after, end_byte)` when the document
/// opens with a closed `---` block.
pub(super) fn frontmatter_span(
    lines: &[LineSpan<'_>],
) -> Option<(usize, usize)> {
    if lines.first().map(|line| line.text.trim()) != Some("---") {
        return None;
    }

    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.text.trim() == "---")
        .map(|(index, line)| (index + 1, line.end))
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TasksSection {
    pub(super) heading_end: usize,
    pub(super) start_line: usize,
    pub(super) end_line: usize,
}

pub(super) fn tasks_section(lines: &[LineSpan<'_>]) -> Option<TasksSection> {
    let headings = markdown_headings(lines);
    let pos = headings
        .iter()
        .position(|heading| heading.title == "Tasks")?;
    let heading_index = headings[pos].line_index;
    let end_line = headings
        .get(pos + 1)
        .map(|heading| heading.line_index)
        .unwrap_or(lines.len());
    Some(TasksSection {
        heading_end: tasks_heading_end_after_badge_row(lines, heading_index),
        start_line: heading_index + 1,
        end_line,
    })
}

pub(super) fn tasks_heading_end_after_badge_row(
    lines: &[LineSpan<'_>],
    heading_index: usize,
) -> usize {
    let mut heading_end = lines[heading_index].end;
    let mut index = heading_index + 1;
    if index < lines.len()
        && task_status_groups::is_legacy_badge_marker(lines[index].text)
    {
        heading_end = lines[index].end;
        index += 1;
    }
    if index < lines.len()
        && task_status_groups::is_badge_row(lines[index].text)
    {
        heading_end = lines[index].end;
    }
    heading_end
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FenceMarker {
    pub(super) character: u8,
    pub(super) length: usize,
}

pub(super) fn fence_marker(line: &str) -> Option<FenceMarker> {
    let (marker, _) = fence_sequence(line)?;
    Some(marker)
}

pub(super) fn closes_fence(line: &str, open_fence: FenceMarker) -> bool {
    let Some((marker, remainder)) = fence_sequence(line) else {
        return false;
    };

    marker.character == open_fence.character
        && marker.length >= open_fence.length
        && remainder.trim().is_empty()
}

pub(super) fn fence_sequence(line: &str) -> Option<(FenceMarker, &str)> {
    let line = markdown_indented_line(line)?;
    let bytes = line.as_bytes();
    let character = *bytes.first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }

    let length = bytes.iter().take_while(|byte| **byte == character).count();
    if length < 3 {
        return None;
    }

    Some((FenceMarker { character, length }, &line[length..]))
}

pub(super) fn markdown_indented_line(line: &str) -> Option<&str> {
    let spaces = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ')
        .count();
    if spaces > 3 {
        return None;
    }
    Some(&line[spaces..])
}

pub(super) fn task_block_end(
    lines: &[LineSpan<'_>],
    task_index: usize,
) -> usize {
    let mut index = task_index + 1;
    while index < lines.len() {
        let line = lines[index].text;
        if is_indented_line(line)
            || (is_blank_line(line)
                && next_nonblank_is_indented(lines, index + 1))
        {
            index += 1;
            continue;
        }
        break;
    }
    lines[index - 1].end
}

pub(super) fn next_nonblank_is_indented(
    lines: &[LineSpan<'_>],
    start_index: usize,
) -> bool {
    lines[start_index..]
        .iter()
        .find(|line| !is_blank_line(line.text))
        .is_some_and(|line| is_indented_line(line.text))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LineSpan<'a> {
    pub(crate) end: usize,
    pub(crate) text: &'a str,
}

pub(crate) fn line_spans(contents: &str) -> Vec<LineSpan<'_>> {
    let mut spans = Vec::new();
    let mut start = 0;
    for segment in contents.split_inclusive('\n') {
        let end = start + segment.len();
        spans.push(LineSpan {
            end,
            text: logical_line(segment),
        });
        start = end;
    }
    spans
}

pub(super) fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

pub(super) fn is_top_level_task_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- [") else {
        return false;
    };
    let mut chars = rest.chars();
    if chars.next().is_none() || chars.next() != Some(']') {
        return false;
    }
    let after_checkbox = chars.as_str();
    after_checkbox
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
        && after_checkbox.contains("#task")
}

pub(super) fn is_indented_line(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

pub(super) fn is_blank_line(line: &str) -> bool {
    line.trim().is_empty()
}

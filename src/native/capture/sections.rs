//! Markdown section/task/bullet insertion and line spans.
use super::*;

use std::collections::BTreeSet;

use crate::native::articles::{heading_identity, heading_plain_identity};
use crate::native::projects::{frontmatter_is_article, parse_frontmatter};

#[cfg(test)]
pub(super) fn parse_capture_text(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<ParsedCaptureText, CaptureError> {
    parse_capture_text_with_options(
        raw_text,
        forced_route,
        forced_section,
        &capture_language::CaptureParseOptions::routing_off(),
    )
}

/// Run the shared capture grammar and re-wrap its message as this command's
/// usage error, which owns the exit code `bob capture` reports.
#[cfg(test)]
pub(super) fn parse_capture_text_with_options(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &capture_language::CaptureParseOptions<'_>,
) -> Result<ParsedCaptureText, CaptureError> {
    capture_language::parse_capture_text_with_options(
        raw_text,
        forced_route,
        forced_section,
        options,
    )
    .map_err(CaptureError::usage)
}

pub(super) fn parse_capture_draft_with_options(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &capture_language::CaptureParseOptions<'_>,
) -> Result<capture_language::ParsedCaptureDraft, CaptureError> {
    capture_language::parse_capture_draft_with_options_coded(
        raw_text,
        forced_route,
        forced_section,
        options,
    )
    .map_err(|error| {
        let mut capture_error = match error.code {
            Some(code) => CaptureError::usage_code(error.message, code),
            None => CaptureError::usage(error.message),
        };
        if let (Some(item), Some(span)) = (error.item, error.span) {
            capture_error = capture_error.with_item_span(item, span);
        }
        capture_error
    })
}

#[cfg(test)]
pub(super) fn extract_trailing_schedule(tokens: &mut Vec<&str>) -> Option<u64> {
    capture_language::extract_terminal_markers(tokens)
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

pub(crate) fn insert_at(
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

pub(crate) fn empty_section_insertion_text(
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

pub(crate) fn document_line_ending(contents: &str) -> &'static str {
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
    let article = destination_is_article(contents);
    let reserved = task_status_groups::reserved_group_heading_lines(contents);
    let section = target_bullet_section(
        &lines,
        &headings,
        section_prefix,
        exact,
        article,
        &reserved,
    );

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
    article: bool,
    reserved: &BTreeSet<usize>,
) -> MarkdownSection {
    // Prefer the first matching non-H1 heading, falling back to the first
    // matching H1 heading only when no non-H1 heading matches.
    let find_preferred = |predicate: &dyn Fn(&MarkdownHeading<'_>) -> bool| {
        headings
            .iter()
            .position(|heading| heading.level != 1 && predicate(heading))
            .or_else(|| {
                headings.iter().position(|heading| {
                    heading.level == 1 && predicate(heading)
                })
            })
    };
    let bullet =
        |heading: &MarkdownHeading<'_>| is_bullet_section(heading, reserved);
    let raw = |heading: &MarkdownHeading<'_>| {
        bullet(heading)
            && raw_heading_matches(heading.title, section_prefix, exact)
    };
    let identity = |heading: &MarkdownHeading<'_>| {
        article
            && bullet(heading)
            && section_prefix.is_some_and(|selector| {
                article_heading_matches(heading.title, selector, exact)
            })
    };
    // Raw title, then article identity, then (non-exact only) whole-slug
    // equality, then slug-prefix. Exact selectors never slug-match.
    let mut target = find_preferred(&raw).or_else(|| find_preferred(&identity));
    if target.is_none() && !exact {
        let slug_eq = |heading: &MarkdownHeading<'_>| {
            bullet(heading)
                && section_prefix.is_some_and(|selector| {
                    slug_heading_matches(heading.title, selector, true)
                })
        };
        let slug_prefix = |heading: &MarkdownHeading<'_>| {
            bullet(heading)
                && section_prefix.is_some_and(|selector| {
                    slug_heading_matches(heading.title, selector, false)
                })
        };
        target =
            find_preferred(&slug_eq).or_else(|| find_preferred(&slug_prefix));
    }

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

/// Whether `title` matches a bullet capture's section selector at the raw
/// title or article-identity tiers. A bare marker (no selector) matches
/// every heading; otherwise exact selectors compare the whole title case
/// insensitively, and prefix selectors compare against the start of `title`
/// case insensitively. Article notes also match on heading identity after
/// the raw-title comparison. Slug matching is a later tier in
/// [`target_bullet_section`] and is not part of this check.
#[cfg(test)]
pub(super) fn heading_matches_bullet_selector(
    title: &str,
    section_prefix: Option<&str>,
    exact: bool,
    article: bool,
) -> bool {
    raw_heading_matches(title, section_prefix, exact)
        || article
            && section_prefix.is_some_and(|selector| {
                article_heading_matches(title, selector, exact)
            })
}

fn raw_heading_matches(
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

fn destination_is_article(contents: &str) -> bool {
    parse_frontmatter(contents)
        .is_some_and(|frontmatter| frontmatter_is_article(&frontmatter))
}

fn article_heading_matches(title: &str, selector: &str, exact: bool) -> bool {
    let heading = [heading_identity(title), heading_plain_identity(title)];
    let selector =
        [heading_identity(selector), heading_plain_identity(selector)];
    if exact {
        heading.iter().any(|heading| {
            !heading.is_empty()
                && selector.iter().any(|selector| heading == selector)
        })
    } else {
        selector.iter().any(|selector| {
            !selector.is_empty()
                && heading
                    .iter()
                    .any(|heading| heading.starts_with(selector.as_str()))
        })
    }
}

fn slug_heading_matches(title: &str, selector: &str, exact: bool) -> bool {
    let title_slug = capture_language::selector_slug(title);
    let selector_slug = capture_language::selector_slug(selector);
    if title_slug.is_empty() || selector_slug.is_empty() {
        return false;
    }
    if exact {
        title_slug == selector_slug
    } else {
        title_slug.starts_with(&selector_slug)
    }
}

/// Heading line of the section `selector` resolves to, or `None` when the
/// resolver falls through to the zeroth (pre-heading) section.
fn resolved_bullet_heading_line(
    contents: &str,
    selector: &str,
    exact: bool,
) -> Option<usize> {
    let lines = line_spans(contents);
    let headings = markdown_headings(&lines);
    let article = destination_is_article(contents);
    let reserved = task_status_groups::reserved_group_heading_lines(contents);
    let section = target_bullet_section(
        &lines,
        &headings,
        Some(selector),
        exact,
        article,
        &reserved,
    );
    section.heading_end?;
    Some(section.start_line.saturating_sub(1))
}

fn quote_escape(title: &str) -> String {
    title.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unquoted_selector_spelling(title: &str) -> bool {
    !title.is_empty()
        && !title.chars().any(char::is_whitespace)
        && !title.starts_with('"')
        && !title.starts_with('“')
}

/// Replacement a section-completion row inserts for `title` at
/// `heading_line_index`: the raw title when it is one unquoted selector
/// that resolves back to this heading, else the hyphen slug when that
/// spelling is also unquoted-safe and round-trips, else a quoted
/// `"Title"` with `"` and `\` escaped when the decoded title reaches
/// this heading as an exact selector. `None` when no offered spelling
/// can address this heading (duplicate titles, an H1 shadowed by a
/// non-H1 match).
fn section_selector_replacement(
    contents: &str,
    heading_line_index: usize,
    title: &str,
) -> Option<String> {
    if unquoted_selector_spelling(title)
        && resolved_bullet_heading_line(contents, title, false)
            == Some(heading_line_index)
    {
        return Some(title.to_string());
    }
    let slug = capture_language::selector_slug(title);
    if unquoted_selector_spelling(&slug)
        && resolved_bullet_heading_line(contents, &slug, false)
            == Some(heading_line_index)
    {
        return Some(slug);
    }
    if resolved_bullet_heading_line(contents, title, true)
        == Some(heading_line_index)
    {
        return Some(format!("\"{}\"", quote_escape(title)));
    }
    None
}

/// One non-`Tasks` heading offered as a section-completion row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectionCompletionRow {
    pub(crate) title: String,
    pub(crate) level: usize,
    pub(crate) replacement: String,
}

/// Ranked section-completion rows for `contents`: title prefix or slug
/// prefix first, then title substring, each group in document order. Each
/// `replacement` round-trips through [`target_bullet_section`] to the same
/// heading; unreachable headings are omitted.
pub(crate) fn section_completion_rows(
    contents: &str,
    query: &str,
) -> Vec<SectionCompletionRow> {
    let reserved = task_status_groups::reserved_group_heading_lines(contents);
    let lines = line_spans(contents);
    let headings: Vec<SectionCompletionRow> = markdown_headings(&lines)
        .into_iter()
        .filter(|heading| is_bullet_section(heading, &reserved))
        .filter_map(|heading| {
            section_selector_replacement(
                contents,
                heading.line_index,
                heading.title,
            )
            .map(|replacement| SectionCompletionRow {
                replacement,
                title: heading.title.to_string(),
                level: heading.level,
            })
        })
        .collect();
    rank_section_rows(headings, query)
}

fn rank_section_rows(
    rows: Vec<SectionCompletionRow>,
    query: &str,
) -> Vec<SectionCompletionRow> {
    if query.is_empty() {
        return rows;
    }
    let query_lower = query.to_lowercase();
    let query_slug = capture_language::selector_slug(query);
    let mut prefix = Vec::new();
    let mut substring = Vec::new();
    for row in rows {
        let title_lower = row.title.to_lowercase();
        let slug = capture_language::selector_slug(&row.title);
        let is_prefix = title_lower.starts_with(&query_lower)
            || (!query_slug.is_empty() && slug.starts_with(&query_slug));
        if is_prefix {
            prefix.push(row);
        } else if title_lower.contains(&query_lower) {
            substring.push(row);
        }
    }
    prefix.extend(substring);
    prefix
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SectionHeading {
    pub(crate) title: String,
    pub(crate) level: usize,
}

/// ATX headings capture can target as bullet sections: every heading except
/// `Tasks` and the generated status-group headings below a `Tasks` heading.
pub(crate) fn non_tasks_section_headings(
    contents: &str,
) -> Vec<SectionHeading> {
    let reserved = task_status_groups::reserved_group_heading_lines(contents);
    let lines = line_spans(contents);
    markdown_headings(&lines)
        .into_iter()
        .filter(|heading| is_bullet_section(heading, &reserved))
        .map(|heading| SectionHeading {
            title: heading.title.to_string(),
            level: heading.level,
        })
        .collect()
}

fn is_bullet_section(
    heading: &MarkdownHeading<'_>,
    reserved: &BTreeSet<usize>,
) -> bool {
    heading.title != "Tasks"
        && !is_generated_notebooks_heading(heading)
        && !is_generated_albums_heading(heading)
        && !reserved.contains(&heading.line_index)
}

/// The bob-written `## Notebooks` catalog heading. Its section belongs to
/// `bob notebooks sync`, so it is never a bullet target or completion row.
fn is_generated_notebooks_heading(heading: &MarkdownHeading<'_>) -> bool {
    heading.level == 2 && heading.title == "Notebooks"
}

/// The bob-written `## Albums` catalog heading. Its section belongs to
/// `bob albums sync`, so it is never a bullet target or completion row.
fn is_generated_albums_heading(heading: &MarkdownHeading<'_>) -> bool {
    heading.level == 2 && heading.title == "Albums"
}

/// Refuse a section selector that can only mean the generated
/// `## Notebooks` catalog. Excluding that heading from targeting alone
/// would let the selector fall through to the pre-heading section and
/// write there silently; an explicit selection must refuse instead.
pub(super) fn reject_generated_notebooks_selector(
    contents: &str,
    selector: Option<&str>,
    exact: bool,
) -> Result<(), CaptureError> {
    let Some(selector) = selector else {
        return Ok(());
    };
    let lines = line_spans(contents);
    let headings = markdown_headings(&lines);
    let Some(generated) = headings
        .iter()
        .find(|heading| is_generated_notebooks_heading(heading))
    else {
        return Ok(());
    };
    if resolved_bullet_heading_line(contents, selector, exact).is_some() {
        return Ok(());
    }
    let matches_generated =
        raw_heading_matches(generated.title, Some(selector), exact)
            || slug_heading_matches(generated.title, selector, exact);
    if !matches_generated {
        return Ok(());
    }
    Err(CaptureError::usage_code(
        "`## Notebooks` is the generated notebook catalog (written by \
         `bob notebooks sync`) and is not a section target; pick another \
         section, or capture into one of its notebooks",
        "notebooks_section_generated",
    ))
}

/// Refuse a section selector that can only mean the generated
/// `## Albums` catalog. Excluding that heading from targeting alone
/// would let the selector fall through to the pre-heading section and
/// write there silently; an explicit selection must refuse instead.
pub(super) fn reject_generated_albums_selector(
    contents: &str,
    selector: Option<&str>,
    exact: bool,
) -> Result<(), CaptureError> {
    let Some(selector) = selector else {
        return Ok(());
    };
    let lines = line_spans(contents);
    let headings = markdown_headings(&lines);
    let Some(generated) = headings
        .iter()
        .find(|heading| is_generated_albums_heading(heading))
    else {
        return Ok(());
    };
    if resolved_bullet_heading_line(contents, selector, exact).is_some() {
        return Ok(());
    }
    let matches_generated =
        raw_heading_matches(generated.title, Some(selector), exact)
            || slug_heading_matches(generated.title, selector, exact);
    if !matches_generated {
        return Ok(());
    }
    Err(CaptureError::usage_code(
        "`## Albums` is the generated album catalog (written by \
         `bob albums sync`) and is not a section target; pick another \
         section, or capture into one of its albums",
        "albums_section_generated",
    ))
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
pub(crate) struct MarkdownHeading<'a> {
    pub(crate) line_index: usize,
    pub(crate) level: usize,
    pub(crate) title: &'a str,
}

/// Collect every ATX heading, skipping YAML frontmatter and fenced code blocks.
pub(crate) fn markdown_headings<'a>(
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

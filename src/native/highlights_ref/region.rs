//! Read-only parser for rendered ref-note bodies.
//!
//! This module sits beside the renderer that owns the format
//! (`sidecar_render.rs`) and never writes. [`parse_managed_region`] parses the
//! managed Highlights region back into annotation blocks, and
//! [`split_note_body`] splits a note body into its anatomy (H1, `^ref`
//! tracker, managed region, own notes, Tasks section).
//!
//! Phase `sync-fixes` shares [`is_marker_mirror_text`]; phase `index` builds
//! annotation counts and note anatomy on this parser.
use super::*;

type RegionResult<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegionBlockKind {
    Highlight,
    Note,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegionBlock {
    pub(crate) page_label: Option<String>,
    pub(crate) kind: RegionBlockKind,
    pub(crate) quote: Option<String>,
    pub(crate) comment: Option<String>,
    pub(crate) asset: Option<String>,
    pub(crate) block_id: String,
    pub(crate) mirror: bool,
    pub(crate) in_preamble: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RegionParse {
    pub(crate) blocks: Vec<RegionBlock>,
    pub(crate) removed: Vec<String>,
    pub(crate) unparsed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegionTask {
    pub(crate) checked: bool,
    pub(crate) mark: char,
    pub(crate) text: String,
    pub(crate) block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteParts {
    pub(crate) h1: Option<String>,
    pub(crate) tracker_line: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) own_notes: String,
    pub(crate) tasks: Vec<RegionTask>,
}

/// Parse a rendered managed Highlights region into annotation blocks.
///
/// Page headings are level-3 headings other than `### Removed highlights`;
/// blocks before the first page heading (in a region that has any) are marked
/// `in_preamble`. Tombstone IDs under `### Removed highlights` go in
/// `removed`. A block the parser cannot classify goes in `unparsed` as raw
/// text and is never dropped silently.
pub(crate) fn parse_managed_region(region: &str) -> RegionParse {
    let mut parsed = RegionParse::default();
    let lines: Vec<&str> = region.lines().collect();
    let first_page_heading = lines
        .iter()
        .position(|line| region_page_heading(line).is_some());
    let mut page_label: Option<String> = None;
    let mut in_removed = false;
    let mut chunk: Vec<&str> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some((level, title)) = markdown::atx_heading(line)
            && level == 3
        {
            flush_stray_chunk(&mut parsed, &mut chunk);
            if title.eq_ignore_ascii_case("removed highlights") {
                in_removed = true;
            } else {
                page_label = Some(title.to_string());
            }
            continue;
        }
        if in_removed {
            if let Some(block_id) = region_block_id(line) {
                parsed.removed.push(block_id);
            }
            continue;
        }
        if let Some(block_id) = region_block_id(line) {
            let in_preamble =
                first_page_heading.is_some_and(|heading| index < heading);
            match classify_region_block(
                &chunk,
                page_label.clone(),
                block_id,
                in_preamble,
            ) {
                Ok(block) => parsed.blocks.push(block),
                Err(raw) => parsed.unparsed.push(raw),
            }
            chunk.clear();
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        chunk.push(line);
    }
    flush_stray_chunk(&mut parsed, &mut chunk);
    parsed
}

/// The shared marker-mirror predicate: true when `text` parses with
/// [`parse_marker`] and carries both `status` and `parent`.
pub(crate) fn is_marker_mirror_text(text: &str) -> bool {
    parse_marker(text).is_ok_and(|projection| {
        projection.contains_key(FIELD_STATUS)
            && projection.contains_key(FIELD_PARENT)
    })
}

/// Split a note body into its anatomy.
///
/// `region` reuses the begin/end constants and duplicate-marker rules of
/// `note::managed_region`: a missing region (legacy migrated bodies) yields
/// `None`, as does a broken marker pair. Legacy bodies pass through
/// `own_notes` verbatim; otherwise `own_notes` is the body outside the region
/// minus the first H1, the `^ref` tracker line, audio embeds, the
/// `## Highlights` heading directly above the begin marker, and the
/// `## Tasks` section, with runs of blank lines collapsed and the result
/// trimmed.
pub(crate) fn split_note_body(body: &str) -> NoteParts {
    let lines: Vec<&str> = body.lines().collect();
    let region_extract = extract_region_content(body, &lines);
    let region = match &region_extract {
        RegionExtract::Present { content, .. } => Some(content.clone()),
        RegionExtract::Absent | RegionExtract::Broken { .. } => None,
    };
    let region_range = match &region_extract {
        RegionExtract::Present {
            begin_line,
            end_line,
            ..
        } => Some(*begin_line..=*end_line),
        RegionExtract::Absent | RegionExtract::Broken { .. } => None,
    };
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let outside_region = |index: usize| {
        region_range
            .as_ref()
            .is_none_or(|range| !range.contains(&index))
            && !fenced.contains(&index)
    };

    let mut h1 = None;
    let mut h1_line = None;
    for (index, line) in lines.iter().enumerate() {
        if !outside_region(index) {
            continue;
        }
        if let Some((level, title)) = markdown::atx_heading(line)
            && level == 1
        {
            h1 = Some(title.to_string());
            h1_line = Some(index);
            break;
        }
    }

    let mut tracker_line = None;
    let mut tracker_index = None;
    for (index, line) in lines.iter().enumerate() {
        if !outside_region(index) {
            continue;
        }
        if is_tracker_line(line) {
            tracker_line = Some(line.trim().to_string());
            tracker_index = Some(index);
            break;
        }
    }

    let tasks_heading = tasks_heading_line_index(body);
    let tasks_end = tasks_heading
        .map(|heading| tasks_section_end_line_index(body, heading));
    let mut tasks = Vec::new();
    if let (Some(heading), Some(end)) = (tasks_heading, tasks_end) {
        let section_fenced =
            markdown::fenced_lines(&lines, heading + 1..end.min(lines.len()));
        for (offset, line) in
            lines[heading + 1..end.min(lines.len())].iter().enumerate()
        {
            if section_fenced.contains(&(heading + 1 + offset)) {
                continue;
            }
            if let Some(task) = parse_region_task(line) {
                tasks.push(task);
            }
        }
    }

    let own_notes = match region_extract {
        RegionExtract::Absent => body.to_string(),
        RegionExtract::Present { .. } | RegionExtract::Broken { .. } => {
            let mut drop = vec![false; lines.len()];
            if let Some(range) = &region_range {
                for index in range.clone() {
                    drop[index] = true;
                }
            }
            if let Some(index) = h1_line {
                drop[index] = true;
            }
            if let Some(index) = tracker_index {
                drop[index] = true;
            }
            for (index, line) in lines.iter().enumerate() {
                if outside_region(index) && is_audio_embed_line(line) {
                    drop[index] = true;
                }
            }
            if let Some(index) =
                highlights_heading_line_index(&lines, &region_extract)
                && outside_region(index)
            {
                drop[index] = true;
            }
            if let (Some(heading), Some(end)) = (tasks_heading, tasks_end) {
                for index in heading..end.min(lines.len()) {
                    drop[index] = true;
                }
            }
            collapse_blank_lines(
                lines
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !drop[*index])
                    .map(|(_, line)| *line),
            )
        }
    };

    NoteParts {
        h1,
        tracker_line,
        region,
        own_notes,
        tasks,
    }
}

fn region_page_heading(line: &str) -> Option<String> {
    match markdown::atx_heading(line) {
        Some((3, title))
            if !title.eq_ignore_ascii_case("removed highlights") =>
        {
            Some(title.to_string())
        }
        _ => None,
    }
}

fn region_block_id(line: &str) -> Option<String> {
    let id = line.trim().strip_prefix('^')?;
    if !id.starts_with("h-") || !is_valid_block_id(id) {
        return None;
    }
    Some(id.to_string())
}

fn flush_stray_chunk(parsed: &mut RegionParse, chunk: &mut Vec<&str>) {
    if !chunk.is_empty() {
        parsed.unparsed.push(chunk.join("\n"));
        chunk.clear();
    }
}

fn classify_region_block(
    chunk: &[&str],
    page_label: Option<String>,
    block_id: String,
    in_preamble: bool,
) -> RegionResult<RegionBlock> {
    let raw = if chunk.is_empty() {
        format!("^{block_id}")
    } else {
        format!("{}\n^{block_id}", chunk.join("\n"))
    };
    let mut stripped = Vec::with_capacity(chunk.len());
    for line in chunk {
        let after_first = line
            .trim_start()
            .strip_prefix('>')
            .ok_or_else(|| raw.clone())?;
        let after_first = after_first.strip_prefix(' ').unwrap_or(after_first);
        if let Some(after_second) = after_first.strip_prefix('>') {
            stripped.push((
                2,
                after_second.strip_prefix(' ').unwrap_or(after_second),
            ));
        } else {
            stripped.push((1, after_first));
        }
    }
    let Some(((first_depth, first_content), remaining)) =
        stripped.split_first()
    else {
        return Err(raw);
    };
    if *first_depth != 1 {
        return Err(raw);
    }
    let (callout, rest) =
        split_callout_header(first_content).ok_or_else(|| raw.clone())?;
    if callout.eq_ignore_ascii_case("quote")
        && (rest == "Image" || rest.starts_with("Image "))
    {
        let asset =
            image_embed_target(&rest[5..]).ok_or_else(|| raw.clone())?;
        let (text_lines, comment_lines) = split_body_lines(remaining, &raw)?;
        if text_lines.len() > 1 {
            return Err(raw);
        }
        let comment = comment_text(comment_lines, &raw)?;
        return Ok(RegionBlock {
            page_label,
            kind: RegionBlockKind::Image,
            quote: None,
            comment,
            asset: Some(asset),
            block_id,
            mirror: false,
            in_preamble,
        });
    }
    if callout.eq_ignore_ascii_case("quote") {
        let (text_lines, comment_lines) = split_body_lines(remaining, &raw)?;
        let mut quote_lines = vec![rest];
        quote_lines.extend(text_lines);
        let comment = comment_text(comment_lines, &raw)?;
        let mirror =
            is_marker_mirror_text(comment.as_deref().unwrap_or_default());
        return Ok(RegionBlock {
            page_label,
            kind: RegionBlockKind::Highlight,
            quote: Some(trim_blank_ends(&quote_lines.join("\n"))),
            comment,
            asset: None,
            block_id,
            mirror,
            in_preamble,
        });
    }
    if callout.eq_ignore_ascii_case("note") {
        if remaining.iter().any(|(depth, _)| *depth != 1) {
            return Err(raw);
        }
        let mut text_lines = vec![rest];
        text_lines.extend(remaining.iter().map(|(_, content)| *content));
        let text = trim_blank_ends(&text_lines.join("\n"));
        return Ok(RegionBlock {
            page_label,
            kind: RegionBlockKind::Note,
            quote: None,
            comment: Some(text.clone()),
            asset: None,
            block_id,
            mirror: is_marker_mirror_text(&text),
            in_preamble,
        });
    }
    Err(raw)
}

/// Split a callout header line into its `(name, rest)` where `name` is the
/// text between `[!` and `]` and `rest` is the first line's text after the
/// header and one optional space.
fn split_callout_header(content: &str) -> Option<(String, &str)> {
    let inner = content.strip_prefix("[!")?;
    let end = inner.find(']')?;
    let rest = inner[end + 1..]
        .strip_prefix(' ')
        .unwrap_or(&inner[end + 1..]);
    Some((inner[..end].to_string(), rest))
}

fn image_embed_target(rest: &str) -> Option<String> {
    let after = rest.trim().strip_prefix("![[")?;
    let end = after.find("]]")?;
    let target = after[..end].to_string();
    (!target.is_empty()).then_some(target)
}

/// Split the lines after a quote or image header into depth-1 quote
/// continuation lines and the optional comment callout lines (starting with
/// the `> > [!note] Comment …` header line). The comment starts at the first
/// bare `>` separator whose next line is depth-2; a bare `>` followed by a
/// depth-1 line is a blank quote line, not a separator.
fn split_body_lines<'a>(
    remaining: &[(usize, &'a str)],
    raw: &str,
) -> RegionResult<(Vec<&'a str>, Option<Vec<&'a str>>)> {
    let separator =
        remaining
            .iter()
            .enumerate()
            .position(|(index, (depth, content))| {
                *depth == 1
                    && content.is_empty()
                    && remaining
                        .get(index + 1)
                        .is_some_and(|(next, _)| *next == 2)
            });
    let Some(start) = separator else {
        if remaining.iter().any(|(depth, _)| *depth != 1) {
            return Err(raw.to_string());
        }
        return Ok((
            remaining.iter().map(|(_, content)| *content).collect(),
            None,
        ));
    };
    if remaining[..start].iter().any(|(depth, _)| *depth != 1)
        || remaining[start + 1..].iter().any(|(depth, _)| *depth != 2)
    {
        return Err(raw.to_string());
    }
    Ok((
        remaining[..start]
            .iter()
            .map(|(_, content)| *content)
            .collect(),
        Some(
            remaining[start + 1..]
                .iter()
                .map(|(_, content)| *content)
                .collect(),
        ),
    ))
}

/// The text of a `> > [!note] Comment …` callout: the header with its label
/// stripped plus the remaining depth-2 lines.
fn comment_text(
    comment_lines: Option<Vec<&str>>,
    raw: &str,
) -> RegionResult<Option<String>> {
    let Some(lines) = comment_lines else {
        return Ok(None);
    };
    let Some((header, rest)) = lines.split_first() else {
        return Err(raw.to_string());
    };
    let (callout, label) =
        split_callout_header(header).ok_or_else(|| raw.to_string())?;
    if !callout.eq_ignore_ascii_case("note") {
        return Err(raw.to_string());
    }
    let after = label
        .strip_prefix("Comment")
        .or_else(|| label.strip_prefix("comment"))
        .ok_or_else(|| raw.to_string())?;
    if !(after.is_empty() || after.starts_with(' ')) {
        return Err(raw.to_string());
    }
    let mut comment_lines =
        vec![after.strip_prefix(' ').unwrap_or(after).to_string()];
    comment_lines.extend(rest.iter().map(|line| line.to_string()));
    Ok(Some(trim_blank_ends(&comment_lines.join("\n"))))
}

fn trim_blank_ends(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut start = 0;
    let mut end = lines.len();
    while start < end && lines[start].trim().is_empty() {
        start += 1;
    }
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    lines[start..end].join("\n")
}

enum RegionExtract {
    Absent,
    Broken {
        begin_line: usize,
    },
    Present {
        begin_line: usize,
        end_line: usize,
        content: String,
    },
}

/// Region extraction with the string-level duplicate-marker rules of
/// `note::managed_region`, plus the marker line indices for anatomy work.
fn extract_region_content(body: &str, lines: &[&str]) -> RegionExtract {
    let Some(begin_offset) = body.find(MANAGED_BODY_BEGIN) else {
        return RegionExtract::Absent;
    };
    let after_begin = begin_offset + MANAGED_BODY_BEGIN.len();
    let begin_line = lines
        .iter()
        .position(|line| line.contains(MANAGED_BODY_BEGIN));
    if body[after_begin..].contains(MANAGED_BODY_BEGIN) {
        return match begin_line {
            Some(begin_line) => RegionExtract::Broken { begin_line },
            None => RegionExtract::Absent,
        };
    }
    let Some(relative_end) = body[after_begin..].find(MANAGED_BODY_END) else {
        return match begin_line {
            Some(begin_line) => RegionExtract::Broken { begin_line },
            None => RegionExtract::Absent,
        };
    };
    let end = after_begin + relative_end;
    if body[end + MANAGED_BODY_END.len()..].contains(MANAGED_BODY_END) {
        return match begin_line {
            Some(begin_line) => RegionExtract::Broken { begin_line },
            None => RegionExtract::Absent,
        };
    }
    match (begin_line, end_line_index(lines, end)) {
        (Some(begin_line), Some(end_line)) => RegionExtract::Present {
            begin_line,
            end_line,
            content: body[after_begin..end].trim_matches('\n').to_string(),
        },
        _ => RegionExtract::Absent,
    }
}

/// Line index containing byte `offset` (the end-marker offset).
fn end_line_index(lines: &[&str], offset: usize) -> Option<usize> {
    let mut cursor = 0;
    for (index, line) in lines.iter().enumerate() {
        let next = cursor + line.len() + 1;
        if cursor <= offset && offset < next {
            return Some(index);
        }
        cursor = next;
    }
    (offset == cursor).then(|| lines.len().saturating_sub(1))
}

fn is_tracker_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    (trimmed.starts_with("- [")
        || trimmed.starts_with("* [")
        || trimmed.starts_with("+ ["))
        && line.split_whitespace().any(|token| token == "^ref")
}

fn is_audio_embed_line(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(inner) = trimmed
        .strip_prefix("![[")
        .and_then(|stripped| stripped.strip_suffix("]]"))
    else {
        return false;
    };
    let target = inner.split('|').next().unwrap_or("").trim();
    target.contains('.')
        && target.rsplit('.').next().is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mp3" | "m4a" | "ogg" | "opus"
            )
        })
}

/// The `## Highlights` heading directly above the begin marker: the last
/// non-blank line before the first begin-marker line.
fn highlights_heading_line_index(
    lines: &[&str],
    region_extract: &RegionExtract,
) -> Option<usize> {
    let begin_line = match region_extract {
        RegionExtract::Present { begin_line, .. }
        | RegionExtract::Broken { begin_line } => *begin_line,
        RegionExtract::Absent => return None,
    };
    let heading = lines[..begin_line]
        .iter()
        .rposition(|line| !line.trim().is_empty())?;
    match markdown::atx_heading(lines[heading]) {
        Some((2, title)) if title.eq_ignore_ascii_case("highlights") => {
            Some(heading)
        }
        _ => None,
    }
}

fn parse_region_task(line: &str) -> Option<RegionTask> {
    let trimmed = line.trim_start();
    let after_bracket = trimmed
        .strip_prefix("- [")
        .or_else(|| trimmed.strip_prefix("* ["))
        .or_else(|| trimmed.strip_prefix("+ ["))?;
    let mut chars = after_bracket.chars();
    let mark = chars.next()?;
    let rest = chars.as_str().strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let block_id = extract_task_block_id(rest).unwrap_or_default();
    let text = strip_obsidian_task_properties(&strip_source_block_link(rest))
        .trim()
        .to_string();
    Some(RegionTask {
        checked: matches!(mark, 'x' | 'X'),
        mark,
        text,
        block_id,
    })
}

fn extract_task_block_id(text: &str) -> Option<String> {
    let hash = text.find("#^")?;
    let after = &text[hash + 2..];
    let end = after
        .find(|character: char| {
            character == '|' || character == ']' || character.is_whitespace()
        })
        .unwrap_or(after.len());
    let block_id = after[..end].to_string();
    (!block_id.is_empty()).then_some(block_id)
}

fn collapse_blank_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut collapsed = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            if collapsed
                .last()
                .is_some_and(|last: &String| last.is_empty())
            {
                continue;
            }
            collapsed.push(String::new());
        } else {
            collapsed.push(line.to_string());
        }
    }
    while collapsed.first().is_some_and(|line| line.is_empty()) {
        collapsed.remove(0);
    }
    while collapsed.last().is_some_and(|line| line.is_empty()) {
        collapsed.pop();
    }
    collapsed.join("\n")
}

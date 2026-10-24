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
    pub(crate) images: Vec<String>,
    pub(crate) unresolved_images: Vec<String>,
    pub(crate) unresolved_image_states: Vec<UnresolvedImageNoteState>,
    /// Present only for tucked continuation groups. The block-level fields
    /// remain the flattened compatibility view of these pieces.
    pub(crate) pieces: Vec<RegionPiece>,
    pub(crate) block_id: String,
    pub(crate) mirror: bool,
    pub(crate) in_preamble: bool,
    /// Section `id` when this block sits under a parsed section card.
    /// Ungrouped and unsectioned blocks leave this `None`.
    pub(crate) section: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegionPiece {
    pub(crate) page_label: Option<String>,
    pub(crate) quote: String,
    pub(crate) comment: Option<String>,
    pub(crate) images: Vec<String>,
    pub(crate) unresolved_images: Vec<String>,
    pub(crate) unresolved_image_states: Vec<UnresolvedImageNoteState>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RegionParse {
    pub(crate) blocks: Vec<RegionBlock>,
    pub(crate) removed: Vec<String>,
    pub(crate) unparsed: Vec<String>,
    /// Live section cards, in heading order. Empty when the region has no
    /// `[!summary|section]` signature.
    pub(crate) sections: Vec<RegionSection>,
    /// `^s-` IDs collected under `### Removed highlights`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) removed_sections: Vec<String>,
}

/// One live section card parsed from a sectioned managed region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegionSection {
    pub(crate) id: String,
    pub(crate) block_id: String,
    pub(crate) title: String,
    pub(crate) tag: String,
    pub(crate) description: String,
    pub(crate) images: Vec<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) unresolved_images: Vec<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) unresolved_image_states: Vec<UnresolvedImageNoteState>,
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
///
/// A region is **sectioned** when at least one H3 is followed (after blank
/// lines only) by a depth-1 `[!summary|section]` callout and then a `^s-<id>`
/// line. Sectioned regions fill [`RegionParse::sections`], set
/// [`RegionBlock::section`], read `####` captions as page labels (resetting
/// the label at every H3), and collect `^s-` tombstones into
/// `removed_sections`. Every region without that card signature parses
/// exactly as it does today.
pub(crate) fn parse_managed_region(region: &str) -> RegionParse {
    let mut parsed = RegionParse::default();
    let lines: Vec<&str> = region.lines().collect();
    let sectioned = region_is_sectioned(&lines);
    let first_page_heading = lines
        .iter()
        .position(|line| region_page_heading(line).is_some());
    let first_h3 = lines
        .iter()
        .position(|line| matches!(markdown::atx_heading(line), Some((3, _))));
    let mut page_label: Option<String> = None;
    let mut in_removed = false;
    let mut current_section: Option<String> = None;
    let mut chunk: Vec<&str> = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if let Some((level, title)) = markdown::atx_heading(line) {
            if level == 3 {
                flush_stray_chunk(&mut parsed, &mut chunk);
                if title.eq_ignore_ascii_case("removed highlights") {
                    in_removed = true;
                    if sectioned {
                        page_label = None;
                        current_section = None;
                    }
                    index += 1;
                    continue;
                }
                if sectioned {
                    if title.eq_ignore_ascii_case("other highlights") {
                        current_section = None;
                        page_label = None;
                        index += 1;
                        continue;
                    }
                    if let Some((section, end)) =
                        parse_section_opening(&lines, index, title)
                    {
                        current_section = Some(section.id.clone());
                        page_label = None;
                        parsed.sections.push(section);
                        index = end + 1;
                        continue;
                    }
                    current_section = None;
                    page_label = Some(title.to_string());
                    index += 1;
                    continue;
                }
                page_label = Some(title.to_string());
                index += 1;
                continue;
            }
            if sectioned && level == 4 {
                flush_stray_chunk(&mut parsed, &mut chunk);
                page_label = Some(title.to_string());
                index += 1;
                continue;
            }
        }
        if in_removed {
            if let Some(block_id) = region_block_id(line) {
                parsed.removed.push(block_id);
            } else if sectioned
                && let Some(section_id) = region_section_id(line)
            {
                parsed.removed_sections.push(section_id);
            }
            index += 1;
            continue;
        }
        if let Some(block_id) = region_block_id(line) {
            let in_preamble = if sectioned {
                first_h3.is_some_and(|heading| index < heading)
            } else {
                first_page_heading.is_some_and(|heading| index < heading)
            };
            match classify_region_block(
                &chunk,
                page_label.clone(),
                block_id,
                in_preamble,
            ) {
                Ok(mut block) => {
                    block.section = current_section.clone();
                    parsed.blocks.push(block);
                }
                Err(raw) => parsed.unparsed.push(raw),
            }
            chunk.clear();
            index += 1;
            continue;
        }
        if sectioned && region_section_id(line).is_some() {
            flush_stray_chunk(&mut parsed, &mut chunk);
            index += 1;
            continue;
        }
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        chunk.push(line);
        index += 1;
    }
    flush_stray_chunk(&mut parsed, &mut chunk);
    parsed
}

/// The shared marker-mirror predicate: true when `text` parses with
/// [`parse_marker`] and carries both `status` and `parent`.
///
/// Color-grouped Highlights exports prepend a setext title to the marker
/// list. Both the raw two-line `title` + `===` underline and the
/// already-rendered single-line `title ===...` form (joined by
/// [`beautify_annotation_text`]) are recognized: the prefix is stripped and
/// the entire remainder must still parse as a `status`/`parent` marker.
/// Blank lines may separate the prefix from the list. A title prefix
/// followed by extra prose, malformed items, or missing required fields
/// stays visible.
pub(crate) fn is_marker_mirror_text(text: &str) -> bool {
    if parse_marker(text).is_ok_and(|projection| {
        projection.contains_key(FIELD_STATUS)
            && projection.contains_key(FIELD_PARENT)
    }) {
        return true;
    }
    strip_setext_title_prefix(text).is_some_and(|remainder| {
        parse_marker(&remainder).is_ok_and(|projection| {
            projection.contains_key(FIELD_STATUS)
                && projection.contains_key(FIELD_PARENT)
        })
    })
}

/// Strip a leading nonempty setext title plus its `===` underline, returning
/// the remainder that must parse as a marker list.
///
/// Supports the raw two-line `title` / `===` shape and the already-rendered
/// single-line `title ===...` shape. Blank lines may separate the prefix
/// from the list. Returns `None` when no title prefix is present.
fn strip_setext_title_prefix(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut non_blank: Vec<usize> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.trim().is_empty() {
            non_blank.push(index);
        }
    }
    if non_blank.is_empty() {
        return None;
    }
    let first = non_blank[0];
    // Raw two-line shape: nonempty title plus an `===` underline.
    if non_blank.len() >= 2 {
        let second = non_blank[1];
        let title = lines[first].trim();
        let underline = lines[second].trim();
        if !title.is_empty()
            && !is_setext_underline(title)
            && !is_marker_list_line_text(title)
            && is_setext_underline(underline)
        {
            return Some(lines[second + 1..].join("\n"));
        }
    }
    // Already-rendered single-line shape: `title ===...` joined by the
    // beautifier. The trailing `=` run must be at least three wide and
    // preceded by whitespace with a nonempty title before it.
    let candidate = lines[first].trim_end();
    let equals_run = candidate.chars().rev().take_while(|c| *c == '=').count();
    if equals_run >= 3 {
        let prefix_len = candidate.len() - equals_run;
        let before = &candidate[..prefix_len];
        if before.ends_with(char::is_whitespace)
            && !before.trim().is_empty()
            && !is_setext_underline(before.trim())
            && !is_marker_list_line_text(before.trim())
        {
            return Some(lines[first + 1..].join("\n"));
        }
    }
    None
}

/// A setext level-one underline: at least three `=` with optional
/// surrounding whitespace.
fn is_setext_underline(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 3 && trimmed.chars().all(|c| c == '=')
}

/// A marker-list item line (`- key: value` / `* key: value` with a `:`).
/// Titles that already look like marker items are never treated as a title
/// prefix; the strict whole-text check owns that shape.
fn is_marker_list_line_text(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(item) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    else {
        return false;
    };
    let Some((key, _)) = item.split_once(':') else {
        return false;
    };
    !key.trim().is_empty()
}

/// Split a note body into its anatomy.
///
/// `region` reuses the begin/end constants and duplicate-marker rules of
/// `note::managed_region`: a missing region (legacy migrated bodies) yields
/// `None`, as does a broken marker pair. Legacy bodies pass through
/// `own_notes` verbatim; otherwise `own_notes` is the body outside the region
/// minus the first H1, the `^ref` tracker line, the managed reading-task
/// embed (exactly the line `find_managed_embed` recognizes, so unrelated
/// authored embeds elsewhere survive), audio embeds, the
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
            if let Some(found) =
                crate::native::ref_tasks::find_managed_embed(body)
                && outside_region(found.line_index)
            {
                drop[found.line_index] = true;
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
                for slot in
                    drop.iter_mut().take(end.min(lines.len())).skip(heading)
                {
                    *slot = true;
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

pub(super) fn region_block_id(line: &str) -> Option<String> {
    let id = line.trim().strip_prefix('^')?;
    if !id.starts_with("h-") || !is_valid_block_id(id) {
        return None;
    }
    Some(id.to_string())
}

pub(super) fn region_section_id(line: &str) -> Option<String> {
    let id = line.trim().strip_prefix('^')?;
    if !id.starts_with("s-") || !is_valid_block_id(id) {
        return None;
    }
    Some(id.to_string())
}

fn region_is_sectioned(lines: &[&str]) -> bool {
    lines.iter().enumerate().any(|(index, line)| {
        match markdown::atx_heading(line) {
            Some((3, title)) => {
                parse_section_opening(lines, index, title).is_some()
            }
            _ => false,
        }
    })
}

/// If the H3 at `heading_index` opens a section, return the card and the
/// index of its `^s-<id>` line.
fn parse_section_opening(
    lines: &[&str],
    heading_index: usize,
    title: &str,
) -> Option<(RegionSection, usize)> {
    if title.eq_ignore_ascii_case("other highlights")
        || title.eq_ignore_ascii_case("removed highlights")
    {
        return None;
    }
    let mut index = heading_index + 1;
    while index < lines.len() && lines[index].trim().is_empty() {
        index += 1;
    }
    let header_line = lines.get(index).copied()?;
    let (depth, content) = strip_blockquote_levels(header_line)?;
    if depth != 1 {
        return None;
    }
    let (callout, metadata, rest) = split_callout_header(content)?;
    if !callout.eq_ignore_ascii_case("summary") {
        return None;
    }
    let metadata = metadata?;
    if !metadata.eq_ignore_ascii_case("section") {
        return None;
    }
    let header_index = index;
    index += 1;
    while index < lines.len() && strip_blockquote_levels(lines[index]).is_some()
    {
        index += 1;
    }
    let card_lines = &lines[header_index..index];
    while index < lines.len() && lines[index].trim().is_empty() {
        index += 1;
    }
    let block_id = region_section_id(*lines.get(index)?)?;
    let stripped = card_lines
        .iter()
        .copied()
        .map(strip_blockquote_levels)
        .collect::<Option<Vec<_>>>()?;
    let remaining = stripped.get(1..).unwrap_or(&[]);
    let raw = format!("{}\n^{block_id}", card_lines.join("\n"));
    let body = parse_callout_body(remaining, &raw, 1).ok()?;
    let tag = rest
        .split_whitespace()
        .next()
        .filter(|token| token.starts_with('@'))
        .unwrap_or("")
        .to_string();
    let description = trim_blank_ends(&body.text_lines.join("\n"));
    let id = block_id.strip_prefix("s-").unwrap_or(&block_id).to_string();
    Some((
        RegionSection {
            id,
            block_id,
            title: title.to_string(),
            tag,
            description,
            images: body.images,
            unresolved_images: body.unresolved_images,
            unresolved_image_states: body.unresolved_image_states,
        },
        index,
    ))
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
    let stripped = chunk
        .iter()
        .map(|line| strip_blockquote_levels(line))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| raw.clone())?;
    let Some(((first_depth, first_content), remaining)) =
        stripped.split_first()
    else {
        return Err(raw);
    };
    if *first_depth != 1 {
        return Err(raw);
    }
    let (callout, metadata, rest) =
        split_callout_header(first_content).ok_or_else(|| raw.clone())?;
    if callout.eq_ignore_ascii_case("quote")
        && (rest == "Image" || rest.starts_with("Image "))
    {
        let asset =
            image_embed_target(&rest[5..]).ok_or_else(|| raw.clone())?;
        let body = parse_callout_body(remaining, &raw, 1)?;
        if body
            .text_lines
            .iter()
            .filter(|line| !line.is_empty())
            .count()
            > 1
        {
            return Err(raw);
        }
        return Ok(RegionBlock {
            page_label,
            kind: RegionBlockKind::Image,
            quote: None,
            comment: body.comment,
            asset: Some(asset),
            images: body.images,
            unresolved_images: body.unresolved_images,
            unresolved_image_states: body.unresolved_image_states,
            pieces: Vec::new(),
            block_id,
            mirror: false,
            in_preamble,
            section: None,
        });
    }
    if callout.eq_ignore_ascii_case("quote") {
        let follower_separators = continuation_follower_separators(remaining);
        if follower_separators.is_empty() {
            let body = parse_callout_body(remaining, &raw, 1)?;
            let quote = piece_quote(rest, body.text_lines);
            let mirror = is_marker_mirror_text(
                body.comment.as_deref().unwrap_or_default(),
            );
            return Ok(RegionBlock {
                page_label,
                kind: RegionBlockKind::Highlight,
                quote: Some(quote),
                comment: body.comment,
                asset: None,
                images: body.images,
                unresolved_images: body.unresolved_images,
                unresolved_image_states: body.unresolved_image_states,
                pieces: Vec::new(),
                block_id,
                mirror,
                in_preamble,
                section: None,
            });
        }

        let (first_blank, _) = follower_separators[0];
        let head_body = parse_callout_body(&remaining[..first_blank], &raw, 1)?;
        let mut pieces = vec![RegionPiece {
            page_label: metadata,
            quote: piece_quote(rest, head_body.text_lines),
            comment: head_body.comment,
            images: head_body.images,
            unresolved_images: head_body.unresolved_images,
            unresolved_image_states: head_body.unresolved_image_states,
        }];
        for (position, &(_, header_index)) in
            follower_separators.iter().enumerate()
        {
            let end = follower_separators
                .get(position + 1)
                .map_or(remaining.len(), |(blank, _)| *blank);
            let follower = &remaining[header_index..end];
            let Some((2, header)) = follower.first().copied() else {
                return Err(raw);
            };
            let (follower_kind, page_label, follower_rest) =
                split_callout_header(header).ok_or_else(|| raw.clone())?;
            if !follower_kind.eq_ignore_ascii_case("quote") {
                return Err(raw);
            }
            let body = parse_callout_body(&follower[1..], &raw, 2)?;
            pieces.push(RegionPiece {
                page_label,
                quote: piece_quote(follower_rest, body.text_lines),
                comment: body.comment,
                images: body.images,
                unresolved_images: body.unresolved_images,
                unresolved_image_states: body.unresolved_image_states,
            });
        }
        let quote = pieces
            .iter()
            .map(|piece| piece.quote.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let comments = pieces
            .iter()
            .filter_map(|piece| piece.comment.as_deref())
            .filter(|comment| !comment.is_empty())
            .collect::<Vec<_>>();
        let comment = (!comments.is_empty()).then(|| comments.join("\n\n"));
        let images = pieces
            .iter()
            .flat_map(|piece| piece.images.iter().cloned())
            .collect();
        let unresolved_images = pieces
            .iter()
            .flat_map(|piece| piece.unresolved_images.iter().cloned())
            .collect();
        let unresolved_image_states = pieces
            .iter()
            .flat_map(|piece| piece.unresolved_image_states.iter().cloned())
            .collect();
        let mirror = is_marker_mirror_text(
            pieces[0].comment.as_deref().unwrap_or_default(),
        );
        return Ok(RegionBlock {
            page_label,
            kind: RegionBlockKind::Highlight,
            quote: Some(quote),
            comment,
            asset: None,
            images,
            unresolved_images,
            unresolved_image_states,
            pieces,
            block_id,
            mirror,
            in_preamble,
            section: None,
        });
    }
    if callout.eq_ignore_ascii_case("note") {
        let body = parse_callout_body(remaining, &raw, 1)?;
        if body.comment.is_some() {
            return Err(raw);
        }
        let mut text_lines = vec![rest.to_string()];
        text_lines.extend(body.text_lines);
        let text = trim_blank_ends(&text_lines.join("\n"));
        return Ok(RegionBlock {
            page_label,
            kind: RegionBlockKind::Note,
            quote: None,
            comment: Some(text.clone()),
            asset: None,
            images: body.images,
            unresolved_images: body.unresolved_images,
            unresolved_image_states: body.unresolved_image_states,
            pieces: Vec::new(),
            block_id,
            mirror: is_marker_mirror_text(&text),
            in_preamble,
            section: None,
        });
    }
    Err(raw)
}

/// Split a callout header into its kind, optional Obsidian metadata, and the
/// first line's text after the header and one optional space.
fn strip_blockquote_levels(line: &str) -> Option<(usize, &str)> {
    let mut rest = line.trim_start();
    let mut depth = 0;
    while let Some(after_marker) = rest.strip_prefix('>') {
        depth += 1;
        rest = after_marker.strip_prefix(' ').unwrap_or(after_marker);
    }
    (depth > 0).then_some((depth, rest))
}

fn split_callout_header(
    content: &str,
) -> Option<(String, Option<String>, &str)> {
    let inner = content.strip_prefix("[!")?;
    let end = inner.find(']')?;
    let header = &inner[..end];
    let (name, metadata) = match header.split_once('|') {
        Some((name, metadata)) => (name, Some(metadata.to_string())),
        None => (header, None),
    };
    let rest = inner[end + 1..]
        .strip_prefix(' ')
        .unwrap_or(&inner[end + 1..]);
    Some((name.to_string(), metadata, rest))
}

fn piece_quote(rest: &str, body_lines: Vec<String>) -> String {
    let mut quote_lines = vec![rest.to_string()];
    quote_lines.extend(body_lines);
    trim_blank_ends(&quote_lines.join("\n"))
}

fn continuation_follower_separators(
    remaining: &[(usize, &str)],
) -> Vec<(usize, usize)> {
    remaining
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| {
            let (1, spacer) = pair[0] else { return None };
            let (2, header) = pair[1] else { return None };
            if !spacer.is_empty() {
                return None;
            }
            split_callout_header(header)
                .is_some_and(|(kind, _, _)| kind.eq_ignore_ascii_case("quote"))
                .then_some((index, index + 1))
        })
        .collect()
}

fn image_embed_target(rest: &str) -> Option<String> {
    let after = rest.trim().strip_prefix("![[")?;
    let end = after.find("]]")?;
    let target = after[..end].to_string();
    (!target.is_empty()).then_some(target)
}

struct ParsedCalloutBody {
    text_lines: Vec<String>,
    images: Vec<String>,
    unresolved_images: Vec<String>,
    unresolved_image_states: Vec<UnresolvedImageNoteState>,
    comment: Option<String>,
}

/// Split the lines after a quote, image, or note header into depth-1 text,
/// image-note embeds, depth-2 warning groups, and an optional trailing
/// Comment callout. Each depth-2 group starts after a bare `>` spacer.
fn parse_callout_body(
    remaining: &[(usize, &str)],
    raw: &str,
    base_depth: usize,
) -> RegionResult<ParsedCalloutBody> {
    let mut body = ParsedCalloutBody {
        text_lines: Vec::new(),
        images: Vec::new(),
        unresolved_images: Vec::new(),
        unresolved_image_states: Vec::new(),
        comment: None,
    };
    let mut index = 0usize;
    while index < remaining.len() {
        if body.comment.is_some() {
            return Err(raw.to_string());
        }
        let (depth, content) = remaining[index];
        if depth == base_depth
            && content.is_empty()
            && remaining
                .get(index + 1)
                .is_some_and(|(next, _)| *next == base_depth + 1)
        {
            let group_start = index + 1;
            let mut group_end = group_start + 1;
            while group_end < remaining.len()
                && remaining[group_end].0 == base_depth + 1
            {
                group_end += 1;
            }
            parse_nested_group(
                &remaining[group_start..group_end],
                raw,
                &mut body,
            )?;
            index = group_end;
            continue;
        }
        if depth != base_depth {
            return Err(raw.to_string());
        }
        if let Some(target) = image_note_embed_target(content) {
            body.images.push(target);
        } else {
            body.text_lines.push(content.to_string());
        }
        index += 1;
    }
    Ok(body)
}

fn parse_nested_group(
    group: &[(usize, &str)],
    raw: &str,
    body: &mut ParsedCalloutBody,
) -> RegionResult<()> {
    let Some((_, header)) = group.first() else {
        return Err(raw.to_string());
    };
    let (callout, _, label) =
        split_callout_header(header).ok_or_else(|| raw.to_string())?;
    if callout.eq_ignore_ascii_case("warning") {
        let state =
            warning_image_state(label).ok_or_else(|| raw.to_string())?;
        let rest = group[1..]
            .iter()
            .map(|(_, content)| *content)
            .collect::<Vec<_>>()
            .join("\n");
        let source = backticked_source(&rest).ok_or_else(|| raw.to_string())?;
        body.unresolved_images.push(source);
        body.unresolved_image_states.push(state);
        return Ok(());
    }
    if callout.eq_ignore_ascii_case("note") {
        body.comment = comment_text(
            Some(group.iter().map(|(_, content)| *content).collect()),
            raw,
        )?;
        return Ok(());
    }
    Err(raw.to_string())
}

fn warning_image_state(label: &str) -> Option<UnresolvedImageNoteState> {
    if label.eq_ignore_ascii_case("Image not found") {
        Some(UnresolvedImageNoteState::NotFound)
    } else if label.eq_ignore_ascii_case("Image unreadable") {
        Some(UnresolvedImageNoteState::Unreadable)
    } else if label.eq_ignore_ascii_case("Image type not supported") {
        Some(UnresolvedImageNoteState::Unsupported)
    } else {
        None
    }
}

fn backticked_source(text: &str) -> Option<String> {
    let start = text.find('`')?;
    let rest = &text[start + 1..];
    let end = rest.find('`')?;
    let source = &rest[..end];
    (!source.is_empty()).then(|| source.to_string())
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
    let (callout, _, label) =
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

/// Shared follow-up task parser: both the `## Tasks` section and the
/// vault-wide `🔖` locator strip the `🔖` link and inline fields identically.
pub(crate) fn parse_follow_up_task(line: &str) -> Option<RegionTask> {
    parse_region_task(line)
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

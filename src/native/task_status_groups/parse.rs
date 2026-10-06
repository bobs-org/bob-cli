//! Marker and badge parsing for task status grouping.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkerState {
    Valid(GroupKind),
    Mismatch,
    Malformed,
    Missing,
}

pub(super) fn group_marker_state(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> MarkerState {
    let Some(line_index) = first_nonblank_line_index(node, lines) else {
        return MarkerState::Missing;
    };
    match parse_group_marker(lines[line_index].content) {
        Some(Ok(kind)) => {
            if GroupKind::from_title(&node.title) == Some(kind) {
                MarkerState::Valid(kind)
            } else {
                MarkerState::Mismatch
            }
        }
        Some(Err(())) => MarkerState::Malformed,
        None => MarkerState::Missing,
    }
}

fn parse_group_marker(line: &str) -> Option<Result<GroupKind, ()>> {
    let inner = markdown::standalone_html_comment(line)?;
    let rest = inner.strip_prefix(MARKER_PREFIX)?;
    match GroupKind::from_marker_id(rest) {
        Some(kind) => Some(Ok(kind)),
        None => Some(Err(())),
    }
}

/// Whether `line` is a generated status-count badge row.
///
/// The row identifies itself by its strict generated grammar: four
/// fixed-order chips joined by `" · "`, each an unlinked `` `{emoji} {n}
/// {label}` `` code span or a linked `` [`{emoji} {n} {label}`]({anchor}) ``
/// chip. Trailing whitespace (and any stray `\r`) is ignored; leading
/// content is not, so prose look-alikes and list items never match.
pub(crate) fn is_badge_row(line: &str) -> bool {
    let line = line.trim_end();
    let chips: Vec<&str> = line.split(" \u{b7} ").collect();
    if chips.len() != 4 {
        return false;
    }
    for (chip, (emoji, label)) in chips.iter().zip(super::BADGE_CHIPS.iter()) {
        if !is_badge_chip(chip, emoji, label) {
            return false;
        }
    }
    true
}

fn is_badge_chip(chip: &str, emoji: &str, label: &str) -> bool {
    if let Some(rest) = chip.strip_prefix('[') {
        let Some(inner) = rest.strip_prefix('`') else {
            return false;
        };
        let Some(end) = inner.find('`') else {
            return false;
        };
        let (span, after) = inner.split_at(end);
        if !is_badge_span(span, emoji, label) {
            return false;
        }
        let after = &after[1..];
        let Some(after) = after.strip_prefix("](") else {
            return false;
        };
        let Some(anchor) = after.strip_suffix(')') else {
            return false;
        };
        return is_badge_anchor(anchor);
    }
    let Some(span) = chip.strip_prefix('`').and_then(|s| s.strip_suffix('`'))
    else {
        return false;
    };
    is_badge_span(span, emoji, label)
}

fn is_badge_span(span: &str, emoji: &str, label: &str) -> bool {
    let Some(rest) = span.strip_prefix(emoji) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(' ') else {
        return false;
    };
    let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    let rest = &rest[digits..];
    let Some(rest) = rest.strip_prefix(' ') else {
        return false;
    };
    rest == label
}

fn is_badge_anchor(anchor: &str) -> bool {
    !anchor.is_empty()
        && anchor.starts_with('#')
        && !anchor.chars().any(|c| c.is_whitespace() || c == ')')
}

/// Whether `line` is a legacy badge-ownership comment.
///
/// Kept permanently so old backups and unsynced machines self-heal on the
/// next rewrite: the grouping intake drops these lines and the emitter
/// regenerates a marker-free row in the slot.
pub(crate) fn is_legacy_badge_marker(line: &str) -> bool {
    markdown::standalone_html_comment(line)
        .is_some_and(|inner| inner.starts_with("bob:task-status-badges:"))
}

fn first_nonblank_line_index(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> Option<usize> {
    let body_lines = line_range_for_bytes(lines, node.body_span.clone());
    body_lines
        .into_iter()
        .find(|&index| !lines[index].content.trim().is_empty())
}

pub(super) fn stray_marker_in_exclusive(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> Option<GroupingSkipCode> {
    for span in exclusive_spans(node) {
        let line_range = line_range_for_bytes(lines, span);
        for index in line_range {
            if parse_group_marker(lines[index].content).is_some() {
                return Some(GroupingSkipCode::MalformedOwnership);
            }
        }
    }
    None
}

pub(super) fn badge_row_in_span(
    lines: &[SourceLine<'_>],
    span: Range<usize>,
) -> bool {
    line_range_for_bytes(lines, span).any(|index| {
        is_badge_row(lines[index].content)
            || is_legacy_badge_marker(lines[index].content)
    })
}

pub(super) fn adoptable_group_body(
    node: &HeadingNode,
    ctx: &TransformCtx<'_>,
) -> bool {
    if !node.children.is_empty() {
        return false;
    }
    let Ok(pieces) = parse_direct_pieces(
        ctx,
        node.body_span.clone(),
        node,
        &mut Vec::new(),
        false,
        false,
    ) else {
        return false;
    };
    pieces.iter().all(|piece| match piece {
        DirectPiece::Task(task) => task.movable,
        DirectPiece::Other { raw, .. } => raw.chars().all(char::is_whitespace),
        DirectPiece::Badges { .. } => false,
    })
}

fn exclusive_spans(node: &HeadingNode) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut cursor = node.body_span.start;
    for child in &node.children {
        if child.section_span.start > cursor {
            spans.push(cursor..child.section_span.start);
        }
        cursor = child.section_span.end;
    }
    if cursor < node.body_span.end {
        spans.push(cursor..node.body_span.end);
    }
    spans
}

pub(super) struct ParsedContainer<'a> {
    pub(super) intake: Vec<DirectPiece<'a>>,
    pub(super) groups: BTreeMap<GroupKind, ParsedGroup<'a>>,
    pub(super) item_warnings: Vec<GroupingWarning>,
    pub(super) has_badges: bool,
}

pub(super) struct ParsedGroup<'a> {
    pub(super) leading: &'a str,
    pub(super) trailing: &'a str,
    pub(super) pieces: Vec<DirectPiece<'a>>,
}

impl<'a> ParsedContainer<'a> {
    pub(super) fn has_groupable(&self) -> bool {
        self.iter_tasks()
            .any(|task| task.bucket.is_groupable() && task.movable)
    }

    fn iter_tasks(&self) -> impl Iterator<Item = &TaskBlock<'a>> {
        self.intake.iter().filter_map(DirectPiece::task).chain(
            self.groups.values().flat_map(|group| {
                group.pieces.iter().filter_map(DirectPiece::task)
            }),
        )
    }
}

pub(super) fn parse_container_regions<'a>(
    node: &'a HeadingNode,
    ctx: &TransformCtx<'a>,
    classified: &ClassifiedChildren<'a>,
) -> Result<ParsedContainer<'a>, GroupingSkipCode> {
    let mut item_warnings = Vec::new();
    let mut intake = Vec::new();
    for span in exclusive_spans(node) {
        let mut pieces = parse_direct_pieces(
            ctx,
            span,
            node,
            &mut item_warnings,
            true,
            true,
        )?;
        intake.append(&mut pieces);
    }
    let has_badges = intake
        .iter()
        .any(|piece| matches!(piece, DirectPiece::Badges { .. }));

    let mut groups = BTreeMap::new();
    for item in &classified.items {
        let ClassifiedChild::Group {
            kind,
            node: group_node,
        } = item
        else {
            continue;
        };
        let body_start = group_body_start(group_node, ctx.lines);
        let pieces = parse_direct_pieces(
            ctx,
            body_start..group_node.body_span.end,
            node,
            &mut item_warnings,
            true,
            false,
        )?;
        let (leading, trailing) = split_group_prose(ctx.contents, &pieces);
        groups.insert(
            *kind,
            ParsedGroup {
                leading,
                trailing,
                pieces,
            },
        );
    }

    Ok(ParsedContainer {
        intake,
        groups,
        item_warnings,
        has_badges,
    })
}

fn group_body_start(node: &HeadingNode, lines: &[SourceLine<'_>]) -> usize {
    let Some(index) = first_nonblank_line_index(node, lines) else {
        return node.body_span.start;
    };
    if parse_group_marker(lines[index].content).is_some() {
        lines[index].byte_range.end
    } else {
        node.body_span.start
    }
}

fn split_group_prose<'a>(
    contents: &'a str,
    pieces: &[DirectPiece<'a>],
) -> (&'a str, &'a str) {
    let first_task = pieces.iter().find_map(|piece| match piece {
        DirectPiece::Task(task) => Some(task.byte_range.start),
        DirectPiece::Other { .. } | DirectPiece::Badges { .. } => None,
    });
    let Some(first_task) = first_task else {
        if pieces.is_empty() {
            return ("", "");
        }
        let start = piece_start(&pieces[0]);
        let end = piece_end(pieces.last().expect("non-empty pieces"));
        return (&contents[start..end], "");
    };
    let leading_end = pieces
        .iter()
        .take_while(|piece| matches!(piece, DirectPiece::Other { .. }))
        .last()
        .map(piece_end)
        .unwrap_or(first_task);
    let leading_start = pieces.first().map(piece_start).unwrap_or(first_task);
    let trailing_start = pieces
        .iter()
        .rev()
        .take_while(|piece| matches!(piece, DirectPiece::Other { .. }))
        .last()
        .map(piece_start);
    let trailing_end = pieces.last().map(piece_end);
    let leading = if leading_start < leading_end && leading_end <= first_task {
        &contents[leading_start..leading_end]
    } else {
        ""
    };
    let trailing = match (trailing_start, trailing_end) {
        (Some(start), Some(end)) if start >= first_task && start < end => {
            &contents[start..end]
        }
        _ => "",
    };
    (leading, trailing)
}

fn piece_start(piece: &DirectPiece<'_>) -> usize {
    match piece {
        DirectPiece::Task(task) => task.byte_range.start,
        DirectPiece::Other { byte_range, .. } => byte_range.start,
        DirectPiece::Badges { byte_range } => byte_range.start,
    }
}

fn piece_end(piece: &DirectPiece<'_>) -> usize {
    match piece {
        DirectPiece::Task(task) => task.byte_range.end,
        DirectPiece::Other { byte_range, .. } => byte_range.end,
        DirectPiece::Badges { byte_range } => byte_range.end,
    }
}

#[derive(Clone)]
pub(super) enum DirectPiece<'a> {
    Task(TaskBlock<'a>),
    Other {
        raw: &'a str,
        byte_range: Range<usize>,
    },
    Badges {
        byte_range: Range<usize>,
    },
}

impl<'a> DirectPiece<'a> {
    pub(super) fn task(&self) -> Option<&TaskBlock<'a>> {
        match self {
            Self::Task(task) => Some(task),
            Self::Other { .. } | Self::Badges { .. } => None,
        }
    }
}

#[derive(Clone)]
pub(super) struct TaskBlock<'a> {
    pub(super) byte_range: Range<usize>,
    pub(super) start_line: usize,
    pub(super) raw: &'a str,
    pub(super) bucket: StatusBucket,
    pub(super) movable: bool,
}

fn parse_direct_pieces<'a>(
    ctx: &TransformCtx<'a>,
    span: Range<usize>,
    container: &HeadingNode,
    warnings: &mut Vec<GroupingWarning>,
    fail_on_ambiguous: bool,
    allow_badges: bool,
) -> Result<Vec<DirectPiece<'a>>, GroupingSkipCode> {
    if span.start >= span.end {
        return Ok(Vec::new());
    }
    let lines = ctx.lines;
    let contents = ctx.contents;
    let mask = ctx.mask;
    let classification = ctx.classification;
    let line_range = line_range_for_bytes(lines, span.clone());
    let mut pieces = Vec::new();
    let mut cursor = span.start;
    let mut index = line_range.start;

    while index < line_range.end {
        let line = &lines[index];
        if line.byte_range.start < span.start {
            index += 1;
            continue;
        }
        if line.byte_range.start >= span.end {
            break;
        }

        // A legacy marker is itself an HTML comment (and therefore masked),
        // so it is claimed before the mask skip, exactly like the old
        // marker branch. A badge row is a plain paragraph line: it is only
        // claimed outside masked spans, so a fenced look-alike survives as
        // prose.
        if is_legacy_badge_marker(line.content)
            || (!mask.contains(&index) && is_badge_row(line.content))
        {
            if !allow_badges {
                return Err(GroupingSkipCode::MisplacedBadgeRow);
            }
            flush_other(
                contents,
                &mut pieces,
                &mut cursor,
                line.byte_range.start,
            );
            let end = line.byte_range.end.min(span.end);
            pieces.push(DirectPiece::Badges {
                byte_range: line.byte_range.start..end,
            });
            cursor = end;
            index += 1;
            continue;
        }

        if mask.contains(&index) {
            index += 1;
            continue;
        }

        if markdown::is_blockquote_line(line.content)
            || markdown::is_indented_code_line(line.content)
            || markdown::html_comment_opens(line.content)
            || markdown::fence_marker(line.content).is_some()
            || markdown::atx_heading(line.content).is_some()
        {
            index += 1;
            continue;
        }

        if let Some(item) = parse_list_item(line.content) {
            let limit = line_range.end;
            let subtree =
                match collect_subtree(lines, index, limit, item.indent) {
                    Ok(subtree) => subtree,
                    Err(code)
                        if code == GroupingSkipCode::AmbiguousBoundary =>
                    {
                        if fail_on_ambiguous {
                            return Err(code);
                        }
                        index += 1;
                        continue;
                    }
                    Err(code) => return Err(code),
                };
            let end_line = subtree.end;
            let byte_range = lines[index].byte_range.start
                ..lines[end_line - 1].byte_range.end.min(span.end);

            if let Some(task) = matching_task(
                &item,
                classification,
                byte_range.clone(),
                contents,
            ) {
                flush_other(
                    contents,
                    &mut pieces,
                    &mut cursor,
                    byte_range.start,
                );
                let movable = !item.ordered;
                if item.ordered {
                    warnings.push(warning_at(
                        container,
                        GroupingSkipCode::UnsupportedOrderedList,
                        line.index + 1,
                    ));
                }
                pieces.push(DirectPiece::Task(TaskBlock {
                    byte_range: byte_range.clone(),
                    start_line: line.index + 1,
                    raw: &contents[byte_range.clone()],
                    bucket: task,
                    movable,
                }));
                cursor = byte_range.end;
                index = end_line;
                continue;
            }

            warn_nested_tasks(
                lines,
                index + 1..end_line,
                classification,
                container,
                warnings,
            );
            index = end_line;
            continue;
        }

        index += 1;
    }

    flush_other(contents, &mut pieces, &mut cursor, span.end);
    Ok(pieces)
}

fn matching_task(
    item: &ListItem<'_>,
    classification: &TaskClassification,
    _byte_range: Range<usize>,
    _contents: &str,
) -> Option<StatusBucket> {
    let symbol = item.checkbox?;
    classification
        .matches_filter(item.body)
        .then(|| classification.bucket(symbol))
}

fn warn_nested_tasks(
    lines: &[SourceLine<'_>],
    range: Range<usize>,
    classification: &TaskClassification,
    container: &HeadingNode,
    warnings: &mut Vec<GroupingWarning>,
) {
    for index in range {
        let Some(item) = parse_list_item(lines[index].content) else {
            continue;
        };
        if item.checkbox.is_some() && classification.matches_filter(item.body) {
            warnings.push(warning_at(
                container,
                GroupingSkipCode::NestedUnderOrdinaryItem,
                lines[index].index + 1,
            ));
        }
    }
}

fn flush_other<'a>(
    contents: &'a str,
    pieces: &mut Vec<DirectPiece<'a>>,
    cursor: &mut usize,
    next: usize,
) {
    if *cursor < next {
        pieces.push(DirectPiece::Other {
            raw: &contents[*cursor..next],
            byte_range: *cursor..next,
        });
        *cursor = next;
    }
}

struct Subtree {
    end: usize,
}

fn collect_subtree(
    lines: &[SourceLine<'_>],
    start: usize,
    limit: usize,
    indent: usize,
) -> Result<Subtree, GroupingSkipCode> {
    let mut index = start + 1;
    let mut last = start;
    let mut in_fence = None;

    while index < limit {
        let content = lines[index].content;
        if let Some(open) = in_fence {
            last = index;
            if markdown::closes_fence(content, open) {
                in_fence = None;
            }
            index += 1;
            continue;
        }

        if content.trim().is_empty() {
            let next_nonblank = (index + 1..limit)
                .find(|&candidate| !lines[candidate].content.trim().is_empty());
            if next_nonblank.is_some_and(|candidate| {
                leading_ws(lines[candidate].content) > indent
            }) {
                last = index;
                index += 1;
                continue;
            }
            break;
        }

        let line_indent = leading_ws(content);
        if line_indent > indent {
            last = index;
            if let Some(marker) = markdown::fence_marker(content) {
                in_fence = Some(marker);
            }
            index += 1;
            continue;
        }

        if markdown::atx_heading(content).is_some()
            || markdown::setext_underline(content).is_some()
            || parse_list_item(content).is_some()
            || markdown::is_blockquote_line(content)
            || markdown::fence_marker(content).is_some()
        {
            break;
        }
        return Err(GroupingSkipCode::AmbiguousBoundary);
    }

    if in_fence.is_some() {
        return Err(GroupingSkipCode::AmbiguousBoundary);
    }
    Ok(Subtree { end: last + 1 })
}

pub(super) struct ListItem<'a> {
    indent: usize,
    ordered: bool,
    checkbox: Option<char>,
    body: &'a str,
}

pub(super) fn parse_list_item(content: &str) -> Option<ListItem<'_>> {
    let indent = leading_ws(content);
    let rest = &content[indent..];
    let (after_marker, ordered) = parse_list_marker(rest)?;
    let bytes = after_marker.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    let after_ws = after_marker.trim_start();
    let checkbox = parse_checkbox(after_ws);
    let body = checkbox.map_or(after_ws, |(_, rest)| rest);
    Some(ListItem {
        indent,
        ordered,
        checkbox: checkbox.map(|(symbol, _)| symbol),
        body,
    })
}

fn parse_list_marker(rest: &str) -> Option<(&str, bool)> {
    let bytes = rest.as_bytes();
    if matches!(bytes.first(), Some(b'-' | b'*' | b'+')) {
        return Some((&rest[1..], false));
    }
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
        return None;
    }
    Some((&rest[digits + 1..], true))
}

fn parse_checkbox(rest: &str) -> Option<(char, &str)> {
    let after_open = rest.strip_prefix('[')?;
    let mut characters = after_open.chars();
    let symbol = characters.next()?;
    let after_symbol = &after_open[symbol.len_utf8()..];
    let after_close = after_symbol.strip_prefix(']')?;
    if !after_close.is_empty() && !after_close.starts_with(char::is_whitespace)
    {
        return None;
    }
    Some((symbol, after_close.trim_start()))
}

fn leading_ws(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

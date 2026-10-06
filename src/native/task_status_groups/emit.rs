//! Grouping plans, emission, and heading-tree helpers.
use super::*;

pub(super) struct GroupingPlan<'a> {
    pub(super) intake_staying: Vec<DirectPiece<'a>>,
    pub(super) recovered: Vec<&'a TaskBlock<'a>>,
    pub(super) groups: BTreeMap<GroupKind, Vec<&'a TaskBlock<'a>>>,
    pub(super) group_leading: BTreeMap<GroupKind, &'a str>,
    pub(super) group_trailing: BTreeMap<GroupKind, &'a str>,
    pub(super) open_count: usize,
    pub(super) counts: [usize; 3],
    pub(super) moved_blocks: Vec<MovedBlock>,
}

pub(super) fn grouping_plan<'a>(
    parsed: &'a ParsedContainer<'a>,
) -> GroupingPlan<'a> {
    let mut recovered = Vec::new();
    let mut groups: BTreeMap<GroupKind, Vec<&TaskBlock<'a>>> = BTreeMap::new();
    for kind in GroupKind::ALL {
        groups.insert(kind, Vec::new());
    }
    let mut moved_blocks = Vec::new();

    let mut document_tasks = Vec::new();
    for piece in &parsed.intake {
        if let DirectPiece::Task(task) = piece {
            document_tasks.push((Destination::Intake, task));
        }
    }
    for kind in GroupKind::ALL {
        if let Some(group) = parsed.groups.get(&kind) {
            for piece in &group.pieces {
                if let DirectPiece::Task(task) = piece {
                    document_tasks.push((Destination::from_group(kind), task));
                }
            }
        }
    }

    for (source, task) in document_tasks {
        let dest = if task.movable {
            task.bucket.destination()
        } else {
            source
        };
        if dest != source {
            moved_blocks.push(MovedBlock {
                original_line: task.start_line,
                destination: dest.label(),
            });
        }
        match dest.group() {
            Some(kind) => groups.get_mut(&kind).expect("group slot").push(task),
            None => {
                if source != Destination::Intake && task.movable {
                    recovered.push(task);
                }
            }
        }
    }

    let mut intake_staying = Vec::new();
    for piece in &parsed.intake {
        match piece {
            DirectPiece::Task(task) if task.movable => {
                if task.bucket.destination() == Destination::Intake {
                    intake_staying.push(piece.clone());
                }
            }
            DirectPiece::Task(_) | DirectPiece::Other { .. } => {
                intake_staying.push(piece.clone());
            }
            DirectPiece::Badges { .. } => {}
        }
    }

    let mut group_leading = BTreeMap::new();
    let mut group_trailing = BTreeMap::new();
    for kind in GroupKind::ALL {
        if let Some(group) = parsed.groups.get(&kind) {
            group_leading.insert(kind, group.leading);
            group_trailing.insert(kind, group.trailing);
        }
    }

    let counts = [
        groups.get(&GroupKind::Active).map_or(0, Vec::len),
        groups.get(&GroupKind::Blocked).map_or(0, Vec::len),
        groups.get(&GroupKind::Closed).map_or(0, Vec::len),
    ];
    let open_count =
        intake_staying.iter().filter_map(DirectPiece::task).count()
            + recovered.len();

    GroupingPlan {
        intake_staying,
        recovered,
        groups,
        group_leading,
        group_trailing,
        open_count,
        counts,
        moved_blocks,
    }
}

impl Destination {
    fn from_group(kind: GroupKind) -> Self {
        match kind {
            GroupKind::Active => Self::Active,
            GroupKind::Blocked => Self::Blocked,
            GroupKind::Closed => Self::Closed,
        }
    }
}

pub(super) fn emit_grouped_body(
    node: &HeadingNode,
    contents: &str,
    classified: &ClassifiedChildren<'_>,
    plan: &GroupingPlan<'_>,
    child_rewrites: &BTreeMap<usize, ContainerRewrite>,
    newline: &str,
    emit_decorations: bool,
    heading_has_line_ending: bool,
) -> String {
    let mut body = String::new();
    let intake = emit_intake(plan, newline);
    if emit_decorations {
        body.push_str(&render_badges(node, plan, newline));
    }
    if !intake.trim().is_empty() {
        let intake = if emit_decorations {
            trim_leading_blank_edges(&intake, newline)
        } else {
            intake.as_str()
        };
        push_block(&mut body, intake, newline);
    }

    for child in classified.authored() {
        let part = if let Some(rewrite) =
            child_rewrites.get(&child.heading_span.start)
        {
            rewrite.section.clone()
        } else {
            contents[child.section_span.clone()].to_string()
        };
        push_block(&mut body, &part, newline);
    }

    if emit_decorations {
        for kind in GroupKind::ALL {
            let group = emit_group(node.level + 1, kind, plan, newline);
            push_block(&mut body, &group, newline);
        }
    }

    if body.is_empty() {
        return String::new();
    }
    if emit_decorations && heading_has_line_ending {
        return body;
    }
    if !body.starts_with('\n') && !body.starts_with("\r\n") {
        let mut prefixed = String::from(newline);
        prefixed.push_str(&body);
        body = prefixed;
    }
    body
}

fn render_badges(
    node: &HeadingNode,
    plan: &GroupingPlan<'_>,
    newline: &str,
) -> String {
    let counts = [
        (plan.open_count, None),
        (plan.counts[0], Some(GroupKind::Active)),
        (plan.counts[1], Some(GroupKind::Blocked)),
        (plan.counts[2], Some(GroupKind::Closed)),
    ];
    let linked = !node.ancestry.iter().any(|segment| segment.contains('#'));
    let row = super::BADGE_CHIPS
        .iter()
        .zip(counts.iter())
        .map(|((emoji, label), (count, group))| {
            let label = format!("`{emoji} {count} {label}`");
            if !linked {
                return label;
            }
            let anchor = match group {
                Some(kind) => {
                    let mut ancestry = node.ancestry.clone();
                    ancestry.push(kind.title().to_string());
                    render_anchor(&ancestry)
                }
                None => render_anchor(&node.ancestry),
            };
            format!("[{label}]({anchor})")
        })
        .collect::<Vec<_>>()
        .join(" \u{b7} ");

    format!("{row}{newline}{newline}")
}

fn render_anchor(segments: &[String]) -> String {
    let mut anchor = String::new();
    for segment in segments {
        anchor.push('#');
        anchor.push_str(&encode_anchor_segment(segment));
    }
    anchor
}

pub(super) fn encode_anchor_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for character in segment.chars() {
        let should_encode = character.is_ascii()
            && (character.is_control()
                || matches!(
                    character,
                    ' ' | '(' | ')' | '<' | '>' | '"' | '%' | '\\'
                ));
        if should_encode {
            let mut bytes = [0; 4];
            for byte in character.encode_utf8(&mut bytes).as_bytes() {
                write!(&mut encoded, "%{byte:02X}").expect("write to string");
            }
        } else {
            encoded.push(character);
        }
    }
    encoded
}

fn trim_leading_blank_edges<'a>(mut raw: &'a str, newline: &str) -> &'a str {
    while let Some(stripped) = raw.strip_prefix(newline) {
        raw = stripped;
    }
    raw
}

fn emit_intake(plan: &GroupingPlan<'_>, newline: &str) -> String {
    let mut buf = String::new();
    let mut last_was_task = false;
    let mut inserted_recovered = false;

    for piece in &plan.intake_staying {
        match piece {
            DirectPiece::Other { raw, .. } => {
                if last_was_task
                    && !inserted_recovered
                    && !plan.recovered.is_empty()
                {
                    for task in &plan.recovered {
                        push_task(&mut buf, task.raw, newline);
                    }
                    inserted_recovered = true;
                }
                buf.push_str(raw);
                last_was_task = false;
            }
            DirectPiece::Task(task) => {
                push_task(&mut buf, task.raw, newline);
                last_was_task = true;
            }
            DirectPiece::Badges { .. } => {}
        }
    }
    if !inserted_recovered {
        if !last_was_task && !plan.recovered.is_empty() && !buf.is_empty() {
            ensure_single_trailing_newline(&mut buf, newline);
        }
        for task in &plan.recovered {
            push_task(&mut buf, task.raw, newline);
        }
    }
    collapse_internal_blank_runs(&mut buf, newline);
    buf
}

fn emit_group(
    level: usize,
    kind: GroupKind,
    plan: &GroupingPlan<'_>,
    newline: &str,
) -> String {
    let hashes = "#".repeat(level);
    let mut buf = format!(
        "{hashes} {title}{newline}{marker}{newline}",
        title = kind.title(),
        marker = kind.marker()
    );
    let leading = plan.group_leading.get(&kind).copied().unwrap_or("");
    let trailing = plan.group_trailing.get(&kind).copied().unwrap_or("");
    let tasks = plan.groups.get(&kind).map_or(&[][..], Vec::as_slice);
    let leading = trim_blank_edges(leading, newline);
    let trailing = trim_blank_edges(trailing, newline);
    if !leading.is_empty() || !tasks.is_empty() || !trailing.is_empty() {
        buf.push_str(newline);
    }
    if !leading.is_empty() {
        buf.push_str(leading);
        if !leading.ends_with('\n') {
            buf.push_str(newline);
        }
        if !tasks.is_empty() {
            ensure_single_trailing_newline(&mut buf, newline);
        }
    }
    for task in tasks {
        push_task(&mut buf, task.raw, newline);
    }
    if !trailing.is_empty() {
        ensure_trailing_blank(&mut buf, newline);
        buf.push_str(trailing);
        if !trailing.ends_with('\n') {
            buf.push_str(newline);
        }
    }
    buf
}

fn push_task(buf: &mut String, raw: &str, newline: &str) {
    buf.push_str(raw);
    if !raw.ends_with('\n') {
        buf.push_str(newline);
    }
}

fn push_block(buf: &mut String, part: &str, newline: &str) {
    if part.is_empty() {
        return;
    }
    if !buf.is_empty() {
        ensure_trailing_blank(buf, newline);
    }
    buf.push_str(part);
}

fn ensure_trailing_blank(buf: &mut String, newline: &str) {
    let double = format!("{newline}{newline}");
    if buf.ends_with(&double) {
        return;
    }
    if buf.ends_with(newline) {
        buf.push_str(newline);
    } else if !buf.is_empty() {
        buf.push_str(newline);
        buf.push_str(newline);
    }
}

fn ensure_single_trailing_newline(buf: &mut String, newline: &str) {
    if buf.is_empty() {
        return;
    }
    if !buf.ends_with('\n') {
        buf.push_str(newline);
    }
}

fn trim_blank_edges<'a>(raw: &'a str, newline: &str) -> &'a str {
    let mut value = raw;
    while let Some(stripped) = value.strip_prefix(newline) {
        value = stripped;
    }
    while let Some(stripped) = value.strip_suffix(newline) {
        if stripped.ends_with('\n') || stripped.is_empty() {
            value = stripped;
        } else {
            break;
        }
    }
    value
}

fn collapse_internal_blank_runs(buf: &mut String, newline: &str) {
    let triple = format!("{newline}{newline}{newline}");
    let double = format!("{newline}{newline}");
    while buf.contains(&triple) {
        *buf = buf.replace(&triple, &double);
    }
}

pub(super) fn section_newline(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> &'static str {
    let heading_lines = line_range_for_bytes(lines, node.heading_span.clone());
    heading_lines
        .into_iter()
        .rev()
        .find_map(|index| match lines[index].ending {
            "\r\n" => Some("\r\n"),
            "\n" => Some("\n"),
            _ => None,
        })
        .or_else(|| {
            line_range_for_bytes(lines, node.body_span.clone())
                .into_iter()
                .find_map(|index| match lines[index].ending {
                    "\r\n" => Some("\r\n"),
                    "\n" => Some("\n"),
                    _ => None,
                })
        })
        .unwrap_or("\n")
}

pub(super) fn apply_final_newline(output: &mut String, original: &str) {
    let original_nl = original.ends_with('\n');
    let output_nl = output.ends_with('\n');
    if original_nl && !output_nl {
        if original.ends_with("\r\n") {
            output.push_str("\r\n");
        } else {
            output.push('\n');
        }
    } else if !original_nl && output_nl {
        if output.ends_with("\r\n") {
            output.truncate(output.len() - 2);
        } else {
            output.pop();
        }
    }
}

pub(super) fn is_tasks_title(title: &str) -> bool {
    title.eq_ignore_ascii_case("Tasks")
}

pub(super) struct SourceLine<'a> {
    pub(super) index: usize,
    pub(super) byte_range: Range<usize>,
    pub(super) content: &'a str,
    pub(super) ending: &'a str,
}

pub(super) fn source_lines(contents: &str) -> Vec<SourceLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (offset, byte) in contents.bytes().enumerate() {
        if byte != b'\n' {
            continue;
        }
        let end = offset + 1;
        let raw = &contents[start..end];
        let (content, ending) = markdown::split_line_ending(raw);
        lines.push(SourceLine {
            index: lines.len(),
            byte_range: start..end,
            content,
            ending,
        });
        start = end;
    }
    if start < contents.len() {
        lines.push(SourceLine {
            index: lines.len(),
            byte_range: start..contents.len(),
            content: &contents[start..],
            ending: "",
        });
    }
    lines
}

pub(super) fn heading_mask(
    lines: &[SourceLine<'_>],
) -> std::collections::BTreeSet<usize> {
    let logical = lines.iter().map(|line| line.content).collect::<Vec<_>>();
    let mut masked = std::collections::BTreeSet::new();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&logical);
    let scan_start = frontmatter_end.map_or(0, |end| {
        for index in 0..=end {
            masked.insert(index);
        }
        end + 1
    });
    for index in markdown::fenced_lines(&logical, scan_start..lines.len()) {
        masked.insert(index);
    }

    let mut index = scan_start;
    while index < lines.len() {
        if masked.contains(&index) {
            index += 1;
            continue;
        }
        let content = lines[index].content;
        if markdown::html_comment_opens(content) {
            masked.insert(index);
            if !markdown::html_comment_closes(content) {
                index += 1;
                while index < lines.len() {
                    masked.insert(index);
                    let closes =
                        markdown::html_comment_closes(lines[index].content);
                    index += 1;
                    if closes {
                        break;
                    }
                }
                continue;
            }
        }
        index += 1;
    }
    masked
}

pub(super) struct FlatHeading {
    level: usize,
    title: String,
    heading_line: usize,
    heading_span: Range<usize>,
}

pub(super) fn discover_headings(
    lines: &[SourceLine<'_>],
    mask: &std::collections::BTreeSet<usize>,
) -> Vec<FlatHeading> {
    let mut headings = Vec::new();
    let mut previous_plain: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        if mask.contains(&index) {
            previous_plain = None;
            continue;
        }
        if line.content.trim().is_empty() {
            previous_plain = None;
            continue;
        }
        if markdown::is_blockquote_line(line.content)
            || markdown::is_indented_code_line(line.content)
        {
            previous_plain = None;
            continue;
        }
        if let Some((level, title)) = markdown::atx_heading(line.content) {
            headings.push(FlatHeading {
                level,
                title: title.to_string(),
                heading_line: index + 1,
                heading_span: line.byte_range.clone(),
            });
            previous_plain = None;
            continue;
        }
        if let Some(level) = markdown::setext_underline(line.content)
            && let Some(previous) = previous_plain.take()
        {
            headings.push(FlatHeading {
                level,
                title: lines[previous].content.trim().to_string(),
                heading_line: previous + 1,
                heading_span: lines[previous].byte_range.start
                    ..line.byte_range.end,
            });
            continue;
        }
        if parse_list_item(line.content).is_some() {
            previous_plain = None;
            continue;
        }
        previous_plain = Some(index);
    }
    headings
}

pub(super) struct HeadingNode {
    pub(super) level: usize,
    pub(super) title: String,
    pub(super) heading_line: usize,
    pub(super) heading_span: Range<usize>,
    pub(super) body_span: Range<usize>,
    pub(super) section_span: Range<usize>,
    pub(super) ancestry: Vec<String>,
    pub(super) children: Vec<HeadingNode>,
}

pub(super) fn build_tree(
    contents_len: usize,
    flats: Vec<FlatHeading>,
) -> Vec<HeadingNode> {
    let ends = section_ends(contents_len, &flats);
    nest_headings(&flats, &ends, 0..flats.len(), Vec::new())
}

fn section_ends(contents_len: usize, flats: &[FlatHeading]) -> Vec<usize> {
    let mut ends = vec![contents_len; flats.len()];
    for (index, heading) in flats.iter().enumerate() {
        if let Some(next) = flats[index + 1..]
            .iter()
            .find(|candidate| candidate.level <= heading.level)
        {
            ends[index] = next.heading_span.start;
        }
    }
    ends
}

fn nest_headings(
    flats: &[FlatHeading],
    ends: &[usize],
    range: Range<usize>,
    parent_ancestry: Vec<String>,
) -> Vec<HeadingNode> {
    let mut nodes = Vec::new();
    let mut index = range.start;
    while index < range.end {
        let heading = &flats[index];
        let mut child_end = index + 1;
        while child_end < range.end && flats[child_end].level > heading.level {
            child_end += 1;
        }
        let mut ancestry = parent_ancestry.clone();
        ancestry.push(heading.title.clone());
        let children =
            nest_headings(flats, ends, index + 1..child_end, ancestry.clone());
        nodes.push(HeadingNode {
            level: heading.level,
            title: heading.title.clone(),
            heading_line: heading.heading_line,
            heading_span: heading.heading_span.clone(),
            body_span: heading.heading_span.end..ends[index],
            section_span: heading.heading_span.start..ends[index],
            ancestry,
            children,
        });
        index = child_end;
    }
    nodes
}

pub(super) fn line_range_for_bytes(
    lines: &[SourceLine<'_>],
    span: Range<usize>,
) -> Range<usize> {
    if span.start >= span.end {
        return 0..0;
    }
    let start = lines
        .iter()
        .position(|line| line.byte_range.end > span.start)
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .position(|line| line.byte_range.start >= span.end)
        .unwrap_or(lines.len());
    start..end
}

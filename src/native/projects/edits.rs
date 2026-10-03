//! Project edit application and Markdown rendering.
use super::*;

pub(super) fn apply_project_changes(
    contents: &str,
    changes: &[ProjectChange],
    desired_subprojects: Option<&[SubprojectEntry]>,
) -> Result<String, String> {
    let mut edits = Vec::new();
    let mut sync_subprojects = false;
    let reconcile_task_schedules = changes.iter().any(|change| {
        matches!(change, ProjectChange::ReconcileTaskSchedules { .. })
    });
    let remove_prj_scheduled = changes
        .iter()
        .any(|change| matches!(change, ProjectChange::RemoveScheduled { .. }));
    let prj_hide_action = changes.iter().find_map(|change| match change {
        ProjectChange::RemoveHideTag => Some(false),
        ProjectChange::AddHideTag { .. } => Some(true),
        _ => None,
    });
    for change in changes {
        match change {
            ProjectChange::Status { to, .. } => {
                edits.push(status_edit(contents, to.label())?);
            }
            ProjectChange::RemoveHideTag => {
                if !remove_prj_scheduled {
                    edits.push(remove_prj_hide_tag_edit(contents)?);
                }
            }
            ProjectChange::AddHideTag { .. } => {
                if !remove_prj_scheduled {
                    edits.push(add_hide_tag_edit(contents)?);
                }
            }
            ProjectChange::ReconcileTaskSchedules {
                scheduled, policy, ..
            } => {
                edits.extend(task_schedule_edits(
                    contents,
                    scheduled,
                    *policy,
                    remove_prj_scheduled,
                )?);
            }
            ProjectChange::RemoveScheduled { .. } => {
                if !reconcile_task_schedules {
                    edits.push(prj_metadata_edit(
                        contents,
                        true,
                        prj_hide_action,
                    )?);
                }
            }
            ProjectChange::AddSubprojectLink { .. }
            | ProjectChange::RemoveSubprojectLink { .. }
            | ProjectChange::MarkSubproject { .. }
            | ProjectChange::AddSubprojectScheduleMarker { .. }
            | ProjectChange::RemoveSubprojectScheduleMarker { .. }
            | ProjectChange::NormalizeSubprojects => {
                sync_subprojects = true;
            }
        }
    }
    if sync_subprojects {
        let desired_subprojects = desired_subprojects.ok_or_else(|| {
            "failed to resolve sub-project line state".to_string()
        })?;
        edits.extend(sync_subprojects_line_edits(
            contents,
            desired_subprojects,
        )?);
    }

    edits.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| right.end.cmp(&left.end))
    });

    for pair in edits.windows(2) {
        let later = &pair[0];
        let earlier = &pair[1];
        if later.start < earlier.end {
            return Err(format!(
                "overlapping project edits at byte ranges {}..{} and {}..{}",
                earlier.start, earlier.end, later.start, later.end
            ));
        }
    }

    let mut output = contents.to_string();
    for edit in edits {
        output.replace_range(edit.start..edit.end, &edit.replacement);
    }
    Ok(output)
}

pub(super) fn task_schedule_edits(
    contents: &str,
    scheduled: &str,
    policy: TaskSchedulePolicy,
    remove_prj_scheduled: bool,
) -> Result<Vec<TextEdit>, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    let lines = line_spans(contents);
    let mut fence = None;
    let mut edits = Vec::new();

    for line in &lines {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = trim_cr(&contents[line.start..line.end]);
        if markdown_fence_line(line_text, &mut fence) {
            continue;
        }
        let Some(task) = parse_task_line(line_text) else {
            continue;
        };
        let is_prj = is_valid_prj_task_line(line_text, task);
        let scheduled_fields = inline_field_spans(task.text, "scheduled");
        if !is_prj && scheduled_fields.len() > 1 {
            continue;
        }
        let mut replacement = line_text.to_string();
        if is_prj {
            replacement = normalize_task_hide_tag(
                &replacement,
                policy.future,
                policy.include_prj,
            );
            if remove_prj_scheduled {
                replacement =
                    remove_all_inline_fields(&replacement, "scheduled");
            }
        } else {
            replacement = remove_all_task_tags(&replacement, HIDE_TAG);
            if is_propagated_schedule_mark(task.mark) {
                replacement = upsert_task_scheduled(
                    &replacement,
                    scheduled,
                    policy.scheduled,
                );
            }
        }
        if replacement != line_text {
            if contents[line.start..line.end].ends_with('\r') {
                replacement.push('\r');
            }
            edits.push(TextEdit {
                start: line.start,
                end: line.end,
                replacement,
            });
        }
    }

    Ok(edits)
}

pub(super) fn normalize_task_hide_tag(
    line: &str,
    future: bool,
    include_prj: bool,
) -> String {
    if !include_prj {
        return line.to_string();
    }
    let Some(task) = parse_task_line(line) else {
        return line.to_string();
    };
    let task_text_offset = task.text.as_ptr() as usize - line.as_ptr() as usize;
    let spans = tag_spans(task.text, HIDE_TAG);
    if !future {
        return remove_tag_spans_from_line(line, task_text_offset, &spans);
    }
    if spans.is_empty() {
        let insertion = task_metadata_insertion_offset(line);
        let mut output = line.to_string();
        output.insert_str(insertion, &format!(" {HIDE_TAG}"));
        return output;
    }
    remove_tag_spans_from_line(line, task_text_offset, &spans[1..])
}

pub(super) fn remove_all_task_tags(line: &str, tag: &str) -> String {
    let Some(task) = parse_task_line(line) else {
        return line.to_string();
    };
    let task_text_offset = task.text.as_ptr() as usize - line.as_ptr() as usize;
    let spans = tag_spans(task.text, tag);
    let removed_trailing_tag = spans
        .last()
        .is_some_and(|(_, end)| *end == task.text.trim_end().len());
    let output = remove_tag_spans_from_line(line, task_text_offset, &spans);
    if removed_trailing_tag {
        output.trim_end().to_string()
    } else {
        output
    }
}

fn upsert_task_scheduled(
    line: &str,
    scheduled: &str,
    scheduled_date: NaiveDate,
) -> String {
    let Some(task) = parse_task_line(line) else {
        return line.to_string();
    };
    let fields = inline_field_spans(task.text, "scheduled");
    if fields.len() > 1 {
        return line.to_string();
    }
    let task_text_offset = task.text.as_ptr() as usize - line.as_ptr() as usize;
    let field_text = format!("[scheduled:: {scheduled}]");
    if let Some(field) = fields.first().copied() {
        let value = task.text[field.value_start..field.value_end].trim();
        if parse_inline_schedule_date(value)
            .is_some_and(|existing| existing >= scheduled_date)
        {
            return line.to_string();
        }
        let mut output = line.to_string();
        output.replace_range(
            task_text_offset + field.start..task_text_offset + field.end,
            &field_text,
        );
        return output;
    }

    let insertion = task_metadata_insertion_offset(line);
    let before = line[..insertion].trim_end();
    let after = &line[insertion..];
    format!("{before} {field_text}{after}")
}

pub(crate) fn remove_all_inline_fields(line: &str, key: &str) -> String {
    let spans = inline_field_spans(line, key);
    let raw_spans = spans
        .iter()
        .map(|field| (field.start, field.end))
        .collect::<Vec<_>>();
    remove_tag_spans_from_line(line, 0, &raw_spans)
}

pub(super) fn remove_tag_spans_from_line(
    line: &str,
    task_text_offset: usize,
    spans: &[(usize, usize)],
) -> String {
    let mut removal_ranges = spans
        .iter()
        .map(|(start, end)| {
            inline_field_removal_range(
                line,
                task_text_offset + start,
                task_text_offset + end,
            )
        })
        .collect::<Vec<_>>();
    removal_ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for range in removal_ranges {
        if let Some(last) = merged.last_mut()
            && range.0 <= last.1
        {
            last.1 = last.1.max(range.1);
        } else {
            merged.push(range);
        }
    }
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in merged {
        output.push_str(&line[cursor..start]);
        cursor = end;
    }
    output.push_str(&line[cursor..]);
    output
}

pub(crate) fn task_metadata_insertion_offset(line: &str) -> usize {
    let trimmed = line.trim_end();
    let without_block_id = strip_trailing_block_id(trimmed);
    if without_block_id.len() < trimmed.len() {
        without_block_id.len()
    } else {
        trimmed.len()
    }
}

pub(super) fn status_edit(
    contents: &str,
    status: &str,
) -> Result<TextEdit, String> {
    let layout = frontmatter_layout(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;

    if let Some(status_line) = layout.status_line {
        return status_value_edit(contents, status_line, status)
            .ok_or_else(|| "failed to locate status value".to_string());
    }

    let type_line = layout
        .type_line
        .ok_or_else(|| "failed to locate project type line".to_string())?;
    Ok(TextEdit {
        start: type_line.next_start,
        end: type_line.next_start,
        replacement: format!(
            "status: {status}{}",
            line_ending(contents, type_line)
        ),
    })
}

pub(super) fn status_value_edit(
    contents: &str,
    line: LineSpan,
    status: &str,
) -> Option<TextEdit> {
    let line_text = trim_cr(&contents[line.start..line.end]);
    let trimmed = line_text.trim_start();
    let leading_width = line_text.len() - trimmed.len();
    let rest = trimmed.strip_prefix("status")?.strip_prefix(':')?;
    let value_offset = leading_width + "status:".len();
    let leading_value_width = rest.len() - rest.trim_start().len();
    let value_start = value_offset + leading_value_width;
    let value_end = value_offset + rest.trim_end().len();
    let replacement = if leading_value_width == 0 {
        format!(" {status}")
    } else {
        status.to_string()
    };

    Some(TextEdit {
        start: line.start + value_start,
        end: line.start + value_end,
        replacement,
    })
}

pub(super) fn add_hide_tag_edit(contents: &str) -> Result<TextEdit, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    for line in line_spans(contents) {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = &contents[line.start..line.end];
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        let anchor_start = line_text
            .rfind("^prj")
            .ok_or_else(|| "failed to locate ^prj anchor".to_string())?;
        return Ok(TextEdit {
            start: line.start + anchor_start,
            end: line.start + anchor_start,
            replacement: format!("{HIDE_TAG} "),
        });
    }

    Err("failed to locate ^prj task".to_string())
}

pub(super) fn remove_prj_hide_tag_edit(
    contents: &str,
) -> Result<TextEdit, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    for line in line_spans(contents) {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = &contents[line.start..line.end];
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        let Some((tag_start, tag_end)) = hide_tag_span(line_text) else {
            continue;
        };
        let (start, end) =
            inline_field_removal_range(line_text, tag_start, tag_end);
        return Ok(TextEdit {
            start: line.start + start,
            end: line.start + end,
            replacement: String::new(),
        });
    }

    Err("failed to locate #hide tag on ^prj task".to_string())
}

pub(super) fn prj_metadata_edit(
    contents: &str,
    remove_scheduled: bool,
    hide: Option<bool>,
) -> Result<TextEdit, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    for line in line_spans(contents) {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = &contents[line.start..line.end];
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        let mut replacement = line_text.to_string();
        if remove_scheduled {
            replacement = remove_all_inline_fields(&replacement, "scheduled");
        }
        if let Some(hide) = hide {
            replacement = normalize_task_hide_tag(&replacement, hide, true);
        }
        if contents[line.start..line.end].ends_with('\r') {
            replacement.push('\r');
        }
        return Ok(TextEdit {
            start: line.start,
            end: line.end,
            replacement,
        });
    }

    Err("failed to locate ^prj task".to_string())
}

pub(super) fn sync_subprojects_line_edits(
    contents: &str,
    desired_entries: &[SubprojectEntry],
) -> Result<Vec<TextEdit>, String> {
    let layout = prj_sub_block_layout(contents)?;
    let first_marker = layout.sub_block.first_marker_line();

    let marker_line_numbers = layout
        .sub_block
        .lines
        .iter()
        .filter(|line| line.is_marker)
        .map(|line| line.line_number)
        .collect::<Vec<_>>();
    let mut edits = Vec::new();

    if desired_entries.is_empty() {
        for line_number in marker_line_numbers {
            let line = line_by_number(&layout.lines, line_number)?;
            edits.push(TextEdit {
                start: line.start,
                end: line.next_start,
                replacement: String::new(),
            });
        }
        return Ok(edits);
    }

    let rendered = render_subprojects_line(&layout.prj_indent, desired_entries);

    if let Some(marker_line) = first_marker {
        let line = line_by_number(&layout.lines, marker_line.line_number)?;
        edits.push(TextEdit {
            start: line.start,
            end: line_content_end(contents, line),
            replacement: rendered,
        });
        for line_number in marker_line_numbers
            .into_iter()
            .filter(|line_number| *line_number != marker_line.line_number)
        {
            let line = line_by_number(&layout.lines, line_number)?;
            edits.push(TextEdit {
                start: line.start,
                end: line.next_start,
                replacement: String::new(),
            });
        }
    } else {
        let ending = line_ending(contents, layout.prj_line);
        let prj_has_ending = layout.prj_line.next_start > layout.prj_line.end;
        let replacement = if prj_has_ending {
            format!("{rendered}{ending}")
        } else {
            format!("{ending}{rendered}")
        };
        edits.push(TextEdit {
            start: layout.prj_line.next_start,
            end: layout.prj_line.next_start,
            replacement,
        });
    }

    Ok(edits)
}

pub(super) fn line_by_number(
    lines: &[LineSpan],
    line_number: usize,
) -> Result<LineSpan, String> {
    lines
        .iter()
        .find(|line| line.line_number == line_number)
        .copied()
        .ok_or_else(|| "failed to locate sub-project marker line".to_string())
}

pub(super) fn line_content_end(contents: &str, line: LineSpan) -> usize {
    if line.end > line.start && contents.as_bytes()[line.end - 1] == b'\r' {
        line.end - 1
    } else {
        line.end
    }
}

pub(super) fn render_subprojects_line(
    indent: &str,
    entries: &[SubprojectEntry],
) -> String {
    format!("{}\t{}", indent, render_subprojects_line_text(entries))
}

pub(super) fn render_subprojects_line_text(
    entries: &[SubprojectEntry],
) -> String {
    let links = sorted_subproject_entries(entries)
        .into_iter()
        .map(render_subproject_entry)
        .collect::<Vec<_>>()
        .join(&format!(" {SUBPROJECTS_SEPARATOR} "));
    format!("- {SUBPROJECTS_MARKER_PREFIX} {links}")
}

pub(super) fn sorted_subproject_entries(
    entries: &[SubprojectEntry],
) -> Vec<&SubprojectEntry> {
    let mut open_entries = entries
        .iter()
        .filter(|entry| entry.state.is_open())
        .collect::<Vec<_>>();
    let mut closed_entries = entries
        .iter()
        .filter(|entry| entry.state.is_terminal())
        .collect::<Vec<_>>();
    open_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    closed_entries.sort_by(|left, right| left.link_name.cmp(&right.link_name));
    open_entries.extend(closed_entries);
    open_entries
}

pub(super) fn render_subproject_entry(entry: &SubprojectEntry) -> String {
    let link = match entry.state.closed_marker() {
        Some(marker) => format!("~~[[{}]]~~ {marker}", entry.stem),
        None => format!("[[{}]]", entry.stem),
    };
    if entry.future_scheduled {
        format!("{SUBPROJECT_FUTURE_SCHEDULE_MARKER} {link}")
    } else {
        link
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrjSubBlockLayout {
    pub(super) lines: Vec<LineSpan>,
    pub(super) prj_line: LineSpan,
    pub(super) prj_indent: String,
    pub(super) sub_block: PrjSubBlock,
}

pub(super) fn prj_sub_block_layout(
    contents: &str,
) -> Result<PrjSubBlockLayout, String> {
    let frontmatter = parse_frontmatter(contents)
        .ok_or_else(|| "failed to locate project frontmatter".to_string())?;
    let lines = line_spans(contents);
    for (index, line) in lines.iter().enumerate() {
        if line.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line_text = trim_cr(&contents[line.start..line.end]);
        if !has_trailing_prj_anchor(line_text) {
            continue;
        }
        return Ok(PrjSubBlockLayout {
            lines: lines.clone(),
            prj_line: *line,
            prj_indent: leading_whitespace(line_text).to_string(),
            sub_block: parse_prj_sub_block(contents, &lines, index),
        });
    }

    Err("failed to locate ^prj task".to_string())
}

fn inline_field_removal_range(
    line_text: &str,
    field_start: usize,
    field_end: usize,
) -> (usize, usize) {
    let bytes = line_text.as_bytes();

    let mut after = field_end;
    while after < bytes.len() && is_inline_field_space(bytes[after]) {
        after += 1;
    }
    if after > field_end {
        return (field_start, after);
    }

    let mut before = field_start;
    while before > 0 && is_inline_field_space(bytes[before - 1]) {
        before -= 1;
    }
    (before, field_end)
}

pub(super) fn frontmatter_layout(contents: &str) -> Option<FrontmatterLayout> {
    let lines = line_spans(contents);
    let first = lines.first()?;
    if trim_cr(&contents[first.start..first.end]) != "---" {
        return None;
    }

    let mut type_line = None;
    let mut status_line = None;
    for line in lines.iter().skip(1) {
        let line_text = trim_cr(&contents[line.start..line.end]);
        if line_text == "---" {
            return Some(FrontmatterLayout {
                body_start_line: line.line_number,
                type_line,
                status_line,
            });
        }
        if frontmatter_line_has_key(line_text, "type") {
            type_line = Some(*line);
        }
        if frontmatter_line_has_key(line_text, "status") {
            status_line = Some(*line);
        }
    }

    None
}

pub(super) fn frontmatter_line_has_key(line: &str, key: &str) -> bool {
    line.strip_prefix(key)
        .is_some_and(|rest| rest.starts_with(':'))
}

pub(super) fn line_spans(contents: &str) -> Vec<LineSpan> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut line_number = 1;

    for (index, byte) in contents.bytes().enumerate() {
        if byte != b'\n' {
            continue;
        }
        lines.push(LineSpan {
            line_number,
            start,
            end: index,
            next_start: index + 1,
        });
        start = index + 1;
        line_number += 1;
    }

    if start < contents.len() {
        lines.push(LineSpan {
            line_number,
            start,
            end: contents.len(),
            next_start: contents.len(),
        });
    }

    lines
}

pub(super) fn line_ending(contents: &str, line: LineSpan) -> &'static str {
    if line.next_start == line.end {
        return "\n";
    }
    if line.end > line.start && contents.as_bytes()[line.end - 1] == b'\r' {
        "\r\n"
    } else {
        "\n"
    }
}

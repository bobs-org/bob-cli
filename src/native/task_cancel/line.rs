//! Pure cancellation edits for task lines and an existing Cancel Log.

use chrono::NaiveDate;

use crate::native::{
    capture_task_toggle::set_task_line_status,
    task_dependencies::blocks::{
        direct_child_lines, is_cancel_log, leading_whitespace,
    },
    task_status_hooks::{fenced_lines, logical_lines},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelLogOutcome {
    None,
    NotPresent,
    Prepended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceEdit {
    pub(crate) content: String,
    pub(crate) log: CancelLogOutcome,
}

pub(crate) fn edit_source(
    content: &str,
    task_line_index: usize,
    date: NaiveDate,
    reasons: &[String],
) -> SourceEdit {
    let mut segments = content
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let Some(segment) = segments.get_mut(task_line_index) else {
        return SourceEdit {
            content: content.to_string(),
            log: CancelLogOutcome::None,
        };
    };
    let ending = line_ending(segment);
    let raw_line = logical_line(segment).to_string();
    let Some(mut updated) = set_task_line_status(&raw_line, '-') else {
        return SourceEdit {
            content: content.to_string(),
            log: CancelLogOutcome::None,
        };
    };
    updated = upsert_cancelled_date(&updated, date);
    *segment = format!("{updated}{ending}");

    let reason = reasons.join(" · ");
    if reason.is_empty() {
        return SourceEdit {
            content: segments.concat(),
            log: CancelLogOutcome::None,
        };
    }
    let lines = logical_lines(content);
    let fenced = fenced_lines(&lines, 0..lines.len());
    let log_line_index = direct_child_lines(&lines, &fenced, task_line_index)
        .into_iter()
        .find(|index| is_cancel_log(lines[*index]));
    let Some(log_line_index) = log_line_index else {
        return SourceEdit {
            content: segments.concat(),
            log: CancelLogOutcome::NotPresent,
        };
    };

    let Some(header_segment) = segments.get(log_line_index) else {
        return SourceEdit {
            content: segments.concat(),
            log: CancelLogOutcome::NotPresent,
        };
    };
    let header = logical_line(header_segment);
    let indent = lines
        .get(log_line_index + 1)
        .filter(|line| {
            leading_whitespace(line).len() > leading_whitespace(header).len()
        })
        .map(|line| leading_whitespace(line).to_string())
        .unwrap_or_else(|| format!("{}  ", leading_whitespace(header)));
    let entry = format!("{indent}- *{date}* — {reason}");
    let newline = predominant_line_ending(content);
    insert_line_after(&mut segments, log_line_index, &entry, newline);
    SourceEdit {
        content: segments.concat(),
        log: CancelLogOutcome::Prepended,
    }
}

/// Cancel exactly like [`edit_source`], but create a managed
/// `❌ **CANCEL LOG**` first child (with this cancel's dated entry) when
/// the task does not already keep one. Existing callers stay on
/// [`edit_source`], whose `NotPresent` outcome is unchanged; only
/// opt-in writers such as `bob gcal sync` use this creator.
pub(crate) fn edit_source_create_log(
    content: &str,
    task_line_index: usize,
    date: NaiveDate,
    reasons: &[String],
) -> SourceEdit {
    let edited = edit_source(content, task_line_index, date, reasons);
    if edited.log != CancelLogOutcome::NotPresent {
        return edited;
    }
    let reason = reasons.join(" · ");
    if reason.is_empty() {
        return edited;
    }
    let mut segments = edited
        .content
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if segments.get(task_line_index).is_none() {
        return edited;
    }
    let logical = logical_lines(&edited.content);
    let task_indent = logical
        .get(task_line_index)
        .map(|line| leading_whitespace(line).to_string())
        .unwrap_or_default();
    let header_indent = format!("{task_indent}  ");
    let entry_indent = format!("{header_indent}  ");
    let header = format!("{header_indent}- ❌ **CANCEL LOG**");
    let entry = format!("{entry_indent}- *{date}* — {reason}");
    let newline = predominant_line_ending(&edited.content);
    // Insert the entry first so the header lands directly after the
    // task line with the entry beneath it.
    insert_line_after(&mut segments, task_line_index, &entry, newline);
    insert_line_after(&mut segments, task_line_index, &header, newline);
    SourceEdit {
        content: segments.concat(),
        log: CancelLogOutcome::Prepended,
    }
}

pub(crate) fn upsert_cancelled_date(line: &str, date: NaiveDate) -> String {
    let date = date.format("%Y-%m-%d").to_string();
    if let Some(start) = line.find("[cancelled::")
        && let Some(end_offset) = line[start..].find(']')
    {
        let end = start + end_offset + 1;
        let mut output = line.to_string();
        output.replace_range(start..end, &format!("[cancelled:: {date}]"));
        return output;
    }

    let trimmed_end = line.trim_end().len();
    let trimmed = &line[..trimmed_end];
    let block_start = trailing_block_id_start(trimmed).unwrap_or(trimmed_end);
    let separator_start = line[..block_start]
        .trim_end_matches(char::is_whitespace)
        .len();
    let mut output = String::with_capacity(line.len() + date.len() + 16);
    output.push_str(&line[..separator_start]);
    output.push_str(" [cancelled:: ");
    output.push_str(&date);
    output.push(']');
    output.push_str(&line[separator_start..]);
    output
}

fn trailing_block_id_start(line: &str) -> Option<usize> {
    let caret = line.rfind('^')?;
    if caret == 0 || !line.as_bytes()[caret - 1].is_ascii_whitespace() {
        return None;
    }
    let id = &line[caret + 1..];
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    .then_some(line[..caret].trim_end().len())
}

fn insert_line_after(
    segments: &mut Vec<String>,
    index: usize,
    line: &str,
    newline: &str,
) {
    let Some(segment) = segments.get(index) else {
        return;
    };
    if segment.ends_with('\n') {
        segments.insert(index + 1, format!("{line}{newline}"));
    } else {
        let next = if index + 1 < segments.len() {
            format!("{newline}{line}{newline}")
        } else {
            format!("{newline}{line}")
        };
        segments[index].push_str(newline);
        segments
            .insert(index + 1, next.trim_start_matches(newline).to_string());
    }
}

fn line_ending(segment: &str) -> &str {
    if segment.ends_with("\r\n") {
        "\r\n"
    } else if segment.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

fn predominant_line_ending(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

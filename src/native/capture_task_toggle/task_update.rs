use chrono::NaiveDate;

use crate::native::{
    capture::{
        first_child_indentation, first_direct_managed_log_start,
        leading_spaces_or_tabs_len, line_spans, list_marker_len,
    },
    capture_schedule_log::{ScheduleLog, SEPARATOR, TRANSITION},
    freshness,
    task_fields::{
        format_calendar_date, inline_fields, parse_strict_calendar_date,
    },
};

use super::text::{
    child_block_end_line, insert_line_after, line_index_at_offset, replace_line,
};

/// `SCHEDULE_LOG_ENTRY_EMPHASIS = "_"` in `plugins/block-id-prompt/main.js`,
/// the `<ctrl+shift+enter>` keymap this epic mirrors. This deliberately
/// diverges from `capture_schedule_log::ENTRY_EMPHASIS` ("*"), which matches
/// `plugins/bob-navigation-hotkeys/main.js`'s `<ctrl+shift+p>` picker
/// instead; both spellings are already live in the vault today. Unifying the
/// two plugins is out of scope for this epic (tracked as a follow-up).
const PULL_FORWARD_ENTRY_EMPHASIS: &str = "_";

pub(crate) const PULL_FORWARD_REASON: &str = "🍅 pulled into today's Pomodoro";

/// `_<from> → <to>_ — 🍅 pulled into today's Pomodoro`: the Schedule Log
/// entry a toggle's pull-forward writes.
pub(crate) fn pull_forward_entry_text(from: &str, to: &str) -> String {
    format!(
        "{PULL_FORWARD_ENTRY_EMPHASIS}{from} {TRANSITION} {to}{PULL_FORWARD_ENTRY_EMPHASIS} {SEPARATOR} {PULL_FORWARD_REASON}"
    )
}

/// The result of toggling a task's status: the note's full postimage plus
/// enough outcome metadata for a caller to render output or emit JSON
/// fields without re-deriving them from the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskTogglePlan {
    pub(crate) content: String,
    pub(crate) previous_status_symbol: char,
    pub(crate) new_status_symbol: char,
    /// The retired future `scheduled` value, when one was retired.
    pub(crate) removed_scheduled: Option<String>,
    /// Populated only when a direct-child Schedule Log marker already
    /// existed and a pull-forward entry was written under it.
    pub(crate) schedule_log: Option<ScheduleLog>,
}

/// Ready `[ ]` or Blocked `[?]` -> Next `[*]`: retire a single strictly
/// future `scheduled` field when present, force the status to Next, and
/// prepend a pull-forward Schedule Log entry when the task already owns a
/// direct-child log marker. Returns `None` when `task_line_index` is out of
/// range or the line is not an Obsidian task/checkbox line.
pub(crate) fn plan_task_next(
    contents: &str,
    task_line_index: usize,
    today: NaiveDate,
) -> Option<TaskTogglePlan> {
    let lines = line_spans(contents);
    let line = *lines.get(task_line_index)?;
    let previous_status_symbol = current_status_symbol(line.text)?;

    let future_field = find_single_future_scheduled_field(line.text, today);
    let mut updated_text = line.text.to_string();
    let mut removed_scheduled = None;
    if let Some(field) = &future_field {
        updated_text = remove_span_with_space_collapse(
            &updated_text,
            field.start,
            field.end,
        );
        removed_scheduled = Some(field.value.clone());
    }
    updated_text = set_task_line_status(&updated_text, '*')?;

    let content_after_status =
        replace_line(contents, &lines, task_line_index, &updated_text);

    let mut final_content = content_after_status.clone();
    let mut schedule_log = None;
    if let Some(old_date) = &removed_scheduled {
        let lines_after = line_spans(&content_after_status);
        let task_end_line = child_block_end_line(&lines_after, task_line_index);
        let task_end_offset = lines_after[task_end_line].end;
        if let Some(marker_offset) = first_direct_managed_log_start(
            &lines_after,
            task_line_index,
            task_end_offset,
        ) {
            let marker_line_index =
                line_index_at_offset(&lines_after, marker_offset);
            let marker_text = lines_after[marker_line_index].text;
            let marker_indent_len = leading_spaces_or_tabs_len(marker_text);
            let marker_indentation = &marker_text[..marker_indent_len];
            let after_indent = &marker_text[marker_indent_len..];
            if let Some(marker_len) = list_marker_len(after_indent) {
                let marker_bullet = &after_indent[..marker_len];
                let marker_end_line =
                    child_block_end_line(&lines_after, marker_line_index);
                let marker_end_offset = lines_after[marker_end_line].end;
                let entry_indent = first_child_indentation(
                    &lines_after,
                    marker_line_index,
                    marker_end_offset,
                    marker_indentation,
                )
                .unwrap_or_else(|| format!("{marker_indentation}\t"));
                let today_text = format_calendar_date(today);
                let entry_text = pull_forward_entry_text(old_date, &today_text);
                let entry_line =
                    format!("{entry_indent}{marker_bullet} {entry_text}");
                final_content = insert_line_after(
                    &content_after_status,
                    &lines_after,
                    marker_line_index,
                    &entry_line,
                );
                schedule_log = Some(ScheduleLog {
                    reason: PULL_FORWARD_REASON.to_string(),
                    lines: vec![entry_line],
                });
            }
        }
    }

    Some(TaskTogglePlan {
        content: final_content,
        previous_status_symbol,
        new_status_symbol: '*',
        removed_scheduled,
        schedule_log,
    })
}

/// Link policy: Ready `[ ]`/`Blocked `[?]` -> Next `[*]`; Next `[*]` and In
/// Progress `[/]` keep their status. Every eligible status still retires a
/// single strictly future `scheduled` field and writes the pull-forward
/// Schedule Log entry when a log already exists, using the Ensure Next
/// rules. Returns `None` for the same reasons as [`plan_task_next`].
pub(crate) fn plan_task_link(
    contents: &str,
    task_line_index: usize,
    today: NaiveDate,
) -> Option<TaskTogglePlan> {
    let lines = line_spans(contents);
    let line = *lines.get(task_line_index)?;
    let previous_status_symbol = current_status_symbol(line.text)?;
    let new_status_symbol = match previous_status_symbol {
        ' ' | '?' => '*',
        '*' | '/' => previous_status_symbol,
        _ => return None,
    };

    let future_field = find_single_future_scheduled_field(line.text, today);
    let mut updated_text = line.text.to_string();
    let mut removed_scheduled = None;
    if let Some(field) = &future_field {
        updated_text = remove_span_with_space_collapse(
            &updated_text,
            field.start,
            field.end,
        );
        removed_scheduled = Some(field.value.clone());
    }
    updated_text = set_task_line_status(&updated_text, new_status_symbol)?;
    // Freshness: stamp the rewritten line as the last transformation,
    // but only when the link changed it (status or retired schedule).
    // A byte-identical Next/In Progress line means no write, matching
    // block-id-prompt. A refusal (recurring/closed/not-a-task) leaves
    // the line as it is.
    if updated_text != line.text {
        let stamped = freshness::stamp_fresh(&updated_text, today);
        if stamped.refused.is_none() {
            updated_text = stamped.line;
        }
    }

    let content_after_status =
        replace_line(contents, &lines, task_line_index, &updated_text);

    let mut final_content = content_after_status.clone();
    let mut schedule_log = None;
    if let Some(old_date) = &removed_scheduled {
        let lines_after = line_spans(&content_after_status);
        let task_end_line = child_block_end_line(&lines_after, task_line_index);
        let task_end_offset = lines_after[task_end_line].end;
        if let Some(marker_offset) = first_direct_managed_log_start(
            &lines_after,
            task_line_index,
            task_end_offset,
        ) {
            let marker_line_index =
                line_index_at_offset(&lines_after, marker_offset);
            let marker_text = lines_after[marker_line_index].text;
            let marker_indent_len = leading_spaces_or_tabs_len(marker_text);
            let marker_indentation = &marker_text[..marker_indent_len];
            let after_indent = &marker_text[marker_indent_len..];
            if let Some(marker_len) = list_marker_len(after_indent) {
                let marker_bullet = &after_indent[..marker_len];
                let marker_end_line =
                    child_block_end_line(&lines_after, marker_line_index);
                let marker_end_offset = lines_after[marker_end_line].end;
                let entry_indent = first_child_indentation(
                    &lines_after,
                    marker_line_index,
                    marker_end_offset,
                    marker_indentation,
                )
                .unwrap_or_else(|| format!("{marker_indentation}\t"));
                let today_text = format_calendar_date(today);
                let entry_text = pull_forward_entry_text(old_date, &today_text);
                let entry_line =
                    format!("{entry_indent}{marker_bullet} {entry_text}");
                final_content = insert_line_after(
                    &content_after_status,
                    &lines_after,
                    marker_line_index,
                    &entry_line,
                );
                schedule_log = Some(ScheduleLog {
                    reason: PULL_FORWARD_REASON.to_string(),
                    lines: vec![entry_line],
                });
            }
        }
    }

    Some(TaskTogglePlan {
        content: final_content,
        previous_status_symbol,
        new_status_symbol,
        removed_scheduled,
        schedule_log,
    })
}

// ---------------------------------------------------------------------------
// Task checkbox and `scheduled` field parsing.
// ---------------------------------------------------------------------------

fn current_status_symbol(line: &str) -> Option<char> {
    let indent_len = leading_spaces_or_tabs_len(line);
    let after_indent = line.get(indent_len..)?;
    let after_open = after_indent.strip_prefix("- [")?;
    let mut chars = after_open.chars();
    let status = chars.next()?;
    let after_status = &after_open[status.len_utf8()..];
    after_status.strip_prefix(']')?;
    Some(status)
}

pub(crate) fn set_task_line_status(
    line: &str,
    new_status: char,
) -> Option<String> {
    let indent_len = leading_spaces_or_tabs_len(line);
    let after_indent = line.get(indent_len..)?;
    let after_open = after_indent.strip_prefix("- [")?;
    let mut chars = after_open.chars();
    let old_status = chars.next()?;
    let after_status = &after_open[old_status.len_utf8()..];
    after_status.strip_prefix(']')?;
    let offset = indent_len + "- [".len();
    let mut result = String::with_capacity(line.len());
    result.push_str(&line[..offset]);
    result.push(new_status);
    result.push_str(&line[offset + old_status.len_utf8()..]);
    Some(result)
}

/// Recognizes the same task-level `scheduled` forms as `bob task reconcile`
/// and `plugins/block-id-prompt/main.js`'s `SCHEDULED_FIELD_RE`, shared with
/// the other task-field consumers through `task_fields`.
pub(crate) struct ScheduledFieldMatch {
    start: usize,
    end: usize,
    value: String,
}

/// Every recognized `scheduled` field on the line. A malformed or duplicate
/// field still counts toward ambiguity, matching `findScheduledFieldMatches`.
fn scheduled_field_matches(line: &str) -> Vec<ScheduledFieldMatch> {
    inline_fields(line, "scheduled")
        .into_iter()
        .map(|field| ScheduledFieldMatch {
            start: field.start,
            end: field.end,
            value: field.value,
        })
        .collect()
}

/// Exactly one recognized `scheduled` field, syntactically valid, and
/// strictly later than `today` -- the only shape that qualifies for
/// future-schedule removal.
fn find_single_future_scheduled_field(
    line: &str,
    today: NaiveDate,
) -> Option<ScheduledFieldMatch> {
    let mut matches = scheduled_field_matches(line);
    if matches.len() != 1 {
        return None;
    }
    let field = matches.pop().expect("exactly one match");
    let parsed = parse_strict_calendar_date(&field.value)?;
    (parsed > today).then_some(field)
}

/// Remove `[start, end)` and collapse the whitespace it exposed so
/// surviving tokens stay separated by exactly one space, without
/// introducing trailing whitespace. Mirrors `removeSpanWithSpaceCollapse`.
fn remove_span_with_space_collapse(
    line: &str,
    start: usize,
    end: usize,
) -> String {
    let before = &line[..start];
    let after = &line[end..];
    let before_trimmed = before.trim_end_matches([' ', '\t']);
    let after_trimmed = after.trim_start_matches([' ', '\t']);
    if !before.trim().is_empty() && !after.trim().is_empty() {
        format!("{before_trimmed} {after_trimmed}")
    } else {
        format!("{before_trimmed}{after_trimmed}")
    }
}

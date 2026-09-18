//! Pure planners for the `@route+block-id` task-status toggle (the
//! `<ctrl+shift+enter>` keymap's semantics, reproduced in Rust).
//!
//! Read `plugins/block-id-prompt/main.js` in the `bob-plugins` repo first --
//! `planTargetTaskUpdate`, `planTargetTaskOpenUpdate`,
//! `planPomodoroLinkInsertion`, `planFuturePomodoroLinkCleanup`,
//! `planAllOpenPomodoroLinkCleanup`, `planPomodoroLinkCleanupForRanges`,
//! `isDedicatedLinkBullet`, and `listItemSubtreeEdit` are the reference this
//! module mirrors. Everything here operates on `&str` note contents and
//! returns a full postimage; nothing touches disk.
//!
//! `bob capture` wires these planners into its staged batch planner, while
//! keeping the note-level mutation rules here as pure functions so the route
//! note and daily ledger behavior can be tested directly.
#![allow(dead_code)]

use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use regex::Regex;

use super::{
    capture::{
        first_child_indentation, first_direct_managed_log_start,
        leading_spaces_or_tabs_len, line_spans, list_item_body,
        list_marker_len, nearest_shallower_list_item_parent, LineSpan,
    },
    capture_pomodoros::{self, NamedSelection, PomodoroEntry, PomodoroState},
    capture_schedule_log::{ScheduleLog, SEPARATOR, TRANSITION},
    markdown, pomodoro,
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

/// Next `[*]` -> Ready `[ ]`, and nothing else. Returns `None` for the same
/// reasons as [`plan_task_next`].
pub(crate) fn plan_task_open(
    contents: &str,
    task_line_index: usize,
) -> Option<TaskTogglePlan> {
    let lines = line_spans(contents);
    let line = *lines.get(task_line_index)?;
    let previous_status_symbol = current_status_symbol(line.text)?;
    let updated_text = set_task_line_status(line.text, ' ')?;
    let content =
        replace_line(contents, &lines, task_line_index, &updated_text);
    Some(TaskTogglePlan {
        content,
        previous_status_symbol,
        new_status_symbol: ' ',
        removed_scheduled: None,
        schedule_log: None,
    })
}

/// Why [`plan_link_insertion`] could not select a destination Pomodoro at
/// all -- distinct from [`LinkInsertionOutcome::NeedsPomodoroCreation`],
/// which is a normal (non-error) outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkPlanError {
    NoPomodorosSection,
    NoEligibleOpenEntry,
    MultipleOpenTimedEntries,
    InvalidPomodoroName,
}

/// Either a resolved insertion plan, or a signal that no existing entry
/// (open or completed) matched the requested name and the caller must
/// create a named future Pomodoro first -- reusing the existing
/// `select_named_pomodoro`/`insert_named_pomodoro_child_block` machinery in
/// `capture.rs`, which already knows how to select-or-create and insert a
/// child block in one step. A freshly created entry can never already carry
/// the link, so the caller does not need to call back into this module for
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkInsertionOutcome {
    Planned(LinkInsertionPlan),
    NeedsPomodoroCreation { canonical_name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkInsertionPlan {
    pub(crate) content: String,
    pub(crate) has_changes: bool,
    /// Where the selected-entry link was added. `None` means the selected
    /// entry already had the link, even if duplicate cleanup changed the
    /// ledger afterward.
    pub(crate) placement: Option<LinkPlacement>,
    /// The selected entry already carried a matching link; nothing was
    /// inserted.
    pub(crate) already_linked: bool,
    /// The resolved entry's name, when it has one.
    pub(crate) pomodoro_name: Option<String>,
    /// Duplicates removed from later still-open Pomodoros.
    pub(crate) removed_links: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkPlacement {
    Inserted,
    Appended,
}

/// Select a destination Pomodoro entry and insert `block_link` as one of
/// its children.
///
/// Selection: with no `pomodoro_name`, the single open timed entry, else
/// the first open entry (matching `@route:block-id`'s implicit rule). With
/// `pomodoro_name`, an existing open name match; a completed-only or
/// missing match instead reports
/// [`LinkInsertionOutcome::NeedsPomodoroCreation`].
///
/// Insertion is idempotent (a matching link already under the selected
/// entry is left alone, `already_linked: true`) and always removes matching
/// duplicates from every later still-open Pomodoro.
pub(crate) fn plan_link_insertion(
    day_contents: &str,
    block_link: &str,
    pomodoro_name: Option<&str>,
) -> Result<LinkInsertionOutcome, LinkPlanError> {
    let scan = capture_pomodoros::scan(day_contents);

    let (entry_line_index, resolved_name) = match pomodoro_name {
        Some(name) => match capture_pomodoros::select_named(&scan, name) {
            NamedSelection::Found(entry) => {
                (entry.line - 1, entry.name.clone())
            }
            NamedSelection::CompletedOnly(_)
            | NamedSelection::Missing { .. } => {
                let canonical =
                    capture_pomodoros::named_creation_name(&scan, name)
                        .ok_or(LinkPlanError::InvalidPomodoroName)?;
                return Ok(LinkInsertionOutcome::NeedsPomodoroCreation {
                    canonical_name: canonical,
                });
            }
        },
        None => {
            let entry = select_implicit_open_entry(&scan)?;
            (entry.line - 1, entry.name.clone())
        }
    };

    let lines = line_spans(day_contents);
    let (content_after_insert, already_linked, placement) =
        insert_link_into_entry(
            day_contents,
            &lines,
            entry_line_index,
            block_link,
        );

    let updated_lines = line_spans(&content_after_insert);
    let updated_scan = capture_pomodoros::scan(&content_after_insert);
    let selected_line = updated_scan
        .entries
        .iter()
        .find(|entry| {
            entry.state == PomodoroState::Open
                && entry.name == resolved_name
                && entry.line - 1 >= entry_line_index
        })
        .map(|entry| entry.line - 1)
        .unwrap_or(entry_line_index);
    let ranges = updated_scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == PomodoroState::Open && entry.line - 1 > selected_line
        })
        .map(|entry| {
            let line_index = entry.line - 1;
            (line_index, child_block_end_line(&updated_lines, line_index))
        })
        .collect::<Vec<_>>();
    let (final_content, removed_links) =
        remove_matching_links(&content_after_insert, block_link, &ranges);

    Ok(LinkInsertionOutcome::Planned(LinkInsertionPlan {
        has_changes: !already_linked || removed_links > 0,
        content: final_content,
        placement,
        already_linked,
        pomodoro_name: resolved_name,
        removed_links,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkRemovalPlan {
    pub(crate) content: String,
    pub(crate) has_changes: bool,
    pub(crate) removed_links: usize,
}

/// Remove every matching `block_link` from every currently open Pomodoro
/// (current and future); a completed entry's links are never touched. A
/// link that is the sole content of its bullet is removed along with the
/// whole bullet and its sub-bullets; a link sharing a bullet with other
/// text has only the link text removed.
pub(crate) fn plan_link_removal(
    day_contents: &str,
    block_link: &str,
) -> LinkRemovalPlan {
    let scan = capture_pomodoros::scan(day_contents);
    if !scan.has_section {
        return LinkRemovalPlan {
            content: day_contents.to_string(),
            has_changes: false,
            removed_links: 0,
        };
    }
    let lines = line_spans(day_contents);
    let ranges = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
        .map(|entry| {
            let line_index = entry.line - 1;
            (line_index, child_block_end_line(&lines, line_index))
        })
        .collect::<Vec<_>>();
    let (content, removed_links) =
        remove_matching_links(day_contents, block_link, &ranges);
    LinkRemovalPlan {
        has_changes: removed_links > 0,
        content,
        removed_links,
    }
}

/// Why [`plan_link_relocation`] could not move a Task Link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkRelocationError {
    NoPomodorosSection,
    NoEligibleOpenEntry,
    MultipleOpenTimedEntries,
    InvalidPomodoroName,
    NoMovableLink,
    MultipleMovableLinks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkRelocationAction {
    Moved,
    AlreadyCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroEndpoint {
    pub(crate) line: usize,
    pub(crate) name: Option<String>,
    pub(crate) time_range: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkRelocationPlan {
    pub(crate) content: String,
    pub(crate) has_changes: bool,
    pub(crate) action: LinkRelocationAction,
    pub(crate) placement: Option<LinkPlacement>,
    pub(crate) source: PomodoroEndpoint,
    pub(crate) destination: PomodoroEndpoint,
    /// True only when this plan created the named destination Pomodoro.
    pub(crate) creates_pomodoro: bool,
}

/// Move the sole dedicated open-Pomodoro Task Link for `block_link` onto
/// either today's implicit current/next open Pomodoro or a named destination.
/// Never synthesizes a missing link or edits completed history. A named
/// selector with no matching open entry creates the canonical named future
/// Pomodoro, then moves the existing subtree beneath it.
pub(crate) fn plan_link_relocation(
    day_contents: &str,
    block_link: &str,
    pomodoro_name: Option<&str>,
) -> Result<LinkRelocationPlan, LinkRelocationError> {
    let scan = capture_pomodoros::scan(day_contents);
    let lines = line_spans(day_contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section = pomodoro::pomodoros_section_range(&line_texts)
        .ok_or(LinkRelocationError::NoPomodorosSection)?;
    let movable =
        find_movable_task_links(&lines, &scan, section.clone(), block_link);
    let source = match movable.as_slice() {
        [] => return Err(LinkRelocationError::NoMovableLink),
        [_, _, ..] => return Err(LinkRelocationError::MultipleMovableLinks),
        [source] => source,
    };
    let source_endpoint = endpoint_from_entry(source.owner);
    let source_entry_line_index = source.entry_line_index;
    let source_line_index = source.line_index;
    let source_subtree_end = source.subtree_end;

    let resolved = resolve_relocation_destination(
        day_contents,
        &scan,
        pomodoro_name,
        source_entry_line_index,
    )?;
    if resolved.already_current {
        return Ok(LinkRelocationPlan {
            content: day_contents.to_string(),
            has_changes: false,
            action: LinkRelocationAction::AlreadyCurrent,
            placement: None,
            source: source_endpoint,
            destination: resolved.destination,
            creates_pomodoro: false,
        });
    }

    let (working, source_line_index, source_subtree_end, dest_entry_index) =
        if resolved.creates_pomodoro {
            let working_scan = capture_pomodoros::scan(&resolved.content);
            let working_lines = line_spans(&resolved.content);
            let working_texts = working_lines
                .iter()
                .map(|line| line.text)
                .collect::<Vec<_>>();
            let working_section =
                pomodoro::pomodoros_section_range(&working_texts)
                    .ok_or(LinkRelocationError::NoPomodorosSection)?;
            let movable = find_movable_task_links(
                &working_lines,
                &working_scan,
                working_section,
                block_link,
            );
            let source = match movable.as_slice() {
                [source] => source,
                _ => return Err(LinkRelocationError::NoMovableLink),
            };
            (
                resolved.content,
                source.line_index,
                source.subtree_end,
                resolved.dest_entry_index,
            )
        } else {
            (
                day_contents.to_string(),
                source_line_index,
                source_subtree_end,
                resolved.dest_entry_index,
            )
        };

    let (content, placement) = move_subtree_to_entry(
        &working,
        source_line_index,
        source_subtree_end,
        dest_entry_index,
    )?;
    let destination = destination_endpoint_after_move(
        &content,
        pomodoro_name,
        &resolved.destination,
        resolved.creates_pomodoro,
    )?;
    Ok(LinkRelocationPlan {
        content,
        has_changes: true,
        action: LinkRelocationAction::Moved,
        placement: Some(placement),
        source: source_endpoint,
        destination,
        creates_pomodoro: resolved.creates_pomodoro,
    })
}

struct ResolvedRelocationDestination {
    content: String,
    dest_entry_index: usize,
    destination: PomodoroEndpoint,
    already_current: bool,
    creates_pomodoro: bool,
}

fn resolve_relocation_destination(
    day_contents: &str,
    scan: &capture_pomodoros::PomodoroScan,
    pomodoro_name: Option<&str>,
    source_entry_line_index: usize,
) -> Result<ResolvedRelocationDestination, LinkRelocationError> {
    match pomodoro_name {
        None => {
            let destination = select_implicit_open_entry(scan)
                .map_err(relocation_from_link_plan_error)?;
            let dest_entry_index = destination.line - 1;
            Ok(ResolvedRelocationDestination {
                content: day_contents.to_string(),
                dest_entry_index,
                destination: endpoint_from_entry(destination),
                already_current: source_entry_line_index == dest_entry_index,
                creates_pomodoro: false,
            })
        }
        Some(selector) => match capture_pomodoros::select_named(scan, selector)
        {
            capture_pomodoros::NamedSelection::Found(entry) => {
                let dest_entry_index = entry.line - 1;
                Ok(ResolvedRelocationDestination {
                    content: day_contents.to_string(),
                    dest_entry_index,
                    destination: endpoint_from_entry(entry),
                    already_current: source_entry_line_index
                        == dest_entry_index,
                    creates_pomodoro: false,
                })
            }
            capture_pomodoros::NamedSelection::CompletedOnly(_)
            | capture_pomodoros::NamedSelection::Missing { .. } => {
                let (content, created_line, name) =
                    insert_named_placeholder(day_contents, selector)?;
                Ok(ResolvedRelocationDestination {
                    dest_entry_index: created_line,
                    destination: PomodoroEndpoint {
                        line: created_line + 1,
                        name: Some(name),
                        time_range: None,
                    },
                    content,
                    already_current: false,
                    creates_pomodoro: true,
                })
            }
        },
    }
}

fn destination_endpoint_after_move(
    content: &str,
    pomodoro_name: Option<&str>,
    planned: &PomodoroEndpoint,
    creates_pomodoro: bool,
) -> Result<PomodoroEndpoint, LinkRelocationError> {
    let scan = capture_pomodoros::scan(content);
    if let Some(selector) = pomodoro_name {
        match capture_pomodoros::select_named(&scan, selector) {
            capture_pomodoros::NamedSelection::Found(entry) => {
                return Ok(endpoint_from_entry(entry));
            }
            _ => {
                if creates_pomodoro
                    && let Some(entry) = scan.entries.iter().find(|entry| {
                        entry.state == PomodoroState::Open
                            && entry.name == planned.name
                    })
                {
                    return Ok(endpoint_from_entry(entry));
                }
                return Err(LinkRelocationError::NoEligibleOpenEntry);
            }
        }
    }
    select_implicit_open_entry(&scan)
        .map(endpoint_from_entry)
        .map_err(relocation_from_link_plan_error)
}

fn move_subtree_to_entry(
    contents: &str,
    source_line_index: usize,
    source_subtree_end: usize,
    dest_entry_index: usize,
) -> Result<(String, LinkPlacement), LinkRelocationError> {
    let lines = line_spans(contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section = pomodoro::pomodoros_section_range(&line_texts)
        .ok_or(LinkRelocationError::NoPomodorosSection)?;
    let dest_child_end = child_block_end_line(&lines, dest_entry_index);
    let dest_indent = pomodoro_child_indentation(
        &lines,
        dest_entry_index,
        dest_child_end,
        section.start,
        section.end,
    );
    let source_indent_len =
        leading_spaces_or_tabs_len(lines[source_line_index].text);
    let source_indent = &lines[source_line_index].text[..source_indent_len];
    let subtree = extract_line_range(
        contents,
        &lines,
        source_line_index,
        source_subtree_end,
    );
    let reindented = reindent_subtree(subtree, source_indent, &dest_indent);
    let new_lines = logical_lines(&reindented);

    let lines_removed_before_insert = if source_line_index <= dest_child_end {
        source_subtree_end - source_line_index + 1
    } else {
        0
    };
    let dest_after = dest_child_end - lines_removed_before_insert;
    let without_source = remove_line_range(
        contents,
        &lines,
        source_line_index,
        source_subtree_end,
    );
    let updated_lines = line_spans(&without_source);
    let placement = if updated_lines[dest_after].end >= without_source.len() {
        LinkPlacement::Appended
    } else {
        LinkPlacement::Inserted
    };
    let content = insert_lines_after(
        &without_source,
        &updated_lines,
        dest_after,
        &new_lines,
    );
    Ok((content, placement))
}

fn insert_named_placeholder(
    contents: &str,
    selector: &str,
) -> Result<(String, usize, String), LinkRelocationError> {
    let scan = capture_pomodoros::scan(contents);
    if !scan.has_section {
        return Err(LinkRelocationError::NoPomodorosSection);
    }
    let timed_open = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == PomodoroState::Open && entry.time_range.is_some()
        })
        .count();
    if timed_open > 1 {
        return Err(LinkRelocationError::MultipleOpenTimedEntries);
    }
    let name = capture_pomodoros::canonicalize_pomodoro_name(selector)
        .ok_or(LinkRelocationError::InvalidPomodoroName)?;
    let lines = line_spans(contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section = pomodoro::pomodoros_section_range(&line_texts)
        .ok_or(LinkRelocationError::NoPomodorosSection)?;
    let placeholder = capture_pomodoros::format_named_placeholder_line(&name);
    let timed = scan.entries.iter().find(|entry| {
        entry.state == PomodoroState::Open && entry.time_range.is_some()
    });
    let last_completed = scan
        .entries
        .iter()
        .rev()
        .find(|entry| entry.state == PomodoroState::Completed);
    let (content, created_line) = if let Some(entry) = timed.or(last_completed)
    {
        let after = child_block_end_line(&lines, entry.line - 1);
        (
            insert_line_after(contents, &lines, after, &placeholder),
            after + 1,
        )
    } else if let Some(first) = scan.entries.first() {
        let index = first.line - 1;
        (
            insert_line_before(contents, &lines, index, &placeholder),
            index,
        )
    } else if section.start < lines.len() {
        (
            insert_line_before(contents, &lines, section.start, &placeholder),
            section.start,
        )
    } else if !lines.is_empty() {
        let after = lines.len() - 1;
        (
            insert_line_after(contents, &lines, after, &placeholder),
            after + 1,
        )
    } else {
        return Err(LinkRelocationError::NoPomodorosSection);
    };
    Ok((content, created_line, name))
}

fn insert_line_before(
    content: &str,
    lines: &[LineSpan<'_>],
    at_index: usize,
    new_line: &str,
) -> String {
    if at_index >= lines.len() {
        return insert_line_after(content, lines, lines.len() - 1, new_line);
    }
    let insert_offset = line_start_offset(lines, at_index);
    let ending = if line_has_crlf(content, lines, at_index) {
        "\r\n"
    } else {
        "\n"
    };
    format!(
        "{}{new_line}{ending}{}",
        &content[..insert_offset],
        &content[insert_offset..]
    )
}

fn relocation_from_link_plan_error(
    error: LinkPlanError,
) -> LinkRelocationError {
    match error {
        LinkPlanError::NoPomodorosSection => {
            LinkRelocationError::NoPomodorosSection
        }
        LinkPlanError::NoEligibleOpenEntry => {
            LinkRelocationError::NoEligibleOpenEntry
        }
        LinkPlanError::MultipleOpenTimedEntries => {
            LinkRelocationError::MultipleOpenTimedEntries
        }
        LinkPlanError::InvalidPomodoroName => {
            LinkRelocationError::InvalidPomodoroName
        }
    }
}

fn select_implicit_open_entry(
    scan: &capture_pomodoros::PomodoroScan,
) -> Result<&PomodoroEntry, LinkPlanError> {
    if !scan.has_section {
        return Err(LinkPlanError::NoPomodorosSection);
    }
    let open_timed_count = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == PomodoroState::Open && entry.time_range.is_some()
        })
        .count();
    if open_timed_count > 1 {
        return Err(LinkPlanError::MultipleOpenTimedEntries);
    }
    scan.entries
        .iter()
        .find(|entry| {
            entry.state == PomodoroState::Open && entry.time_range.is_some()
        })
        .or_else(|| {
            scan.entries
                .iter()
                .find(|entry| entry.state == PomodoroState::Open)
        })
        .ok_or(LinkPlanError::NoEligibleOpenEntry)
}

fn endpoint_from_entry(entry: &PomodoroEntry) -> PomodoroEndpoint {
    PomodoroEndpoint {
        line: entry.line,
        name: entry.name.clone(),
        time_range: entry.time_range.clone(),
    }
}

struct MovableLink<'a> {
    owner: &'a PomodoroEntry,
    entry_line_index: usize,
    line_index: usize,
    subtree_end: usize,
}

fn find_movable_task_links<'a>(
    lines: &[LineSpan<'_>],
    scan: &'a capture_pomodoros::PomodoroScan,
    section: std::ops::Range<usize>,
    block_link: &str,
) -> Vec<MovableLink<'a>> {
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let fenced = markdown::fenced_lines(&line_texts, section.clone());
    let mut found = Vec::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        let entry_line_index = entry.line - 1;
        let child_end = child_block_end_line(lines, entry_line_index);
        if child_end < entry_line_index + 1 {
            continue;
        }
        for line_index in entry_line_index + 1..=child_end {
            if fenced.contains(&line_index) {
                continue;
            }
            let text = lines[line_index].text;
            if nearest_shallower_list_item_parent(lines, line_index)
                != Some(entry_line_index)
            {
                continue;
            }
            let Some(body) = list_item_body(text).map(str::trim_end) else {
                continue;
            };
            if body != block_link {
                continue;
            }
            found.push(MovableLink {
                owner: entry,
                entry_line_index,
                line_index,
                subtree_end: child_block_end_line(lines, line_index),
            });
        }
    }
    found
}

fn extract_line_range<'a>(
    contents: &'a str,
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> &'a str {
    let start = line_start_offset(lines, start_line);
    &contents[start..lines[end_line].end]
}

fn remove_line_range(
    contents: &str,
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> String {
    let start = line_start_offset(lines, start_line);
    format!("{}{}", &contents[..start], &contents[lines[end_line].end..])
}

fn reindent_subtree(
    subtree: &str,
    from_indent: &str,
    to_indent: &str,
) -> String {
    let mut result = String::new();
    for (line, ending) in physical_line_pieces(subtree) {
        if line.trim().is_empty() {
            result.push_str(line);
        } else if let Some(suffix) = line.strip_prefix(from_indent) {
            result.push_str(to_indent);
            result.push_str(suffix);
        } else {
            result.push_str(line);
        }
        result.push_str(ending);
    }
    result
}

fn logical_lines(text: &str) -> Vec<String> {
    physical_line_pieces(text)
        .into_iter()
        .map(|(line, _)| line.to_string())
        .collect()
}

fn physical_line_pieces(text: &str) -> Vec<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            let ending_start = if index > start && bytes[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            lines.push((
                &text[start..ending_start],
                &text[ending_start..index + 1],
            ));
            index += 1;
            start = index;
        } else {
            index += 1;
        }
    }
    if start < text.len() {
        lines.push((&text[start..], ""));
    }
    lines
}

fn insert_lines_after(
    content: &str,
    lines: &[LineSpan<'_>],
    after_index: usize,
    new_lines: &[String],
) -> String {
    if new_lines.is_empty() {
        return content.to_string();
    }
    let insert_offset = lines[after_index].end;
    let ending = if line_has_crlf(content, lines, after_index) {
        "\r\n"
    } else {
        "\n"
    };
    let at_eof_no_nl =
        insert_offset == content.len() && !content.ends_with('\n');
    let mut block = String::new();
    for (index, line) in new_lines.iter().enumerate() {
        let last = index + 1 == new_lines.len();
        block.push_str(line);
        if at_eof_no_nl {
            if !last {
                block.push_str(ending);
            }
        } else {
            block.push_str(ending);
        }
    }
    if at_eof_no_nl {
        format!("{content}{ending}{block}")
    } else {
        format!(
            "{}{block}{}",
            &content[..insert_offset],
            &content[insert_offset..]
        )
    }
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

fn set_task_line_status(line: &str, new_status: char) -> Option<String> {
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

/// Recognizes the same task-level `scheduled` forms as `bob task-status-hooks`
/// and `plugins/block-id-prompt/main.js`'s `SCHEDULED_FIELD_RE`:
/// `[scheduled:: YYYY-MM-DD]` and `(scheduled:: YYYY-MM-DD)`, anywhere on the
/// line and in any field order.
static SCHEDULED_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[scheduled::([^\]\n]*)\]|\(scheduled::([^)\n]*)\)")
        .expect("valid scheduled field regex")
});

struct ScheduledFieldMatch {
    start: usize,
    end: usize,
    value: String,
}

/// Every recognized `scheduled` field on the line. A malformed or duplicate
/// field still counts toward ambiguity, matching `findScheduledFieldMatches`.
fn scheduled_field_matches(line: &str) -> Vec<ScheduledFieldMatch> {
    SCHEDULED_FIELD_RE
        .captures_iter(line)
        .map(|captures| {
            let whole = captures.get(0).expect("whole match");
            let value = captures
                .get(1)
                .or_else(|| captures.get(2))
                .expect("scheduled value group")
                .as_str()
                .trim()
                .to_string();
            ScheduledFieldMatch {
                start: whole.start(),
                end: whole.end(),
                value,
            }
        })
        .collect()
}

fn parse_strict_calendar_date(value: &str) -> Option<NaiveDate> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let all_digits = |range: std::ops::Range<usize>| {
        bytes[range].iter().all(u8::is_ascii_digit)
    };
    if !all_digits(0..4) || !all_digits(5..7) || !all_digits(8..10) {
        return None;
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
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

fn format_calendar_date(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

// ---------------------------------------------------------------------------
// Shared line utilities.
// ---------------------------------------------------------------------------

/// The exclusive-of-nothing-past-it last line index of `parent_index`'s
/// child block: every later line that is blank or indented deeper than the
/// parent, stopping at the first nonblank line indented at or shallower
/// than the parent. Mirrors `findChildBlockEndLine`; used for both a task's
/// own child block (to bound the Schedule Log marker search) and a Schedule
/// Log marker's or Pomodoro entry's own children (both always top-level, so
/// this naturally stops at the Pomodoros section boundary too).
fn child_block_end_line(lines: &[LineSpan<'_>], parent_index: usize) -> usize {
    let parent_indent = leading_spaces_or_tabs_len(lines[parent_index].text);
    let mut end_index = parent_index;
    let mut index = parent_index + 1;
    while index < lines.len() {
        let text = lines[index].text;
        if text.trim().is_empty() {
            index += 1;
            continue;
        }
        if leading_spaces_or_tabs_len(text) > parent_indent {
            end_index = index;
            index += 1;
            continue;
        }
        break;
    }
    end_index
}

fn line_start_offset(lines: &[LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines[index - 1].end
    }
}

fn line_index_at_offset(lines: &[LineSpan<'_>], offset: usize) -> usize {
    lines.iter().take_while(|line| line.end <= offset).count()
}

/// Replace the logical content of line `index` with `new_text`, preserving
/// whatever line-ending bytes (`\n`, `\r\n`, or none at EOF) followed it.
fn replace_line(
    contents: &str,
    lines: &[LineSpan<'_>],
    index: usize,
    new_text: &str,
) -> String {
    let start = line_start_offset(lines, index);
    let text_end = start + lines[index].text.len();
    format!("{}{new_text}{}", &contents[..start], &contents[text_end..])
}

fn line_has_crlf(content: &str, lines: &[LineSpan<'_>], index: usize) -> bool {
    let start = line_start_offset(lines, index);
    content[start..lines[index].end].ends_with("\r\n")
}

/// Insert `new_line` as a new physical line immediately after line
/// `after_index`, matching that line's `\n`/`\r\n` ending. Handles the edge
/// case where `after_index` is the last line and the file has no trailing
/// newline at all (a leading separator is needed then; every other case --
/// including `after_index` being the last line when the file *does* end
/// with a newline -- needs no special-casing, since `content[insert..]` is
/// already empty there).
fn insert_line_after(
    content: &str,
    lines: &[LineSpan<'_>],
    after_index: usize,
    new_line: &str,
) -> String {
    let insert_offset = lines[after_index].end;
    let ending = if line_has_crlf(content, lines, after_index) {
        "\r\n"
    } else {
        "\n"
    };
    if insert_offset == content.len() && !content.ends_with('\n') {
        format!("{content}{ending}{new_line}")
    } else {
        format!(
            "{}{new_line}{ending}{}",
            &content[..insert_offset],
            &content[insert_offset..]
        )
    }
}

// ---------------------------------------------------------------------------
// Pomodoro child indentation and link insertion.
// ---------------------------------------------------------------------------

/// An existing unordered (`-`/`*`/`+`) child bullet's indentation. Unlike
/// `list_item_body`'s marker matching, ordered-list children are not
/// candidates, matching `unorderedChildIndentation`'s comment: this mirrors
/// `bob capture`'s own insertion-target rule.
fn unordered_child_indentation(line: &str) -> Option<&str> {
    let indent_len = leading_spaces_or_tabs_len(line);
    if indent_len == 0 {
        return None;
    }
    let indentation = &line[..indent_len];
    let after_indent = &line.as_bytes()[indent_len..];
    let is_bullet = matches!(after_indent.first(), Some(b'-' | b'*' | b'+'))
        && matches!(after_indent.get(1), Some(b' ' | b'\t'));
    is_bullet.then_some(indentation)
}

/// The indentation for a new sub-bullet under the selected Pomodoro entry:
/// reuse an existing direct child's indentation when the entry already has
/// one, else the section's own established child indentation anywhere else
/// in the Pomodoros section, else the canonical tab fallback. Mirrors
/// `findPomodoroChildIndentation` exactly.
fn pomodoro_child_indentation(
    lines: &[LineSpan<'_>],
    entry_index: usize,
    entry_end_index: usize,
    section_start: usize,
    section_end: usize,
) -> String {
    for line in &lines[entry_index + 1..=entry_end_index] {
        if let Some(indentation) = unordered_child_indentation(line.text) {
            return indentation.to_string();
        }
    }
    for line in &lines[section_start..section_end] {
        if let Some(indentation) = unordered_child_indentation(line.text) {
            return indentation.to_string();
        }
    }
    "\t".to_string()
}

/// Insert `block_link` as a child of the entry at `entry_line_index`,
/// skipping insertion when a matching link is already among that entry's
/// children (idempotence).
fn insert_link_into_entry(
    day_contents: &str,
    lines: &[LineSpan<'_>],
    entry_line_index: usize,
    block_link: &str,
) -> (String, bool, Option<LinkPlacement>) {
    let entry_end_line = child_block_end_line(lines, entry_line_index);
    let child_start_offset = lines[entry_line_index].end;
    let child_end_offset = lines[entry_end_line].end;
    let already_linked =
        day_contents[child_start_offset..child_end_offset].contains(block_link);
    if already_linked {
        return (day_contents.to_string(), true, None);
    }

    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section = pomodoro::pomodoros_section_range(&line_texts)
        .unwrap_or(entry_line_index..entry_line_index + 1);
    let indentation = pomodoro_child_indentation(
        lines,
        entry_line_index,
        entry_end_line,
        section.start,
        section.end,
    );
    let new_line = format!("{indentation}- {block_link}");
    let placement = if lines[entry_end_line].end >= day_contents.len() {
        LinkPlacement::Appended
    } else {
        LinkPlacement::Inserted
    };
    let content =
        insert_line_after(day_contents, lines, entry_end_line, &new_line);
    (content, false, Some(placement))
}

/// Remove every occurrence of `block_link` found among the children of the
/// given `(entry_line_index, child_end_line_index)` ranges (both inclusive,
/// 0-based). A line whose entire (trimmed) body is the link is removed
/// along with its own child subtree; otherwise only the link text is
/// stripped from the line. Mirrors `planPomodoroLinkCleanupForRanges`.
fn remove_matching_links(
    contents: &str,
    block_link: &str,
    ranges: &[(usize, usize)],
) -> (String, usize) {
    let lines = line_spans(contents);
    let mut covered = vec![false; lines.len()];
    let mut removed_line_ranges: Vec<(usize, usize)> = Vec::new();
    let mut token_edits: Vec<(usize, String)> = Vec::new();
    let mut removed_count = 0usize;

    for &(entry_line, child_end_line) in ranges {
        if child_end_line < entry_line + 1 {
            continue;
        }
        for line_index in entry_line + 1..=child_end_line {
            if covered[line_index] {
                continue;
            }
            let text = lines[line_index].text;
            if !text.contains(block_link) {
                continue;
            }
            removed_count += 1;
            let sole_content =
                list_item_body(text).map(str::trim_end) == Some(block_link);
            if sole_content {
                let subtree_end = child_block_end_line(&lines, line_index);
                for covered in &mut covered[line_index..=subtree_end] {
                    *covered = true;
                }
                removed_line_ranges.push((line_index, subtree_end));
            } else {
                covered[line_index] = true;
                token_edits
                    .push((line_index, text.replacen(block_link, "", 1)));
            }
        }
    }

    if removed_count == 0 {
        return (contents.to_string(), 0);
    }

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for &(start_line, end_line) in &removed_line_ranges {
        let start = line_start_offset(&lines, start_line);
        let end = lines[end_line].end;
        edits.push((start, end, String::new()));
    }
    for (line_index, replacement) in &token_edits {
        let start = line_start_offset(&lines, *line_index);
        let text_end = start + lines[*line_index].text.len();
        let ending = &contents[text_end..lines[*line_index].end];
        edits.push((
            start,
            lines[*line_index].end,
            format!("{replacement}{ending}"),
        ));
    }
    edits.sort_by_key(|edit| edit.0);

    let mut result = String::with_capacity(contents.len());
    let mut cursor = 0;
    for (start, end, replacement) in &edits {
        result.push_str(&contents[cursor..*start]);
        result.push_str(replacement);
        cursor = *end;
    }
    result.push_str(&contents[cursor..]);

    (result, removed_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    fn planned(outcome: LinkInsertionOutcome) -> LinkInsertionPlan {
        match outcome {
            LinkInsertionOutcome::Planned(plan) => plan,
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    #[test]
    fn pull_forward_entry_text_matches_vault_fixture() {
        // Byte-for-byte parity with the recorded example in sase.md, per the
        // plan's design doc.
        assert_eq!(
            pull_forward_entry_text("2026-09-01", "2026-08-25"),
            "_2026-09-01 → 2026-08-25_ — 🍅 pulled into today's Pomodoro"
        );
    }

    #[test]
    fn sets_next_without_schedule_field() {
        let contents = "- [ ] Buy milk ^task1\n";
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        assert_eq!(plan.previous_status_symbol, ' ');
        assert_eq!(plan.new_status_symbol, '*');
        assert_eq!(plan.removed_scheduled, None);
        assert!(plan.schedule_log.is_none());
        assert_eq!(plan.content, "- [*] Buy milk ^task1\n");
    }

    #[test]
    fn blocked_is_forced_to_next() {
        let contents = "- [?] Waiting ^task1\n";
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        assert_eq!(plan.previous_status_symbol, '?');
        assert_eq!(plan.new_status_symbol, '*');
        assert_eq!(plan.content, "- [*] Waiting ^task1\n");
    }

    #[test]
    fn removes_single_future_scheduled_field() {
        let contents = "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n";
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        assert_eq!(plan.removed_scheduled.as_deref(), Some("2026-11-02"));
        assert_eq!(plan.content, "- [*] Ship it ^task1\n");
        assert!(plan.schedule_log.is_none());
    }

    #[test]
    fn two_scheduled_fields_retire_nothing() {
        let contents = "- [ ] Ship it [scheduled:: 2026-11-02] [scheduled:: 2026-12-01] ^task1\n";
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        assert_eq!(plan.removed_scheduled, None);
        assert_eq!(plan.new_status_symbol, '*');
        assert!(plan.content.contains("[scheduled:: 2026-11-02]"));
        assert!(plan.content.contains("[scheduled:: 2026-12-01]"));
    }

    #[test]
    fn past_or_today_schedule_retires_nothing() {
        let contents = "- [ ] Ship it [scheduled:: 2026-06-15] ^task1\n";
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        assert_eq!(plan.removed_scheduled, None);
        assert!(plan.content.contains("[scheduled:: 2026-06-15]"));
    }

    #[test]
    fn writes_pull_forward_entry_reusing_existing_indentation() {
        let contents = concat!(
            "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01 → 2026-07-01* — some other reason\n",
        );
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        let schedule_log = plan.schedule_log.expect("schedule log");
        assert_eq!(schedule_log.reason, PULL_FORWARD_REASON);
        assert_eq!(
            schedule_log.lines,
            vec![
                "\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro"
                    .to_string()
            ]
        );
        assert_eq!(
            plan.content,
            concat!(
                "- [*] Ship it ^task1\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro\n",
                "\t\t- *2026-08-01 → 2026-07-01* — some other reason\n",
            )
        );
    }

    #[test]
    fn schedule_log_falls_back_to_marker_indent_plus_tab_with_no_existing_entries(
    ) {
        let contents = concat!(
            "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
        );
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        let schedule_log = plan.schedule_log.expect("schedule log");
        assert_eq!(
            schedule_log.lines,
            vec![
                "\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro"
                    .to_string()
            ]
        );
    }

    #[test]
    fn no_schedule_log_marker_means_no_entry_even_when_field_removed() {
        let contents = "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n";
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        assert_eq!(plan.removed_scheduled.as_deref(), Some("2026-11-02"));
        assert!(plan.schedule_log.is_none());
    }

    #[test]
    fn preserves_crlf_line_endings() {
        let contents = "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\r\n\t- 🗓️ **SCHEDULE LOG**\r\n";
        let plan =
            plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
        assert_eq!(
            plan.content,
            "- [*] Ship it ^task1\r\n\t- 🗓️ **SCHEDULE LOG**\r\n\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro\r\n"
        );
    }

    #[test]
    fn plan_task_open_sets_ready_status_only() {
        let contents = "- [*] Doing it ^task1\n";
        let plan = plan_task_open(contents, 0).expect("plan");
        assert_eq!(plan.previous_status_symbol, '*');
        assert_eq!(plan.new_status_symbol, ' ');
        assert_eq!(plan.content, "- [ ] Doing it ^task1\n");
        assert_eq!(plan.removed_scheduled, None);
        assert!(plan.schedule_log.is_none());
    }

    #[test]
    fn returns_none_for_non_task_or_out_of_range_lines() {
        assert!(plan_task_next("plain text\n", 0, date(2026, 6, 15)).is_none());
        assert!(plan_task_open("plain text\n", 0).is_none());
        assert!(plan_task_next("- [ ] Task\n", 5, date(2026, 6, 15)).is_none());
        assert!(plan_task_open("- [ ] Task\n", 5).is_none());
    }

    #[test]
    fn implicit_insertion_targets_single_open_timed_entry() {
        let contents = "## Pomodoros\n- [ ] (0900-0930) — CURRENT\n";
        let plan = planned(
            plan_link_insertion(contents, "[[cash#^goog-exit]]", None)
                .expect("plan"),
        );
        assert!(!plan.already_linked);
        assert_eq!(plan.removed_links, 0);
        assert!(plan.has_changes);
        assert_eq!(
            plan.content,
            "## Pomodoros\n- [ ] (0900-0930) — CURRENT\n\t- [[cash#^goog-exit]]\n"
        );
    }

    #[test]
    fn implicit_insertion_falls_back_to_first_open_entry_without_timed() {
        let contents = "## Pomodoros\n- [ ] () — PLANNED\n- [ ] () — LATER\n";
        let plan = planned(
            plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
        );
        assert_eq!(
            plan.content,
            "## Pomodoros\n- [ ] () — PLANNED\n\t- [[cash#^id]]\n- [ ] () — LATER\n"
        );
    }

    #[test]
    fn errors_when_no_eligible_open_entry() {
        let contents = "## Pomodoros\n- [x] (0900-0930) — DONE\n";
        let error =
            plan_link_insertion(contents, "[[cash#^id]]", None).unwrap_err();
        assert_eq!(error, LinkPlanError::NoEligibleOpenEntry);
    }

    #[test]
    fn errors_on_multiple_open_timed_entries() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — ONE\n",
            "- [ ] (0935-1005) — TWO\n",
        );
        let error =
            plan_link_insertion(contents, "[[cash#^id]]", None).unwrap_err();
        assert_eq!(error, LinkPlanError::MultipleOpenTimedEntries);
    }

    #[test]
    fn errors_when_no_pomodoros_section() {
        let error =
            plan_link_insertion("# Day\n", "[[cash#^id]]", None).unwrap_err();
        assert_eq!(error, LinkPlanError::NoPomodorosSection);
    }

    #[test]
    fn named_selection_targets_existing_open_entry() {
        let contents = "## Pomodoros\n- [ ] () — MEMORY\n- [ ] () — FOCUS\n";
        let plan = planned(
            plan_link_insertion(contents, "[[cash#^id]]", Some("memory"))
                .expect("plan"),
        );
        assert_eq!(plan.pomodoro_name.as_deref(), Some("MEMORY"));
        assert!(plan
            .content
            .contains("- [ ] () — MEMORY\n\t- [[cash#^id]]\n"));
    }

    #[test]
    fn named_selection_reports_creation_needed() {
        let contents = "## Pomodoros\n- [ ] () — FOCUS\n";
        let outcome =
            plan_link_insertion(contents, "[[cash#^id]]", Some("memory"))
                .expect("plan");
        assert_eq!(
            outcome,
            LinkInsertionOutcome::NeedsPomodoroCreation {
                canonical_name: "MEMORY".to_string()
            }
        );
    }

    #[test]
    fn invalid_pomodoro_name_is_rejected() {
        let contents = "## Pomodoros\n- [ ] () — FOCUS\n";
        let error =
            plan_link_insertion(contents, "[[cash#^id]]", Some("bad_id"))
                .unwrap_err();
        assert_eq!(error, LinkPlanError::InvalidPomodoroName);
    }

    #[test]
    fn idempotent_insertion_skips_when_already_linked() {
        let contents =
            "## Pomodoros\n- [ ] (0900-0930) — CURRENT\n\t- [[cash#^id]]\n";
        let plan = planned(
            plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
        );
        assert!(plan.already_linked);
        assert!(!plan.has_changes);
        assert_eq!(plan.content, contents);
    }

    #[test]
    fn insertion_removes_duplicate_from_later_open_entry() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — CURRENT\n",
            "- [ ] () — LATER\n",
            "\t- [[cash#^id]]\n",
        );
        let plan = planned(
            plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
        );
        assert!(!plan.already_linked);
        assert_eq!(plan.removed_links, 1);
        assert!(plan.has_changes);
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] (0900-0930) — CURRENT\n",
                "\t- [[cash#^id]]\n",
                "- [ ] () — LATER\n",
            )
        );
    }

    #[test]
    fn removal_never_touches_completed_pomodoros() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [x] (0900-0930) — DONE\n",
            "\t- [[cash#^id]]\n",
            "- [ ] () — OPEN\n",
        );
        let plan = plan_link_removal(contents, "[[cash#^id]]");
        assert_eq!(plan.removed_links, 0);
        assert!(!plan.has_changes);
        assert_eq!(plan.content, contents);
    }

    #[test]
    fn removal_deletes_whole_subtree_for_sole_content_bullet() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — OPEN\n",
            "\t- [[cash#^id]]\n",
            "\t\t- note about it\n",
            "- [ ] () — NEXT\n",
        );
        let plan = plan_link_removal(contents, "[[cash#^id]]");
        assert_eq!(plan.removed_links, 1);
        assert_eq!(
            plan.content,
            concat!("## Pomodoros\n", "- [ ] () — OPEN\n", "- [ ] () — NEXT\n",)
        );
    }

    #[test]
    fn removal_only_strips_the_link_when_bullet_has_other_text() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — OPEN\n",
            "\t- see also [[cash#^id]] for context\n",
        );
        let plan = plan_link_removal(contents, "[[cash#^id]]");
        assert_eq!(plan.removed_links, 1);
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — OPEN\n",
                "\t- see also  for context\n",
            )
        );
    }

    #[test]
    fn removal_reports_no_changes_when_nothing_matches() {
        let contents = "## Pomodoros\n- [ ] () — OPEN\n";
        let plan = plan_link_removal(contents, "[[cash#^id]]");
        assert_eq!(plan.removed_links, 0);
        assert!(!plan.has_changes);
        assert_eq!(plan.content, contents);
    }

    #[test]
    fn link_operations_preserve_crlf() {
        let contents = "## Pomodoros\r\n- [ ] (0900-0930) — CURRENT\r\n";
        let plan = planned(
            plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
        );
        assert_eq!(
            plan.content,
            "## Pomodoros\r\n- [ ] (0900-0930) — CURRENT\r\n\t- [[cash#^id]]\r\n"
        );

        let removal = plan_link_removal(&plan.content, "[[cash#^id]]");
        assert_eq!(removal.content, contents);
        assert_eq!(removal.removed_links, 1);
    }

    fn relocated(contents: &str, block_link: &str) -> LinkRelocationPlan {
        plan_link_relocation(contents, block_link, None).expect("relocate")
    }

    fn relocated_named(
        contents: &str,
        block_link: &str,
        name: &str,
    ) -> LinkRelocationPlan {
        plan_link_relocation(contents, block_link, Some(name))
            .expect("relocate")
    }

    #[test]
    fn relocation_moves_a_later_link_into_the_timed_current_pomodoro() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - keep\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^id]]\n",
        );
        let plan = relocated(contents, "[[cash#^id]]");
        assert_eq!(plan.action, LinkRelocationAction::Moved);
        assert!(plan.has_changes);
        assert_eq!(plan.source.name.as_deref(), Some("LATER"));
        assert_eq!(plan.destination.name.as_deref(), Some("CURRENT"));
        assert_eq!(plan.destination.time_range.as_deref(), Some("0900-0930"));
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0900-0930**) — CURRENT\n",
                "  - keep\n",
                "  - [[cash#^id]]\n",
                "- [ ] () — LATER\n",
            )
        );
    }

    #[test]
    fn relocation_moves_an_earlier_link_into_a_later_destination() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — EARLIER\n",
            "  - [[cash#^id]]\n",
            "- [ ] (**1330-1400**) — CURRENT\n",
            "  - context\n",
        );
        let plan = relocated(contents, "[[cash#^id]]");
        assert_eq!(plan.action, LinkRelocationAction::Moved);
        assert_eq!(plan.source.name.as_deref(), Some("EARLIER"));
        assert_eq!(plan.destination.name.as_deref(), Some("CURRENT"));
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — EARLIER\n",
                "- [ ] (**1330-1400**) — CURRENT\n",
                "  - context\n",
                "  - [[cash#^id]]\n",
            )
        );
    }

    #[test]
    fn relocation_falls_back_to_the_first_open_entry_without_timed() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — PLANNED\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^id]]\n",
        );
        let plan = relocated(contents, "[[cash#^id]]");
        assert_eq!(plan.destination.name.as_deref(), Some("PLANNED"));
        assert!(plan
            .content
            .contains("- [ ] () — PLANNED\n  - [[cash#^id]]\n"));
        assert!(plan.content.contains("- [ ] () — LATER\n"));
        assert!(!plan.content.contains("- [ ] () — LATER\n  - [[cash#^id]]"));
    }

    #[test]
    fn relocation_reports_an_unnamed_endpoint() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**)\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^id]]\n",
        );
        let plan = relocated(contents, "[[cash#^id]]");
        assert!(plan.destination.name.is_none());
        assert_eq!(plan.destination.time_range.as_deref(), Some("0900-0930"));
        assert_eq!(plan.destination.line, 2);
    }

    #[test]
    fn relocation_moves_a_descendant_bearing_task_link_as_a_subtree() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^id]]\n",
            "    - review notes\n",
            "      - nested\n",
        );
        let plan = relocated(contents, "[[cash#^id]]");
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0900-0930**) — CURRENT\n",
                "  - [[cash#^id]]\n",
                "    - review notes\n",
                "      - nested\n",
                "- [ ] () — LATER\n",
            )
        );
    }

    #[test]
    fn relocation_uses_the_destination_child_indentation() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "\t- existing\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^id]]\n",
            "    - note\n",
        );
        let plan = relocated(contents, "[[cash#^id]]");
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0900-0930**) — CURRENT\n",
                "\t- existing\n",
                "\t- [[cash#^id]]\n",
                "\t  - note\n",
                "- [ ] () — LATER\n",
            )
        );
    }

    #[test]
    fn relocation_preserves_crlf_and_a_missing_final_newline() {
        let contents =
            "## Pomodoros\r\n- [ ] (**0900-0930**) — CURRENT\r\n- [ ] () — LATER\r\n  - [[cash#^id]]";
        let plan = relocated(contents, "[[cash#^id]]");
        assert_eq!(
            plan.content,
            "## Pomodoros\r\n- [ ] (**0900-0930**) — CURRENT\r\n  - [[cash#^id]]\r\n- [ ] () — LATER\r\n"
        );
        assert_eq!(plan.placement, Some(LinkPlacement::Inserted));
    }

    #[test]
    fn relocation_ignores_completed_history_and_mixed_text_lookalikes() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0830**) — DONE\n",
            "  - [[cash#^id]]\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - see [[cash#^id]] later\n",
            "  - context\n",
            "    - [[cash#^id]]\n",
        );
        let error =
            plan_link_relocation(contents, "[[cash#^id]]", None).unwrap_err();
        assert_eq!(error, LinkRelocationError::NoMovableLink);
    }

    #[test]
    fn relocation_errors_when_the_link_is_missing() {
        let contents = "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n";
        assert_eq!(
            plan_link_relocation(contents, "[[cash#^id]]", None).unwrap_err(),
            LinkRelocationError::NoMovableLink
        );
    }

    #[test]
    fn relocation_errors_on_duplicate_movable_links() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^id]]\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^id]]\n",
        );
        assert_eq!(
            plan_link_relocation(contents, "[[cash#^id]]", None).unwrap_err(),
            LinkRelocationError::MultipleMovableLinks
        );
    }

    #[test]
    fn relocation_is_a_noop_when_the_link_is_already_current() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^id]]\n",
            "    - notes\n",
            "- [ ] () — LATER\n",
        );
        let plan = relocated(contents, "[[cash#^id]]");
        assert_eq!(plan.action, LinkRelocationAction::AlreadyCurrent);
        assert!(!plan.has_changes);
        assert_eq!(plan.content, contents);
        assert_eq!(plan.source.name.as_deref(), Some("CURRENT"));
        assert_eq!(plan.destination.name.as_deref(), Some("CURRENT"));
        assert!(plan.placement.is_none());
    }

    #[test]
    fn relocation_reuses_implicit_selection_errors() {
        assert_eq!(
            plan_link_relocation("# Day\n", "[[cash#^id]]", None).unwrap_err(),
            LinkRelocationError::NoPomodorosSection
        );
        assert_eq!(
            plan_link_relocation(
                "## Pomodoros\n- [x] (**0900-0930**) — DONE\n",
                "[[cash#^id]]",
                None
            )
            .unwrap_err(),
            LinkRelocationError::NoMovableLink
        );
        assert_eq!(
            plan_link_relocation(
                "## Pomodoros\n- [ ] (**0900-0930**) — A\n- [ ] (**0930-1000**) — B\n  - [[cash#^id]]\n",
                "[[cash#^id]]",
                None
            )
            .unwrap_err(),
            LinkRelocationError::MultipleOpenTimedEntries
        );
    }

    #[test]
    fn named_relocation_moves_to_an_exact_open_match() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^id]]\n",
            "    - notes\n",
            "- [ ] () — DEEP+WORK\n",
            "  - keep\n",
        );
        let plan = relocated_named(contents, "[[cash#^id]]", "deep+work");
        assert_eq!(plan.action, LinkRelocationAction::Moved);
        assert!(!plan.creates_pomodoro);
        assert_eq!(plan.source.name.as_deref(), Some("CURRENT"));
        assert_eq!(plan.destination.name.as_deref(), Some("DEEP+WORK"));
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0900-0930**) — CURRENT\n",
                "- [ ] () — DEEP+WORK\n",
                "  - keep\n",
                "  - [[cash#^id]]\n",
                "    - notes\n",
            )
        );
    }

    #[test]
    fn named_relocation_prefix_match_loses_to_a_whole_slug() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — CODE\n",
            "  - [[cash#^id]]\n",
            "- [ ] () — CODING\n",
        );
        let plan = relocated_named(contents, "[[cash#^id]]", "cod");
        assert_eq!(plan.destination.name.as_deref(), Some("CODE"));
        assert!(plan.content.contains("- [ ] () — CODE\n  - [[cash#^id]]\n"));
        assert!(!plan.content.contains("- [ ] () — CODING\n  - [[cash#^id]]"));

        let exact = relocated_named(contents, "[[cash#^id]]", "coding");
        assert_eq!(exact.destination.name.as_deref(), Some("CODING"));
        assert!(exact
            .content
            .contains("- [ ] () — CODING\n  - [[cash#^id]]\n"));
    }

    #[test]
    fn named_relocation_is_a_noop_when_already_at_the_named_destination() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "- [ ] () — DEEP+WORK\n",
            "  - [[cash#^id]]\n",
            "    - notes\n",
        );
        let plan = relocated_named(contents, "[[cash#^id]]", "deep+work");
        assert_eq!(plan.action, LinkRelocationAction::AlreadyCurrent);
        assert!(!plan.has_changes);
        assert!(!plan.creates_pomodoro);
        assert_eq!(plan.content, contents);
        assert_eq!(plan.source.name.as_deref(), Some("DEEP+WORK"));
        assert_eq!(plan.destination.name.as_deref(), Some("DEEP+WORK"));
        assert!(plan.placement.is_none());
    }

    #[test]
    fn named_relocation_creates_on_completed_only_and_missing_names() {
        let completed_only = concat!(
            "## Pomodoros\n",
            "- [x] () — BUGS\n",
            "  - old bug\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^id]]\n",
            "    - review\n",
            "- [ ] () — MEMORY\n",
        );
        let plan = relocated_named(completed_only, "[[cash#^id]]", "bugs");
        assert_eq!(plan.action, LinkRelocationAction::Moved);
        assert!(plan.creates_pomodoro);
        assert_eq!(plan.destination.name.as_deref(), Some("BUGS"));
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [x] () — BUGS\n",
                "  - old bug\n",
                "- [ ] (**0900-0930**) — CURRENT\n",
                "- [ ] () — BUGS\n",
                "  - [[cash#^id]]\n",
                "    - review\n",
                "- [ ] () — MEMORY\n",
            )
        );
        assert_eq!(plan.destination.line, 5);

        let missing = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^id]]\n",
        );
        let created = relocated_named(missing, "[[cash#^id]]", "deep+work");
        assert!(created.creates_pomodoro);
        assert_eq!(created.destination.name.as_deref(), Some("DEEP+WORK"));
        assert_eq!(
            created.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0900-0930**) — CURRENT\n",
                "- [ ] () — DEEP+WORK\n",
                "  - [[cash#^id]]\n",
            )
        );
        assert_eq!(created.source.name.as_deref(), Some("CURRENT"));
        assert_eq!(created.destination.line, 3);
    }

    #[test]
    fn named_relocation_inserts_before_the_first_future_entry() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — ALPHA\n",
            "  - [[cash#^id]]\n",
            "- [ ] () — BRAVO\n",
        );
        let plan = relocated_named(contents, "[[cash#^id]]", "zzzz");
        assert!(plan.creates_pomodoro);
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — ZZZZ\n",
                "  - [[cash#^id]]\n",
                "- [ ] () — ALPHA\n",
                "- [ ] () — BRAVO\n",
            )
        );
        assert_eq!(plan.destination.line, 2);
        assert_eq!(plan.source.name.as_deref(), Some("ALPHA"));
    }

    #[test]
    fn named_relocation_selects_an_existing_name_despite_multiple_timed() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (0800-0830) — ONE\n",
            "  - [[cash#^id]]\n",
            "- [ ] (**0900-0930**) — TWO\n",
        );
        let plan = relocated_named(contents, "[[cash#^id]]", "two");
        assert!(!plan.creates_pomodoro);
        assert_eq!(plan.destination.name.as_deref(), Some("TWO"));
        assert!(plan
            .content
            .contains("- [ ] (**0900-0930**) — TWO\n  - [[cash#^id]]\n"));
    }

    #[test]
    fn named_relocation_rejects_creation_with_multiple_timed_entries() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (0800-0830) — ONE\n",
            "  - [[cash#^id]]\n",
            "- [ ] (**0900-0930**) — TWO\n",
        );
        assert_eq!(
            plan_link_relocation(contents, "[[cash#^id]]", Some("three"))
                .unwrap_err(),
            LinkRelocationError::MultipleOpenTimedEntries
        );
    }

    #[test]
    fn named_relocation_rejects_an_invalid_name() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^id]]\n",
        );
        assert_eq!(
            plan_link_relocation(contents, "[[cash#^id]]", Some("bad_id"))
                .unwrap_err(),
            LinkRelocationError::InvalidPomodoroName
        );
    }

    #[test]
    fn named_relocation_preserves_descendants_and_destination_indent() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "\t- existing\n",
            "\t- [[cash#^id]]\n",
            "\t  - nested\n",
            "- [ ] () — LATER\n",
        );
        let plan = relocated_named(contents, "[[cash#^id]]", "later");
        assert_eq!(
            plan.content,
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0900-0930**) — CURRENT\n",
                "\t- existing\n",
                "- [ ] () — LATER\n",
                "\t- [[cash#^id]]\n",
                "\t  - nested\n",
            )
        );
    }

    #[test]
    fn named_relocation_preserves_crlf_when_creating() {
        let contents = "## Pomodoros\r\n- [x] Done\r\n\t- old child\r\n- [ ] (**0900-0930**) — CURRENT\r\n  - [[cash#^id]]\r\n";
        let plan = relocated_named(contents, "[[cash#^id]]", "crlf-name");
        assert!(plan.creates_pomodoro);
        assert_eq!(
            plan.content,
            "## Pomodoros\r\n- [x] Done\r\n\t- old child\r\n- [ ] (**0900-0930**) — CURRENT\r\n- [ ] () — CRLF-NAME\r\n\t- [[cash#^id]]\r\n"
        );
    }

    #[test]
    fn named_relocation_same_location_noop_does_not_edit_bytes() {
        let contents = "## Pomodoros\n- [ ] () — DEEP+WORK\n  - [[cash#^id]]\n";
        let plan = relocated_named(contents, "[[cash#^id]]", "DEEP+WORK");
        assert_eq!(plan.action, LinkRelocationAction::AlreadyCurrent);
        assert_eq!(plan.content, contents);
    }
}

use crate::native::{
    capture::{
        line_spans, list_item_body, nearest_shallower_list_item_parent,
        LineSpan,
    },
    capture_pomodoros::{self, NamedSelection, PomodoroEntry, PomodoroState},
    markdown, pomodoro,
};

use super::text::{
    child_block_end_line, insert_line_after, line_start_offset,
    pomodoro_child_indentation,
};

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
/// `capture/`, which already knows how to select-or-create and insert a
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

pub(crate) fn select_implicit_open_entry(
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

pub(crate) struct MovableLink<'a> {
    pub(crate) owner: &'a PomodoroEntry,
    pub(crate) entry_line_index: usize,
    pub(crate) line_index: usize,
    pub(crate) subtree_end: usize,
}

pub(crate) fn find_movable_task_links<'a>(
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

/// Insert `block_link` as a child of the entry at `entry_line_index`,
/// skipping insertion when a matching link is already among that entry's
/// children (idempotence).
pub(super) fn insert_link_into_entry(
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

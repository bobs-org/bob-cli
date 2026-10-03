use crate::native::{
    capture::{leading_spaces_or_tabs_len, line_spans},
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    pomodoro,
};

use super::{
    links::{
        find_movable_task_links, select_implicit_open_entry, LinkPlacement,
        LinkPlanError,
    },
    text::{
        child_block_end_line, extract_line_range, insert_line_after,
        insert_line_before, insert_lines_after, logical_lines,
        pomodoro_child_indentation, reindent_subtree, remove_line_range,
    },
};

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

pub(super) fn destination_endpoint_after_move(
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

pub(crate) fn move_subtree_to_entry(
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

pub(crate) fn insert_named_placeholder(
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

pub(super) fn relocation_from_link_plan_error(
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

pub(crate) fn endpoint_from_entry(entry: &PomodoroEntry) -> PomodoroEndpoint {
    PomodoroEndpoint {
        line: entry.line,
        name: entry.name.clone(),
        time_range: entry.time_range.clone(),
    }
}

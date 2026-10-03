use crate::native::{
    capture::{line_spans, list_item_body, nearest_shallower_list_item_parent},
    capture_pomodoros::{self, PomodoroState},
    markdown, pomodoro,
};

use super::{
    links::{
        find_movable_task_links, insert_link_into_entry,
        select_implicit_open_entry, LinkPlacement,
    },
    relocation::{
        destination_endpoint_after_move, endpoint_from_entry,
        insert_named_placeholder, move_subtree_to_entry,
        relocation_from_link_plan_error, LinkRelocationError, PomodoroEndpoint,
    },
    text::child_block_end_line,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroLinkLedgerAction {
    Linked,
    Moved,
    AlreadyCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroLinkLedgerPlan {
    pub(crate) content: String,
    pub(crate) has_changes: bool,
    pub(crate) action: PomodoroLinkLedgerAction,
    pub(crate) placement: Option<LinkPlacement>,
    pub(crate) source: Option<PomodoroEndpoint>,
    pub(crate) destination: PomodoroEndpoint,
    pub(crate) creates_pomodoro: bool,
}

/// Queue-respecting Task Link planner for solo `@`/`^` links.
///
/// Let _Q_ be the open Pomodoro whose children hold the task's dedicated
/// Task Link. With `#name`, move _Q_'s subtree to the named destination
/// (creating it when missing) or insert a new link when there is no _Q_;
/// when _Q_ is already the destination nothing changes. With no name and an
/// existing _Q_, respect the queue: the destination is _Q_ and nothing
/// moves. With no name and no _Q_, insert under the implicit current/next
/// Pomodoro.
pub(crate) fn plan_pomodoro_link_ledger(
    day_contents: &str,
    block_link: &str,
    pomodoro_name: Option<&str>,
) -> Result<PomodoroLinkLedgerPlan, LinkRelocationError> {
    let scan = capture_pomodoros::scan(day_contents);
    let lines = line_spans(day_contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section = pomodoro::pomodoros_section_range(&line_texts)
        .ok_or(LinkRelocationError::NoPomodorosSection)?;
    let movable =
        find_movable_task_links(&lines, &scan, section.clone(), block_link);
    let queue = match movable.as_slice() {
        [] => None,
        [_, _, ..] => return Err(LinkRelocationError::MultipleMovableLinks),
        [source] => Some(source),
    };

    if let Some(selector) = pomodoro_name {
        match capture_pomodoros::select_named(&scan, selector) {
            capture_pomodoros::NamedSelection::Found(entry) => {
                let dest_index = entry.line - 1;
                let destination = endpoint_from_entry(entry);
                if let Some(q) = queue {
                    let source_endpoint = endpoint_from_entry(q.owner);
                    if q.entry_line_index == dest_index {
                        return Ok(PomodoroLinkLedgerPlan {
                            content: day_contents.to_string(),
                            has_changes: false,
                            action: PomodoroLinkLedgerAction::AlreadyCurrent,
                            placement: None,
                            source: Some(source_endpoint),
                            destination,
                            creates_pomodoro: false,
                        });
                    }
                    let (content, placement) = move_subtree_to_entry(
                        day_contents,
                        q.line_index,
                        q.subtree_end,
                        dest_index,
                    )?;
                    let destination = destination_endpoint_after_move(
                        &content,
                        pomodoro_name,
                        &destination,
                        false,
                    )?;
                    return Ok(PomodoroLinkLedgerPlan {
                        content,
                        has_changes: true,
                        action: PomodoroLinkLedgerAction::Moved,
                        placement: Some(placement),
                        source: Some(source_endpoint),
                        destination,
                        creates_pomodoro: false,
                    });
                }
                let (content, already_linked, placement) =
                    insert_link_into_entry(
                        day_contents,
                        &lines,
                        dest_index,
                        block_link,
                    );
                if already_linked {
                    return Ok(PomodoroLinkLedgerPlan {
                        content,
                        has_changes: false,
                        action: PomodoroLinkLedgerAction::AlreadyCurrent,
                        placement: None,
                        source: None,
                        destination,
                        creates_pomodoro: false,
                    });
                }
                return Ok(PomodoroLinkLedgerPlan {
                    content,
                    has_changes: true,
                    action: PomodoroLinkLedgerAction::Linked,
                    placement,
                    source: None,
                    destination,
                    creates_pomodoro: false,
                });
            }
            capture_pomodoros::NamedSelection::CompletedOnly(_)
            | capture_pomodoros::NamedSelection::Missing { .. } => {
                let (with_placeholder, created_line, name) =
                    insert_named_placeholder(day_contents, selector)?;
                if let Some(q) = queue {
                    let source_endpoint = endpoint_from_entry(q.owner);
                    let working_scan =
                        capture_pomodoros::scan(&with_placeholder);
                    let working_lines = line_spans(&with_placeholder);
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
                    let relocated = match movable.as_slice() {
                        [source] => source,
                        _ => return Err(LinkRelocationError::NoMovableLink),
                    };
                    let (content, placement) = move_subtree_to_entry(
                        &with_placeholder,
                        relocated.line_index,
                        relocated.subtree_end,
                        created_line,
                    )?;
                    let scan_after = capture_pomodoros::scan(&content);
                    let destination = scan_after
                        .entries
                        .iter()
                        .find(|entry| {
                            entry.state == PomodoroState::Open
                                && entry.name.as_deref() == Some(name.as_str())
                        })
                        .map(endpoint_from_entry)
                        .unwrap_or(PomodoroEndpoint {
                            line: created_line + 1,
                            name: Some(name.clone()),
                            time_range: None,
                        });
                    return Ok(PomodoroLinkLedgerPlan {
                        content,
                        has_changes: true,
                        action: PomodoroLinkLedgerAction::Moved,
                        placement: Some(placement),
                        source: Some(source_endpoint),
                        destination,
                        creates_pomodoro: true,
                    });
                }
                let created_lines = line_spans(&with_placeholder);
                let (content, already_linked, placement) =
                    insert_link_into_entry(
                        &with_placeholder,
                        &created_lines,
                        created_line,
                        block_link,
                    );
                debug_assert!(!already_linked);
                let scan_after = capture_pomodoros::scan(&content);
                let destination = scan_after
                    .entries
                    .iter()
                    .find(|entry| {
                        entry.state == PomodoroState::Open
                            && entry.name.as_deref() == Some(name.as_str())
                    })
                    .map(endpoint_from_entry)
                    .unwrap_or(PomodoroEndpoint {
                        line: created_line + 1,
                        name: Some(name.clone()),
                        time_range: None,
                    });
                return Ok(PomodoroLinkLedgerPlan {
                    content,
                    has_changes: true,
                    action: PomodoroLinkLedgerAction::Linked,
                    placement,
                    source: None,
                    destination,
                    creates_pomodoro: true,
                });
            }
        }
    }

    if let Some(q) = queue {
        let endpoint = endpoint_from_entry(q.owner);
        return Ok(PomodoroLinkLedgerPlan {
            content: day_contents.to_string(),
            has_changes: false,
            action: PomodoroLinkLedgerAction::AlreadyCurrent,
            placement: None,
            source: Some(endpoint.clone()),
            destination: endpoint,
            creates_pomodoro: false,
        });
    }

    let destination_entry = select_implicit_open_entry(&scan)
        .map_err(relocation_from_link_plan_error)?;
    let dest_index = destination_entry.line - 1;
    let destination = endpoint_from_entry(destination_entry);
    let (content, already_linked, placement) =
        insert_link_into_entry(day_contents, &lines, dest_index, block_link);
    if already_linked {
        return Ok(PomodoroLinkLedgerPlan {
            content,
            has_changes: false,
            action: PomodoroLinkLedgerAction::AlreadyCurrent,
            placement: None,
            source: None,
            destination,
            creates_pomodoro: false,
        });
    }
    Ok(PomodoroLinkLedgerPlan {
        content,
        has_changes: true,
        action: PomodoroLinkLedgerAction::Linked,
        placement,
        source: None,
        destination,
        creates_pomodoro: false,
    })
}

/// Read-only helper for the active-task discovery phase: each open entry's
/// dedicated Task Links (`[[route#^id]]` sole-content child bullets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenEntryLinks {
    pub(crate) line: usize,
    pub(crate) name: Option<String>,
    pub(crate) time_range: Option<String>,
    pub(crate) is_current: bool,
    pub(crate) links: Vec<String>,
}

pub(crate) fn list_open_entry_links(day_contents: &str) -> Vec<OpenEntryLinks> {
    let scan = capture_pomodoros::scan(day_contents);
    if !scan.has_section {
        return Vec::new();
    }
    let lines = line_spans(day_contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let Some(section) = pomodoro::pomodoros_section_range(&line_texts) else {
        return Vec::new();
    };
    let fenced = markdown::fenced_lines(&line_texts, section.clone());
    let mut result = Vec::new();
    for entry in scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
    {
        let entry_line_index = entry.line - 1;
        let child_end = child_block_end_line(&lines, entry_line_index);
        let mut links = Vec::new();
        if child_end >= entry_line_index + 1 {
            for line_index in entry_line_index + 1..=child_end {
                if fenced.contains(&line_index) {
                    continue;
                }
                if nearest_shallower_list_item_parent(&lines, line_index)
                    != Some(entry_line_index)
                {
                    continue;
                }
                let Some(body) =
                    list_item_body(lines[line_index].text).map(str::trim_end)
                else {
                    continue;
                };
                if body.starts_with("[[")
                    && body.ends_with("]]")
                    && body.contains("#^")
                {
                    links.push(body.to_string());
                }
            }
        }
        result.push(OpenEntryLinks {
            line: entry.line,
            name: entry.name.clone(),
            time_range: entry.time_range.clone(),
            is_current: entry.is_current,
            links,
        });
    }
    result
}

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

/// Where an existing-task Pomodoro link should land.
///
/// Capture maps only to [`Self::Implicit`] and [`Self::Named`]. Exact-name
/// and stale-safe entry refs are `bob ref today`-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroLinkTarget<'a> {
    Implicit,
    Named(&'a str),
    ExactName(&'a str),
    Entry(&'a capture_pomodoros::PomodoroRef),
}

impl PomodoroLinkTarget<'_> {
    pub(crate) fn named_selector(&self) -> Option<&str> {
        match self {
            Self::Named(name) | Self::ExactName(name) => Some(name),
            Self::Implicit | Self::Entry(_) => None,
        }
    }
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
    target: PomodoroLinkTarget<'_>,
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

    match target {
        PomodoroLinkTarget::Named(selector) => {
            match capture_pomodoros::select_named(&scan, selector) {
                capture_pomodoros::NamedSelection::Found(entry) => {
                    plan_found_open_entry(
                        day_contents,
                        &lines,
                        block_link,
                        queue,
                        entry,
                        Some(selector),
                    )
                }
                capture_pomodoros::NamedSelection::CompletedOnly(_)
                | capture_pomodoros::NamedSelection::Missing { .. } => {
                    plan_created_named_entry(
                        day_contents,
                        block_link,
                        queue,
                        selector,
                    )
                }
            }
        }
        PomodoroLinkTarget::ExactName(selector) => {
            match capture_pomodoros::select_named_exact(&scan, selector) {
                capture_pomodoros::NamedSelection::Found(entry) => {
                    plan_found_open_entry(
                        day_contents,
                        &lines,
                        block_link,
                        queue,
                        entry,
                        Some(selector),
                    )
                }
                capture_pomodoros::NamedSelection::CompletedOnly(_)
                | capture_pomodoros::NamedSelection::Missing { .. } => {
                    plan_created_named_entry(
                        day_contents,
                        block_link,
                        queue,
                        selector,
                    )
                }
            }
        }
        PomodoroLinkTarget::Entry(pomodoro_ref) => {
            match scan.by_ref(pomodoro_ref) {
                capture_pomodoros::PomodoroRefLookup::Found(entry)
                    if entry.state == PomodoroState::Open =>
                {
                    plan_found_open_entry(
                        day_contents,
                        &lines,
                        block_link,
                        queue,
                        entry,
                        None,
                    )
                }
                capture_pomodoros::PomodoroRefLookup::Found(_) => {
                    Err(LinkRelocationError::PomodoroClosed)
                }
                capture_pomodoros::PomodoroRefLookup::Stale
                | capture_pomodoros::PomodoroRefLookup::Ambiguous => {
                    Err(LinkRelocationError::StalePomodoro)
                }
            }
        }
        PomodoroLinkTarget::Implicit => {
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
            let (content, already_linked, placement) = insert_link_into_entry(
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
    }
}

/// The default destination for an implicit link: _Q_ when the task already
/// has a dedicated live link, otherwise the implicit current/next entry.
/// `None` when there is no unique default (no open entry, or several
/// running timed entries and no _Q_).
pub(crate) fn default_pomodoro_link_destination<'a>(
    scan: &'a capture_pomodoros::PomodoroScan,
    queue_owner_line: Option<usize>,
) -> Option<&'a capture_pomodoros::PomodoroEntry> {
    if let Some(line) = queue_owner_line {
        return scan.entries.iter().find(|entry| {
            entry.state == PomodoroState::Open && entry.line == line
        });
    }
    select_implicit_open_entry(scan).ok()
}

fn plan_found_open_entry(
    day_contents: &str,
    lines: &[crate::native::capture::LineSpan<'_>],
    block_link: &str,
    queue: Option<&super::links::MovableLink<'_>>,
    entry: &capture_pomodoros::PomodoroEntry,
    selector: Option<&str>,
) -> Result<PomodoroLinkLedgerPlan, LinkRelocationError> {
    let dest_index = entry.line - 1;
    let dest_digest = entry.pomodoro_ref.digest.clone();
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
        let destination = destination_after_found_move(
            &content,
            selector,
            &dest_digest,
            &destination,
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
        insert_link_into_entry(day_contents, lines, dest_index, block_link);
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

fn plan_created_named_entry(
    day_contents: &str,
    block_link: &str,
    queue: Option<&super::links::MovableLink<'_>>,
    selector: &str,
) -> Result<PomodoroLinkLedgerPlan, LinkRelocationError> {
    let (with_placeholder, created_line, name) =
        insert_named_placeholder(day_contents, selector)?;
    if let Some(q) = queue {
        let source_endpoint = endpoint_from_entry(q.owner);
        let working_scan = capture_pomodoros::scan(&with_placeholder);
        let working_lines = line_spans(&with_placeholder);
        let working_texts = working_lines
            .iter()
            .map(|line| line.text)
            .collect::<Vec<_>>();
        let working_section = pomodoro::pomodoros_section_range(&working_texts)
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
        let destination = created_destination(&content, created_line, &name);
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
    let (content, already_linked, placement) = insert_link_into_entry(
        &with_placeholder,
        &created_lines,
        created_line,
        block_link,
    );
    debug_assert!(!already_linked);
    let destination = created_destination(&content, created_line, &name);
    Ok(PomodoroLinkLedgerPlan {
        content,
        has_changes: true,
        action: PomodoroLinkLedgerAction::Linked,
        placement,
        source: None,
        destination,
        creates_pomodoro: true,
    })
}

fn created_destination(
    content: &str,
    created_line: usize,
    name: &str,
) -> PomodoroEndpoint {
    let scan_after = capture_pomodoros::scan(content);
    scan_after
        .entries
        .iter()
        .find(|entry| {
            entry.state == PomodoroState::Open
                && entry.name.as_deref() == Some(name)
        })
        .map(endpoint_from_entry)
        .unwrap_or(PomodoroEndpoint {
            line: created_line + 1,
            name: Some(name.to_string()),
            time_range: None,
        })
}

fn destination_after_found_move(
    content: &str,
    selector: Option<&str>,
    dest_digest: &str,
    planned: &PomodoroEndpoint,
) -> Result<PomodoroEndpoint, LinkRelocationError> {
    if let Some(name) = selector {
        return destination_endpoint_after_move(
            content,
            Some(name),
            planned,
            false,
        );
    }
    let scan = capture_pomodoros::scan(content);
    let mut matches = scan.entries.iter().filter(|entry| {
        entry.state == PomodoroState::Open
            && entry.pomodoro_ref.digest == dest_digest
    });
    match (matches.next(), matches.next()) {
        (Some(entry), None) => Ok(endpoint_from_entry(entry)),
        _ => Ok(planned.clone()),
    }
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

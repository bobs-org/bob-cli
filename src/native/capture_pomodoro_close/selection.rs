//! Numbered Task Links and outcome selection for the pure close planner.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use super::super::{
    capture::{leading_spaces_or_tabs_len, line_spans, list_marker_len},
    markdown,
};
use super::ledger::{sub_bullet_range, RunningPomodoro};
use super::links::{
    bare_embedded_link, bare_plain_link, move_only_destination,
    range_is_struck, strikethrough_inner_spans, strip_pomodoro_markers,
    wikilink_tokens, WikiToken,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseSelection {
    pub in_progress: Option<BTreeSet<u32>>,
    pub complete: BTreeSet<u32>,
    pub drop: BTreeSet<u32>,
    pub raw: String,
}

impl CloseSelection {
    pub(crate) fn new(
        in_progress: Option<BTreeSet<u32>>,
        complete: BTreeSet<u32>,
        drop: BTreeSet<u32>,
        raw: impl Into<String>,
    ) -> Self {
        Self {
            in_progress,
            complete,
            drop,
            raw: raw.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskLinkMarker {
    Plain,
    Deferred,
    Embedded,
}

impl TaskLinkMarker {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Deferred => "deferred",
            Self::Embedded => "embedded",
        }
    }

    fn ledger_outcome(self) -> TaskLinkOutcome {
        match self {
            Self::Plain => TaskLinkOutcome::InProgress,
            Self::Deferred => TaskLinkOutcome::Deferred,
            Self::Embedded => TaskLinkOutcome::Complete,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskLinkOutcome {
    InProgress,
    Deferred,
    Complete,
    Dropped,
}

impl TaskLinkOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Deferred => "deferred",
            Self::Complete => "complete",
            Self::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskLinkSource {
    Ledger,
    Listed,
    Unlisted,
}

impl TaskLinkSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ledger => "ledger",
            Self::Listed => "listed",
            Self::Unlisted => "unlisted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NumberedTaskLink {
    pub index: u32,
    pub line: usize,
    pub block_link: String,
    pub path_part: String,
    pub block_id: String,
    pub marker: TaskLinkMarker,
    pub outcome: TaskLinkOutcome,
    pub source: TaskLinkSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseSelectionError {
    OutOfRange {
        raw: String,
        bad_numbers: Vec<u32>,
        total: usize,
        running_name: Option<String>,
    },
    ConflictingDuplicate {
        indices: Vec<u32>,
        block_link: String,
    },
}

impl fmt::Display for CloseSelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange {
                raw,
                bad_numbers,
                total,
                running_name,
            } => {
                let owner = match running_name.as_deref() {
                    Some(name) if !name.is_empty() => name.to_string(),
                    _ => "the running Pomodoro".to_string(),
                };
                let names = if bad_numbers.len() == 1 {
                    format!("names task {}", bad_numbers[0])
                } else {
                    format!("names tasks {}", join_numbers(bad_numbers))
                };
                if *total == 0 {
                    write!(
                        f,
                        "`{raw}` {names}, but {owner} has no numbered Task Links; close it with `=x`"
                    )
                } else if *total == 1 {
                    write!(
                        f,
                        "`{raw}` {names}, but {owner} has 1 numbered Task Link (1)"
                    )
                } else {
                    write!(
                        f,
                        "`{raw}` {names}, but {owner} has {total} numbered Task Links (1\u{2013}{total})"
                    )
                }
            }
            Self::ConflictingDuplicate {
                indices,
                block_link,
            } => {
                write!(
                    f,
                    "tasks {} both link `{block_link}` but get different outcomes; give them the same one",
                    join_numbers(indices)
                )
            }
        }
    }
}

impl std::error::Error for CloseSelectionError {}

fn join_numbers(numbers: &[u32]) -> String {
    match numbers {
        [] => String::new(),
        [single] => single.to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let (last, rest) = numbers.split_last().expect("non-empty");
            format!(
                "{} and {last}",
                rest.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn embedded_with_trailing_hash(stripped: &str) -> Option<WikiToken> {
    if !stripped.trim_end_matches([' ', '\t']).ends_with('#') {
        // Fast path; the precise body check below still decides.
    }
    let tokens = wikilink_tokens(stripped);
    if tokens.len() != 1 {
        return None;
    }
    let token = tokens.into_iter().next()?;
    if !token.embedded {
        return None;
    }
    // Body must be exactly `![[T]]#` with no space before the `#`.
    // Reuse the same trimmed-body geometry as `bare_*` plus the
    // `move_only_destination` `#` adjacency rule.
    let indent = leading_spaces_or_tabs_len(stripped);
    let after_indent = stripped.get(indent..)?;
    let marker_len = list_marker_len(after_indent)?;
    let after_marker = after_indent.get(marker_len..)?;
    let ws = leading_spaces_or_tabs_len(after_marker);
    if ws == 0 {
        return None;
    }
    let body_start = indent + marker_len + ws;
    let trimmed = stripped[body_start..].trim_end_matches([' ', '\t']);
    if trimmed.len() < 2 || !trimmed.ends_with('#') {
        return None;
    }
    let directive = body_start + trimmed.len() - 1;
    if token.start != body_start || token.end != directive {
        return None;
    }
    let strike_spans = strikethrough_inner_spans(stripped);
    if range_is_struck(token.start, token.end, &strike_spans) {
        return None;
    }
    Some(token)
}

struct NumberedCandidate {
    block_link: String,
    path_part: String,
    block_id: String,
    marker: TaskLinkMarker,
}

fn classify_numbered_line(stripped: &str) -> Option<NumberedCandidate> {
    let tokens = wikilink_tokens(stripped);
    if tokens.len() != 1 {
        return None;
    }
    if let Some(token) = bare_plain_link(stripped) {
        let strike_spans = strikethrough_inner_spans(stripped);
        if range_is_struck(token.start, token.end, &strike_spans) {
            return None;
        }
        return Some(NumberedCandidate {
            block_link: token.token.clone(),
            path_part: token.path_part.clone(),
            block_id: token.block_id.clone(),
            marker: TaskLinkMarker::Plain,
        });
    }
    if move_only_destination(stripped).is_some() {
        let token = tokens.into_iter().find(|t| !t.embedded)?;
        let strike_spans = strikethrough_inner_spans(stripped);
        if range_is_struck(token.start, token.end, &strike_spans) {
            return None;
        }
        return Some(NumberedCandidate {
            block_link: token.token.clone(),
            path_part: token.path_part.clone(),
            block_id: token.block_id.clone(),
            marker: TaskLinkMarker::Deferred,
        });
    }
    if let Some(token) = bare_embedded_link(stripped) {
        let strike_spans = strikethrough_inner_spans(stripped);
        if range_is_struck(token.start, token.end, &strike_spans) {
            return None;
        }
        return Some(NumberedCandidate {
            block_link: token.token.clone(),
            path_part: token.path_part.clone(),
            block_id: token.block_id.clone(),
            marker: TaskLinkMarker::Embedded,
        });
    }
    if let Some(token) = embedded_with_trailing_hash(stripped) {
        return Some(NumberedCandidate {
            block_link: token.token.clone(),
            path_part: token.path_part.clone(),
            block_id: token.block_id.clone(),
            marker: TaskLinkMarker::Embedded,
        });
    }
    None
}

pub(crate) fn number_task_links(
    contents: &str,
    running: &RunningPomodoro,
) -> Vec<NumberedTaskLink> {
    let spans = line_spans(contents);
    let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
    let entry_index = running.line.saturating_sub(1);
    if line_text.get(entry_index).is_none() {
        return Vec::new();
    }
    let range = sub_bullet_range(&line_text, entry_index);
    let fenced = markdown::fenced_lines(&line_text, 0..line_text.len());
    let mut links = Vec::new();
    for index in range {
        if fenced.contains(&index) {
            continue;
        }
        let line = line_text[index];
        let stripped = strip_pomodoro_markers(line);
        let Some(candidate) = classify_numbered_line(&stripped) else {
            continue;
        };
        let marker = candidate.marker;
        let outcome = marker.ledger_outcome();
        links.push(NumberedTaskLink {
            index: (links.len() + 1) as u32,
            line: index + 1,
            block_link: candidate.block_link,
            path_part: candidate.path_part,
            block_id: candidate.block_id,
            marker,
            outcome,
            source: TaskLinkSource::Ledger,
        });
    }
    links
}

fn outcome_for(
    index: u32,
    marker: TaskLinkMarker,
    selection: &CloseSelection,
) -> (TaskLinkOutcome, TaskLinkSource) {
    if selection.complete.contains(&index) {
        return (TaskLinkOutcome::Complete, TaskLinkSource::Listed);
    }
    if selection.drop.contains(&index) {
        return (TaskLinkOutcome::Dropped, TaskLinkSource::Listed);
    }
    if let Some(in_progress) = selection.in_progress.as_ref() {
        if in_progress.contains(&index) {
            return (TaskLinkOutcome::InProgress, TaskLinkSource::Listed);
        }
        if marker == TaskLinkMarker::Embedded {
            return (TaskLinkOutcome::Complete, TaskLinkSource::Unlisted);
        }
        return (TaskLinkOutcome::Deferred, TaskLinkSource::Unlisted);
    }
    (marker.ledger_outcome(), TaskLinkSource::Ledger)
}

fn line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

pub(crate) fn apply_close_selection(
    contents: &str,
    running: &RunningPomodoro,
    selection: &CloseSelection,
) -> Result<(String, Vec<NumberedTaskLink>), CloseSelectionError> {
    let mut lineup = number_task_links(contents, running);
    let total = lineup.len();
    let mut bad: BTreeSet<u32> = BTreeSet::new();
    if let Some(in_progress) = selection.in_progress.as_ref() {
        for number in in_progress {
            if *number == 0 {
                continue;
            }
            if *number < 1 || (*number as usize) > total {
                bad.insert(*number);
            }
        }
    }
    for number in &selection.complete {
        if *number < 1 || (*number as usize) > total {
            bad.insert(*number);
        }
    }
    for number in &selection.drop {
        if *number < 1 || (*number as usize) > total {
            bad.insert(*number);
        }
    }
    if !bad.is_empty() {
        return Err(CloseSelectionError::OutOfRange {
            raw: selection.raw.clone(),
            bad_numbers: bad.into_iter().collect(),
            total,
            running_name: running.name.clone(),
        });
    }
    for link in lineup.iter_mut() {
        let (outcome, source) = outcome_for(link.index, link.marker, selection);
        link.outcome = outcome;
        link.source = source;
    }
    let mut by_target: BTreeMap<(String, String), Vec<u32>> = BTreeMap::new();
    let mut link_by_index: BTreeMap<u32, &NumberedTaskLink> = BTreeMap::new();
    for link in &lineup {
        by_target
            .entry((link.path_part.clone(), link.block_id.clone()))
            .or_default()
            .push(link.index);
        link_by_index.insert(link.index, link);
    }
    for ((_, _), indices) in &by_target {
        if indices.len() < 2 {
            continue;
        }
        let first_outcome = link_by_index[&indices[0]].outcome;
        if indices
            .iter()
            .any(|index| link_by_index[index].outcome != first_outcome)
        {
            let block_link = link_by_index[&indices[0]].block_link.clone();
            return Err(CloseSelectionError::ConflictingDuplicate {
                indices: indices.clone(),
                block_link,
            });
        }
    }
    let spans = line_spans(contents);
    let mut lines: Vec<String> =
        spans.iter().map(|span| span.text.to_string()).collect();
    for link in &lineup {
        if link.outcome == link.marker.ledger_outcome() {
            continue;
        }
        let zero_based = link.line.saturating_sub(1);
        let Some(original) = lines.get(zero_based).cloned() else {
            continue;
        };
        let indent = leading_spaces_or_tabs_len(&original);
        let Some(after_indent) = original.get(indent..) else {
            continue;
        };
        let Some(marker_len) = list_marker_len(after_indent) else {
            continue;
        };
        let Some(after_marker) = after_indent.get(marker_len..) else {
            continue;
        };
        let ws = leading_spaces_or_tabs_len(after_marker);
        if ws == 0 {
            continue;
        }
        let Some(prefix) = original.get(..indent + marker_len + ws) else {
            continue;
        };
        let body = match link.outcome {
            TaskLinkOutcome::InProgress => link.block_link.clone(),
            TaskLinkOutcome::Deferred => format!("{}#", link.block_link),
            TaskLinkOutcome::Complete => format!("!{}", link.block_link),
            // A dropped link is rewritten with a `~` prefix the ledger
            // planner recognizes and removes: never carried, never
            // started. The marker is transient — the line is skipped from
            // the closed entry, so it never reaches the day note.
            TaskLinkOutcome::Dropped => format!("~{}", link.block_link),
        };
        lines[zero_based] = format!("{prefix}{body}");
    }
    let ending = line_ending(contents);
    let had_final_newline = contents.ends_with('\n');
    let mut rebuilt = lines.join(ending);
    if had_final_newline {
        rebuilt.push_str(ending);
    }
    Ok((rebuilt, lineup))
}

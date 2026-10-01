//! Numbered Task Links and outcome selection for the pure close planner.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use super::super::{
    capture::{
        leading_spaces_or_tabs_len, line_spans, list_marker_len,
        nearest_shallower_list_item_parent,
    },
    capture_language::CloseLogEntry,
    markdown,
};
use super::ledger::{sub_bullet_range, RunningPomodoro};
use super::links::{
    bare_embedded_link, bare_plain_link, move_only_destination,
    range_is_struck, strikethrough_inner_spans, strip_pomodoro_markers,
    wikilink_tokens, WikiToken,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CloseSelection {
    pub in_progress: Option<BTreeSet<u32>>,
    pub park: BTreeSet<u32>,
    pub complete: BTreeSet<u32>,
    pub drop: BTreeSet<u32>,
    pub log: Vec<CloseLogEntry>,
    pub raw: String,
}

impl CloseSelection {
    pub(crate) fn new(
        in_progress: Option<BTreeSet<u32>>,
        park: BTreeSet<u32>,
        complete: BTreeSet<u32>,
        drop: BTreeSet<u32>,
        raw: impl Into<String>,
    ) -> Self {
        Self {
            in_progress,
            park,
            complete,
            drop,
            log: Vec::new(),
            raw: raw.into(),
        }
    }

    /// Selection mode is active when `<N>` was typed (including `=x0`) or a
    /// nonempty `*<P>` group is present. Star-only closes defer unlisted
    /// plain links exactly like ordinary closes.
    pub(crate) fn has_work_selection(&self) -> bool {
        self.in_progress.is_some() || !self.park.is_empty()
    }

    pub(crate) fn with_log(mut self, log: Vec<CloseLogEntry>) -> Self {
        self.log = log;
        self
    }
}

/// The rewritten day contents after applying a close selection, the
/// renumbered Task Link lineup, and the 1-based line numbers of the
/// inserted typed Work Log sub-bullets (in typed order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedCloseSelection {
    pub contents: String,
    pub lineup: Vec<NumberedTaskLink>,
    pub inserted_lines: Vec<usize>,
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
    Parked,
    Deferred,
    Complete,
    Dropped,
}

impl TaskLinkOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Parked => "parked",
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
    LogOutOfRange {
        index: u32,
        total: usize,
        running_name: Option<String>,
    },
    LogDeferred {
        index: u32,
        block_link: String,
    },
    LogDropped {
        index: u32,
        block_link: String,
    },
    LogNested {
        index: u32,
        block_link: String,
        running_name: Option<String>,
    },
    LogLineupChanged,
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
            Self::LogOutOfRange {
                index,
                total,
                running_name,
            } => {
                let owner = owner_name(running_name);
                let range = range_words(*total);
                write!(
                    f,
                    "`- {index}` logs to task {index}, but {owner} has {range}"
                )
            }
            Self::LogDeferred { index, block_link } => {
                write!(
                    f,
                    "task {index} `{block_link}` is deferred, so it can't take a Work Log entry; list it in `<N>` or `!<M>` to log to it"
                )
            }
            Self::LogDropped { index, block_link } => {
                write!(
                    f,
                    "task {index} `{block_link}` is dropped, so it can't take a Work Log entry"
                )
            }
            Self::LogNested {
                index,
                block_link,
                running_name,
            } => {
                let owner = owner_name(running_name);
                write!(
                    f,
                    "task {index} `{block_link}` is nested under another bullet, so the close can't write its Work Log; move it to the top level of {owner}"
                )
            }
            Self::LogLineupChanged => {
                write!(f, "Work Log entry changed the Task Link lineup")
            }
        }
    }
}

fn owner_name(running_name: &Option<String>) -> String {
    match running_name.as_deref() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "the running Pomodoro".to_string(),
    }
}

fn range_words(total: usize) -> String {
    if total == 0 {
        "no numbered Task Links".to_string()
    } else if total == 1 {
        "1 numbered Task Link (1)".to_string()
    } else {
        format!("{total} numbered Task Links (1\u{2013}{total})")
    }
}

impl std::error::Error for CloseSelectionError {}

pub(crate) fn join_numbers(numbers: &[u32]) -> String {
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
    if selection.park.contains(&index) {
        return (TaskLinkOutcome::Parked, TaskLinkSource::Listed);
    }
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
    if !selection.park.is_empty() {
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
) -> Result<AppliedCloseSelection, CloseSelectionError> {
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
    for number in &selection.park {
        if *number < 1 || (*number as usize) > total {
            bad.insert(*number);
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
            TaskLinkOutcome::InProgress | TaskLinkOutcome::Parked => {
                link.block_link.clone()
            }
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
    if selection.log.is_empty() {
        let mut rebuilt = lines.join(ending);
        if had_final_newline {
            rebuilt.push_str(ending);
        }
        return Ok(AppliedCloseSelection {
            contents: rebuilt,
            lineup,
            inserted_lines: Vec::new(),
        });
    }
    validate_close_log_entries(contents, running, &lineup, selection)?;
    let mut tags: Vec<LineTag> =
        (0..lines.len()).map(LineTag::Original).collect();
    for (ordinal, entry) in selection.log.iter().enumerate() {
        insert_close_log_entry(
            &mut lines, &mut tags, running, &lineup, entry, ordinal,
        );
    }
    let mut rebuilt = lines.join(ending);
    if had_final_newline {
        rebuilt.push_str(ending);
    }
    reline_close_log_lineup(&rebuilt, running, &mut lineup, &tags)?;
    let mut inserted_lines = vec![0usize; selection.log.len()];
    for (position, tag) in tags.iter().enumerate() {
        if let LineTag::Inserted(ordinal) = tag {
            inserted_lines[*ordinal] = position + 1;
        }
    }
    Ok(AppliedCloseSelection {
        contents: rebuilt,
        lineup,
        inserted_lines,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineTag {
    Original(usize),
    Inserted(usize),
    /// A detail line under the entry inserted for `ordinal`, so
    /// `inserted_lines[ordinal]` stays the entry line.
    InsertedDetail(usize),
}

fn validate_close_log_entries(
    contents: &str,
    running: &RunningPomodoro,
    lineup: &[NumberedTaskLink],
    selection: &CloseSelection,
) -> Result<(), CloseSelectionError> {
    let spans = line_spans(contents);
    let entry_index = running.line.saturating_sub(1);
    let by_index: BTreeMap<u32, &NumberedTaskLink> =
        lineup.iter().map(|link| (link.index, link)).collect();
    for entry in &selection.log {
        let Some(link) = by_index.get(&entry.index).copied() else {
            return Err(CloseSelectionError::LogOutOfRange {
                index: entry.index,
                total: lineup.len(),
                running_name: running.name.clone(),
            });
        };
        match link.outcome {
            TaskLinkOutcome::InProgress
            | TaskLinkOutcome::Parked
            | TaskLinkOutcome::Complete => {}
            TaskLinkOutcome::Deferred => {
                return Err(CloseSelectionError::LogDeferred {
                    index: entry.index,
                    block_link: link.block_link.clone(),
                });
            }
            TaskLinkOutcome::Dropped => {
                return Err(CloseSelectionError::LogDropped {
                    index: entry.index,
                    block_link: link.block_link.clone(),
                });
            }
        }
        let link_zero = link.line.saturating_sub(1);
        if link_zero >= spans.len()
            || nearest_shallower_list_item_parent(&spans, link_zero)
                != Some(entry_index)
        {
            return Err(CloseSelectionError::LogNested {
                index: entry.index,
                block_link: link.block_link.clone(),
                running_name: running.name.clone(),
            });
        }
    }
    Ok(())
}

fn insert_close_log_entry(
    lines: &mut Vec<String>,
    tags: &mut Vec<LineTag>,
    running: &RunningPomodoro,
    lineup: &[NumberedTaskLink],
    entry: &CloseLogEntry,
    ordinal: usize,
) {
    let original_zero = lineup
        .iter()
        .find(|link| link.index == entry.index)
        .map(|link| link.line.saturating_sub(1));
    let Some(original_zero) = original_zero else {
        return;
    };
    let Some(target) = tags
        .iter()
        .position(|tag| *tag == LineTag::Original(original_zero))
    else {
        return;
    };
    let texts: Vec<&str> = lines.iter().map(String::as_str).collect();
    let running_original = running.line.saturating_sub(1);
    let entry_current = tags
        .iter()
        .position(|tag| *tag == LineTag::Original(running_original))
        .unwrap_or(target);
    let range = sub_bullet_range(&texts, entry_current);
    let mut end = child_block_end(&texts, target);
    end = end.min(range.end.saturating_sub(1).max(target));
    let indent = close_log_child_indent(&texts, target, end);
    lines.insert(end + 1, format!("{indent}- {}", entry.text));
    tags.insert(end + 1, LineTag::Inserted(ordinal));
    if !entry.details.is_empty() {
        // Step one level deeper exactly as the entry did: the entry
        // indent with the link indent stripped as a prefix is the unit.
        let link_indent = lines
            .get(target)
            .map(|line| line[..leading_spaces_or_tabs_len(line)].to_string())
            .unwrap_or_default();
        let unit = indent
            .strip_prefix(link_indent.as_str())
            .filter(|rest| !rest.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| child_indent_unit(&indent));
        let detail_indent = format!("{indent}{unit}");
        for (offset, detail) in entry.details.iter().enumerate() {
            lines
                .insert(end + 2 + offset, format!("{detail_indent}- {detail}"));
            tags.insert(end + 2 + offset, LineTag::InsertedDetail(ordinal));
        }
    }
}

fn child_block_end(lines: &[&str], parent: usize) -> usize {
    let Some(parent_line) = lines.get(parent) else {
        return parent;
    };
    let parent_indent = leading_spaces_or_tabs_len(parent_line);
    let mut end = parent;
    for (index, line) in lines.iter().enumerate().skip(parent + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if leading_spaces_or_tabs_len(line) > parent_indent {
            end = index;
            continue;
        }
        break;
    }
    end
}

fn close_log_child_indent(lines: &[&str], target: usize, end: usize) -> String {
    let probe = lines.join("\n");
    let spans = line_spans(&probe);
    let first_child = (target + 1..=end.min(spans.len().saturating_sub(1)))
        .find(|&index| {
            !spans[index].text.trim().is_empty()
                && list_marker_len(
                    &spans[index].text
                        [leading_spaces_or_tabs_len(spans[index].text)..],
                )
                .is_some()
                && nearest_shallower_list_item_parent(&spans, index)
                    == Some(target)
        });
    if let Some(child) = first_child
        && let Some(line) = lines.get(child)
    {
        return line[..leading_spaces_or_tabs_len(line)].to_string();
    }
    let link_indent = lines
        .get(target)
        .map(|line| line[..leading_spaces_or_tabs_len(line)].to_string())
        .unwrap_or_default();
    format!("{link_indent}{}", child_indent_unit(&link_indent))
}

fn child_indent_unit(parent_indent: &str) -> String {
    if !parent_indent.is_empty() && !parent_indent.contains('\t') {
        parent_indent.to_string()
    } else {
        "\t".to_string()
    }
}

fn marker_for_outcome(outcome: TaskLinkOutcome) -> TaskLinkMarker {
    match outcome {
        TaskLinkOutcome::InProgress | TaskLinkOutcome::Parked => {
            TaskLinkMarker::Plain
        }
        TaskLinkOutcome::Deferred => TaskLinkMarker::Deferred,
        TaskLinkOutcome::Complete | TaskLinkOutcome::Dropped => {
            TaskLinkMarker::Embedded
        }
    }
}

fn reline_close_log_lineup(
    rebuilt: &str,
    running: &RunningPomodoro,
    lineup: &mut [NumberedTaskLink],
    tags: &[LineTag],
) -> Result<(), CloseSelectionError> {
    let relined = number_task_links(rebuilt, running);
    let mut kept = lineup
        .iter_mut()
        .filter(|link| link.outcome != TaskLinkOutcome::Dropped)
        .collect::<Vec<_>>();
    if relined.len() != kept.len() {
        return Err(CloseSelectionError::LogLineupChanged);
    }
    for (fresh, link) in relined.into_iter().zip(kept.iter_mut()) {
        if fresh.block_link != link.block_link
            || fresh.path_part != link.path_part
            || fresh.block_id != link.block_id
            || fresh.marker != marker_for_outcome(link.outcome)
        {
            return Err(CloseSelectionError::LogLineupChanged);
        }
        link.line = fresh.line;
    }
    for link in lineup
        .iter_mut()
        .filter(|link| link.outcome == TaskLinkOutcome::Dropped)
    {
        let original_zero = link.line.saturating_sub(1);
        if let Some(position) = tags
            .iter()
            .position(|tag| *tag == LineTag::Original(original_zero))
        {
            link.line = position + 1;
        }
    }
    Ok(())
}

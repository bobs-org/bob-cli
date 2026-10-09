//! Note-free `=x0` reset planning: return the current Pomodoro to the
//! front of the future queue instead of closing it.
//!
//! Reset applies only to the explicit empty in-progress selection (`=x0`,
//! including `=X0` and surrounding whitespace) with no park/complete/drop
//! group or wildcard and no typed Work Log input. Any stand-alone session
//! note forces the existing close path unchanged.

use std::collections::BTreeSet;

use super::super::{
    capture::{
        leading_spaces_or_tabs_len, line_spans, list_item_body,
        list_marker_len, nearest_shallower_list_item_parent,
    },
    capture_language::PomodoroCloseSpec,
    capture_pomodoros, markdown, pomodoro,
};
use super::ledger::RunningPomodoro;
use super::links::{
    bare_embedded_link, bare_plain_link, move_only_destination,
    strip_pomodoro_markers, wikilink_tokens,
};
use super::selection::CloseSelection;

/// Shared reset eligibility from the parsed selection. Explicit empty
/// in-progress list only; no park/complete/drop group or wildcard and no
/// typed Work Log input. Valid modifier variants (`=x0*2`, `=x0!2`,
/// `=x0~2`, `=x0*`, wildcards) and ordinary `=x` stay on the close path.
pub(crate) fn is_reset_selection(selection: &CloseSelection) -> bool {
    matches!(selection.in_progress.as_ref(), Some(numbers) if numbers.is_empty())
        && selection.park.is_empty()
        && !selection.park_all
        && selection.complete.is_empty()
        && !selection.complete_all
        && selection.drop.is_empty()
        && selection.log.is_empty()
}

/// Same eligibility from the capture spec, so whole/link/new-task callers
/// cannot diverge from the shared planner.
pub(crate) fn is_reset_spec(spec: &PomodoroCloseSpec) -> bool {
    matches!(&spec.in_progress, Some(numbers) if numbers.is_empty())
        && spec.park.is_empty()
        && !spec.park_all
        && spec.complete.is_empty()
        && !spec.complete_all
        && spec.drop.is_empty()
        && spec.log.is_empty()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResetPlan {
    pub contents: String,
    pub pomodoro_name: Option<String>,
    pub previous_pomodoro_line: usize,
    pub pomodoro_line: usize,
    pub previous_entry_line: String,
    pub entry_line: String,
    pub previous_time_range: String,
    pub moved: bool,
}

/// Decide reset versus close for already-validated input. Returns `Some`
/// when the selection is reset-eligible and the staged session block has
/// no stand-alone note. Returns `None` for every close case, including
/// explicit-modifier selections and note-bearing sessions.
pub(crate) fn reset_if_eligible(
    contents: &str,
    running: &RunningPomodoro,
    selection: Option<&CloseSelection>,
) -> Option<ResetPlan> {
    let selection = selection?;
    if !is_reset_selection(selection) {
        return None;
    }
    if has_standalone_note(contents, running.line) {
        return None;
    }
    plan_reset(contents, running).ok()
}

/// True when the running session block owns at least one stand-alone note
/// about the session. Dedicated task-link roots (plain, alias, deferred
/// `#`, embed, struck wrappers, Pomodoro markers) and their entire
/// subtrees are task-attached details, never notes. Everything else
/// meaningful in the block counts.
pub(crate) fn has_standalone_note(contents: &str, running_line: usize) -> bool {
    let spans = line_spans(contents);
    let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
    let entry_index = running_line.saturating_sub(1);
    if line_text.get(entry_index).is_none() {
        return false;
    }
    let section_end = pomodoro::pomodoros_section_range(&line_text)
        .map(|section| section.end)
        .unwrap_or(line_text.len());
    let block = capture_pomodoros::pomodoro_block_range(
        &line_text,
        entry_index,
        section_end,
    );
    if block.len() <= 1 {
        return false;
    }
    let fenced = markdown::fenced_lines(&line_text, 0..line_text.len());
    // Direct-child roots of the headline: index -> dedicated?
    let mut dedicated_root: std::collections::BTreeMap<usize, bool> =
        std::collections::BTreeMap::new();
    for index in block.clone().skip(1) {
        if spans.get(index).is_none() {
            continue;
        }
        let parent = nearest_shallower_list_item_parent(&spans, index);
        if parent == Some(entry_index) {
            let line = line_text.get(index).copied().unwrap_or("");
            // Only list items can be roots; continuation text directly
            // under the headline counts when meaningful.
            if list_item_body(line).is_some() {
                dedicated_root.insert(index, is_dedicated_root(line));
            } else if !line.trim().is_empty() && !fenced.contains(&index) {
                return true;
            } else if !line.trim().is_empty()
                && fenced.contains(&index)
                && is_meaningful_fenced_line(line)
            {
                // Fenced prose directly under the headline with no owning
                // bullet is still an authored session note.
                return true;
            }
        }
    }
    // Any non-dedicated root with a nonempty body is a note. An empty
    // root counts only through meaningful descendants. Dedicated roots
    // and their subtrees never count.
    for index in block.clone().skip(1) {
        let line = line_text.get(index).copied().unwrap_or("");
        if line.trim().is_empty() {
            continue;
        }
        let is_fenced = fenced.contains(&index);
        let parent = nearest_shallower_list_item_parent(&spans, index);
        // Owning direct-child root, if any.
        let root = owning_root(&spans, entry_index, index);
        if let Some(root_index) = root {
            let dedicated =
                dedicated_root.get(&root_index).copied().unwrap_or(false);
            if dedicated {
                continue;
            }
            // Non-dedicated subtree: the root itself already decides,
            // but descendants of an empty root also count.
            if root_index == index {
                if is_fenced {
                    if is_meaningful_fenced_line(line) {
                        // A fence marker alone is syntax; content lines
                        // count. A direct-child fence marker with no
                        // content is not a note by itself.
                        continue;
                    }
                    // Non-marker fenced line directly as root content.
                    return true;
                }
                if is_empty_bullet(line) {
                    continue;
                }
                return true;
            }
            // Descendant of a non-dedicated root.
            if is_fenced && !is_meaningful_fenced_line(line) {
                continue;
            }
            if is_empty_bullet(line) {
                continue;
            }
            return true;
        }
        // No owning root: continuation/fenced text directly under the
        // headline outside any dedicated subtree.
        if is_fenced {
            if is_meaningful_fenced_line(line) {
                return true;
            }
            continue;
        }
        // Non-list continuation text.
        if list_item_body(line).is_none() {
            return true;
        }
        // List item whose parent chain skipped the headline (unusual
        // layout): meaningful content outside a dedicated subtree.
        let _ = parent;
        if is_empty_bullet(line) {
            continue;
        }
        return true;
    }
    false
}

fn owning_root(
    spans: &[super::super::capture::LineSpan<'_>],
    entry_index: usize,
    mut index: usize,
) -> Option<usize> {
    loop {
        let parent = nearest_shallower_list_item_parent(spans, index)?;
        if parent == entry_index {
            return Some(index);
        }
        index = parent;
    }
}

fn is_empty_bullet(line: &str) -> bool {
    match list_item_body(line) {
        Some(body) => body.trim().is_empty(),
        None => {
            // A list marker with no body (`-` alone) is also a stub.
            let indent = leading_spaces_or_tabs_len(line);
            let after = line.get(indent..).unwrap_or("");
            match list_marker_len(after) {
                Some(marker_len) => after[marker_len..].trim().is_empty(),
                None => false,
            }
        }
    }
}

fn is_meaningful_fenced_line(line: &str) -> bool {
    if markdown::fence_marker(line).is_some() {
        return false;
    }
    !line.trim().is_empty()
}

/// A direct-child bullet whose body is exclusively one dedicated task
/// block link. Covers plain `[[note#^id]]`, its alias form, deferred
/// `#`, embed `![[...]]` (including trailing `#`), strikethrough
/// wrappers, and Pomodoro markers. Syntactic identity is sufficient.
fn is_dedicated_root(line: &str) -> bool {
    let stripped = strip_pomodoro_markers(line);
    if bare_plain_link(&stripped).is_some() {
        return true;
    }
    if bare_embedded_link(&stripped).is_some() {
        return true;
    }
    if move_only_destination(&stripped).is_some() {
        return true;
    }
    if is_embedded_trailing_hash(&stripped) {
        return true;
    }
    // Strikethrough wrappers: `~~[[note#^id]]~~` and friends.
    let unstruck = stripped.replace("~~", "");
    if unstruck != stripped {
        if bare_plain_link(&unstruck).is_some() {
            return true;
        }
        if bare_embedded_link(&unstruck).is_some() {
            return true;
        }
        if move_only_destination(&unstruck).is_some() {
            return true;
        }
        if is_embedded_trailing_hash(&unstruck) {
            return true;
        }
    }
    // A body with exactly one block-link token and nothing else besides
    // struck/marker syntax is still dedicated even when the `bare_*`
    // geometry misses an alias edge: fall back to token exclusivity.
    is_single_block_link_body(&stripped)
        || (unstruck != stripped && is_single_block_link_body(&unstruck))
}

fn is_embedded_trailing_hash(line: &str) -> bool {
    let indent = leading_spaces_or_tabs_len(line);
    let after_indent = line.get(indent..).unwrap_or("");
    let Some(marker_len) = list_marker_len(after_indent) else {
        return false;
    };
    let after_marker = after_indent.get(marker_len..).unwrap_or("");
    let ws = leading_spaces_or_tabs_len(after_marker);
    if ws == 0 {
        return false;
    }
    let body_start = indent + marker_len + ws;
    let trimmed = line[body_start..].trim_end_matches([' ', '\t']);
    if trimmed.len() < 2 || !trimmed.ends_with('#') {
        return false;
    }
    let without_hash = trimmed[..trimmed.len() - 1].trim_end();
    let tokens = wikilink_tokens(line);
    if tokens.len() != 1 {
        return false;
    }
    let token = &tokens[0];
    token.embedded && line[token.start..token.end].to_string() == without_hash
}

fn is_single_block_link_body(line: &str) -> bool {
    let indent = leading_spaces_or_tabs_len(line);
    let after_indent = line.get(indent..).unwrap_or("");
    let Some(marker_len) = list_marker_len(after_indent) else {
        return false;
    };
    let after_marker = after_indent.get(marker_len..).unwrap_or("");
    let ws = leading_spaces_or_tabs_len(after_marker);
    if ws == 0 {
        return false;
    }
    let body_start = indent + marker_len + ws;
    let body = line[body_start..].trim();
    if body.is_empty() {
        return false;
    }
    // Strip one trailing defer marker.
    let body = body.strip_suffix('#').unwrap_or(body).trim();
    let tokens = wikilink_tokens(line);
    if tokens.len() != 1 {
        return false;
    }
    let token = &tokens[0];
    // Token must cover the whole body modulo alias/anchor syntax: the
    // body starts with `[[` (or `![[`) and ends with `]]`.
    let token_text = line[token.start..token.end].trim();
    if token_text != body {
        return false;
    }
    !token.path_part.is_empty() && !token.block_id.is_empty()
}

/// Clear the current headline's parenthesized timing payload to `()` and
/// move the whole block before the earliest other future placeholder, so
/// the reset session becomes first future. Preserves every child byte,
/// interior blanks, indentation, line endings, and final-newline behavior.
pub(crate) fn plan_reset(
    contents: &str,
    running: &RunningPomodoro,
) -> Result<ResetPlan, String> {
    let spans = line_spans(contents);
    let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
    let entry_index = running.line.saturating_sub(1);
    let headline = line_text
        .get(entry_index)
        .ok_or_else(|| "running Pomodoro is out of range".to_string())?
        .to_string();
    let previous_entry_line = headline.clone();
    let cleared = clear_timing_payload(&headline).ok_or_else(|| {
        "running Pomodoro timing could not be safely identified".to_string()
    })?;
    let ending = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let had_final_newline = contents.ends_with('\n');
    let mut lines: Vec<String> =
        line_text.iter().map(|line| (*line).to_string()).collect();
    lines[entry_index] = cleared.clone();
    let section_end = pomodoro::pomodoros_section_range(&line_text)
        .map(|section| section.end)
        .unwrap_or(line_text.len());
    // Post-clear scan to find the earliest other future placeholder.
    let mut rejoined = lines.join(ending);
    if had_final_newline {
        rejoined.push_str(ending);
    }
    let scan = capture_pomodoros::scan(&rejoined);
    let futures: Vec<usize> = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.placeholder
                && entry.time_range.is_none()
        })
        .map(|entry| entry.line)
        .collect();
    let reset_line = entry_index + 1;
    let earliest_other = futures
        .iter()
        .copied()
        .filter(|line| *line != reset_line)
        .min();
    let (final_lines, pomodoro_line, moved) = match earliest_other {
        Some(target) if target < reset_line => {
            // Reset block already precedes everything else? No: target
            // is earlier, so move reset before it. Removal is after
            // target, so the target index is stable.
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            let block = capture_pomodoros::pomodoro_block_range(
                &refs,
                entry_index,
                section_end,
            );
            let block_lines = lines[block.clone()].to_vec();
            let mut without = lines.clone();
            without.drain(block);
            let target_index = target.saturating_sub(1);
            let insert_at = target_index.min(without.len());
            for (offset, line) in block_lines.into_iter().enumerate() {
                without.insert(insert_at + offset, line);
            }
            let new_line = insert_at + 1;
            (without, new_line, true)
        }
        Some(target) if target > reset_line => {
            // Reset is already first future; later placeholders stay.
            let _ = target;
            (lines, reset_line, false)
        }
        _ => (lines, reset_line, false),
    };
    let mut rebuilt = final_lines.join(ending);
    if had_final_newline {
        rebuilt.push_str(ending);
    }
    // If the file had no final newline, do not add one.
    Ok(ResetPlan {
        contents: rebuilt,
        pomodoro_name: running.name.clone(),
        previous_pomodoro_line: running.line,
        pomodoro_line,
        previous_entry_line,
        entry_line: cleared,
        previous_time_range: running.time_range.clone(),
        moved,
    })
}

/// Replace the identified leading timing span with `()`. Uses the parsed
/// adjustment range first (canonical and legacy range spellings with any
/// `[t:: ...]` or range-local metadata), then the ledger's parenthetical
/// time-range discovery. Never matches parentheses in the name.
fn clear_timing_payload(headline: &str) -> Option<String> {
    if let Some(range) = super::super::capture::parse_adjustment_range(headline)
    {
        let mut updated = String::with_capacity(headline.len());
        updated.push_str(&headline[..range.start_ch]);
        updated.push_str("()");
        updated.push_str(&headline[range.end_ch..]);
        return Some(updated);
    }
    if let Some((raw_range, _, _)) = pomodoro::task_time_range(headline) {
        return Some(headline.replacen(raw_range, "()", 1));
    }
    None
}

#[allow(dead_code)]
pub(crate) fn fenced_set(contents: &str) -> BTreeSet<usize> {
    let spans = line_spans(contents);
    let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
    markdown::fenced_lines(&line_text, 0..line_text.len())
}

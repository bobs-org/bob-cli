//! Pure note-local task settlement shared by reconcile and note writers.
//!
//! Settlement applies the local future-schedule Blocked rule, then the
//! status-group and badge transform for grouping-eligible notes. It does not
//! inspect the vault, resolve dependencies, read the clock, or write files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use similar::{Algorithm, DiffOp};

use super::{
    freshness::placement::prune_unblocked,
    pomodoro as native_pomodoro,
    task_status_groups::{
        self, GroupedSection, GroupingWarning, TaskClassification,
        TransformOutput,
    },
    task_status_hooks::{self, grouping_eligible_relative_note},
};

/// Result of the pure note-local settle pass. Line-map keys and values are
/// zero-based physical line indices; removed input lines have no entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettleOutput {
    pub(crate) contents: String,
    pub(crate) changed: bool,
    pub(crate) line_map: BTreeMap<usize, usize>,
    pub(crate) grouped_sections: Vec<GroupedSection>,
    pub(crate) warnings: Vec<GroupingWarning>,
}

/// Configuration shared by the writers settling several notes in one write.
/// Construct once per command so the Tasks settings, daily path, and date are
/// stable across every staged note.
#[derive(Debug, Clone)]
pub(crate) struct SettleContext {
    today: NaiveDate,
    classification: TaskClassification,
    daily_path: PathBuf,
}

impl SettleContext {
    pub(crate) fn for_vault(vault: &Path) -> Self {
        let daily_path = native_pomodoro::day_file_for(vault);
        let daily_relative = daily_path
            .strip_prefix(vault)
            .unwrap_or(&daily_path)
            .to_path_buf();
        let today = task_status_hooks::daily_anchor_date(
            &daily_path,
            crate::native::env::current_datetime().date(),
        );
        let settings = task_status_hooks::read_tasks_settings(vault);
        Self {
            today,
            classification: task_status_hooks::task_group_classification(
                &settings,
            ),
            daily_path: daily_relative,
        }
    }

    /// Settle a staged note by its vault-relative path.
    pub(crate) fn settle_written_note(
        &self,
        relative_path: &Path,
        contents: &str,
    ) -> SettleOutput {
        settle_note(
            relative_path,
            contents,
            self.today,
            &self.classification,
            &self.daily_path,
        )
    }

    /// Settle every staged note in a writer's postimage map before staging.
    /// Keys are vault-relative paths. Settled notes pass through unchanged,
    /// so composing this into a writer is idempotent.
    pub(crate) fn settle_postimages(
        &self,
        postimages: &mut BTreeMap<PathBuf, String>,
    ) {
        for (relative, contents) in postimages.iter_mut() {
            let settled = self.settle_written_note(relative, contents);
            if settled.changed {
                *contents = settled.contents;
            }
        }
    }
}

/// Settle every staged note in a writer's postimage map before staging.
/// Builds one [`SettleContext`] per call so the Tasks settings, daily path,
/// and date stay stable across every staged note in the write.
pub(crate) fn settle_plan_postimages(
    vault: &Path,
    postimages: &mut BTreeMap<PathBuf, String>,
) {
    SettleContext::for_vault(vault).settle_postimages(postimages);
}

/// Settle note-local state without reading the filesystem or clock.
/// `relative_path` and `daily_path` are vault-relative paths.
pub(crate) fn settle_note(
    relative_path: &Path,
    contents: &str,
    today: NaiveDate,
    classification: &TaskClassification,
    daily_path: &Path,
) -> SettleOutput {
    let scheduled = settle_local_schedules(contents, today, classification);
    let transformed = if grouping_eligible_relative_note(
        relative_path,
        &scheduled,
        daily_path,
    ) {
        task_status_groups::transform(&scheduled, classification)
    } else {
        TransformOutput {
            contents: scheduled.clone(),
            changed: false,
            grouped_sections: Vec::new(),
            warnings: Vec::new(),
        }
    };
    let line_map =
        map_surviving_lines(contents, &transformed.contents, classification);
    SettleOutput {
        changed: transformed.contents != contents,
        contents: transformed.contents,
        line_map,
        grouped_sections: transformed.grouped_sections,
        warnings: transformed.warnings,
    }
}

/// Apply only the local future-schedule part of reconcile. This intentionally
/// does not stamp `[unblocked::]`; it only mirrors the `MarkBlocked` status
/// swap and stale `[unblocked::]` prune on rows with a future local schedule.
fn settle_local_schedules(
    contents: &str,
    today: NaiveDate,
    classification: &TaskClassification,
) -> String {
    let lines = task_status_hooks::logical_lines(contents);
    let mut segments = contents
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() || segments.is_empty() || lines.len() != segments.len()
    {
        return contents.to_string();
    }

    let mut changed = false;
    for (line_index, line) in lines.iter().enumerate() {
        let leading = line
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let Some(marker_end) =
            task_status_hooks::after_list_marker(line, leading)
        else {
            continue;
        };
        let mut checkbox_start = marker_end;
        while line
            .as_bytes()
            .get(checkbox_start)
            .is_some_and(u8::is_ascii_whitespace)
        {
            checkbox_start += 1;
        }
        if line.as_bytes().get(checkbox_start) != Some(&b'[') {
            continue;
        }
        let Some(close_offset) = line[checkbox_start + 1..].find(']') else {
            continue;
        };
        let close = checkbox_start + 1 + close_offset;
        let mut symbols = line[checkbox_start + 1..close].chars();
        let Some(status) = symbols.next() else {
            continue;
        };
        if symbols.next().is_some() || !classification.is_open_status(status) {
            continue;
        }
        let after_checkbox = &line[close + 1..];
        if !after_checkbox.is_empty()
            && !after_checkbox.starts_with(char::is_whitespace)
        {
            continue;
        }
        let body = after_checkbox.trim_start();
        if !classification.matches_task_filter(body) {
            continue;
        }
        let block_id = task_status_hooks::trailing_block_id(body);
        let metadata =
            task_status_hooks::task_metadata(body, block_id.as_deref());
        if !metadata
            .scheduled
            .is_some_and(|scheduled| scheduled > today)
        {
            continue;
        }

        let mut rewritten = (*line).to_string();
        if status != '?' {
            let local_status_offset = checkbox_start + 1;
            let status_end = local_status_offset + status.len_utf8();
            rewritten.replace_range(local_status_offset..status_end, "?");
        }
        if let Some(pruned) = prune_unblocked(&rewritten, today) {
            rewritten = pruned;
        }
        if rewritten != *line {
            let segment = &mut segments[line_index];
            let ending = if segment.ends_with("\r\n") {
                "\r\n"
            } else if segment.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            *segment = format!("{rewritten}{ending}");
            changed = true;
        }
    }

    if changed {
        segments.concat()
    } else {
        contents.to_string()
    }
}

/// Map surviving lines through scheduling and regrouping. Myers matches the
/// stable runs; exact-text fallback accounts for moved subtrees, and a stable
/// task-row key follows rows whose status or stale `[unblocked::]` field was
/// rewritten.
fn map_surviving_lines(
    before: &str,
    after: &str,
    classification: &TaskClassification,
) -> BTreeMap<usize, usize> {
    let before_lines = task_status_hooks::logical_lines(before);
    let after_lines = task_status_hooks::logical_lines(after);
    let mut mapped = BTreeMap::new();
    let mut used = BTreeSet::new();
    for operation in similar::capture_diff_slices(
        Algorithm::Myers,
        &before_lines,
        &after_lines,
    ) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = operation
        {
            for offset in 0..len {
                mapped.insert(old_index + offset, new_index + offset);
                used.insert(new_index + offset);
            }
        }
    }

    for (old_index, line) in before_lines.iter().enumerate() {
        if mapped.contains_key(&old_index) {
            continue;
        }
        let exact = after_lines
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !used.contains(index) && candidate == &line
            })
            .min_by_key(|(index, _)| index.abs_diff(old_index))
            .map(|(index, _)| index);
        let task_key = task_line_key(line, classification);
        let rewritten_task = task_key.as_ref().and_then(|key| {
            after_lines
                .iter()
                .enumerate()
                .filter(|(index, candidate)| {
                    !used.contains(index)
                        && task_line_key(candidate, classification).as_ref()
                            == Some(key)
                })
                .min_by_key(|(index, _)| index.abs_diff(old_index))
                .map(|(index, _)| index)
        });
        if let Some(new_index) = exact.or(rewritten_task) {
            mapped.insert(old_index, new_index);
            used.insert(new_index);
        }
    }
    mapped
}

/// Stable identity for a task row across status changes and stale
/// `[unblocked::]` pruning. The checkbox status and the prunable field are
/// omitted; the list marker and remaining task tokens still distinguish rows.
fn task_line_key(
    line: &str,
    classification: &TaskClassification,
) -> Option<String> {
    let leading = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let marker_end = task_status_hooks::after_list_marker(line, leading)?;
    let mut checkbox_start = marker_end;
    while line
        .as_bytes()
        .get(checkbox_start)
        .is_some_and(u8::is_ascii_whitespace)
    {
        checkbox_start += 1;
    }
    if line.as_bytes().get(checkbox_start) != Some(&b'[') {
        return None;
    }
    let close = checkbox_start + 1 + line[checkbox_start + 1..].find(']')?;
    let mut status = line[checkbox_start + 1..close].chars();
    status.next()?;
    if status.next().is_some() {
        return None;
    }
    let body = line[close + 1..].trim_start();
    if !classification.matches_task_filter(body) {
        return None;
    }
    let mut skipping_unblocked = false;
    let stable_tokens = body
        .split_whitespace()
        .filter(|token| {
            if skipping_unblocked {
                skipping_unblocked = !token.contains(']');
                return false;
            }
            if token.starts_with("[unblocked::") {
                skipping_unblocked = !token.contains(']');
                return false;
            }
            true
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!("{}[?] {stable_tokens}", &line[..checkbox_start]))
}

#[cfg(test)]
pub(crate) mod tests;

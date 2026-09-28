//! Byte-for-byte parity with the Obsidian `Ctrl+Shift+P` picker's
//! `🗓️ **SCHEDULE LOG**` marker and roll-reason text. Mirrors the constants
//! near `plugins/bob-navigation-hotkeys/main.js:250-274` and the plugin's
//! schedule-log/priority-roll formatters; `scripts/test-navigation-hotkeys.cjs`
//! keeps the picker's own fixture of the exact bytes reproduced here.

//! Randomize insertions and reasons live here too; see [`randomize_reason`]
//! and [`plan_entry_insertion`].

use serde::Serialize;

use super::capture::{
    dominant_indent_unit, first_child_indentation,
    first_direct_managed_log_start, leading_spaces_or_tabs_len, line_spans,
    LineSpan,
};

/// `SCHEDULE_LOG_EMOJI` + `SCHEDULE_LOG_LABEL` in main.js. The calendar emoji
/// is followed by the variation selector `U+FE0F`; dropping it still renders
/// in most fonts but fails the plugin's `SCHEDULE_LOG_PARENT_RE`.
pub(crate) const MARKER_TEXT: &str = "🗓️ **SCHEDULE LOG**";
/// `SCHEDULE_LOG_ENTRY_EMPHASIS` in main.js.
pub(crate) const ENTRY_EMPHASIS: &str = "*";
/// `SCHEDULE_LOG_SEPARATOR` in main.js, without its surrounding spaces.
/// Em dash, `U+2014`.
pub(crate) const SEPARATOR: &str = "—";
/// `SCHEDULE_LOG_TRANSITION` in main.js, without its surrounding spaces.
/// Right arrow, `U+2192`.
pub(crate) const TRANSITION: &str = "→";
/// `SCHEDULE_LOG_AUTO_REASON_EMOJI` in main.js. Die, `U+1F3B2`.
pub(crate) const AUTO_REASON_EMOJI: &str = "🎲";
/// `SCHEDULE_LOG_AUTO_REASON_SEPARATOR` in main.js, without its surrounding
/// spaces. Middle dot, `U+00B7`.
pub(crate) const AUTO_REASON_SEPARATOR: &str = "·";
/// `IMPLICIT_PRIORITY_LEVEL_LABEL` in main.js.
pub(crate) const IMPLICIT_LEVEL_LABEL: &str = "P0";

/// `formatPriorityRollScheduleReason` in main.js, restricted to the
/// `source: "priority"` branch that a `p:<N>` capture always takes. The die
/// emoji marks this as machine-rolled; the roll window's en dash (`U+2013`) is
/// distinct from both the transition arrow and the entry separator's em dash.
pub(crate) fn priority_roll_reason(
    from_label: &str,
    to_label: &str,
    rolled_days: u64,
    min_days: u64,
    max_days: u64,
) -> String {
    let head = if from_label == to_label {
        to_label.to_string()
    } else {
        format!("{from_label} {TRANSITION} {to_label}")
    };
    format!(
        "{AUTO_REASON_EMOJI} {head} {AUTO_REASON_SEPARATOR} in **{rolled_days}** ({min_days}\u{2013}{max_days}) days"
    )
}

/// `formatScheduleLogEntryText` in main.js: `*<from> → <to>* — <reason>`,
/// or `*<to>* — <reason>` when there is no previous value.
pub(crate) fn entry_text(from: Option<&str>, to: &str, reason: &str) -> String {
    match from {
        Some(from) => format!(
            "{ENTRY_EMPHASIS}{from} {TRANSITION} {to}{ENTRY_EMPHASIS} {SEPARATOR} {reason}"
        ),
        None => {
            format!("{ENTRY_EMPHASIS}{to}{ENTRY_EMPHASIS} {SEPARATOR} {reason}")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ScheduleLog {
    pub(crate) reason: String,
    pub(crate) lines: Vec<String>,
}

/// `formatScheduleLogParentBullet` + `formatScheduleLogEntryBullet` in
/// main.js, restricted to a brand-new task with no existing schedule log:
/// always a two-line block, the marker bullet then one entry with no
/// `<from> → ` half.
pub(crate) fn plan(
    indent_unit: &str,
    scheduled: &str,
    reason: String,
) -> ScheduleLog {
    let entry = entry_text(None, scheduled, &reason);
    ScheduleLog {
        reason,
        lines: vec![
            format!("{indent_unit}- {MARKER_TEXT}"),
            format!("{indent_unit}{indent_unit}- {entry}"),
        ],
    }
}

/// The Schedule Log reason head a `bob randomize` re-roll writes:
/// `🎲 P2 randomize · in **21** (8–30) days`, with an optional
/// ` from <until>` suffix when the roll base is after today. The head names
/// the tool (unlike the picker's `🎲 P2 roll`) while keeping the picker
/// grammar that `SCHEDULE_LOG_ENTRY_RE` accepts.
pub(crate) fn randomize_reason(
    label: &str,
    rolled_days: u64,
    min_days: u64,
    max_days: u64,
    from: Option<&str>,
) -> String {
    let suffix = from.map(|date| format!(" from {date}")).unwrap_or_default();
    format!(
        "{AUTO_REASON_EMOJI} {label} randomize {AUTO_REASON_SEPARATOR} in **{rolled_days}** ({min_days}\u{2013}{max_days}) days{suffix}"
    )
}

/// Whether [`plan_entry_insertion`] found an existing log or created one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogInsertion {
    Prepended,
    Created,
}

/// Insert one Schedule Log `entry` line for the task at `task_line_index`:
/// prepend it as the first child of an existing direct-child log marker
/// (newest first), or append a marker plus the entry as the task's last
/// direct child, mirroring how the picker creates a missing log. Marker
/// recognition (including the legacy `**Schedule log**` label) comes from
/// `capture::first_direct_managed_log_start`, so a marker nested under
/// another child is ignored. Indentation reuses the first existing child
/// indentation, then the note's dominant indent unit, then a tab. Bullets
/// are always `-`. Returns the full postimage and how the entry landed.
pub(crate) fn plan_entry_insertion(
    contents: &str,
    task_line_index: usize,
    entry: &str,
) -> (String, LogInsertion) {
    let lines = line_spans(contents);
    let Some(task_line) = lines.get(task_line_index) else {
        return (contents.to_string(), LogInsertion::Created);
    };
    let task_indentation =
        &task_line.text[..leading_spaces_or_tabs_len(task_line.text)];
    let task_indentation = task_indentation.to_string();
    let task_end_line = child_block_end_line(&lines, task_line_index);
    let task_end_offset = lines[task_end_line].end;
    let indent_unit = dominant_indent_unit(&lines).unwrap_or("\t").to_string();

    if let Some(marker_offset) =
        first_direct_managed_log_start(&lines, task_line_index, task_end_offset)
    {
        let marker_line_index = line_index_at_start(&lines, marker_offset);
        let marker_text = lines[marker_line_index].text;
        let marker_indent_len = leading_spaces_or_tabs_len(marker_text);
        let marker_indentation = marker_text[..marker_indent_len].to_string();
        let marker_end_line = child_block_end_line(&lines, marker_line_index);
        let marker_end_offset = lines[marker_end_line].end;
        let entry_indent = first_child_indentation(
            &lines,
            marker_line_index,
            marker_end_offset,
            &marker_indentation,
        )
        .unwrap_or_else(|| format!("{marker_indentation}{indent_unit}"));
        let entry_line = format!("{entry_indent}- {entry}");
        let updated =
            insert_line_after(contents, &lines, marker_line_index, &entry_line);
        return (updated, LogInsertion::Prepended);
    }

    let child_indent = first_child_indentation(
        &lines,
        task_line_index,
        task_end_offset,
        &task_indentation,
    )
    .unwrap_or_else(|| format!("{task_indentation}{indent_unit}"));
    let marker_line = format!("{child_indent}- {MARKER_TEXT}");
    let entry_line = format!("{child_indent}{indent_unit}- {entry}");
    let block = format!("{marker_line}\n{entry_line}");
    let updated = insert_line_after(contents, &lines, task_end_line, &block);
    (updated, LogInsertion::Created)
}

/// The last line index of `parent_index`'s child block: every later line
/// that is blank or indented deeper than the parent, stopping at the first
/// nonblank line indented at or shallower than the parent.
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

fn line_index_at_start(lines: &[LineSpan<'_>], offset: usize) -> usize {
    let mut start = 0;
    for (index, line) in lines.iter().enumerate() {
        if start == offset {
            return index;
        }
        start = line.end;
    }
    lines.len().saturating_sub(1)
}

fn line_start_offset(lines: &[LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines[index - 1].end
    }
}

fn line_has_crlf(content: &str, lines: &[LineSpan<'_>], index: usize) -> bool {
    let start = line_start_offset(lines, index);
    content[start..lines[index].end].ends_with("\r\n")
}

/// Insert `new_text` (one or more `\n`-joined physical lines) as new lines
/// immediately after line `after_index`, matching that line's `\n`/`\r\n`
/// ending and preserving the file's final-newline state.
fn insert_line_after(
    content: &str,
    lines: &[LineSpan<'_>],
    after_index: usize,
    new_text: &str,
) -> String {
    let insert_offset = lines[after_index].end;
    let ending = if line_has_crlf(content, lines, after_index) {
        "\r\n"
    } else {
        "\n"
    };
    let block = new_text.replace('\n', ending);
    if insert_offset == content.len() && !content.ends_with('\n') {
        format!("{content}{ending}{block}")
    } else {
        format!(
            "{}{block}{ending}{}",
            &content[..insert_offset],
            &content[insert_offset..]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // scripts/test-navigation-hotkeys.cjs:4365-4367 is the picker's own
    // fixture for this exact block; keep this test byte-for-byte in sync
    // with it.
    #[test]
    fn plan_matches_the_picker_fixture() {
        let reason = priority_roll_reason("P0", "P4", 91, 91, 365);
        let log = plan("\t", "2026-11-02", reason);
        assert_eq!(
            log.lines,
            vec![
                "\t- 🗓️ **SCHEDULE LOG**",
                "\t\t- *2026-11-02* — 🎲 P0 → P4 · in **91** (91–365) days",
            ]
        );
    }

    #[test]
    fn marker_text_keeps_the_variation_selector() {
        let mut chars = MARKER_TEXT.chars();
        assert_eq!(chars.next(), Some('\u{1F5D3}'));
        assert_eq!(chars.next(), Some('\u{FE0F}'));
    }

    #[test]
    fn entry_line_uses_the_exact_codepoints() {
        let reason = priority_roll_reason("P0", "P4", 91, 91, 365);
        let entry = entry_text(None, "2026-11-02", &reason);
        assert!(entry.contains('\u{2014}'), "missing em dash separator");
        assert!(entry.contains('\u{1F3B2}'), "missing die emoji");
        assert!(entry.contains('\u{2192}'), "missing transition arrow");
        assert!(entry.contains('\u{00B7}'), "missing middle dot separator");
        assert!(entry.contains('\u{2013}'), "missing en dash roll window");
    }

    #[test]
    fn plan_uses_a_two_space_indent_unit() {
        let reason = priority_roll_reason("P0", "P1", 2, 2, 7);
        let log = plan("  ", "2026-08-05", reason);
        assert_eq!(log.lines[0], "  - 🗓️ **SCHEDULE LOG**");
        assert!(log.lines[1].starts_with("    - "));
    }

    #[test]
    fn entry_text_renders_the_transition_form_with_a_prior_date() {
        let reason = "some reason".to_string();
        let entry = entry_text(Some("2026-08-13"), "2026-09-02", &reason);
        assert_eq!(entry, "*2026-08-13 → 2026-09-02* — some reason");
    }

    #[test]
    fn entry_text_renders_the_short_form_with_no_prior_date() {
        let reason = "some reason".to_string();
        let entry = entry_text(None, "2026-09-02", &reason);
        assert_eq!(entry, "*2026-09-02* — some reason");
    }

    #[test]
    fn priority_roll_reason_collapses_when_the_level_is_unchanged() {
        assert_eq!(
            priority_roll_reason("P2", "P2", 17, 8, 30),
            "🎲 P2 · in **17** (8–30) days"
        );
    }

    #[test]
    fn priority_roll_reason_keeps_fixed_window_endpoints() {
        assert_eq!(
            priority_roll_reason("P0", "P1", 4, 4, 4),
            "🎲 P0 → P1 · in **4** (4–4) days"
        );
    }

    #[test]
    fn randomize_reason_names_the_tool_without_a_from_suffix() {
        assert_eq!(
            randomize_reason("P2", 21, 8, 30, None),
            "🎲 P2 randomize · in **21** (8–30) days"
        );
    }

    #[test]
    fn randomize_reason_appends_the_until_base() {
        assert_eq!(
            randomize_reason("P2", 17, 8, 30, Some("2026-10-12")),
            "🎲 P2 randomize · in **17** (8–30) days from 2026-10-12"
        );
    }

    #[test]
    fn randomize_reason_keeps_a_fixed_window() {
        assert_eq!(
            randomize_reason("P1", 4, 4, 4, None),
            "🎲 P1 randomize · in **4** (4–4) days"
        );
    }

    #[test]
    fn insertion_prepends_under_a_tabbed_marker() {
        let contents = "- [ ] Task [scheduled:: 2026-09-10] #task\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- *2026-09-01* — old\n";
        let (updated, landed) =
            plan_entry_insertion(contents, 0, "*new* — 🎲 P2 randomize");
        assert_eq!(landed, LogInsertion::Prepended);
        assert_eq!(
            updated,
            "- [ ] Task [scheduled:: 2026-09-10] #task\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- *new* — 🎲 P2 randomize\n\t\t- *2026-09-01* — old\n"
        );
    }

    #[test]
    fn insertion_prepends_under_a_spaced_marker() {
        let contents =
            "- [ ] Task #task\n  - 🗓️ **SCHEDULE LOG**\n    - *old* — kept\n";
        let (updated, landed) = plan_entry_insertion(contents, 0, "fresh");
        assert_eq!(landed, LogInsertion::Prepended);
        assert_eq!(
            updated,
            "- [ ] Task #task\n  - 🗓️ **SCHEDULE LOG**\n    - fresh\n    - *old* — kept\n"
        );
    }

    #[test]
    fn insertion_prepends_under_a_legacy_marker() {
        let contents =
            "- [ ] Task #task\n\t- **Schedule log**\n\t\t- *old* — kept\n";
        let (updated, landed) = plan_entry_insertion(contents, 0, "fresh");
        assert_eq!(landed, LogInsertion::Prepended);
        assert_eq!(
            updated,
            "- [ ] Task #task\n\t- **Schedule log**\n\t\t- fresh\n\t\t- *old* — kept\n"
        );
    }

    #[test]
    fn insertion_ignores_a_marker_nested_under_another_child() {
        let contents =
            "- [ ] Task #task\n\t- note\n\t\t- 🗓️ **SCHEDULE LOG**\n";
        let (updated, landed) = plan_entry_insertion(contents, 0, "fresh");
        assert_eq!(landed, LogInsertion::Created);
        assert_eq!(
            updated,
            "- [ ] Task #task\n\t- note\n\t\t- 🗓️ **SCHEDULE LOG**\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- fresh\n"
        );
    }

    #[test]
    fn insertion_creates_a_marker_after_existing_children() {
        let contents = "- [ ] Task #task\n\t- note\n- [ ] Next #task\n";
        let (updated, landed) = plan_entry_insertion(contents, 0, "fresh");
        assert_eq!(landed, LogInsertion::Created);
        assert_eq!(
            updated,
            "- [ ] Task #task\n\t- note\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- fresh\n- [ ] Next #task\n"
        );
    }

    #[test]
    fn insertion_creates_a_marker_for_a_childless_task() {
        let contents = "- [ ] Task #task\n- [ ] Next #task\n";
        let (updated, landed) = plan_entry_insertion(contents, 0, "fresh");
        assert_eq!(landed, LogInsertion::Created);
        assert_eq!(
            updated,
            "- [ ] Task #task\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- fresh\n- [ ] Next #task\n"
        );
    }

    #[test]
    fn insertion_preserves_crlf_endings() {
        let contents =
            "- [ ] Task #task\r\n\t- 🗓️ **SCHEDULE LOG**\r\n\t\t- old\r\n";
        let (updated, landed) = plan_entry_insertion(contents, 0, "fresh");
        assert_eq!(landed, LogInsertion::Prepended);
        assert_eq!(
            updated,
            "- [ ] Task #task\r\n\t- 🗓️ **SCHEDULE LOG**\r\n\t\t- fresh\r\n\t\t- old\r\n"
        );
    }

    #[test]
    fn insertion_preserves_a_missing_final_newline() {
        let contents = "- [ ] Task #task";
        let (updated, landed) = plan_entry_insertion(contents, 0, "fresh");
        assert_eq!(landed, LogInsertion::Created);
        assert_eq!(
            updated,
            "- [ ] Task #task\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- fresh"
        );
    }
}

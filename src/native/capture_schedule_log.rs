//! Byte-for-byte parity with the Obsidian `Ctrl+Shift+P` picker's
//! `🗓️ **SCHEDULE LOG**` marker and roll-reason text. Mirrors the constants
//! near `plugins/bob-navigation-hotkeys/main.js:250-274` and the plugin's
//! schedule-log/priority-roll formatters; `scripts/test-navigation-hotkeys.cjs`
//! keeps the picker's own fixture of the exact bytes reproduced here.

use serde::Serialize;

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
}

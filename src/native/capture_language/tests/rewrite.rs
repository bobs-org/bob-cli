//! Draft rewrite tests.

use super::super::editor_model::EditorMode;
use super::super::editor_parse::*;
use super::super::rewrite::*;

#[test]
fn rewrite_draft_absorbs_a_trailing_local_marker() {
    let raw = "Buy milk @dev @@";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
    assert_eq!(rewrite.text, "Buy milk @@dev");
    assert_eq!(rewrite.cursor, Some(14));
    assert_eq!(rewrite.summary.as_deref(), Some("Moved @dev into @@dev"));
    assert_eq!(
        rewrite.edits,
        vec![
            TextEdit {
                start: 9,
                end: 14,
                replacement: String::new(),
            },
            TextEdit {
                start: 14,
                end: 16,
                replacement: "@@dev".to_string(),
            },
        ]
    );
}

#[test]
fn rewrite_draft_absorbs_a_leading_local_marker() {
    let raw = "@dev Buy milk @@";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
    assert_eq!(rewrite.text, "Buy milk @@dev");
}

#[test]
fn rewrite_draft_absorbs_a_parent_lines_marker_from_a_child_lines_bare_at_at() {
    let raw = "Buy milk @dev\n- more detail @@";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
    assert_eq!(rewrite.text, "Buy milk\n- more detail @@dev");
}

#[test]
fn rewrite_draft_absorbs_a_sub_bullet_local_marker() {
    let raw = "Buy stock @cash+goog-exit @@";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
    assert_eq!(rewrite.text, "Buy stock @@cash+goog-exit");
}

#[test]
fn rewrite_draft_absorbs_a_declaration_only_line_into_a_later_items_bare_at_at()
{
    let raw = "@@foo\nBuy milk @@";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbDeclaration));
    assert_eq!(rewrite.text, "Buy milk @@foo");
    assert_eq!(
        rewrite.summary.as_deref(),
        Some("Moved the @@foo declaration here")
    );
}

#[test]
fn rewrite_draft_reports_rule_a5_notices_for_non_absorbable_markers() {
    for (raw, needle) in [
        ("note @notes#Ideas @@", "cannot take a section"),
        ("note @dev^id @@", "cannot take a block ID"),
        ("note @dev:id @@", "cannot take a Pomodoro link"),
        ("note @dev:id#bugs @@", "cannot take a Pomodoro link"),
        ("note this # @@", "cannot take a Pomodoro note"),
        ("note @dev^id+ @@", "cannot take a project note"),
        ("note @dev^id+#bugs @@", "cannot take a project note"),
        (
            "@sase:deep-fix @@",
            "@@ cannot take a Pomodoro link: leave @sase:deep-fix on this item, or delete it",
        ),
        (
            "^sase:deep-fix @@",
            "@@ cannot take a Pomodoro link: leave ^sase:deep-fix on this item, or delete it",
        ),
    ] {
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None, "{raw}");
        assert_eq!(rewrite.text, raw, "{raw}");
        assert_eq!(rewrite.notices.len(), 1, "{raw}");
        assert!(
            rewrite.notices[0].contains(needle),
            "{raw}: {}",
            rewrite.notices[0]
        );
    }
}

#[test]
fn rewrite_draft_declines_when_the_item_has_two_local_markers() {
    let raw = "Buy milk @dev @@\n- child @notes#Ideas";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
    assert!(rewrite.notices.is_empty());
}

#[test]
fn rewrite_draft_is_a_no_op_without_a_bare_at_at() {
    let raw = "Buy milk @dev";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
    assert_eq!(rewrite.cursor, None);
    assert!(rewrite.edits.is_empty());
}

#[test]
fn rewrite_draft_selects_the_bare_at_at_under_the_cursor_else_the_last() {
    let raw = "Buy milk @dev @@ @@";
    let claimed_start = |rewrite: &DraftRewrite| {
        rewrite
            .edits
            .iter()
            .find(|edit| edit.replacement == "@@dev")
            .map(|edit| edit.start)
            .expect("replace edit")
    };

    assert_eq!(claimed_start(&rewrite_draft(raw, Some(15))), 14);
    assert_eq!(claimed_start(&rewrite_draft(raw, Some(18))), 17);
    assert_eq!(claimed_start(&rewrite_draft(raw, None)), 17);
}

#[test]
fn rewrite_draft_is_idempotent() {
    let raw = "Buy milk @dev @@";
    let first = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(first.rule, Some(RewriteRule::AbsorbLocalMarker));

    let second = rewrite_draft(&first.text, first.cursor);
    assert_eq!(second.rule, None);
    assert_eq!(second.text, first.text);
}

#[test]
fn rewrite_draft_avoids_double_spaces_and_the_result_parses_cleanly() {
    let raw = "@dev note @@ more text";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
    assert_eq!(rewrite.text, "note @@dev more text");
    assert!(!rewrite.text.contains("  "), "{}", rewrite.text);

    let parsed = parse_for_editor(&rewrite.text);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
}

#[test]
fn rewrite_draft_keeps_offsets_on_char_boundaries_with_multibyte_input() {
    let raw = "caf\u{e9} \u{1f680} @dev @@";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
    for edit in &rewrite.edits {
        assert!(raw.is_char_boundary(edit.start));
        assert!(raw.is_char_boundary(edit.end));
    }
    assert_eq!(rewrite.text, "caf\u{e9} \u{1f680} @@dev");
}

fn switch_at(raw: &str) -> DraftRewrite {
    rewrite_draft(raw, Some(raw.len()))
}

fn assert_switch(
    raw: &str,
    expected: &str,
    from: &str,
    to: &str,
    start: usize,
) {
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(
        rewrite.rule,
        Some(RewriteRule::SwitchBlockIdSeparator),
        "{raw}"
    );
    assert_eq!(rewrite.text, expected, "{raw}");
    assert_eq!(rewrite.cursor, Some(expected.len()), "{raw}");
    assert_eq!(
        rewrite.summary.as_deref(),
        Some(format!("Changed {from} to {to}").as_str()),
        "{raw}"
    );
    assert_eq!(
        rewrite.edits,
        vec![TextEdit {
            start,
            end: raw.len(),
            replacement: to.to_string(),
        }],
        "{raw}"
    );
    assert!(raw.is_char_boundary(start), "{raw}");
    assert!(raw.is_char_boundary(raw.len()), "{raw}");

    let again = rewrite_draft(&rewrite.text, rewrite.cursor);
    assert_eq!(again.rule, None, "idempotent {raw}");
    assert_eq!(again.text, rewrite.text, "idempotent {raw}");
}

#[test]
fn rewrite_draft_switches_a_trailing_colon_marker_to_caret() {
    assert_switch(
        "Do work @file:id^",
        "Do work @file^id",
        "@file:id",
        "@file^id",
        8,
    );
}

#[test]
fn rewrite_draft_switches_a_trailing_caret_marker_to_colon() {
    assert_switch(
        "Do work @file^id:",
        "Do work @file:id",
        "@file^id",
        "@file:id",
        8,
    );
}

#[test]
fn rewrite_draft_switches_solo_and_leading_markers() {
    assert_switch("@file:id^", "@file^id", "@file:id", "@file^id", 0);
    assert_switch("@file^id:", "@file:id", "@file^id", "@file:id", 0);
    let raw = "@file:id^ Do work";
    let caret_after_id = "@file:id^".len();
    let rewrite = rewrite_draft(raw, Some(caret_after_id));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "@file^id Do work");
    assert_eq!(rewrite.cursor, Some("@file^id".len()));
}

#[test]
fn rewrite_draft_switches_a_child_line_trailing_marker() {
    let raw = "Do work\n- detail @file:id^";
    let rewrite = switch_at(raw);
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work\n- detail @file^id");
    assert_eq!(rewrite.cursor, Some(rewrite.text.len()));
}

#[test]
fn rewrite_draft_switches_one_item_in_a_batch() {
    let first = "First @file:id^";
    let raw = format!("{first}\n\nSecond @file:id");
    let rewrite = rewrite_draft(&raw, Some(first.len()));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "First @file^id\n\nSecond @file:id");
    assert_eq!(rewrite.cursor, Some("First @file^id".len()));
}

#[test]
fn rewrite_draft_switches_uppercase_and_hyphenated_ids() {
    assert_switch(
        "Do work @Cash:Goog-Exit^",
        "Do work @Cash^Goog-Exit",
        "@Cash:Goog-Exit",
        "@Cash^Goog-Exit",
        8,
    );
}

#[test]
fn rewrite_draft_switches_beside_emoji_and_preserves_crlf() {
    let raw = "caf\u{e9} \u{1f680} @file:id^";
    let rewrite = switch_at(raw);
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "caf\u{e9} \u{1f680} @file^id");
    for edit in &rewrite.edits {
        assert!(raw.is_char_boundary(edit.start));
        assert!(raw.is_char_boundary(edit.end));
    }

    let raw = "Do work @file:id^\r\nNext";
    let cursor = "Do work @file:id^".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work @file^id\r\nNext");
    assert!(rewrite.text.contains("\r\n"));
}

#[test]
fn rewrite_draft_switches_before_a_schedule_marker() {
    let raw = "Do work @file:id^ s:2";
    let cursor = "Do work @file:id^".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work @file^id s:2");
    assert_eq!(rewrite.cursor, Some("Do work @file^id".len()));
}

#[test]
fn rewrite_draft_separator_toggle_alternates() {
    let first = switch_at("Do work @file:id^");
    assert_eq!(first.text, "Do work @file^id");
    let second_raw = format!("{}:", first.text);
    let second = switch_at(&second_raw);
    assert_eq!(second.text, "Do work @file:id");
    let third_raw = format!("{}^", second.text);
    let third = switch_at(&third_raw);
    assert_eq!(third.text, "Do work @file^id");
}

#[test]
fn rewrite_draft_separator_toggle_requires_a_cursor() {
    let raw = "Do work @file:id^";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
    assert_eq!(rewrite.cursor, None);
}

#[test]
fn rewrite_draft_separator_toggle_keeps_absorption_when_it_does_not_claim() {
    let raw = "Do work @file:id\n\nBuy milk @dev @@";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
    assert_eq!(rewrite.text, "Do work @file:id\n\nBuy milk @@dev");
}

#[test]
fn rewrite_draft_separator_toggle_exclusions() {
    let inside_id = "Do work @file:i^d";
    let mid_prose = "see @file:id^ here";
    let cases = [
        ("Do work @file:id:", "Do work @file:id:".len()),
        ("Do work @file^id^", "Do work @file^id^".len()),
        ("Do work @file:id ^", "Do work @file:id ^".len()),
        ("Do work @file:id\n^", "Do work @file:id\n^".len()),
        ("@file:id#name^", "@file:id#name^".len()),
        ("@file:id+^", "@file:id+^".len()),
        ("@file:id=3^", "@file:id=3^".len()),
        ("@file^id+^", "@file^id+^".len()),
        ("@file^id+#bugs:", "@file^id+#bugs:".len()),
        ("@@file:id^", "@@file:id^".len()),
        ("@!file:id^", "@!file:id^".len()),
        ("^file:id^", "^file:id^".len()),
        ("&note:id^", "&note:id^".len()),
        ("!note:id^", "!note:id^".len()),
        (":id^", ":id^".len()),
        ("^id:", "^id:".len()),
        ("@file:^", "@file:^".len()),
        ("@file^:", "@file^:".len()),
        (mid_prose, "see @file:id^".len()),
        (inside_id, "Do work @file:i^".len()),
    ];
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw}");
        assert_eq!(rewrite.text, raw, "{raw}");
    }
}

#[test]
fn rewrite_draft_separator_toggle_parses_as_the_switched_mode() {
    let caret = switch_at("Do work @file:id^");
    let parsed = parse_for_editor(&caret.text);
    assert_eq!(parsed.mode, EditorMode::Task);
    assert_eq!(parsed.block_id.as_deref(), Some("id"));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let colon = switch_at("Do work @file^id:");
    let parsed = parse_for_editor(&colon.text);
    assert_eq!(parsed.mode, EditorMode::PomodoroTask);
    assert_eq!(parsed.block_id.as_deref(), Some("id"));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let raw = parse_for_editor("Do work @file:id^");
    assert_ne!(raw.block_id.as_deref(), Some("id"));
}

//! Draft rewrite tests.

use super::super::alias_tip::AliasId;
use super::super::editor_model::{EditorMode, Severity, SpanKind};
use super::super::editor_parse::*;
use super::super::rewrite::*;
use super::assert_alias_applied;

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
fn rewrite_draft_keeps_a_sub_bullet_local_marker_local() {
    let raw = "Buy stock @cash+goog-exit @@";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
    assert!(
        rewrite.notices[0].contains("cannot take an existing task destination")
    );
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
        ("note this %n @@", "cannot take a Pomodoro note"),
        ("note @dev^id@ @@", "cannot take a project note"),
        ("note @dev^id@#bugs @@", "cannot take a project note"),
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
fn rewrite_draft_leaves_template_drafts_untouched() {
    let raw = "Do work @file:{0}^ %t\n\nrow";
    let rewrite = rewrite_draft(raw, Some("Do work @file:{0}^".len()));
    assert_eq!(rewrite.rule, None);
    assert!(rewrite.edits.is_empty());
    assert_eq!(rewrite.text, raw);
}

#[test]
fn rewrite_draft_does_not_start_templates_from_a_third_at() {
    for (raw, cursor) in [("@@@", 3), ("Read {0} @@@", 12)] {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert!(rewrite.edits.is_empty(), "{raw:?}");
        assert_eq!(rewrite.text, raw);
    }
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
fn rewrite_draft_dot_toggles_block_id_separators() {
    assert_switch(
        "Do work @file:id.",
        "Do work @file^id",
        "@file:id",
        "@file^id",
        8,
    );
    assert_switch(
        "Do work @file^id.",
        "Do work @file:id",
        "@file^id",
        "@file:id",
        8,
    );
    assert_switch(
        "Create sase-gallery that gives agents access to all of my personal photos (Google Photos?)! @sase:sase-gallery.",
        "Create sase-gallery that gives agents access to all of my personal photos (Google Photos?)! @sase^sase-gallery",
        "@sase:sase-gallery",
        "@sase^sase-gallery",
        "Create sase-gallery that gives agents access to all of my personal photos (Google Photos?)! ".len(),
    );
    assert_switch(
        "Do work @file:id@.",
        "Do work @file^id@",
        "@file:id@",
        "@file^id@",
        8,
    );
    assert_switch(
        "Do work @file^id@.",
        "Do work @file:id@",
        "@file^id@",
        "@file:id@",
        8,
    );
    assert_switch(
        "café 🚀 @Cash:Goog-Exit.",
        "café 🚀 @Cash^Goog-Exit",
        "@Cash:Goog-Exit",
        "@Cash^Goog-Exit",
        "café 🚀 ".len(),
    );
    assert_switch(
        "Do work\n- detail @file:id.",
        "Do work\n- detail @file^id",
        "@file:id",
        "@file^id",
        "Do work\n- detail ".len(),
    );
    assert_switch(
        "First @file:id\n\nSecond @file:id.",
        "First @file:id\n\nSecond @file^id",
        "@file:id",
        "@file^id",
        "First @file:id\n\nSecond ".len(),
    );

    let crlf = "Do work @file:id.\r\nNext";
    let cursor = "Do work @file:id.".len();
    let rewrite = rewrite_draft(crlf, Some(cursor));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work @file^id\r\nNext");
    assert_eq!(rewrite.cursor, Some("Do work @file^id".len()));

    let first =
        rewrite_draft("Do work @file:id.", Some("Do work @file:id.".len()));
    let second_input = format!("{}.", first.text);
    let second = rewrite_draft(&second_input, Some(second_input.len()));
    let third_input = format!("{}.", second.text);
    let third = rewrite_draft(&third_input, Some(third_input.len()));
    assert_eq!(first.text, "Do work @file^id");
    assert_eq!(second.text, "Do work @file:id");
    assert_eq!(third.text, "Do work @file^id");
}

#[test]
fn rewrite_draft_comma_switches_block_id_separators_to_plus() {
    for (raw, expected, marker, from, start) in [
        (
            "Do work @file:id,",
            "Do work @file+id",
            "@file+id",
            "@file:id",
            "Do work ".len(),
        ),
        (
            "Do work @file^id,",
            "Do work @file+id",
            "@file+id",
            "@file^id",
            "Do work ".len(),
        ),
        (
            "Create sase-gallery! @sase:sase-gallery,",
            "Create sase-gallery! @sase+sase-gallery",
            "@sase+sase-gallery",
            "@sase:sase-gallery",
            "Create sase-gallery! ".len(),
        ),
        (
            "@cash:goog-exit,",
            "@cash+goog-exit",
            "@cash+goog-exit",
            "@cash:goog-exit",
            0,
        ),
        (
            "@cash^goog-exit,",
            "@cash+goog-exit",
            "@cash+goog-exit",
            "@cash^goog-exit",
            0,
        ),
        (
            "- [ ] Task @work:a,",
            "- [ ] Task @work+a",
            "@work+a",
            "@work:a",
            "- [ ] Task ".len(),
        ),
        (
            "Parent\n- detail @work:a,",
            "Parent\n- detail @work+a",
            "@work+a",
            "@work:a",
            "Parent\n- detail ".len(),
        ),
        (
            "First @file:id\n\nSecond @file:id,",
            "First @file:id\n\nSecond @file+id",
            "@file+id",
            "@file:id",
            "First @file:id\n\nSecond ".len(),
        ),
        (
            "café 🚀 @Cash:Goog-Exit,",
            "café 🚀 @Cash+Goog-Exit",
            "@Cash+Goog-Exit",
            "@Cash:Goog-Exit",
            "café 🚀 ".len(),
        ),
    ] {
        assert_switch(raw, expected, from, marker, start);
    }

    let crlf = "Do work @file:id,\r\nNext";
    let rewrite = rewrite_draft(crlf, Some("Do work @file:id,".len()));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work @file+id\r\nNext");
    assert_eq!(rewrite.cursor, Some("Do work @file+id".len()));
    assert!(rewrite.alias.is_none());
}

#[test]
fn rewrite_draft_comma_matches_plus_in_transfer_merge_and_run_slots() {
    for (prefix, suffix, expected) in [
        ("@inbox:bench", " > @db", "@inbox+bench > @db"),
        ("@a+x > @c:tid", "", "@a+x > @c+tid"),
        ("Create task < @cash:line", "", "Create task < @cash+line"),
        ("@work+a @dev:b", "", "@work+a @dev+b"),
        ("note @work:a", " @dev+b", "note @work+a @dev+b"),
    ] {
        let before_plus = format!("{prefix}+{suffix}");
        let before_comma = format!("{prefix},{suffix}");
        let cursor = prefix.len() + 1;
        let plus = rewrite_draft(&before_plus, Some(cursor));
        let comma = rewrite_draft(&before_comma, Some(cursor));
        assert_eq!(comma.rule, plus.rule, "{before_comma}");
        assert_eq!(comma.text, plus.text, "{before_comma}");
        assert_eq!(comma.cursor, plus.cursor, "{before_comma}");
        assert_eq!(comma.summary, plus.summary, "{before_comma}");
        assert_eq!(comma.edits, plus.edits, "{before_comma}");
        assert_eq!(comma.text, expected, "{before_comma}");
    }

    let first =
        rewrite_draft("@cash:goog-exit,", Some("@cash:goog-exit,".len()));
    assert_eq!(first.text, "@cash+goog-exit");
    let next = "@cash+goog-exit,";
    let second = rewrite_draft(next, Some(next.len()));
    assert_eq!(second.rule, Some(RewriteRule::ExpandSigilAlias));
    assert_eq!(second.text, "@cash+goog-exit +");
    assert_eq!(
        second.alias.as_ref().map(|alias| alias.id),
        Some(AliasId::NextParent)
    );
}

#[test]
fn rewrite_draft_dot_toggles_task_bullet_ids() {
    for (raw, expected) in [
        ("@dev\n- Task :task-id.", "@dev\n- Task ^task-id"),
        ("@dev\n- Task ^task-id.", "@dev\n- Task :task-id"),
        (
            "Project @cash^project-id@\n- Task :task-id.",
            "Project @cash^project-id@\n- Task ^task-id",
        ),
        (
            "Project @cash^project-id@\n- Task ^task-id.",
            "Project @cash^project-id@\n- Task :task-id",
        ),
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::SwitchBlockIdSeparator),
            "{raw}"
        );
        assert_eq!(rewrite.text, expected, "{raw}");
        assert_eq!(rewrite.cursor, Some(expected.len()), "{raw}");
        assert!(rewrite.alias.is_none(), "{raw}");
    }

    let move_alias = "@cash^goog-exit@\n- ^.";
    let rewrite = rewrite_draft(move_alias, Some(move_alias.len()));
    assert_eq!(rewrite.text, "@cash^goog-exit@\n- ^^", "{move_alias}");
    assert_ne!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
}

#[test]
fn rewrite_draft_dot_does_not_toggle_ineligible_block_id_spots() {
    let cases = [
        ("@file+id.", "@file+id.".len()),
        ("Do work @file:i.d", "Do work @file:i.".len()),
        ("@file:id#name.", "@file:id#name.".len()),
        ("@file:id=3.", "@file:id=3.".len()),
        ("@@file:id.", "@@file:id.".len()),
        ("@!file:id.", "@!file:id.".len()),
        ("^file:id.", "^file:id.".len()),
        ("&note:id.", "&note:id.".len()),
        ("!note:id.", "!note:id.".len()),
        ("@file:.", "@file:.".len()),
        ("@file^.", "@file^.".len()),
        ("@file^id@#bugs.", "@file^id@#bugs.".len()),
        ("see @file:id. here", "see @file:id.".len()),
        ("Do work @file:id\n.", "Do work @file:id\n.".len()),
        ("@inbox:bench. > @db", "@inbox:bench.".len()),
    ];
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_ne!(
            rewrite.rule,
            Some(RewriteRule::SwitchBlockIdSeparator),
            "{raw:?}"
        );
    }

    let spaced = "Do work @file:id .";
    let rewrite = rewrite_draft(spaced, Some(spaced.len()));
    assert_ne!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));

    let no_cursor = rewrite_draft("Do work @file:id.", None);
    assert_eq!(no_cursor.rule, None);
    assert_eq!(no_cursor.text, "Do work @file:id.");
}

#[test]
fn rewrite_draft_switches_a_colon_marker_to_plus() {
    assert_switch(
        "Do work @file:id+",
        "Do work @file+id",
        "@file:id",
        "@file+id",
        8,
    );
    assert_switch("@file:id+", "@file+id", "@file:id", "@file+id", 0);
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
fn rewrite_draft_switches_plus_markers_back_to_colon_or_caret() {
    assert_switch(
        "Do work @file+id:",
        "Do work @file:id",
        "@file+id",
        "@file:id",
        8,
    );
    assert_switch(
        "Do work @file+id^",
        "Do work @file^id",
        "@file+id",
        "@file^id",
        8,
    );
    assert_switch("@file+id:", "@file:id", "@file+id", "@file:id", 0);
    assert_switch("@file+id^", "@file^id", "@file+id", "@file^id", 0);
}

#[test]
fn rewrite_draft_switches_plus_markers_in_editor_positions() {
    let raw = "@file:id+ Do work";
    let rewrite = rewrite_draft(raw, Some("@file:id+".len()));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "@file+id Do work");
    assert_eq!(rewrite.cursor, Some("@file+id".len()));
    assert_eq!(rewrite.edits[0].start, 0);
    assert_eq!(rewrite.edits[0].end, "@file:id+".len());

    let raw = "Do work\n- detail @file:id+";
    let rewrite = switch_at(raw);
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work\n- detail @file+id");
    assert_eq!(rewrite.cursor, Some(rewrite.text.len()));

    let first = "First @file:id+";
    let raw = format!("{first}\n\nSecond @file:id");
    let rewrite = rewrite_draft(&raw, Some(first.len()));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "First @file+id\n\nSecond @file:id");
    assert_eq!(rewrite.cursor, Some("First @file+id".len()));
}

#[test]
fn rewrite_draft_switches_plus_markers_with_unicode_crlf_and_schedule() {
    assert_switch(
        "café 🚀 @Cash:Goog-Exit+",
        "café 🚀 @Cash+Goog-Exit",
        "@Cash:Goog-Exit",
        "@Cash+Goog-Exit",
        "café 🚀 ".len(),
    );

    let raw = "Do work 🚀 @file:id+\r\nNext";
    let cursor = "Do work 🚀 @file:id+".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work 🚀 @file+id\r\nNext");
    assert!(rewrite.text.contains("\r\n"));

    let raw = "Do work @file:id+ s:2";
    let cursor = "Do work @file:id+".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work @file+id s:2");
    assert_eq!(rewrite.cursor, Some("Do work @file+id".len()));
}

#[test]
fn rewrite_draft_caret_plus_switches_to_plus() {
    assert_switch(
        "Do work @file^id+",
        "Do work @file+id",
        "@file^id",
        "@file+id",
        8,
    );
    assert_switch("@file^id+", "@file+id", "@file^id", "@file+id", 0);

    // A sealed project-note marker never toggles.
    let raw = "@file^id@";
    let rewrite = switch_at(raw);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);

    let parsed = parse_for_editor(raw);
    assert_eq!(parsed.mode, EditorMode::ProjectNote);
}

#[test]
fn rewrite_draft_switches_sealed_project_note_separators() {
    assert_switch(
        "Do work @file^id@:",
        "Do work @file:id@",
        "@file^id@",
        "@file:id@",
        8,
    );
    assert_switch(
        "Do work @file:id@^",
        "Do work @file^id@",
        "@file:id@",
        "@file^id@",
        8,
    );

    // The same separator, or `+`, after a seal stays literal.
    for raw in [
        "Do work @file^id@^",
        "Do work @file:id@:",
        "Do work @file^id@+",
        "Do work @file:id@+",
    ] {
        let rewrite = switch_at(raw);
        assert_eq!(rewrite.rule, None, "{raw}");
        assert_eq!(rewrite.text, raw, "{raw}");
    }

    let sealed_offer = parse_for_editor("Do work @file:id@");
    assert_eq!(sealed_offer.mode, EditorMode::PomodoroProjectNote);
}

#[test]
fn rewrite_draft_separator_toggle_supports_three_way_reverse_from_plus() {
    assert_switch(
        "Do work @file:id+",
        "Do work @file+id",
        "@file:id",
        "@file+id",
        8,
    );
    assert_switch(
        "Do work @file+id^",
        "Do work @file^id",
        "@file+id",
        "@file^id",
        8,
    );
    assert_switch(
        "Do work @file+id:",
        "Do work @file:id",
        "@file+id",
        "@file:id",
        8,
    );
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

    let raw = "Do work @file:id+";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
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
        ("Do work @file:id +", "Do work @file:id +".len()),
        ("Do work @file:id\n+", "Do work @file:id\n+".len()),
        ("@file:+", "@file:+".len()),
        ("@file:id#name+", "@file:id#name+".len()),
        ("@file:id=3+", "@file:id=3+".len()),
        ("@@file", "@@file".len()),
        ("@!file:id+", "@!file:id+".len()),
        ("^file:id+", "^file:id+".len()),
        ("&note:id+", "&note:id+".len()),
        ("!note:id+", "!note:id+".len()),
        (":id+", ":id+".len()),
        ("^id@", "^id@".len()),
        ("see @file:id+ here", "see @file:id+".len()),
        ("Do work @file:i+d", "Do work @file:i+".len()),
        ("@file+id+", "@file+id+".len()),
        ("@file+id!:", "@file+id!:".len()),
        ("@file+id#sec^", "@file+id#sec^".len()),
        ("@@file", "@@file".len()),
        ("@file+id!: ", "@file+id!: ".len()),
        ("@file+: ", "@file+: ".len()),
        ("@file+:", "@file+:".len()),
        ("@file^id@", "@file^id@".len()),
        ("@file:id=3^", "@file:id=3^".len()),
        ("@file^id@^", "@file^id@^".len()),
        ("@file^id@#bugs:", "@file^id@#bugs:".len()),
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
fn rewrite_draft_attaches_a_pipe_after_a_relation_token() {
    let cases = [
        (
            "Fix trip ~tasks:target |",
            "Fix trip ~tasks:target|",
            "~tasks:target",
            "|",
            1usize,
        ),
        (
            "Fix trip ~~tasks:source |",
            "Fix trip ~~tasks:source|",
            "~~tasks:source",
            "|",
            1usize,
        ),
        (
            "Fix ~tasks:target \t |",
            "Fix ~tasks:target|",
            "~tasks:target",
            "|",
            1usize,
        ),
        (
            "Fix ~\"My Note\":target |",
            "Fix ~\"My Note\":target|",
            "~\"My Note\":target",
            "|",
            1usize,
        ),
        (
            "~tasks:target | Fix it",
            "~tasks:target| Fix it",
            "~tasks:target",
            "|",
            1usize,
        ),
        (
            "Fix ~tasks:target | @work",
            "Fix ~tasks:target| @work",
            "~tasks:target",
            "|",
            1usize,
        ),
        // A double pipe glues both pipes: the one-off spelling survives.
        (
            "Fix trip ~tasks:target ||",
            "Fix trip ~tasks:target||",
            "~tasks:target",
            "||",
            2usize,
        ),
        (
            "Fix trip ~~tasks:source ||",
            "Fix trip ~~tasks:source||",
            "~~tasks:source",
            "||",
            2usize,
        ),
        (
            "Fix ~tasks:target || @work",
            "Fix ~tasks:target|| @work",
            "~tasks:target",
            "||",
            2usize,
        ),
    ];
    for (raw, expected, token, pipes, pipe_len) in cases {
        // The cursor sits right after the typed pipe(s).
        let pipe = raw.rfind('|').expect("pipe") + 1 - pipe_len;
        let cursor = pipe + pipe_len;
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::AttachRelationLabel),
            "{raw}"
        );
        assert_eq!(rewrite.text, expected, "{raw}");
        let run_start = expected.find('|').expect("glued pipe");
        assert_eq!(rewrite.cursor, Some(run_start + pipe_len), "{raw}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some(format!("Attached {pipes} to {token}").as_str()),
            "{raw}"
        );
        assert_eq!(
            rewrite.edits,
            vec![TextEdit {
                start: run_start,
                end: pipe,
                replacement: String::new(),
            }],
            "{raw}"
        );
        assert!(rewrite.notices.is_empty(), "{raw}");
        assert!(raw.is_char_boundary(run_start), "{raw}");

        // Idempotent: the glued output has no whitespace left to delete.
        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw}");
        assert_eq!(again.text, rewrite.text, "idempotent {raw}");
    }
}

#[test]
fn rewrite_draft_attach_relation_label_exclusions() {
    let mid_prose = "Fix ~tasks:target | then more";
    let cases = [
        ("Fix ~tasks:target|", "Fix ~tasks:target|".len()),
        ("a |", "a |".len()),
        ("Fix @work |", "Fix @work |".len()),
        ("Fix &a:b |", "Fix &a:b |".len()),
        (mid_prose, "Fix ~tasks:target |".len()),
        ("Fix ~tasks:target|x |", "Fix ~tasks:target|x |".len()),
        ("Fix ~tasks:target\n|", "Fix ~tasks:target\n|".len()),
        ("Fix ~tasks: |", "Fix ~tasks: |".len()),
        ("Fix \\~tasks:target |", "Fix \\~tasks:target |".len()),
        ("Fix ~tasks:target |||", "Fix ~tasks:target |||".len()),
        ("Fix ~tasks:target |x", "Fix ~tasks:target |x".len()),
    ];
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw}");
        assert_eq!(rewrite.text, raw, "{raw}");
    }
}

#[test]
fn rewrite_draft_attach_relation_label_requires_a_cursor() {
    let raw = "Fix trip ~tasks:target |";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
    assert_eq!(rewrite.cursor, None);
}

#[test]
fn rewrite_draft_expands_sigil_aliases_in_route_picker_positions() {
    for (raw, expected, alias, sigil) in [
        ("@r.", "@r^", ".", "^"),
        ("Buy milk @r.", "Buy milk @r^", ".", "^"),
        ("- Call mom @home.", "- Call mom @home^", ".", "^"),
        ("@r;", "@r:", ";", ":"),
        ("Fix it @r;", "Fix it @r:", ";", ":"),
        (";", ":", ";", ":"),
        ("Buy milk\n\n;", "Buy milk\n\n:", ";", ":"),
    ] {
        let cursor = raw.len();
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some(format!("Typed {sigil} for {alias}").as_str()),
            "{raw:?}"
        );
        assert_eq!(rewrite.notices, Vec::<String>::new(), "{raw:?}");
        assert_eq!(
            rewrite.edits,
            vec![TextEdit {
                start: cursor - 1,
                end: cursor,
                replacement: sigil.to_string(),
            }],
            "{raw:?}"
        );

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }

    let raw = "café @r.";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_alias_applied(raw, &rewrite);
    assert_eq!(rewrite.text, "café @r^");
    assert_eq!(rewrite.edits[0].start, raw.len() - 1);
    assert!(raw.is_char_boundary(rewrite.edits[0].start));

    let transfer = "@inbox; > @db";
    let cursor = transfer.find(';').expect("transfer alias") + 1;
    let candidate = "@inbox: > @db";
    if super::super::completion::completion_field_at(candidate, cursor)
        .is_some_and(|field| {
            field.context
                == super::super::completion::CompletionContext::PomodoroBlockId
                && field.replacement.0 == cursor
        })
    {
        let rewrite = rewrite_draft(transfer, Some(cursor));
        assert_alias_applied(transfer, &rewrite);
        assert_eq!(rewrite.text, "@inbox: > @db");
    }
}

#[test]
fn rewrite_draft_expands_relation_equals_alias() {
    for (raw, expected) in [
        ("Fix it =", "Fix it ~"),
        ("Fix it\t=", "Fix it\t~"),
        ("@bob+fix-retries =", "@bob+fix-retries ~"),
        ("Fix it &dev:build =", "Fix it &dev:build ~"),
        ("Buy milk\n- note =", "Buy milk\n- note ~"),
        ("Buy milk\n\nFix it =", "Buy milk\n\nFix it ~"),
        ("Fix [[note]] =", "Fix [[note]] ~"),
        ("Fix it `code` =", "Fix it `code` ~"),
        ("Café run =", "Café run ~"),
    ] {
        let cursor = raw.len();
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed ~ for ="),
            "{raw:?}"
        );
        assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].start, cursor - 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].end, cursor, "{raw:?}");
        assert_eq!(rewrite.edits[0].replacement, "~", "{raw:?}");
        assert!(raw.is_char_boundary(rewrite.edits[0].start), "{raw:?}");

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_relation_equals_alias_stays_literal() {
    let mut cases = vec![
        ("=", 1),
        ("=3", 1),
        ("  =", 3),
        ("Buy milk\n=", 10),
        ("Buy milk\n  =", 12),
        ("- =", 3),
        ("* =", 3),
        ("+ =", 3),
        ("Fix it\n\n- =", 11),
        ("fix=", 4),
        ("Fix it ==", 9),
        ("x =", 3),
        ("x=2 =", 5),
        ("=/ =", 4),
        ("=bugs =", 7),
        ("+5 =", 4),
        ("x\n- 1 work =", 12),
        ("see [[foo =", 11),
        ("Fix ((foo =", 11),
        ("run `cargo =", 12),
    ];
    for raw in ["Fix it = about", "Fix it =foo"] {
        cases.push((raw, raw.find('=').expect("equals") + 1));
    }
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?} at {cursor}");
        assert_eq!(rewrite.text, raw, "{raw:?} at {cursor}");
    }

    let raw = "Fix it =";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);

    let raw = "Task {0} %t\n\nFix it =";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
}

#[test]
fn rewrite_draft_expands_dot_route_alias() {
    for (raw, expected) in [
        ("Buy milk .", "Buy milk @"),
        ("Buy milk\t.", "Buy milk\t@"),
        ("Buy\n- buy eggs .", "Buy\n- buy eggs @"),
        ("Fix it &dev:build .", "Fix it &dev:build @"),
        ("Buy milk #fix .", "Buy milk #fix @"),
        ("@a+x > .", "@a+x > @"),
        ("Buy milk < .", "Buy milk < @"),
        ("Fix it `code` .", "Fix it `code` @"),
        ("Buy milk\n\nFix it .", "Buy milk\n\nFix it @"),
        ("Café run .", "Café run @"),
        ("@@sase Buy .", "@@sase Buy @"),
    ] {
        let cursor = raw.len();
        assert!(raw.is_char_boundary(cursor), "{raw:?}");
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed @ for ."),
            "{raw:?}"
        );
        assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].start, cursor - 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].end, cursor, "{raw:?}");
        assert_eq!(rewrite.edits[0].replacement, "@", "{raw:?}");
        assert!(raw.is_char_boundary(rewrite.edits[0].start), "{raw:?}");

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_dot_route_alias_stays_literal() {
    let mut cases = vec![
        (".", 1),
        ("  .", 3),
        ("- .", 3),
        ("Buy milk\n.", "Buy milk\n.".len()),
        ("Buy\n  - foo .", "Buy\n  - foo .".len()),
        (
            "@cash^goog-exit@\n- [ ] .",
            "@cash^goog-exit@\n- [ ] .".len(),
        ),
        ("Buy milk.", "Buy milk.".len()),
        ("Buy milk @.", "Buy milk @.".len()),
        ("x .", 3),
        ("=deep .", 7),
        ("+5 .", 4),
        ("x log work .", 12),
        ("run `cargo .", 12),
        ("see [[foo .", 11),
        ("Fix ((foo .", 11),
    ];
    for raw in ["Buy . milk", "Buy .foo"] {
        cases.push((raw, raw.find('.').expect("dot") + 1));
    }
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?} at {cursor}");
        assert_eq!(rewrite.text, raw, "{raw:?} at {cursor}");
    }

    let raw = "Buy milk .";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);

    let raw = "Task {0} %t\n\nBuy milk .";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
}

/// Assert one `expand_sigil_alias` rewrite: rule, text, cursor, the single
/// one-byte edit at the typed key, the summary, and that rewriting the output
/// is a no-op.
fn assert_modifier_alias(raw: &str, expected: &str, key: &str, sigil: &str) {
    let cursor = raw.len();
    assert!(raw.is_char_boundary(cursor), "{raw:?}");
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_alias_applied(raw, &rewrite);
    assert_eq!(rewrite.text, expected, "{raw:?}");
    assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
    assert_eq!(
        rewrite.summary.as_deref(),
        Some(format!("Typed {sigil} for {key}").as_str()),
        "{raw:?}"
    );
    assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
    assert_eq!(rewrite.edits[0].start, cursor - 1, "{raw:?}");
    assert_eq!(rewrite.edits[0].end, cursor, "{raw:?}");
    assert_eq!(rewrite.edits[0].replacement, sigil, "{raw:?}");
    assert!(raw.is_char_boundary(rewrite.edits[0].start), "{raw:?}");

    let again = rewrite_draft(&rewrite.text, rewrite.cursor);
    assert_eq!(again.rule, None, "idempotent {raw:?}");
    assert_eq!(again.text, expected, "idempotent {raw:?}");
}

#[test]
fn rewrite_draft_expands_dot_dependency_alias() {
    for (raw, expected) in [
        ("@home .", "@home &"),
        ("@home\t.", "@home\t&"),
        ("@sase Buy milk .", "@sase Buy milk &"),
        ("@sase Buy #fix .", "@sase Buy #fix &"),
        ("@sase Buy &foo:bar .", "@sase Buy &foo:bar &"),
        ("@sase:abc .", "@sase:abc &"),
        ("@home Caf\u{e9} .", "@home Caf\u{e9} &"),
        ("@sase Buy\n\n@home Fix .", "@sase Buy\n\n@home Fix &"),
    ] {
        assert_modifier_alias(raw, expected, ".", "&");
    }
}

#[test]
fn rewrite_draft_dot_prefers_dependency_once_item_is_routed() {
    for (raw, expected) in [
        ("Buy milk @foo:bar .", "Buy milk @foo:bar &"),
        ("Buy milk @foo^bar .", "Buy milk @foo^bar &"),
        ("Buy milk @foo+bar .", "Buy milk @foo+bar &"),
        ("Buy milk @foo:bar\t.", "Buy milk @foo:bar\t&"),
        ("Buy milk @foo:bar@ .", "Buy milk @foo:bar@ &"),
        ("Buy milk @foo:bar#DEEP .", "Buy milk @foo:bar#DEEP &"),
        ("Buy milk @work+a @dev:b .", "Buy milk @work+a @dev:b &"),
        ("Buy milk @foo:bar &x:y .", "Buy milk @foo:bar &x:y &"),
        ("Buy milk @foo:bar #type .", "Buy milk @foo:bar #type &"),
        ("- [ ] Task @work:a .", "- [ ] Task @work:a &"),
        (
            "Buy milk @foo:bar\n- detail .",
            "Buy milk @foo:bar\n- detail &",
        ),
        (
            "Buy milk @foo:bar\n- detail\n  - nested .",
            "Buy milk @foo:bar\n- detail\n  - nested &",
        ),
        (
            "First\n\nBuy milk @foo:bar .",
            "First\n\nBuy milk @foo:bar &",
        ),
        (
            "caf\u{e9} \u{1f680} @Cash:Goog-Exit .",
            "caf\u{e9} \u{1f680} @Cash:Goog-Exit &",
        ),
        ("Buy milk @foo .", "Buy milk @foo &"),
        ("Buy milk @foo#sec .", "Buy milk @foo#sec &"),
    ] {
        assert_modifier_alias(raw, expected, ".", "&");
    }
}

#[test]
fn rewrite_draft_dot_keeps_route_alias_when_item_is_not_routed_or_and_cannot_open(
) {
    for (raw, expected) in [
        ("Buy milk .", "Buy milk @"),
        ("@@sase Buy .", "@@sase Buy @"),
        (
            "Buy milk @foo:bar some text .",
            "Buy milk @foo:bar some text @",
        ),
        (
            "Buy milk @foo:bar\n\nFix it .",
            "Buy milk @foo:bar\n\nFix it @",
        ),
        ("@a+x > .", "@a+x > @"),
        ("@a+x > @c+tid .", "@a+x > @c+tid @"),
        ("Create task < @cash:line .", "Create task < @cash:line @"),
        ("Create task < .", "Create task < @"),
    ] {
        assert_modifier_alias(raw, expected, ".", "@");
    }
}

#[test]
fn rewrite_draft_expands_semicolon_blocked_task_alias() {
    for (raw, expected) in [
        ("Buy milk ;", "Buy milk *"),
        ("Buy milk\t;", "Buy milk\t*"),
        ("@home ;", "@home *"),
        ("@sase Buy #fix ;", "@sase Buy #fix *"),
        ("Buy &foo:bar ;", "Buy &foo:bar *"),
        ("Buy milk *foo:bar ;", "Buy milk *foo:bar *"),
        ("Buy milk\n- note ;", "Buy milk\n- note *"),
        ("- [ ] Buy ;", "- [ ] Buy *"),
        ("Buy milk 2026-11-10 ;", "Buy milk 2026-11-10 *"),
        ("^sase:abc ;", "^sase:abc *"),
        ("Buy milk\n\nFix it ;", "Buy milk\n\nFix it *"),
        ("Caf\u{e9} run ;", "Caf\u{e9} run *"),
    ] {
        assert_modifier_alias(raw, expected, ";", "*");
    }
}

#[test]
fn rewrite_draft_modifier_aliases_keep_existing_alias_precedence() {
    for (raw, expected, key, sigil) in [
        ("Buy milk .", "Buy milk @", ".", "@"),
        ("Buy &foo:bar .", "Buy &foo:bar @", ".", "@"),
        ("@errands.", "@errands^", ".", "^"),
        ("@cash^goog-exit@\n- .", "@cash^goog-exit@\n- ^", ".", "^"),
        (";", ":", ";", ":"),
        ("  ;", "  :", ";", ":"),
        ("@errands;", "@errands:", ";", ":"),
        ("@cash:goog-exit@\n- ;", "@cash:goog-exit@\n- :", ";", ":"),
    ] {
        assert_modifier_alias(raw, expected, key, sigil);
    }
}

#[test]
fn rewrite_draft_modifier_aliases_stay_literal() {
    let mut cases = vec![
        ("- ;", 3),
        ("- [ ] ;", 7),
        ("Buy milk\n;", "Buy milk\n;".len()),
        ("Buy milk;", "Buy milk;".len()),
        ("Buy\n  - foo ;", "Buy\n  - foo ;".len()),
        ("@cash\n- ;", "@cash\n- ;".len()),
        ("x ;", 3),
        ("=deep ;", 7),
        ("+5 ;", 4),
        ("x log work ;", 12),
        ("@a+x > @db ;", 12),
        ("run `cargo ;", 12),
        ("see [[foo ;", 11),
        ("Fix ((foo ;", 11),
        ("@sase Fix `code .", 17),
        ("@sase see [[foo .", 17),
        ("@cash\n- .", "@cash\n- .".len()),
    ];
    for raw in ["Buy ; milk", "Buy ;foo"] {
        cases.push((raw, raw.find(';').expect("semicolon") + 1));
    }
    for raw in ["@sase Buy . milk", "@sase Buy .foo"] {
        cases.push((raw, raw.find('.').expect("dot") + 1));
    }
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?} at {cursor}");
        assert_eq!(rewrite.text, raw, "{raw:?} at {cursor}");
    }

    for raw in ["Buy milk ;", "@sase Buy milk ."] {
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None, "no cursor {raw:?}");
        assert_eq!(rewrite.text, raw, "no cursor {raw:?}");
    }

    for raw in ["Task {0} %t\n\nBuy milk ;", "Task {0} %t\n\n@sase Buy ."] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "template {raw:?}");
        assert_eq!(rewrite.text, raw, "template {raw:?}");
    }
}

#[test]
fn rewrite_draft_expands_comma_to_parent_task_picker() {
    for (raw, expected) in [
        ("@cash,", "@cash+"),
        ("Buy milk @cash,", "Buy milk @cash+"),
        ("- Call mom @home,", "- Call mom @home+"),
        ("@a+x @b,", "@a+x @b+"),
        ("@a+x > @c,", "@a+x > @c+"),
        (",", "+"),
        ("Buy milk ,", "Buy milk +"),
        ("@cash+goog-exit ,", "@cash+goog-exit +"),
        ("- Draft ,", "- Draft +"),
        ("x ,", "x +"),
        ("@cash+goog-exit,", "@cash+goog-exit +"),
        ("Buy milk @cash+goog-exit,", "Buy milk @cash+goog-exit +"),
        ("@a+x! @b+y,", "@a+x! @b+y +"),
        (
            "Parent\n- detail @cash+goog-exit,",
            "Parent\n- detail @cash+goog-exit +",
        ),
        ("café @cash+goog-exit,", "café @cash+goog-exit +"),
    ] {
        let cursor = raw.len();
        assert!(raw.is_char_boundary(cursor), "{raw:?}");
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(expected.len()), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed + for ,"),
            "{raw:?}"
        );
        assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].start, raw.len() - 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].end, raw.len(), "{raw:?}");
        assert_eq!(
            rewrite.edits[0].replacement,
            if expected.len() == raw.len() + 1 {
                " +"
            } else {
                "+"
            },
            "{raw:?}"
        );
        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_comma_does_not_switch_in_ineligible_block_id_positions() {
    for raw in [
        "@file:id@,",
        "@file^id@,",
        "@file:id#name,",
        "@file:id=3,",
        "@@file:id,",
        "@!file:id,",
        "^file:id,",
        "&note:id,",
        "@a+x > @c+tid,",
        "@dev\n- Task :task-id,",
        "@cash^p@\n- Task ^task-id,",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_ne!(
            rewrite.rule,
            Some(RewriteRule::SwitchBlockIdSeparator),
            "{raw:?}"
        );
    }

    let raw = "Do work @file:id,";
    let no_cursor = rewrite_draft(raw, None);
    assert_eq!(no_cursor.rule, None);
    assert_eq!(no_cursor.text, raw);

    let template = "Task {0} %t\n\nDo work @file:id,";
    let template_rewrite = rewrite_draft(template, Some(template.len()));
    assert_eq!(template_rewrite.rule, None);
    assert_eq!(template_rewrite.text, template);

    let spaced = "Do work @file:id ,";
    let spaced_rewrite = rewrite_draft(spaced, Some(spaced.len()));
    assert_ne!(
        spaced_rewrite.rule,
        Some(RewriteRule::SwitchBlockIdSeparator)
    );
}

#[test]
fn rewrite_draft_keeps_comma_literal_outside_parent_task_picker_positions() {
    for raw in [
        "Buy milk,",
        "1,000",
        "x1,",
        "x!1,",
        "=~1,",
        "x=3=1,",
        "=bugs,",
        "=bugs,fix",
        "@cash+goog-exit#req,",
        "@a+x > @c+tid,",
        "@a+x, > @c",
        "@,",
        "&note:id,",
        ":query,",
        "^route,",
        "x1, work log",
        "@a+x,note",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.find(',').unwrap() + 1));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw, "{raw:?}");
    }

    let raw = "@cash,";
    let no_cursor = rewrite_draft(raw, None);
    assert_eq!(no_cursor.rule, None);
    assert_eq!(no_cursor.text, raw);

    let template = "Task {0} %t\n\n@cash,";
    let template_rewrite = rewrite_draft(template, Some(template.len()));
    assert_eq!(template_rewrite.rule, None);
    assert_eq!(template_rewrite.text, template);
}

#[test]
fn rewrite_draft_sigil_aliases_exclude_other_contexts() {
    for raw in [
        "Buy milk.",
        "foo@bar.",
        "&note;",
        "^r;",
        "@r^id;",
        "@r.foo",
        "x\n- 1 work;",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw, "{raw:?}");
    }

    // `#sec^id` is valid block-ID grammar on every route, so a glued `.`
    // after a section now aliases to `^` there.
    {
        let raw = "@r#Sec.";
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::ExpandSigilAlias),
            "{raw:?}"
        );
        assert_eq!(rewrite.text, "@r#Sec^", "{raw:?}");
    }

    let raw = "@r.";
    let no_cursor = rewrite_draft(raw, None);
    assert_eq!(no_cursor.rule, None);
    assert_eq!(no_cursor.text, raw);

    let mid_token = "@r.foo";
    let mid_token_rewrite = rewrite_draft(mid_token, Some(3));
    assert_eq!(mid_token_rewrite.rule, None);
    assert_eq!(mid_token_rewrite.text, mid_token);

    let template = "Task {0} %t\n\n@r.";
    let template_rewrite = rewrite_draft(template, Some(template.len()));
    assert_eq!(template_rewrite.rule, None);
    assert_eq!(template_rewrite.text, template);
}

#[test]
fn rewrite_draft_expands_move_bullet_sigil_aliases() {
    for (raw, expected) in [
        ("@cash^goog-exit@\n- .", "@cash^goog-exit@\n- ^"),
        ("@cash^goog-exit@\n- ;", "@cash^goog-exit@\n- :"),
        ("@cash:goog-exit@\n- .", "@cash:goog-exit@\n- ^"),
        ("@cash:goog-exit@\n- ;", "@cash:goog-exit@\n- :"),
        ("@cash^goog-exit@\n* .", "@cash^goog-exit@\n* ^"),
        ("@cash^goog-exit@\n+ ;", "@cash^goog-exit@\n+ :"),
        (
            "Create\n- detail @cash^goog-exit@\n- .",
            "Create\n- detail @cash^goog-exit@\n- ^",
        ),
        (
            "caf\u{e9} @cash^goog-exit@\n- .",
            "caf\u{e9} @cash^goog-exit@\n- ^",
        ),
    ] {
        let cursor = raw.len();
        assert!(raw.is_char_boundary(cursor));
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].start, cursor - 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].end, cursor, "{raw:?}");
        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
    // Summaries match the route-position aliases.
    let dot = rewrite_draft(
        "@cash^goog-exit@\n- .",
        Some("@cash^goog-exit@\n- .".len()),
    );
    assert_eq!(dot.summary.as_deref(), Some("Typed ^ for ."));
    let semi = rewrite_draft(
        "@cash:goog-exit@\n- ;",
        Some("@cash:goog-exit@\n- ;".len()),
    );
    assert_eq!(semi.summary.as_deref(), Some("Typed : for ;"));
}

#[test]
fn rewrite_draft_expands_move_choice_dot_alias() {
    for (raw, expected) in [
        ("@cash^goog-exit@\n- ^.", "@cash^goog-exit@\n- ^^"),
        ("@cash:goog-exit@\n- ^.", "@cash:goog-exit@\n- ^^"),
    ] {
        let cursor = raw.len();
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(rewrite.summary.as_deref(), Some("Typed ^ for ."));
        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
    }
    for raw in [
        "@cash^goog-exit@\n- ^;",
        "@cash^goog-exit@\n- :.",
        "@cash^goog-exit@\n  - ^.",
        "@cash^goog-exit@\n- ^^.",
        "@cash^goog-exit@\n- [ ] ^.",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw, "{raw:?}");
    }
}

#[test]
fn rewrite_draft_leaves_non_move_bullet_dots_literal() {
    for raw in [
        "@cash^goog-exit@\n  - .",
        "@cash^goog-exit@\n- [ ] .",
        "@cash^goog-exit@\n- \\.",
        "@cash^goog-exit@\n- .bashrc",
        "@cash^goog-exit@\n- . foo",
        "@cash\n- .",
        "@cash+id\n- .",
        "Create @cash^x\n- .",
        "@cash:goog-exit@\n  - ;",
        "@cash:goog-exit@\n- [ ] ;",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw, "{raw:?}");
    }
}

#[test]
fn rewrite_draft_separator_toggle_parses_as_the_switched_mode() {
    let caret = switch_at("Do work @file:id^");
    let parsed = parse_for_editor(&caret.text);
    assert_eq!(parsed.mode, EditorMode::Task);
    assert_eq!(parsed.block_id.as_deref(), Some("id"));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let sub_bullet = switch_at("Do work @file:id+");
    let parsed = parse_for_editor(&sub_bullet.text);
    assert_eq!(parsed.mode, EditorMode::SubBullet);
    assert_eq!(parsed.block_id.as_deref(), Some("id"));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let task_toggle = switch_at("@file:id+");
    let parsed = parse_for_editor(&task_toggle.text);
    assert_eq!(parsed.mode, EditorMode::TaskToggle);
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

#[test]
fn rewrite_draft_separator_toggle_leaves_type_markers_intact() {
    let raw = "Do work @file:id^ #fix";
    let cursor = "Do work @file:id^".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "Do work @file^id #fix");
    assert_eq!(rewrite.cursor, Some("Do work @file^id".len()));
    // The toggled draft still parses with the marker as a type span.
    let parsed = parse_for_editor(&rewrite.text);
    assert_eq!(parsed.body, "Do work");
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    // A cursor at the end of an incomplete marker is not a separator.
    let raw = "Fix it #\"";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.text, raw);
}

#[test]
fn rewrite_draft_switches_a_token_that_becomes_a_run_member() {
    // Each token, once switched, joins a target run: the switch is allowed
    // even though the stripped form is not the line's single marker. The
    // cursor sits right after the trigger being switched, which is at the
    // end of the line for trailing tokens and mid-line for leading ones.
    let trailing = "@work+a @dev:b+";
    let rewrite = switch_at(trailing);
    assert_eq!(rewrite.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(rewrite.text, "@work+a @dev+b");
    assert_eq!(rewrite.cursor, Some("@work+a @dev+b".len()));
    assert_eq!(parse_for_editor(&rewrite.text).targets.len(), 2);

    for (raw, cursor, expected, expected_cursor) in [
        (
            "note @work:a+ @dev+b",
            "note @work:a+".len(),
            "note @work+a @dev+b",
            "note @work+a".len(),
        ),
        (
            "@work:a+ @dev+b",
            "@work:a+".len(),
            "@work+a @dev+b",
            "@work+a".len(),
        ),
    ] {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::SwitchBlockIdSeparator),
            "{raw}"
        );
        assert_eq!(rewrite.text, expected, "{raw}");
        assert_eq!(rewrite.cursor, Some(expected_cursor), "{raw}");
        let parsed = parse_for_editor(&rewrite.text);
        assert_eq!(parsed.targets.len(), 2, "{raw}: {parsed:?}");
    }
}

#[test]
fn rewrite_draft_reports_a_run_for_bare_at_at() {
    // A target run shares one route slot, so `@@` gets the existing
    // non-absorbable notice instead of Rule A6's silent no-op.
    let raw = "@work+a @dev+b @@";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
    assert_eq!(rewrite.notices.len(), 1, "{:?}", rewrite.notices);
    assert!(
        rewrite.notices[0].contains("cannot take an existing task destination"),
        "{}",
        rewrite.notices[0]
    );
}

#[test]
fn rewrite_draft_switches_transfer_source_colon_to_plus() {
    // Typing `+` after a colon source on a transfer line switches to the
    // transfer spelling.
    let raw = "@inbox:bench+ > @db";
    let rewrite = rewrite_draft(raw, Some(13));
    assert_eq!(
        rewrite.rule,
        Some(RewriteRule::SwitchBlockIdSeparator),
        "{raw}"
    );
    assert_eq!(rewrite.text, "@inbox+bench > @db");
    assert_eq!(rewrite.cursor, Some(12));
    assert_eq!(
        rewrite.summary.as_deref(),
        Some("Changed @inbox:bench to @inbox+bench")
    );
    assert_eq!(
        rewrite.edits,
        vec![TextEdit {
            start: 0,
            end: 13,
            replacement: "@inbox+bench".to_string(),
        }]
    );
    let again = rewrite_draft(&rewrite.text, rewrite.cursor);
    assert_eq!(again.rule, None, "idempotent");
}

#[test]
fn rewrite_draft_switches_transfer_destination_caret_to_plus() {
    // Typing `+` after a caret destination switches to a merge target.
    let raw = "@a+x > @c:tid+";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(
        rewrite.rule,
        Some(RewriteRule::SwitchBlockIdSeparator),
        "{raw}"
    );
    assert_eq!(rewrite.text, "@a+x > @c+tid");
    assert_eq!(rewrite.summary.as_deref(), Some("Changed @c:tid to @c+tid"));
}

#[test]
fn rewrite_draft_leaves_transfer_plus_triggers_for_other_marks_literal() {
    // `:` after a transfer `+id` would break the transfer, so it stays
    // literal; prose `+` triggers never switch either.
    let raw = "@a+x > @c+tid:";
    assert_eq!(rewrite_draft(raw, Some(raw.len())).rule, None);
    let source = "@inbox+bench > @c+tid:";
    assert_eq!(rewrite_draft(source, Some(source.len())).rule, None);
}

#[test]
fn rewrite_draft_switches_and_continues_merge_in_sources() {
    let raw = "Create task < @cash:line+";
    let switched = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(switched.rule, Some(RewriteRule::SwitchBlockIdSeparator));
    assert_eq!(switched.text, "Create task < @cash+line");

    for raw in ["Create task < @cash+line:", "Create task < @cash+line^"] {
        assert_eq!(rewrite_draft(raw, Some(raw.len())).rule, None, "{raw}");
    }

    let raw = "Create task < @cash+line,";
    let continued = rewrite_draft(raw, Some(raw.len()));
    assert_alias_applied(raw, &continued);
    assert_eq!(continued.text, "Create task < @cash+line +");
    let cursor = continued.cursor.expect("next picker caret");
    let field =
        super::super::completion::completion_field_at(&continued.text, cursor)
            .expect("vault-wide next source picker");
    assert_eq!(field.context, super::super::CompletionContext::TaskParent);
}

#[test]
fn rewrite_draft_expands_backslash_hash_alias() {
    for (raw, expected) in [
        ("Buy milk \\", "Buy milk #"),
        ("foo #a \\", "foo #a #"),
        ("@cash\\", "@cash#"),
        ("Buy milk @cash\\", "Buy milk @cash#"),
        ("a > @cash\\", "a > @cash#"),
        ("Buy milk @cash:goog-exit\\", "Buy milk @cash:goog-exit#"),
        ("Buy milk @cash^newid@\\", "Buy milk @cash^newid@#"),
        ("^cash:goog-exit\\", "^cash:goog-exit#"),
        ("Buy milk @cash+goog-exit\\", "Buy milk @cash+goog-exit#"),
        ("@cash^goog-exit@\n- \\", "@cash^goog-exit@\n- #"),
        ("@cash:goog-exit@\n- \\", "@cash:goog-exit@\n- #"),
        ("Buy milk\n- \\", "Buy milk\n- #"),
        ("@cash^goog-exit@\n  - \\", "@cash^goog-exit@\n  - #"),
        ("=\\", "=#"),
        ("=25\\", "=25#"),
        ("x=25\\", "x=25#"),
        ("=/\\", "=/#"),
        ("=*fix\\", "=*fix#"),
        ("[[cash\\", "[[cash#"),
        ("[[\\", "[[#"),
    ] {
        let cursor = raw.len();
        assert!(raw.is_char_boundary(cursor), "{raw:?}");
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed # for \\"),
            "{raw:?}"
        );
        assert_eq!(rewrite.notices, Vec::<String>::new(), "{raw:?}");
        assert_eq!(
            rewrite.edits,
            vec![TextEdit {
                start: cursor - 1,
                end: cursor,
                replacement: "#".to_string(),
            }],
            "{raw:?}"
        );

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }

    let raw = "@cash+goog-exit\\ Buy milk";
    let cursor = "@cash+goog-exit\\".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_alias_applied(raw, &rewrite);
    assert_eq!(rewrite.text, "@cash+goog-exit# Buy milk");
    assert_eq!(rewrite.cursor, Some(cursor));
    assert_eq!(rewrite.summary.as_deref(), Some("Typed # for \\"));
    assert_eq!(
        rewrite.edits,
        vec![TextEdit {
            start: cursor - 1,
            end: cursor,
            replacement: "#".to_string(),
        }]
    );
    let again = rewrite_draft(&rewrite.text, Some(cursor));
    assert_eq!(again.rule, None);
    assert_eq!(again.text, "@cash+goog-exit# Buy milk");

    let raw = "caf\u{e9} \\";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_alias_applied(raw, &rewrite);
    assert_eq!(rewrite.text, "caf\u{e9} #");
    assert!(raw.is_char_boundary(rewrite.edits[0].start));
    assert_eq!(rewrite.edits[0].start, raw.len() - 1);
    let again = rewrite_draft(&rewrite.text, rewrite.cursor);
    assert_eq!(again.rule, None);
}

#[test]
fn rewrite_draft_leaves_backslash_hash_alias_literal() {
    for raw in [
        "\\",
        "  \\",
        "Buy milk\n\\",
        "Buy milk\n\n\\",
        "dir\\",
        "C:\\",
        "C:\\Users\\",
        "foo \\\\",
        "@cash\\\\",
        "- \\\\",
        "Buy milk\n- \\\\",
        "x foo \\",
        "x\n- 1 \\",
        "foo |\\",
        "foo ~cash:goog-exit\\",
        ":\\",
        "Buy milk @cash:goog-exit#a\\",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw, "{raw:?}");
    }

    let raw = "\\ foo";
    let rewrite = rewrite_draft(raw, Some(1));
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);

    let raw = "foo \\alpha";
    let cursor = "foo \\".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, None, "{raw:?}@{cursor}");
    assert_eq!(rewrite.text, raw);

    let raw = "foo \\ bar";
    let cursor = "foo \\".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, None, "{raw:?}@{cursor}");
    assert_eq!(rewrite.text, raw);

    let raw = "Buy milk \\";
    let no_cursor = rewrite_draft(raw, None);
    assert_eq!(no_cursor.rule, None);
    assert_eq!(no_cursor.text, raw);

    let template = "Do work @file:{0}^ %t\n\nrow\\";
    let template_rewrite = rewrite_draft(template, Some(template.len()));
    assert_eq!(template_rewrite.rule, None);
    assert_eq!(template_rewrite.text, template);
}

#[test]
fn rewrite_draft_expands_slash_hash_alias() {
    for (raw, expected) in [
        ("Buy milk /", "Buy milk #"),
        ("foo #a /", "foo #a #"),
        ("caf\u{e9} /", "caf\u{e9} #"),
        ("@cash+goog-exit /", "@cash+goog-exit #"),
        ("Buy milk\n  - /", "Buy milk\n  - #"),
        ("Buy milk\n- /", "Buy milk\n- #"),
        ("@cash^goog-exit@\n- /", "@cash^goog-exit@\n- #"),
        ("@cash:goog-exit@\n- /", "@cash:goog-exit@\n- #"),
        ("@cash/", "@cash#"),
        ("Buy milk @cash/", "Buy milk @cash#"),
        ("a > @cash/", "a > @cash#"),
        ("@cash+goog-exit/", "@cash+goog-exit#"),
        ("Buy milk @cash+goog-exit!/", "Buy milk @cash+goog-exit!#"),
        ("@cash+/", "@cash+#"),
        ("Buy milk @cash:goog-exit/", "Buy milk @cash:goog-exit#"),
        ("Buy milk @cash:/", "Buy milk @cash:#"),
        ("Buy milk @cash^newid@/", "Buy milk @cash^newid@#"),
        ("Buy milk @cash^goog-exit/", "Buy milk @cash^goog-exit#"),
        ("^cash:goog-exit/", "^cash:goog-exit#"),
        ("=25/", "=25#"),
        ("=25-30/", "=25-30#"),
        ("x=25/", "x=25#"),
        ("x=/", "x=#"),
        ("=//", "=/#"),
        ("=*/", "=*#"),
    ] {
        let cursor = raw.len();
        assert!(raw.is_char_boundary(cursor), "{raw:?}");
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed # for /"),
            "{raw:?}"
        );
        assert_eq!(rewrite.notices, Vec::<String>::new(), "{raw:?}");
        assert_eq!(
            rewrite.edits,
            vec![TextEdit {
                start: cursor - 1,
                end: cursor,
                replacement: "#".to_string(),
            }],
            "{raw:?}"
        );
        assert!(raw.is_char_boundary(rewrite.edits[0].start), "{raw:?}");

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_leaves_slash_hash_alias_literal() {
    for raw in [
        "/",
        "  /",
        "Buy milk\n/",
        "Buy milk\n\n/",
        "=/",
        "=/fix/",
        "=*fix/",
        "[[cash/",
        "[[/",
        "[[folder/",
        "@cash#Q&A/",
        "Buy milk @cash#/",
        ":/",
        "!/",
        "((foo/",
        "dir/",
        "w/",
        "and/",
        "see /etc/",
        "Buy milk >11/",
        "foo //",
        "@cash//",
        "- //",
        "foo #/",
        "foo \\/",
        "x foo /",
        "x\n- 1 /",
        "foo |/",
        "foo ~cash:goog-exit/",
        "^/",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw, "{raw:?}");
    }

    let raw = "/ foo";
    let rewrite = rewrite_draft(raw, Some(1));
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);

    let raw = "foo /alpha";
    let cursor = "foo /".len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_eq!(rewrite.rule, None, "{raw:?}@{cursor}");
    assert_eq!(rewrite.text, raw);

    let raw = "Buy milk /";
    let no_cursor = rewrite_draft(raw, None);
    assert_eq!(no_cursor.rule, None);
    assert_eq!(no_cursor.text, raw);

    let template = "Do work @file:{0}^ %t\n\nrow/";
    let template_rewrite = rewrite_draft(template, Some(template.len()));
    assert_eq!(template_rewrite.rule, None);
    assert_eq!(template_rewrite.text, template);
}

#[test]
fn rewrite_draft_collapses_glued_trigger_after_complete_global_route() {
    for (raw, cursor, expected) in [
        ("@@cash#", 7, "@cash#"),
        ("@@cash/", 7, "@cash#"),
        ("@@cash\\", 7, "@cash#"),
        ("@@cash^", 7, "@cash^"),
        ("@@cash.", 7, "@cash^"),
        ("@@cash+", 7, "@cash+"),
        ("@@cash,", 7, "@cash+"),
        ("@@r.", 4, "@r^"),
        ("@@my_note#", 10, "@my_note#"),
        ("@@my-note,", 10, "@my-note+"),
        ("Buy milk @@cash/", 16, "Buy milk @cash#"),
        ("@@cash/ pick up milk", 7, "@cash# pick up milk"),
        ("caf\u{e9} @@cash/", 13, "caf\u{e9} @cash#"),
        ("@@cash/ caf\u{e9}", 7, "@cash# caf\u{e9}"),
    ] {
        assert!(raw.is_char_boundary(cursor), "{raw:?}");
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::CollapseGlobalDestination),
            "{raw:?}"
        );
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.alias, None, "{raw:?}");
        // Single native edit; applying it yields the returned text and the
        // cursor maps through it.
        assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
        let edit = &rewrite.edits[0];
        assert_eq!(
            &raw[edit.start..edit.end],
            &raw[edit.start..cursor],
            "{raw:?}"
        );
        assert_eq!(edit.end, cursor, "{raw:?}");
        assert_eq!(
            rewrite.cursor,
            Some(edit.start + edit.replacement.len()),
            "{raw:?}"
        );
        let applied = {
            let mut out = String::new();
            out.push_str(&raw[..edit.start]);
            out.push_str(&edit.replacement);
            out.push_str(&raw[edit.end..]);
            out
        };
        assert_eq!(applied, expected, "{raw:?}");
        assert_eq!(applied, rewrite.text, "{raw:?}");
        // Undo restores the exact typed input.
        let mut undone = String::new();
        undone.push_str(&rewrite.text[..edit.start]);
        undone.push_str(&raw[edit.start..edit.end]);
        undone.push_str(&rewrite.text[edit.start + edit.replacement.len()..]);
        assert_eq!(undone, raw, "{raw:?}");
        // Idempotent: the collapsed form does not collapse again.
        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.text, rewrite.text, "idempotent {raw:?}");
    }
    // No-cursor calls retain strict grammar and never collapse.
    for raw in ["@@cash/", "@@cash#", "@@cash,"] {
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None, "{raw:?} no-cursor");
        assert_eq!(rewrite.text, raw, "{raw:?} no-cursor");
    }
    // Non-firing cases stay literal.
    for (raw, cursor) in [
        ("@@", 2),
        ("@@ ", 2),
        ("@@cash", 6),
        ("@@cash #", 7),
        ("@@cash/extra", 7),
        ("@@cash#sec", 7),
        ("foo@@cash/", 10),
        ("\\@@cash/", 8),
        ("[[@@cash/", 8),
        ("`@@cash/`", 8),
        ("\"@@cash/\"", 8),
    ] {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?}@{cursor}");
        assert_eq!(rewrite.text, raw, "{raw:?}@{cursor}");
    }
    // Trigger earlier than the caret never collapses.
    let raw = "@@cash/ ";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, None, "{raw:?} trailing space");
    assert_eq!(rewrite.text, raw);
    // Template drafts never collapse.
    let template = "Task {0} %t\n\n@@cash/";
    let template_rewrite = rewrite_draft(template, Some(template.len()));
    assert_eq!(template_rewrite.rule, None);
    assert_eq!(template_rewrite.text, template);
    // Bare `@@` absorption still works when no collapse trigger is present.
    let bare = "Buy milk @dev @@";
    let absorbed = rewrite_draft(bare, Some(bare.len()));
    assert_eq!(absorbed.rule, Some(RewriteRule::AbsorbLocalMarker));
}

#[test]
fn rewrite_draft_keeps_backslash_hash_alias_where_slash_stays_literal() {
    for (raw, expected) in [
        ("=\\", "=#"),
        ("[[cash\\", "[[cash#"),
        ("=/fix\\", "=/fix#"),
        ("=*fix\\", "=*fix#"),
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed # for \\"),
            "{raw:?}"
        );
    }
}

#[test]
fn rewrite_draft_reports_transfer_for_bare_at_at() {
    // A bare `@@` on a transfer line is refused: the destination is not
    // a declaration payload.
    for raw in ["@a+x > @c @@", "@a+x > @c+tid @@"] {
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None, "{raw}");
        assert_eq!(rewrite.text, raw, "{raw}");
        assert_eq!(rewrite.notices.len(), 1, "{raw}: {:?}", rewrite.notices);
        assert!(
            rewrite.notices[0].contains("@@ cannot take a capture transfer"),
            "{raw}: {}",
            rewrite.notices[0]
        );
    }
}

#[test]
fn rewrite_draft_expands_relation_flip_alias() {
    for (raw, expected) in [
        ("Fix it ~=", "Fix it ~~"),
        ("Fix it\t~=", "Fix it\t~~"),
        ("@bob+fix-retries ~=", "@bob+fix-retries ~~"),
        ("Fix it &dev:build ~=", "Fix it &dev:build ~~"),
        ("Buy milk\n- note ~=", "Buy milk\n- note ~~"),
        ("Fix [[note]] ~=", "Fix [[note]] ~~"),
        ("Fix it `code` ~=", "Fix it `code` ~~"),
        ("Café run ~=", "Café run ~~"),
        ("- ~=", "- ~~"),
        ("~=", "~~"),
    ] {
        let cursor = raw.len();
        assert!(raw.is_char_boundary(cursor), "{raw:?}");
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed ~ for ="),
            "{raw:?}"
        );
        assert_eq!(
            rewrite.edits,
            vec![TextEdit {
                start: cursor - 1,
                end: cursor,
                replacement: "~".to_string(),
            }],
            "{raw:?}"
        );
        assert!(raw.is_char_boundary(rewrite.edits[0].start), "{raw:?}");

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_expands_relation_reverse_flip_alias() {
    for (raw, expected) in [
        ("Fix it ~~=", "Fix it ~"),
        ("Fix it\t~~=", "Fix it\t~"),
        ("@bob+fix-retries ~~=", "@bob+fix-retries ~"),
        ("Buy milk\n- note ~~=", "Buy milk\n- note ~"),
        ("Café run ~~=", "Café run ~"),
    ] {
        let cursor = raw.len();
        assert!(raw.is_char_boundary(cursor), "{raw:?}");
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor - 2), "{raw:?}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Typed ~ for ~~="),
            "{raw:?}"
        );
        assert_eq!(
            rewrite.edits,
            vec![TextEdit {
                start: cursor - 2,
                end: cursor,
                replacement: String::new(),
            }],
            "{raw:?}"
        );
        assert!(raw.is_char_boundary(rewrite.edits[0].start), "{raw:?}");

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_toggles_complete_relation_direction() {
    for (raw, trigger, expected, summary) in [
        (
            "Fix it ~tasks:target~",
            '~',
            "Fix it ~~tasks:target",
            "Flipped to Linked From",
        ),
        (
            "Fix it ~tasks:target=",
            '=',
            "Fix it ~~tasks:target",
            "Flipped to Linked From",
        ),
        (
            "Fix it ~~tasks:source~",
            '~',
            "Fix it ~tasks:source",
            "Flipped to Links To",
        ),
        (
            "Fix it ~~tasks:source=",
            '=',
            "Fix it ~tasks:source",
            "Flipped to Links To",
        ),
        (
            "Fix it ~\"My Note\":target=",
            '=',
            "Fix it ~~\"My Note\":target",
            "Flipped to Linked From",
        ),
        (
            "Fix it ~café:target~",
            '~',
            "Fix it ~~café:target",
            "Flipped to Linked From",
        ),
        (
            "Fix it ~tasks:target|see~",
            '~',
            "Fix it ~~tasks:target|see",
            "Flipped to Linked From",
        ),
        (
            "Fix it ~~tasks:source||see~",
            '~',
            "Fix it ~tasks:source||see",
            "Flipped to Links To",
        ),
        (
            "Fix it ~tasks:target= ~tasks:other",
            '=',
            "Fix it ~~tasks:target ~tasks:other",
            "Flipped to Linked From",
        ),
    ] {
        let trigger_start = raw.rfind(trigger).expect("trigger");
        let cursor = trigger_start + trigger.len_utf8();
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::ToggleRelationDirection),
            "{raw:?}"
        );
        assert_eq!(rewrite.text, expected, "{raw:?}");
        let expected_cursor = expected
            .find(" and later")
            .or_else(|| expected.find(" ~tasks:other"))
            .unwrap_or(expected.len());
        assert_eq!(rewrite.cursor, Some(expected_cursor), "{raw:?}");
        assert_eq!(rewrite.summary.as_deref(), Some(summary), "{raw:?}");
        assert_eq!(rewrite.edits.len(), 2, "{raw:?}");
        assert!(
            rewrite
                .edits
                .windows(2)
                .all(|pair| pair[0].end <= pair[1].start),
            "{raw:?}: {:?}",
            rewrite.edits
        );
        for edit in &rewrite.edits {
            assert!(raw.is_char_boundary(edit.start), "{raw:?}");
            assert!(raw.is_char_boundary(edit.end), "{raw:?}");
        }

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_relation_direction_toggle_exclusions() {
    let glued_after = "Fix it ~tasks:target~tail";
    let glued_cursor = glued_after.find("~tail").expect("trigger") + 1;
    let cases = [
        ("Fix it ~tasks:target ~", "Fix it ~tasks:target ~".len()),
        (glued_after, glued_cursor),
        (
            "Fix it ~tasks:target|see=",
            "Fix it ~tasks:target|see=".len(),
        ),
        (r"Fix it \~tasks:target~", r"Fix it \~tasks:target~".len()),
        ("Fix `~tasks:target` ~", "Fix `~tasks:target` ~".len()),
        ("Fix [[~tasks:target]] ~", "Fix [[~tasks:target]] ~".len()),
        ("Fix it ~tasks:~", "Fix it ~tasks:~".len()),
        (
            "Fix it ~tasks:bad|||label~",
            "Fix it ~tasks:bad|||label~".len(),
        ),
        ("Fix it ~~struck~~=", "Fix it ~~struck~~=".len()),
        ("x=~~=", "x=~~=".len()),
    ];
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?} at {cursor}");
        assert_eq!(rewrite.text, raw, "{raw:?} at {cursor}");
    }

    let raw = "Fix it ~tasks:target~";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
}

#[test]
fn rewrite_draft_relation_flip_alias_stays_literal() {
    let mut cases = vec![
        ("=", 1),
        ("Fix it -", 8),
        ("Fix it -=", 9),
        ("Fix it~=", 8),
        ("x=~~=", 5),
        ("x ~=", 4),
        ("=bugs ~=", 8),
        ("+5 ~=", 5),
        ("see [[foo ~=", 12),
        ("Fix ((foo ~=", 12),
        ("Fix it ~tar=", 12),
        ("-=", 2),
        ("- -=", 4),
        ("* -=", 4),
        ("fix-=", 5),
        ("Fix it --=", 10),
        ("x -=", 4),
        ("=bugs -=", 8),
        ("see [[foo -=", 12),
        ("Fix ((foo -=", 12),
        ("run `cargo -=", 13),
    ];
    for raw in ["Fix it ~= about", "Fix it ~=about"] {
        cases.push((raw, raw.find('=').expect("equals") + 1));
    }
    cases.push(("Fix it -= about", "Fix it -=".len()));
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?} at {cursor}");
        assert_eq!(rewrite.text, raw, "{raw:?} at {cursor}");
    }

    let raw = "Fix it ~=";
    let rewrite = rewrite_draft(raw, None);
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);

    let raw = "Task {0} %t\n\nFix it ~~=";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
}

#[test]
fn rewrite_draft_expands_past_segment_dash_alias() {
    for (raw, expected) in [
        ("x=-", "x=~"),
        ("x=2-", "x=2~"),
        ("x=fix-", "x=fix~"),
        ("x=fix2-", "x=fix2~"),
        ("x=#\"DEEP WORK\"-", "x=#\"DEEP WORK\"~"),
        ("x=~-", "x=~~"),
        ("x=2~-", "x=2~~"),
    ] {
        let cursor = raw.len();
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_alias_applied(raw, &rewrite);
        assert_eq!(rewrite.text, expected, "{raw:?}");
        assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
        assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
        assert_eq!(rewrite.edits[0].replacement, "~", "{raw:?}");

        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw:?}");
        assert_eq!(again.text, expected, "idempotent {raw:?}");
    }
}

#[test]
fn rewrite_draft_past_segment_dash_stays_literal() {
    // (draft, cursor): every `-` here must not alias to `~`.
    let cases = [
        ("x=#deep-", 8),
        ("x=~2-", 5),
        ("x=2!-", 5),
        ("x=2=-", 5),
        ("x=3 -2", 5),
        ("-", 1),
        ("buy milk-", 9),
        ("=2-", 3),
    ];
    for (raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?} at {cursor} should not alias");
        assert_eq!(rewrite.text, raw, "{raw:?} at {cursor}");
    }
    // Prose and bullet lines stay literal even at the end.
    for raw in ["- buy milk", "buy milk -", "Fix it - about"] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        if rewrite.rule == Some(RewriteRule::ExpandSigilAlias)
            && rewrite.text.contains("x=")
        {
            panic!("{raw:?} must not alias to a past segment");
        }
    }
}

fn assert_close_outcome_alias(
    raw: &str,
    expected: &str,
    alias: &str,
    sigil: char,
) {
    let cursor = raw.len();
    let rewrite = rewrite_draft(raw, Some(cursor));
    assert_alias_applied(raw, &rewrite);
    assert_eq!(rewrite.text, expected, "{raw:?}");
    assert_eq!(rewrite.cursor, Some(cursor), "{raw:?}");
    assert_eq!(rewrite.edits.len(), 1, "{raw:?}");
    assert_eq!(rewrite.edits[0].start, cursor - 1, "{raw:?}");
    assert_eq!(rewrite.edits[0].end, cursor, "{raw:?}");
    assert_eq!(rewrite.edits[0].replacement, sigil.to_string(), "{raw:?}");
    assert_eq!(
        rewrite.summary.as_deref(),
        Some(format!("Typed {sigil} for {alias}").as_str()),
        "{raw:?}"
    );
    let again = rewrite_draft(&rewrite.text, rewrite.cursor);
    assert_eq!(again.rule, None, "idempotent {raw:?}");
    assert_eq!(again.text, expected, "idempotent {raw:?}");
    assert_close_parse(expected);
}

fn assert_close_parse(text: &str) {
    let parsed = parse_for_editor(text);
    let has_close = parsed.items.iter().any(|item| {
        item.pomodoro_close.is_some()
            || item.pomodoro_past.is_some()
            || item.spans.iter().any(|span| {
                matches!(
                    span.kind,
                    SpanKind::PomodoroClose
                        | SpanKind::PomodoroCloseInProgress
                        | SpanKind::PomodoroClosePark
                        | SpanKind::PomodoroCloseComplete
                        | SpanKind::PomodoroCloseDrop
                        | SpanKind::PomodoroPast
                )
            })
    });
    assert!(
        has_close,
        "{text:?} should parse as a close, link close, or past close; modes {:?}",
        parsed
            .items
            .iter()
            .map(|item| item.mode)
            .collect::<Vec<_>>()
    );
    for diagnostic in parsed
        .items
        .iter()
        .flat_map(|item| item.diagnostics.iter())
        .chain(parsed.diagnostics.iter())
    {
        if diagnostic.severity == Severity::Error {
            assert!(
                diagnostic
                    .message
                    .contains("competing park-all and complete-all"),
                "{text:?} unexpected error: {}",
                diagnostic.message
            );
        }
    }
}

#[test]
fn rewrite_draft_expands_close_outcome_aliases() {
    for (raw, expected, alias, sigil) in [
        ("xx", "x!", "x", '!'),
        ("xs", "x*", "s", '*'),
        ("x1x", "x1!", "x", '!'),
        ("x1s", "x1*", "s", '*'),
        ("x!s", "x!*", "s", '*'),
        ("x*x", "x*!", "x", '!'),
        ("x*1x", "x*1!", "x", '!'),
        ("x@1s", "x@1*", "s", '*'),
        ("x1~2s", "x1~2*", "s", '*'),
        ("x=1=s", "x=1=*", "s", '*'),
        ("x=1=x", "x=1=!", "x", '!'),
        ("x==s", "x==*", "s", '*'),
        ("x=fix=s", "x=fix=*", "s", '*'),
        ("x=1!=x", "x=1!=!", "x", '!'),
        ("x=1=1s", "x=1=1*", "s", '*'),
        ("+2 xs", "+2 x*", "s", '*'),
        ("@r:id=xx", "@r:id=x!", "x", '!'),
        ("^r:id=x1s", "^r:id=x1*", "s", '*'),
        ("Fix it @r:id=xs", "Fix it @r:id=x*", "s", '*'),
        ("Plan\n\nxx", "Plan\n\nx!", "x", '!'),
        ("2xx", "2x!", "x", '!'),
        ("2x1s", "2x1*", "s", '*'),
        ("^r:id=2xs", "^r:id=2x*", "s", '*'),
        ("--1 2xs", "--1 2x*", "s", '*'),
    ] {
        assert_close_outcome_alias(raw, expected, alias, sigil);
    }
}

#[test]
fn rewrite_draft_close_outcome_aliases_stay_literal() {
    let cases: &[(&str, usize)] = &[
        ("x=1s", 4),
        ("x=s", 3),
        ("x=xs", 4),
        ("x=1x", 4),
        ("x!x", 3),
        ("x*s", 3),
        ("x1,s", 4),
        ("x@s", 3),
        ("Xx", 2),
        ("Xs", 2),
        ("@r:id=Xs", 8),
        ("xS", 2),
        ("xX", 2),
        ("x log xs", 8),
        ("x\n- xs", 6),
        ("=xs", 3),
        ("=x", 2),
        ("Plan xs", 7),
        ("x 2xx", 5),
        ("= 2xx", 5),
        ("Plan 2xx", 8),
        ("2Xx", 3),
        ("tax", 3),
        ("session", 1),
        ("s:2", 1),
        ("x1s2", 3),
        ("[[xx]]", 4),
        ("`xx`", 3),
    ];
    for &(raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(
            rewrite.rule, None,
            "{raw:?} at {cursor} should stay literal, got {:?}",
            rewrite.text
        );
        assert_eq!(rewrite.text, raw, "{raw:?}");
    }
    let after_s = rewrite_draft("=xs", Some(3));
    assert_eq!(after_s.rule, None, "=x + s should stay literal");
    assert_eq!(after_s.text, "=xs");
}

#[test]
fn rewrite_draft_close_outcome_alias_requires_cursor() {
    for raw in ["xx", "xs", "x=1=s", "@r:id=xx", "+2 xs"] {
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None, "{raw:?} without cursor");
        assert_eq!(rewrite.text, raw, "{raw:?} without cursor");
    }
}

#[test]
fn rewrite_draft_close_outcome_alias_utf8_and_crlf() {
    let emoji = "🚀\n\nxs";
    let cursor = emoji.len();
    let rewrite = rewrite_draft(emoji, Some(cursor));
    assert_alias_applied(emoji, &rewrite);
    assert_eq!(rewrite.text, "🚀\n\nx*");
    assert_eq!(rewrite.cursor, Some(cursor));
    assert!(emoji.is_char_boundary(rewrite.edits[0].start));
    assert!(emoji.is_char_boundary(rewrite.edits[0].end));
    assert_close_parse(&rewrite.text);

    let crlf = "Plan\r\n\r\nxx";
    let rewrite = rewrite_draft(crlf, Some(crlf.len()));
    assert_alias_applied(crlf, &rewrite);
    assert_eq!(rewrite.text, "Plan\r\n\r\nx!");
    assert_close_parse(&rewrite.text);
}

#[test]
fn rewrite_draft_close_outcome_alias_leaves_templates() {
    let raw = "Do work @file:{0}^ %t\n\nxx";
    let rewrite = rewrite_draft(raw, Some(raw.len()));
    assert_eq!(rewrite.rule, None);
    assert_eq!(rewrite.text, raw);
}

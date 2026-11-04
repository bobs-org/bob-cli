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
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::ExpandSigilAlias),
            "{raw:?}"
        );
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
    assert_eq!(rewrite.rule, Some(RewriteRule::ExpandSigilAlias));
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
        assert_eq!(rewrite.rule, Some(RewriteRule::ExpandSigilAlias));
        assert_eq!(rewrite.text, "@inbox: > @db");
    }
}

#[test]
fn rewrite_draft_sigil_aliases_exclude_other_contexts() {
    for raw in [
        "Buy milk.",
        "foo@bar.",
        "@@r.",
        "@r#Sec.",
        "&note;",
        "^r;",
        "@r:id.",
        "@r.foo",
        "x\n- 1 work;",
    ] {
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, None, "{raw:?}");
        assert_eq!(rewrite.text, raw, "{raw:?}");
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
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::ExpandSigilAlias),
            "{raw:?}"
        );
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
        assert_eq!(
            rewrite.rule,
            Some(RewriteRule::ExpandSigilAlias),
            "{raw:?}"
        );
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

//! Draft rewrite tests.

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
        ("note @dev:id+ @@", "cannot take a project note"),
        ("note @dev:id+#bugs @@", "cannot take a project note"),
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

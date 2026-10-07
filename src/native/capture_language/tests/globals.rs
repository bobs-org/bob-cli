//! Global declaration tests.

use super::super::draft::*;
use super::super::editor_model::*;
use super::super::editor_parse::*;
use super::super::markers::*;
use super::super::model::*;
use super::*;

#[test]
fn split_capture_draft_strips_declaration_only_lines() {
    assert_eq!(
        draft_items("@@foo\nFirst task\n\nSecond task"),
        vec![(0, 2, "First task"), (1, 4, "Second task")]
    );
    assert_eq!(
        draft_items("@@foo\n\nFirst task\n\nSecond task"),
        vec![(0, 3, "First task"), (1, 5, "Second task")]
    );
}

#[test]
fn split_capture_draft_ignores_leading_blanks_and_crlf() {
    let raw = "\r\n\n@@foo\r\nFirst";
    let draft = split_capture_draft(raw);
    let declaration = draft.declarations[0];
    assert_eq!(declaration.token.text, "@@foo");
    assert_eq!(
        &raw[declaration.token.start..declaration.token.end],
        "@@foo"
    );
    assert_eq!(declaration.line_number, 3);
    assert_eq!(draft_items(raw), vec![(0, 4, "First")]);
}

#[test]
fn execution_inherits_a_global_task_route_unless_an_item_overrides() {
    let draft = parse_capture_draft_with_clip_control(
        "@@Foo\nFirst task\n\nSecond task @bar\n\nThird task",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    let global = draft.global.expect("global");
    assert_eq!(global.route, "foo");
    assert_eq!(global.block_id, None);
    assert_eq!(draft.items[0].parsed.body, "First task");
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("foo"));
    assert_eq!(draft.items[0].parsed.kind, CaptureKind::Task);
    assert_eq!(draft.items[1].parsed.body, "Second task");
    assert_eq!(draft.items[1].parsed.route.as_deref(), Some("bar"));
    assert_eq!(draft.items[2].parsed.body, "Third task");
    assert_eq!(draft.items[2].parsed.route.as_deref(), Some("foo"));
}

#[test]
fn execution_inherits_a_global_sub_bullet_and_keeps_authored_children() {
    let draft = parse_capture_draft_with_clip_control(
        "@@foo+a-id\nFirst note\n- authored detail\n\nSecond note",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    assert_eq!(
        draft.global.as_ref().unwrap().block_id.as_deref(),
        Some("a-id")
    );
    assert_eq!(
        draft.items[0].parsed.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("a-id".to_string()),
            section: None,
        }
    );
    assert_eq!(
        sub_bullet_bodies(&draft.items[0].parsed.sub_bullets),
        vec!["authored detail"]
    );
    assert_eq!(
        draft.items[1].parsed.kind,
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId("a-id".to_string()),
            section: None,
        }
    );
}

#[test]
fn execution_an_ensure_next_item_participates_normally_in_a_multi_item_draft() {
    let draft = parse_capture_draft_with_clip_control(
        "First task @dev\n\n@cash+goog-exit\n\nThird task @dev",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    assert_eq!(draft.items.len(), 3);
    assert_eq!(draft.items[0].parsed.kind, CaptureKind::Task);
    assert_eq!(
        draft.items[1].parsed.kind,
        CaptureKind::TaskToggle {
            block_id: "goog-exit".to_string(),
            pomodoro_name: None,
            intent: TaskToggleIntent::EnsureNext,
        }
    );
    assert_eq!(draft.items[1].parsed.body, "");
    assert_eq!(draft.items[1].parsed.route.as_deref(), Some("cash"));
    assert_eq!(draft.items[2].parsed.kind, CaptureKind::Task);
}

#[test]
fn execution_an_explicit_toggle_item_participates_in_a_multi_item_draft() {
    let draft = parse_capture_draft_with_clip_control(
        "First task @dev\n\n@cash+goog-exit!\n\nThird task @dev",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    assert_eq!(draft.items.len(), 3);
    assert_eq!(
        draft.items[1].parsed.kind,
        CaptureKind::TaskToggle {
            block_id: "goog-exit".to_string(),
            pomodoro_name: None,
            intent: TaskToggleIntent::Toggle,
        }
    );
    assert_eq!(draft.items[1].parsed.body, "");
}

#[test]
fn execution_local_markers_override_a_global_declaration() {
    let draft = parse_capture_draft_with_clip_control(
        "@@foo+a-id\nKeep\n\nBullet @bar#Ideas\n\nId @bar^b-id\n\nPomo @bar:p-id\n\nChild @bar+b-id\n\nNote #",
        None,
        None, &CaptureParseOptions::routing_off(true))
    .expect("parse");
    assert!(matches!(
        draft.items[0].parsed.kind,
        CaptureKind::SubBullet { .. }
    ));
    assert!(matches!(
        draft.items[1].parsed.kind,
        CaptureKind::Bullet { .. }
    ));
    assert_eq!(draft.items[1].parsed.route.as_deref(), Some("bar"));
    assert!(matches!(
        draft.items[2].parsed.kind,
        CaptureKind::TaskWithBlockId { .. }
    ));
    assert!(matches!(
        draft.items[3].parsed.kind,
        CaptureKind::Pomodoro { .. }
    ));
    assert!(matches!(
        draft.items[4].parsed.kind,
        CaptureKind::SubBullet { .. }
    ));
    assert_eq!(draft.items[4].parsed.route.as_deref(), Some("bar"));
    assert_eq!(draft.items[5].parsed.kind, CaptureKind::PomodoroNote);
    assert_eq!(draft.items[5].parsed.route, None);
}

#[test]
fn execution_rejects_a_declaration_only_draft() {
    let error = parse_capture_draft_with_clip_control(
        "@@foo",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .unwrap_err();
    assert_eq!(error, MISSING_CAPTURE_ITEM_ERROR);
}

#[test]
fn execution_rejects_unsupported_global_forms() {
    for (raw, needle) in [
        ("@@foo#Ideas\nTask", "not supported"),
        ("@@foo^id\nTask", "not supported"),
        ("@@foo:id\nTask", "not supported"),
        ("@@foo+id#sec\nTask", "not supported"),
    ] {
        let error = parse_capture_draft_with_clip_control(
            raw,
            None,
            None,
            &CaptureParseOptions::routing_off(true),
        )
        .unwrap_err();
        assert!(error.contains(needle), "{raw}: {error}");
    }
}

#[test]
fn execution_accepts_a_later_declaration_only_line() {
    let draft = parse_capture_draft_with_clip_control(
        "First task\n\n@@foo\nSecond",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    assert_eq!(draft.global.as_ref().unwrap().line, 3);
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("foo"));
    assert_eq!(draft.items[1].parsed.route.as_deref(), Some("foo"));
}

#[test]
fn execution_strips_inline_declarations_before_terminal_markers() {
    let draft = parse_capture_draft_with_clip_control(
        "Buy milk s:2 @@Groceries",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    let global = draft.global.expect("global");
    assert_eq!(global.route, "groceries");
    assert_eq!(global.line, 1);
    assert_eq!(draft.items[0].parsed.body, "Buy milk");
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("groceries"));
    assert_eq!(draft.items[0].parsed.scheduled_offset, Some(2));
}

#[test]
fn execution_warns_when_a_local_marker_shadows_its_declaration() {
    let draft = parse_capture_draft_with_clip_control(
        "Buy milk @dev @@groceries\n\nOther",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("dev"));
    assert_eq!(draft.items[1].parsed.route.as_deref(), Some("groceries"));
    assert_eq!(
        draft.warnings,
        vec![
            "this item's @dev marker overrides the @@groceries destination it declares; move @@groceries to an item without a local marker, or delete @dev"
                .to_string()
        ]
    );
}

#[test]
fn editor_inherits_global_destination_and_keeps_local_overrides() {
    let parse = editor("@@foo\nFirst\n\nSecond @bar");
    let global = parse.global_destination.as_ref().expect("global");
    assert_eq!(global.route.as_deref(), Some("foo"));
    assert_eq!(parse.route.as_deref(), Some("foo"));
    assert_eq!(parse.items[0].route.as_deref(), Some("foo"));
    assert_eq!(parse.items[1].route.as_deref(), Some("bar"));
    assert!(parse.items[1].has_local_destination);
    assert_eq!(ranges(&parse)[0], (0, 5, SpanKind::GlobalRoute));
}

#[test]
fn editor_reports_incomplete_and_declaration_only_globals() {
    let incomplete = editor("@@");
    assert_eq!(incomplete.mode, EditorMode::Incomplete);
    assert_eq!(incomplete.needs, vec![Need::Route]);
    assert_eq!(codes(&incomplete), vec!["missing_capture_item"]);

    let declaration_only = editor("@@foo");
    assert_eq!(
        declaration_only
            .global_destination
            .as_ref()
            .unwrap()
            .route
            .as_deref(),
        Some("foo")
    );
    assert_eq!(codes(&declaration_only), vec!["missing_capture_item"]);
}

#[test]
fn task_complete_items_skip_global_inheritance() {
    // A `@@` declaration routes ordinary items but never turns a `!`
    // item into a task or changes its destination: execution leaves
    // the `!` item a `TaskComplete` with no inherited route or tags.
    let draft = parse_capture_draft_with_clip_control(
        "@@cash\n\n!sase:x",
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect("parse");
    assert_eq!(draft.items.len(), 1);
    assert!(
        matches!(draft.items[0].parsed.kind, CaptureKind::TaskComplete { .. }),
        "{:?}",
        draft.items[0].parsed.kind
    );
    assert_eq!(draft.items[0].parsed.route, None);
    assert!(draft.items[0].parsed.dependencies.is_empty());

    // The editor reports the same item in `task_complete` mode.
    let parse = editor("@@cash\n\n!sase:x");
    assert_eq!(parse.items.len(), 1);
    assert_eq!(parse.items[0].mode, EditorMode::TaskComplete);
    assert_eq!(parse.mode, EditorMode::TaskComplete);
}

#[test]
fn editor_item_at_uses_the_inherited_global_route() {
    let raw = "@@sase\nSee [[#De";
    let item = editor_item_at(raw, raw.len()).expect("item");
    assert_eq!(item.route.as_deref(), Some("sase"));
    assert!(!item.has_local_destination);
}

//! Bare whole-item `x` close grammar: what it claims, what stays prose, and
//! how it chains.

use super::super::close_selection::*;
use super::super::editor_model::*;
use super::super::item::*;
use super::super::model::*;
use super::*;

fn span_ranges(parse: &EditorParse) -> Vec<(usize, usize, SpanKind)> {
    parse
        .items
        .iter()
        .flat_map(|item| item.spans.iter())
        .map(|span| (span.start, span.end, span.kind))
        .collect()
}

#[test]
fn bare_close_spans_cover_the_one_byte_x_and_each_list() {
    let parse = parse_for_editor("x1*2!3~4");
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroClose);
    assert_eq!(
        span_ranges(&parse),
        vec![
            (0, 1, SpanKind::PomodoroClose),
            (1, 2, SpanKind::PomodoroCloseInProgress),
            (2, 4, SpanKind::PomodoroClosePark),
            (4, 6, SpanKind::PomodoroCloseComplete),
            (6, 8, SpanKind::PomodoroCloseDrop),
        ]
    );
    assert_eq!(
        parse.items[0].pomodoro_close.as_ref().expect("close").raw,
        "x1*2!3~4"
    );
    let plain = parse_for_editor("x");
    assert_eq!(span_ranges(&plain), vec![(0, 1, SpanKind::PomodoroClose)]);
}

#[test]
fn bare_close_selection_shapes_report_the_same_close_everywhere() {
    for raw in [
        "x",
        "x1,3!2",
        "x0",
        "x*",
        "x!",
        "x*!2",
        "x1*",
        "x~2",
        "x@2",
        "x!1@2",
        "x2,3 2 foo",
        "x\n- 1 wired",
    ] {
        let edited = parse_for_editor(raw);
        assert_eq!(edited.items[0].mode, EditorMode::PomodoroClose, "{raw}");
        assert!(edited.items[0].diagnostics.is_empty(), "{raw}");
        let executed = execute_draft(raw).expect("close");
        let spec = match &executed.items[0].parsed.kind {
            CaptureKind::PomodoroClose { spec } => spec,
            other => panic!("{raw}: expected close, got {other:?}"),
        };
        assert_eq!(
            edited.items[0].pomodoro_close.as_ref().expect("close").raw,
            spec.raw,
            "{raw}"
        );
        assert_eq!(
            spec.raw,
            raw.lines().next().unwrap().split(' ').next().unwrap()
        );
    }
    for raw in ["x1,", "x~", "x@"] {
        let edited = parse_for_editor(raw);
        assert_eq!(edited.items[0].mode, EditorMode::Incomplete, "{raw}");
        assert!(execute_draft(raw).is_err(), "{raw}");
    }
    for raw in ["x 1", "x @2 foo", "x0 wired"] {
        let edited = parse_for_editor(raw);
        let executed = execute_draft(raw);
        assert!(
            executed.is_err()
                || !edited.items[0].diagnostics.is_empty()
                || edited.items[0].mode == EditorMode::Incomplete,
            "{raw}"
        );
    }
}

#[test]
fn bare_close_classifier_claims_only_the_documented_shapes() {
    for (token, expected) in [
        ("x", Some("")),
        ("x1,3!2", Some("1,3!2")),
        ("x,", Some(",")),
        ("x#bugs", None),
        ("=x", None),
        ("=x2", None),
        ("=*", None),
        ("X", None),
        ("X2", None),
        ("xx", None),
        ("xa", None),
        ("x.", None),
        ("x-ray", None),
        ("xylophone", None),
        ("Plan x", None),
    ] {
        assert_eq!(classify_whole_item_close(token), expected, "{token}");
    }
    for token in ["x", "x2", "x#bugs"] {
        assert!(is_session_chain_token(token), "{token}");
    }
    for token in ["X", "X2", "xx", "x-ray"] {
        assert!(!is_session_chain_token(token), "{token}");
    }
}

#[test]
fn prose_and_uppercase_x_remain_tasks() {
    for raw in [
        "xx",
        "xa",
        "x.",
        "x-ray",
        "xylophone",
        "Plan x",
        "X",
        "X2",
        "X post about the launch",
        "X11 forwarding",
        "X *job:cleanup",
    ] {
        let edited = parse_for_editor(raw);
        assert_eq!(edited.items[0].mode, EditorMode::Task, "{raw}");
        assert!(edited.items[0].pomodoro_close.is_none(), "{raw}");
        let executed = execute_draft(raw).expect("task prose");
        assert!(
            matches!(executed.items[0].parsed.kind, CaptureKind::Task),
            "{raw}"
        );
    }
}

#[test]
fn bare_close_heads_session_chains_and_inline_trails() {
    for raw in [
        "+2 x",
        "x =",
        "x =#bugs",
        "x =~2",
        "x~2 =",
        "x wired the lexer =",
        "x =\n- 1 wired it",
        "- x",
    ] {
        let edited = parse_for_editor(raw);
        let executed = execute_draft(raw).expect("chain");
        assert!(!edited.items.is_empty(), "{raw}");
        assert!(!executed.items.is_empty(), "{raw}");
        assert!(
            edited.items.iter().any(|item| {
                item.mode == EditorMode::PomodoroClose
                    || item.pomodoro_close.is_some()
            }),
            "{raw}: {:?}",
            edited.items
        );
        assert!(
            executed.items.iter().any(|item| {
                matches!(item.parsed.kind, CaptureKind::PomodoroClose { .. })
            }),
            "{raw}: {:?}",
            executed.items
        );
    }
}

#[test]
fn bare_close_suppresses_marker_completion_in_work_log_text() {
    for raw in ["x @dev:some-id#", "x\n- @dev:some-id#"] {
        assert_eq!(completion_field_at(raw, raw.len()), None, "{raw}");
    }
}

//! Bare whole-item `x` close grammar and parity with the existing `=x` form.

use super::super::close_selection::*;
use super::super::draft::*;
use super::super::editor_model::*;
use super::super::item::*;
use super::super::model::*;
use super::*;

fn execute_draft(raw: &str) -> Result<ParsedCaptureDraft, String> {
    parse_capture_draft_with_clip_control(
        raw,
        None,
        None,
        &CaptureParseOptions::routing_off(true),
    )
}

fn close_spec(kind: &CaptureKind) -> &PomodoroCloseSpec {
    match kind {
        CaptureKind::PomodoroClose { spec } => spec,
        other => panic!("expected close, got {other:?}"),
    }
}

fn normalized_close(spec: &PomodoroCloseSpec) -> PomodoroCloseSpec {
    let mut normalized = spec.clone();
    normalized.raw.clear();
    for entry in &mut normalized.log {
        if let CloseLogOrigin::Inline { close_token, .. } = &mut entry.origin {
            close_token.clear();
        }
    }
    normalized
}

fn bare_range_from_legacy(
    range: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    range.map(|(start, end)| (start.saturating_sub(1), end.saturating_sub(1)))
}

fn assert_close_pair(bare: &str, legacy: &str) {
    let bare_editor = parse_for_editor(bare);
    let legacy_editor = parse_for_editor(legacy);
    assert_eq!(bare_editor.items.len(), legacy_editor.items.len(), "{bare}");
    let bare_item = &bare_editor.items[0];
    let legacy_item = &legacy_editor.items[0];
    assert_eq!(bare_item.mode, legacy_item.mode, "{bare}");
    assert_eq!(bare_item.needs, legacy_item.needs, "{bare}");
    assert_eq!(bare_item.spans.len(), legacy_item.spans.len(), "{bare}");
    for (bare_span, legacy_span) in
        bare_item.spans.iter().zip(&legacy_item.spans)
    {
        assert_eq!(bare_span.kind, legacy_span.kind, "{bare}");
        if bare_span.kind == SpanKind::PomodoroClose {
            assert_eq!(bare_span.start, legacy_span.start, "{bare}");
            assert_eq!(bare_span.end, legacy_span.end - 1, "{bare}");
        } else {
            assert_eq!(bare_span.start + 1, legacy_span.start, "{bare}");
            assert_eq!(bare_span.end + 1, legacy_span.end, "{bare}");
        }
    }
    assert_eq!(
        bare_item.diagnostics.len(),
        legacy_item.diagnostics.len(),
        "{bare}"
    );
    for (bare_diag, legacy_diag) in
        bare_item.diagnostics.iter().zip(&legacy_item.diagnostics)
    {
        assert_eq!(bare_diag.code, legacy_diag.code, "{bare}");
        assert_eq!(
            bare_diag.range,
            bare_range_from_legacy(legacy_diag.range),
            "{bare}"
        );
        if bare.ends_with("#bugs") {
            assert_eq!(bare_diag.message, legacy_diag.message, "{bare}");
        } else {
            assert_eq!(
                bare_diag.message,
                legacy_diag.message.replace("=x", "x"),
                "{bare}"
            );
        }
    }
    match (&bare_item.pomodoro_close, &legacy_item.pomodoro_close) {
        (Some(bare_spec), Some(legacy_spec)) => assert_eq!(
            normalized_close(bare_spec),
            normalized_close(legacy_spec),
            "{bare}"
        ),
        (None, None) => {}
        other => panic!("close spec mismatch for {bare}: {other:?}"),
    }

    let bare_execution = execute_draft(bare);
    let legacy_execution = execute_draft(legacy);
    match (bare_execution, legacy_execution) {
        (Ok(bare_draft), Ok(legacy_draft)) => {
            assert_eq!(
                bare_draft.items.len(),
                legacy_draft.items.len(),
                "{bare}"
            );
            let bare_spec = close_spec(&bare_draft.items[0].parsed.kind);
            let legacy_spec = close_spec(&legacy_draft.items[0].parsed.kind);
            assert_eq!(
                normalized_close(bare_spec),
                normalized_close(legacy_spec),
                "{bare}"
            );
        }
        (Err(bare_error), Err(legacy_error)) if bare.ends_with("#bugs") => {
            assert_eq!(bare_error, legacy_error, "{bare}");
        }
        (Err(bare_error), Err(legacy_error)) => {
            assert_eq!(bare_error, legacy_error.replace("=x", "x"), "{bare}");
        }
        (bare_result, legacy_result) => {
            panic!("execution result mismatch for {bare}: {bare_result:?} / {legacy_result:?}")
        }
    }
}

#[test]
fn bare_close_forms_match_legacy_selection_behavior() {
    for (bare, legacy) in [
        ("x", "=x"),
        ("x1,3!2", "=x1,3!2"),
        ("x0", "=x0"),
        ("x*", "=x*"),
        ("x!", "=x!"),
        ("x*!2", "=x*!2"),
        ("x1*", "=x1*"),
        ("x~2", "=x~2"),
        ("x@2", "=x@2"),
        ("x!1@2", "=x!1@2"),
        ("x1,", "=x1,"),
        ("x~", "=x~"),
        ("x@", "=x@"),
        ("x 1", "=x 1"),
        ("x more", "=x more"),
        ("x0 1 foo", "=x0 1 foo"),
        ("x2,3 2 foo", "=x2,3 2 foo"),
        ("x @2 foo", "=x @2 foo"),
        ("x\n- 1 wired", "=x\n- 1 wired"),
        ("x#bugs", "=x#bugs"),
    ] {
        assert_close_pair(bare, legacy);
    }
}

#[test]
fn bare_close_classifier_claims_only_the_documented_shapes() {
    for (token, expected) in [
        ("x", Some(("", 1))),
        ("x1,3!2", Some(("1,3!2", 1))),
        ("x,", Some((",", 1))),
        ("x#bugs", None),
        ("X", None),
        ("X2", None),
        ("xx", None),
        ("xa", None),
        ("x.", None),
        ("x-ray", None),
        ("xylophone", None),
        ("Plan x", None),
    ] {
        assert_eq!(classify_bare_whole_item_close(token), expected, "{token}");
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

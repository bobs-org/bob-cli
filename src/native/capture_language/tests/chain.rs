//! Same-line Pomodoro session-operator chain tests.

use super::super::draft::*;
use super::super::editor_model::*;
use super::super::item::*;
use super::super::model::*;
use super::*;

fn execute_draft(raw: &str) -> Result<ParsedCaptureDraft, String> {
    parse_capture_draft_with_clip_control(raw, None, None, true)
}

fn execute_forced(
    raw: &str,
    forced_route: Option<&str>,
) -> Result<ParsedCaptureDraft, String> {
    parse_capture_draft_with_clip_control(raw, forced_route, None, true)
}

#[test]
fn chain_token_predicate_covers_the_documented_table() {
    for token in [
        "+",
        "-2",
        "++3",
        "--",
        "+0",
        "+5more",
        "++3@work",
        "+99999999999999999999999",
        "=",
        "=-",
        "=3",
        "=-2",
        "=2-1",
        "=3x",
        "=99999999999999999999999",
        "=#bugs",
        "=3#bugs",
        "=-2#bugs",
        "=#",
        "=#bugs=3",
        "=#b!",
        "=x#bugs",
        "=x",
        "=X",
        "=x2",
        "=x1,3!2",
        "=x0",
        "=x!2",
        "=x1a",
        "=x1,1",
        "=x1,",
        "=x!",
    ] {
        assert!(is_session_chain_token(token), "{token}");
    }
    for token in [
        "foo", "==", "=xx", "=xa", "+++", "+-", "-foo", "--aside", "1,3", "!2",
        "^r:id=", "@r:id=x", "s:1", "p:2", "%", "#", "@@r", "#bugs", "bugs",
    ] {
        assert!(!is_session_chain_token(token), "{token}");
    }
}

#[test]
fn chain_token_predicate_equals_claimed_for_single_tokens() {
    let tokens = [
        "+",
        "-2",
        "++3",
        "--",
        "+0",
        "+5more",
        "++3@work",
        "+99999999999999999999999",
        "=",
        "=-",
        "=3",
        "=-2",
        "=2-1",
        "=3x",
        "=99999999999999999999999",
        "=#bugs",
        "=3#bugs",
        "=-2#bugs",
        "=#",
        "=#bugs=3",
        "=#b!",
        "=x#bugs",
        "=x",
        "=X",
        "=x2",
        "=x1,3!2",
        "=x0",
        "=x!2",
        "=x1a",
        "=x1,1",
        "=x1,",
        "=x!",
        "foo",
        "==",
        "=xx",
        "=xa",
        "+++",
        "+-",
        "-foo",
        "--aside",
        "1,3",
        "!2",
        "^r:id=",
        "@r:id=x",
        "s:1",
        "p:2",
        "%",
        "#",
        "@@r",
        "#bugs",
        "bugs",
    ];
    for token in tokens {
        let draft = split_capture_draft(token);
        if draft.items.is_empty() {
            // Declaration-only lines never become items; they can never be
            // chain tokens.
            assert!(!is_session_chain_token(token), "{token}");
            continue;
        }
        assert_eq!(draft.items.len(), 1, "{token}");
        let item = &draft.items[0];
        // A single token is never a chain.
        assert_eq!(draft.items[0].lines.len(), 1, "{token}");
        let equals =
            parse_pomodoro_equals_item(item, &item.lines[0], None, None);
        let adjust =
            parse_pomodoro_adjust_item(item, &item.lines[0], None, None);
        let claimed =
            !matches!(equals, Ok(None)) || !matches!(adjust, Ok(None));
        assert_eq!(is_session_chain_token(token), claimed, "{token}");
    }
}

#[test]
fn draft_splits_a_two_token_chain_with_absolute_ranges() {
    assert_eq!(draft_items("+2 =x"), vec![(0, 1, "+2"), (1, 1, "=x")]);
    let raw = "+2 =x";
    let draft = split_capture_draft(raw);
    assert_eq!(draft.items.len(), 2);
    assert_eq!((draft.items[0].start, draft.items[0].end), (0, 2));
    assert_eq!((draft.items[1].start, draft.items[1].end), (3, 5));
    assert_eq!(&raw[draft.items[0].start..draft.items[0].end], "+2");
    assert_eq!(&raw[draft.items[1].start..draft.items[1].end], "=x");
}

#[test]
fn draft_splits_tabs_and_runs_of_spaces() {
    let raw = "  +2\t =x  ";
    let draft = split_capture_draft(raw);
    assert_eq!(draft.items.len(), 2);
    assert_eq!(&raw[draft.items[0].start..draft.items[0].end], "+2");
    assert_eq!(&raw[draft.items[1].start..draft.items[1].end], "=x");
    assert_eq!(draft_items(raw), vec![(0, 1, "+2"), (1, 1, "=x")]);
}

#[test]
fn draft_keeps_sequential_indices_across_crlf_with_a_chain() {
    let raw = "+2 =x\r\n\r\nhello";
    let draft = split_capture_draft(raw);
    assert_eq!(draft.items.len(), 3);
    assert_eq!(draft.items[0].index, 0);
    assert_eq!(draft.items[1].index, 1);
    assert_eq!(draft.items[2].index, 2);
    assert_eq!(draft.items[0].line_start, 1);
    assert_eq!(draft.items[1].line_start, 1);
    assert_eq!(draft.items[2].line_start, 3);
    assert_eq!(&raw[draft.items[0].start..draft.items[0].end], "+2");
    assert_eq!(&raw[draft.items[1].start..draft.items[1].end], "=x");
}

#[test]
fn draft_chain_works_with_a_global_declaration() {
    let draft = split_capture_draft("@@work\n+2 =x");
    assert_eq!(draft.declarations.len(), 1);
    assert_eq!(draft.items.len(), 2);
    assert_eq!(&draft.items[0].lines[0].raw.text, &"+2");
    assert_eq!(&draft.items[1].lines[0].raw.text, &"=x");
}

#[test]
fn draft_attaches_chain_child_lines_to_the_close() {
    let raw = "+2 =x\n- child";
    let draft = split_capture_draft(raw);
    assert_eq!(draft.items.len(), 2);
    assert_eq!(draft.items[0].lines.len(), 1);
    assert_eq!(draft.items[1].lines.len(), 2);
    assert_eq!(draft.items[1].line_start, 1);
    assert_eq!(draft.items[1].line_end, 2);
    assert_eq!(draft.items[1].end, draft.items[1].lines[1].raw.end);
    // `=x =` with children attaches them to the close (the first token),
    // so the close item's range contains the later start token's range.
    let raw = "=x =\n- 1 foo";
    let draft = split_capture_draft(raw);
    assert_eq!(draft.items.len(), 2);
    assert_eq!(draft.items[0].lines.len(), 2);
    assert_eq!(draft.items[1].lines.len(), 1);
    assert_eq!(draft.items[0].line_end, 2);
    assert!(draft.items[0].start < draft.items[1].start);
    assert!(draft.items[0].end > draft.items[1].end);
    // With no `=x` on the line, children still attach to the last token.
    let raw = "= +2\n- child";
    let draft = split_capture_draft(raw);
    assert_eq!(draft.items.len(), 2);
    assert_eq!(draft.items[0].lines.len(), 1);
    assert_eq!(draft.items[1].lines.len(), 2);
}

#[test]
fn execution_runs_a_chain_left_to_right() {
    let draft = execute_draft("+2 =x").expect("chain executes");
    assert_eq!(draft.items.len(), 2);
    match &draft.items[0].parsed.kind {
        CaptureKind::PomodoroAdjust { spec } => {
            assert!(spec.plus);
            assert_eq!(spec.units, 2);
        }
        other => panic!("expected adjust, got {other:?}"),
    }
    match &draft.items[1].parsed.kind {
        CaptureKind::PomodoroClose { spec } => {
            assert_eq!(spec.raw, "=x");
            assert_eq!(spec.in_progress, None);
            assert!(spec.complete.is_empty());
        }
        other => panic!("expected close, got {other:?}"),
    }
}

#[test]
fn execution_switches_sessions_with_close_then_start() {
    let draft = execute_draft("=x =").expect("switch executes");
    assert_eq!(draft.items.len(), 2);
    assert!(
        matches!(
            draft.items[0].parsed.kind,
            CaptureKind::PomodoroClose { .. }
        ),
        "first is close"
    );
    assert!(
        matches!(
            draft.items[1].parsed.kind,
            CaptureKind::PomodoroStart { .. }
        ),
        "second is start"
    );
}

#[test]
fn execution_distinguishes_spaced_start_adjust_from_offset_start() {
    let spaced = execute_draft("= -2").expect("spaced executes");
    assert_eq!(spaced.items.len(), 2);
    match &spaced.items[0].parsed.kind {
        CaptureKind::PomodoroStart { spec, .. } => {
            assert_eq!(spec.raw, "");
        }
        other => panic!("expected start, got {other:?}"),
    }
    match &spaced.items[1].parsed.kind {
        CaptureKind::PomodoroAdjust { spec } => {
            assert!(!spec.plus);
            assert_eq!(spec.units, 2);
        }
        other => panic!("expected adjust, got {other:?}"),
    }
    let single = execute_draft("=-2").expect("single executes");
    assert_eq!(single.items.len(), 1);
    match &single.items[0].parsed.kind {
        CaptureKind::PomodoroStart { spec, .. } => {
            assert_eq!(spec.raw, "-2");
            assert_eq!(spec.offset_units, 2);
        }
        other => panic!("expected start, got {other:?}"),
    }
}

#[test]
fn execution_carries_a_close_selection_through_a_chain() {
    let draft = execute_draft("=x1,3!2 +").expect("selection chain");
    assert_eq!(draft.items.len(), 2);
    match &draft.items[0].parsed.kind {
        CaptureKind::PomodoroClose { spec } => {
            assert_eq!(spec.raw, "=x1,3!2");
            assert_eq!(spec.in_progress, Some(vec![1, 3]));
            assert_eq!(spec.complete, vec![2]);
        }
        other => panic!("expected close, got {other:?}"),
    }
    assert!(
        matches!(
            draft.items[1].parsed.kind,
            CaptureKind::PomodoroAdjust { .. }
        ),
        "second is adjust"
    );
}

#[test]
fn execution_reports_a_broken_second_token_on_its_own_range() {
    let error = execute_draft("+2 +0").unwrap_err();
    assert!(
        error.contains("capture item 2 starting on line 1"),
        "{error}"
    );
    assert!(error.contains("+0"), "{error}");
}

#[test]
fn execution_attaches_chain_child_lines_to_the_close() {
    // `+2 =x` plus a valid bullet: a close with an entry.
    let draft = execute_draft("+2 =x\n- 1 foo").expect("close with entry");
    assert_eq!(draft.items.len(), 2);
    match &draft.items[1].parsed.kind {
        CaptureKind::PomodoroClose { spec } => {
            assert_eq!(spec.raw, "=x");
            let texts = spec
                .log
                .iter()
                .map(|entry| (entry.index, entry.text.clone()))
                .collect::<Vec<_>>();
            assert_eq!(texts, vec![(1, "foo".to_string())]);
        }
        other => panic!("expected close, got {other:?}"),
    }
    // A bullet with no number reports the missing-number error instead of
    // the old child-shape error.
    for raw in ["+2 =x\n- child", "+ =x\n- child"] {
        let error = execute_draft(raw).unwrap_err();
        assert!(
            error.contains("capture item 2 starting on line 1"),
            "{raw}: {error}"
        );
        assert!(
            error.contains("start each Work Log bullet"),
            "{raw}: {error}"
        );
    }
    // `=x =` plus a bullet attaches the bullet to the close, not the start.
    let draft = execute_draft("=x =\n- 1 foo").expect("close then start");
    assert_eq!(draft.items.len(), 2);
    match &draft.items[0].parsed.kind {
        CaptureKind::PomodoroClose { spec } => {
            assert_eq!(spec.log.len(), 1);
            assert_eq!(spec.log[0].index, 1);
            assert_eq!(spec.log[0].text, "foo");
        }
        other => panic!("expected close, got {other:?}"),
    }
    assert!(
        matches!(
            draft.items[1].parsed.kind,
            CaptureKind::PomodoroStart { .. }
        ),
        "second is start"
    );
    // The close item's range nests the later parent-line token: it runs
    // from its token through its last bullet.
    assert!(draft.items[0].start < draft.items[1].start);
    assert!(draft.items[0].end > draft.items[1].end);
    // With no `=x` on the line, children still fail the last token's shape
    // rule.
    let error = execute_draft("= +2\n- child").unwrap_err();
    assert!(
        error.contains("Pomodoro adjustment items must contain only"),
        "{error}"
    );
}

#[test]
fn execution_rejects_forced_destinations_on_the_first_chain_token() {
    let error = execute_forced("+2 =x", Some("foo")).unwrap_err();
    assert!(
        error.contains("capture item 1 starting on line 1"),
        "{error}"
    );
    assert!(
        error.contains(
            "Pomodoro adjustment `+N`/`-N` cannot be combined with --route"
        ),
        "{error}"
    );
}

#[test]
fn execution_leaves_non_chains_unchanged() {
    let error = execute_draft("+2 more").unwrap_err();
    assert!(
        error.contains("Pomodoro adjustment items must contain only"),
        "{error}"
    );
    let error = execute_draft("=x 1,3").unwrap_err();
    assert!(error.contains("with no spaces"), "{error}");
    let error = execute_draft("=x ^bob:ready=").unwrap_err();
    assert!(error.contains("can't end with the Task Link"), "{error}");
    for raw in ["Plan +2 =x", "- foo"] {
        let draft = execute_draft(raw).expect("stays a task");
        assert_eq!(draft.items.len(), 1, "{raw}");
        assert!(
            matches!(draft.items[0].parsed.kind, CaptureKind::Task),
            "{raw}"
        );
    }
    let single = execute_draft("+2").expect("single token");
    assert_eq!(single.items.len(), 1);
}

#[test]
fn editor_reports_two_items_with_absolute_spans_for_a_chain() {
    let parse = parse_for_editor("+2 =x");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroAdjust);
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroClose);
    assert_eq!((parse.items[0].start, parse.items[0].end), (0, 2));
    assert_eq!((parse.items[1].start, parse.items[1].end), (3, 5));
    assert_eq!(
        parse.items[0]
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![(0, 2, SpanKind::PomodoroAdjust)]
    );
    assert_eq!(
        parse.items[1]
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![(3, 5, SpanKind::PomodoroClose)]
    );
    assert_eq!(parse.mode, EditorMode::PomodoroAdjust);
    let kinds = parse
        .spans
        .iter()
        .map(|span| (span.start, span.end, span.kind))
        .collect::<Vec<_>>();
    assert!(
        kinds.contains(&(0, 2, SpanKind::PomodoroAdjust)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(3, 5, SpanKind::PomodoroClose)),
        "{kinds:?}"
    );
}

#[test]
fn editor_reports_selection_spans_at_absolute_offsets_in_a_chain() {
    let parse = parse_for_editor("+2 =x1,3!2");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroClose);
    let spans = parse.items[1]
        .spans
        .iter()
        .map(|span| (span.start, span.end, span.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        spans,
        vec![
            (3, 5, SpanKind::PomodoroClose),
            (5, 8, SpanKind::PomodoroCloseInProgress),
            (8, 10, SpanKind::PomodoroCloseComplete),
        ]
    );
}

#[test]
fn editor_reports_a_dangling_close_separator_inside_a_chain() {
    let parse = parse_for_editor("+2 =x1,");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[1].mode, EditorMode::Incomplete);
    assert_eq!(parse.items[1].needs, vec![Need::PomodoroCloseTask]);
    let spans = parse.items[1]
        .spans
        .iter()
        .map(|span| (span.start, span.end, span.kind))
        .collect::<Vec<_>>();
    assert!(
        spans.contains(&(6, 7, SpanKind::InteractivePlaceholder)),
        "{spans:?}"
    );
}

#[test]
fn editor_reports_a_broken_second_token_with_its_own_diagnostic() {
    let parse = parse_for_editor("+2 +0");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroAdjust);
    assert_eq!(parse.items[1].diagnostics.len(), 1);
    assert_eq!(
        parse.items[1].diagnostics[0].code,
        "invalid_pomodoro_adjustment"
    );
    assert_eq!(parse.items[1].diagnostics[0].range, Some((3, 5)));
}

#[test]
fn editor_reports_a_named_start_chain_with_absolute_spans() {
    let parse = parse_for_editor("=x =#bugs");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroClose);
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroStart);
    assert_eq!(parse.items[1].section.as_deref(), Some("bugs"));
    assert!(parse.items[1].needs.is_empty());
    assert_eq!((parse.items[0].start, parse.items[0].end), (0, 2));
    assert_eq!((parse.items[1].start, parse.items[1].end), (3, 9));
    assert_eq!(
        parse.items[1]
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![
            (3, 4, SpanKind::PomodoroStart),
            (5, 9, SpanKind::PomodoroName),
        ]
    );
    assert!(parse.items[1].diagnostics.is_empty());
    assert!(parse.items[1].pomodoro_start.is_some());
}

#[test]
fn editor_reports_an_incomplete_named_start_inside_a_chain() {
    let parse = parse_for_editor("=x =#");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[1].mode, EditorMode::Incomplete);
    assert_eq!(parse.items[1].needs, vec![Need::PomodoroName]);
    assert_eq!(
        parse.items[1]
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![
            (3, 4, SpanKind::PomodoroStart),
            (4, 5, SpanKind::InteractivePlaceholder),
        ]
    );
    assert!(parse.items[1].diagnostics.is_empty());
}

#[test]
fn editor_reports_a_named_start_plus_adjustment_chain() {
    let parse = parse_for_editor("=#bugs +2");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroStart);
    assert_eq!(parse.items[0].section.as_deref(), Some("bugs"));
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroAdjust);
    assert_eq!(
        parse.items[0]
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![
            (0, 1, SpanKind::PomodoroStart),
            (2, 6, SpanKind::PomodoroName),
        ]
    );
}

#[test]
fn editor_chain_items_do_not_inherit_a_global_declaration() {
    let parse = parse_for_editor("@@work\n+2 =x");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroAdjust);
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroClose);
    assert!(parse.items[0].route.is_none());
    assert!(parse.items[1].route.is_none());
}

#[test]
fn completion_requests_nothing_anywhere_on_a_chain() {
    for cursor in 0..=5 {
        assert!(
            completion_field_at("+2 =x", cursor).is_none(),
            "cursor {cursor}"
        );
    }
    let item = editor_item_at("+2 =x", 4).expect("close item");
    assert_eq!(item.mode, EditorMode::PomodoroClose);
}

//! Sized `[N]x` close grammar: claim, size, teaching errors, chains, and
//! aliases.

use super::super::editor_model::*;
use super::super::item::*;
use super::super::model::*;
use super::super::rewrite::*;
use super::*;

fn size(raw: &str) -> CloseSize {
    CloseSize::from_raw(raw).expect("valid size")
}

fn close_spec(raw: &str) -> PomodoroCloseSpec {
    let edited = parse_for_editor(raw);
    edited.items[0]
        .pomodoro_close
        .clone()
        .unwrap_or_else(|| panic!("{raw}: editor close spec"))
}

fn execute_close(raw: &str) -> PomodoroCloseSpec {
    match &execute_draft(raw).expect(raw).items[0].parsed.kind {
        CaptureKind::PomodoroClose { spec } => spec.clone(),
        other => panic!("{raw}: expected close, got {other:?}"),
    }
}

fn assert_close_parity(raw: &str, expected: &PomodoroCloseSpec) {
    let edited = close_spec(raw);
    let executed = execute_close(raw);
    assert_eq!(&edited, expected, "{raw} editor");
    assert_eq!(&executed, expected, "{raw} execute");
    assert_eq!(edited, executed, "{raw} parity");
}

fn assert_error_parity(raw: &str, expected: &str) {
    let err = execute(raw).expect_err(raw);
    assert_eq!(err, expected, "{raw} execute");
    let parse = editor(raw);
    assert_eq!(codes(&parse), vec!["invalid_pomodoro_close"], "{raw}");
    assert_eq!(parse.diagnostics[0].message, expected, "{raw} editor");
}

#[test]
fn sized_closes_parse_in_both_parsers() {
    for (raw, digits, blocks) in [
        ("2x", "2", 2),
        ("02x", "02", 2),
        ("12x", "12", 12),
        ("99x", "99", 99),
    ] {
        let expected = PomodoroCloseSpec::plain(raw.to_string())
            .with_size(Some(size(digits)));
        assert_eq!(expected.size.as_ref().unwrap().blocks, blocks, "{raw}");
        assert_eq!(
            expected.size.as_ref().unwrap().minutes,
            blocks * 5,
            "{raw}"
        );
        assert_close_parity(raw, &expected);
    }

    let selected = PomodoroCloseSpec {
        raw: "2x!2".to_string(),
        size: Some(size("2")),
        in_progress: None,
        park: Vec::new(),
        park_all: false,
        complete: vec![2],
        complete_all: false,
        drop: Vec::new(),
        log: Vec::new(),
    };
    assert_close_parity("2x!2", &selected);

    let wildcard = close_spec("2x*!2");
    assert_eq!(wildcard.size, Some(size("2")));
    assert!(wildcard.park_all);
    assert_eq!(wildcard.complete, vec![2]);
    assert_eq!(execute_close("2x*!2"), wildcard);

    let listed = close_spec("2x1,3!2");
    assert_eq!(listed.size, Some(size("2")));
    assert_eq!(listed.in_progress, Some(vec![1, 3]));
    assert_eq!(listed.complete, vec![2]);
    assert_eq!(execute_close("2x1,3!2"), listed);

    let typed_last = close_spec("2x!2@1");
    assert_eq!(typed_last.size, Some(size("2")));
    assert_eq!(typed_last.in_progress, Some(vec![1]));
    assert_eq!(typed_last.complete, vec![2]);
    assert_eq!(execute_close("2x!2@1"), typed_last);

    let reset_shape = close_spec("2x0");
    assert_eq!(reset_shape.size, Some(size("2")));
    assert_eq!(reset_shape.in_progress, Some(vec![]));
    assert_eq!(execute_close("2x0"), reset_shape);
}

#[test]
fn sized_close_logs_and_front_chains_keep_the_size() {
    let inline = execute_close("2x wired the lexer");
    assert_eq!(inline.size, Some(size("2")));
    assert_eq!(inline.log.len(), 1);
    assert_eq!(inline.log[0].text, "wired the lexer");
    assert_eq!(close_spec("2x wired the lexer"), inline);

    let bullets = execute_close("2x\n- 1 wired");
    assert_eq!(bullets.size, Some(size("2")));
    assert_eq!(bullets.log.len(), 1);
    assert_eq!(close_spec("2x\n- 1 wired"), bullets);

    let star = execute_close("2x\n- * starred");
    assert_eq!(star.size, Some(size("2")));
    assert!(star.log[0].all);

    let chain = execute_draft("2x =bugs").expect("front chain");
    assert_eq!(chain.items.len(), 2);
    match &chain.items[0].parsed.kind {
        CaptureKind::PomodoroClose { spec } => {
            assert_eq!(spec.size, Some(size("2")));
        }
        other => panic!("expected close, got {other:?}"),
    }
    assert!(matches!(
        chain.items[1].parsed.kind,
        CaptureKind::PomodoroStart { .. }
    ));

    let adjust = execute_draft("--1 2x!2").expect("adjust then close");
    assert_eq!(adjust.items.len(), 2);
    match &adjust.items[1].parsed.kind {
        CaptureKind::PomodoroClose { spec } => {
            assert_eq!(spec.raw, "2x!2");
            assert_eq!(spec.size, Some(size("2")));
        }
        other => panic!("expected close, got {other:?}"),
    }

    let plus = execute_draft("+1 --1 2x").expect("adjusts then close");
    assert_eq!(plus.items.len(), 3);
    match &plus.items[2].parsed.kind {
        CaptureKind::PomodoroClose { spec } => {
            assert_eq!(spec.size, Some(size("2")));
        }
        other => panic!("expected close, got {other:?}"),
    }
}

#[test]
fn sized_link_and_task_suffixes_carry_size() {
    for raw in ["@r:id=2x!1", "^r:id=2x", "Text @r:id=2x", "^r:id=02X!1"] {
        let edited = parse_for_editor(raw);
        let spec = edited.items[0]
            .pomodoro_close
            .as_ref()
            .unwrap_or_else(|| panic!("{raw}: editor suffix"));
        assert!(spec.size.is_some(), "{raw}");
        let executed = execute_draft(raw).expect(raw);
        let close = match &executed.items[0].parsed.kind {
            CaptureKind::PomodoroLink {
                close: Some(close), ..
            }
            | CaptureKind::Pomodoro {
                close: Some(close), ..
            } => close,
            other => panic!("{raw}: expected link/task close, got {other:?}"),
        };
        assert_eq!(close.size, spec.size, "{raw}");
        assert_eq!(close.raw, spec.raw, "{raw}");
    }
}

#[test]
fn sized_incomplete_keeps_size_and_execution_refuses() {
    for raw in ["2x~", "2x1,", "@r:id=2x~", "^r:id=2x1,"] {
        let parse = parse_for_editor(raw);
        assert_eq!(parse.items[0].mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(
            parse.items[0].needs,
            vec![Need::PomodoroCloseTask],
            "{raw}"
        );
        let spec = parse.items[0].pomodoro_close.as_ref().expect(raw);
        assert!(spec.size.is_some(), "{raw}");
        assert!(execute_draft(raw).is_err(), "{raw}");
    }
}

#[test]
fn sized_looking_prose_stays_text() {
    for raw in [
        "2X",
        "2X4 lumber",
        "2 x!2",
        "2xx",
        "2x-ray",
        "2.5x",
        "２x",
        "Plan 2x",
    ] {
        let edited = parse_for_editor(raw);
        assert_eq!(edited.items[0].mode, EditorMode::Task, "{raw}");
        assert!(edited.items[0].pomodoro_close.is_none(), "{raw}");
        let executed = execute_draft(raw).expect(raw);
        assert!(
            matches!(executed.items[0].parsed.kind, CaptureKind::Task),
            "{raw}"
        );
    }

    let log = execute_close("x made it 2x");
    assert!(log.size.is_none());
    assert_eq!(log.log[0].text, "made it 2x");

    let faster = execute_close("x 2x faster");
    assert!(faster.size.is_none());
    assert_eq!(faster.log[0].text, "2x faster");
    assert_eq!(parse_for_editor("x 2x faster").items.len(), 1);

    for raw in ["= 2x", "=bugs 2x"] {
        let edited = parse_for_editor(raw);
        assert!(
            edited.items.iter().all(|item| item.pomodoro_close.is_none()
                || item
                    .pomodoro_close
                    .as_ref()
                    .is_some_and(|spec| spec.size.is_none())),
            "{raw}"
        );
        assert_eq!(edited.items.len(), 1, "{raw} stays one item");
    }
}

#[test]
fn sized_teaching_errors_match_in_both_parsers() {
    assert_error_parity(
        "0x",
        "`0x` needs at least 1 block: write `1x` to close at 5 minutes, or `x0` to reset the session",
    );
    assert_error_parity(
        "00x",
        "`00x` needs at least 1 block: write `1x` to close at 5 minutes, or `x0` to reset the session",
    );
    assert_error_parity(
        "100x",
        "`100x` is too large: a close size is 1–99 five-minute blocks (`12x` is an hour)",
    );
    assert_error_parity(
        "0910x",
        "`0910x` is too large: a close size is 1–99 five-minute blocks (`12x` is an hour)",
    );
    assert_error_parity(
        "001x",
        "`001x` is too large: a close size is 1–99 five-minute blocks (`12x` is an hour)",
    );
    let long = format!("{}x", "9".repeat(40));
    let err = execute(&long).expect_err("long run");
    assert!(err.contains("too large"), "{err}");
    assert!(!err.contains("overflow"), "{err}");

    assert_error_parity(
        "=2x",
        "`=2x` is not a start: write `2x` to close at 10 minutes, or `=2#x` to start a Pomodoro named X",
    );
    assert_error_parity(
        "=3x",
        "`=3x` is not a start: write `3x` to close at 15 minutes, or `=3#x` to start a Pomodoro named X",
    );
    assert_error_parity(
        "2x=3",
        "`2x=3` mixes a close size with a past record: `x=` takes its size after `=`; write `x=3` to record 15 minutes, or `2x =3` to close at 10 minutes and then start",
    );
    let hash = execute("2x#bugs").expect_err("hash");
    assert!(hash.contains("`2x` always closes"), "{hash}");
    assert!(hash.contains("2x =bugs"), "{hash}");
    let parse = editor("2x#bugs");
    assert_eq!(parse.diagnostics[0].message, hash);
    assert_eq!(parse.diagnostics[0].code, "invalid_pomodoro_close");
}

#[test]
fn unsized_neighbours_keep_their_meanings() {
    execute("x=3").expect("past");
    execute("=3").expect("start");
    execute("=2#x").expect("hash start named X");
    let retired = execute("=x").expect_err("retired");
    assert!(retired.contains("is retired"), "{retired}");
    assert!(is_session_chain_token("=3x"));
    assert!(is_session_chain_token("2x"));
    assert_eq!(split_capture_draft("x 2x faster").items.len(), 1);
}

#[test]
fn sized_operator_spans_cover_digits_and_x() {
    let parse = parse_for_editor("2x!2");
    assert_eq!(
        ranges(&parse.items[0]),
        vec![
            (0, 2, SpanKind::PomodoroClose),
            (2, 4, SpanKind::PomodoroCloseComplete),
        ]
    );
    let twelve = parse_for_editor("12x");
    assert_eq!(
        ranges(&twelve.items[0]),
        vec![(0, 3, SpanKind::PomodoroClose)]
    );
    let utf8 = parse_for_editor("café\n\n2x");
    assert_eq!(utf8.items.len(), 2);
    let start = "café\n\n".len();
    assert_eq!(
        ranges(&utf8.items[1]),
        vec![(start, start + 2, SpanKind::PomodoroClose)]
    );
}

#[test]
fn sized_completion_needs_sit_after_the_prefix() {
    let pending = parse_for_editor("2x1,");
    assert_eq!(pending.items[0].mode, EditorMode::Incomplete);
    assert_eq!(pending.items[0].needs, vec![Need::PomodoroCloseTask]);
    assert_eq!(
        pending.items[0].pomodoro_close.as_ref().unwrap().size,
        Some(size("2"))
    );
    assert_eq!(
        ranges(&pending.items[0]),
        vec![
            (0, 2, SpanKind::PomodoroClose),
            (2, 3, SpanKind::PomodoroCloseInProgress),
            (3, 4, SpanKind::InteractivePlaceholder),
        ]
    );
    let tilde = parse_for_editor("2x~");
    assert_eq!(tilde.items[0].needs, vec![Need::PomodoroCloseTask]);
    assert_eq!(
        tilde.items[0].pomodoro_close.as_ref().unwrap().size,
        Some(size("2"))
    );
    assert!(field("12x", 1).is_none(), "size digits are not a field");
    assert!(field("12x", 3).is_none());
}

#[test]
fn rewrite_draft_expands_sized_close_outcome_aliases() {
    for (raw, expected, alias, sigil) in [
        ("2xx", "2x!", "x", '!'),
        ("2x1s", "2x1*", "s", '*'),
        ("^r:id=2xs", "^r:id=2x*", "s", '*'),
        ("--1 2xs", "--1 2x*", "s", '*'),
        ("02xx", "02x!", "x", '!'),
    ] {
        let cursor = raw.len();
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, Some(RewriteRule::ExpandSigilAlias), "{raw}");
        assert_eq!(rewrite.text, expected, "{raw}");
        assert_eq!(rewrite.edits[0].start, cursor - 1, "{raw}");
        assert_eq!(rewrite.edits[0].replacement, sigil.to_string(), "{raw}");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some(format!("Typed {sigil} for {alias}").as_str()),
            "{raw}"
        );
        let again = rewrite_draft(&rewrite.text, rewrite.cursor);
        assert_eq!(again.rule, None, "idempotent {raw}");
        let parsed = parse_for_editor(expected);
        assert!(
            parsed.items.iter().any(|item| item
                .pomodoro_close
                .as_ref()
                .is_some_and(|spec| spec.size.is_some())),
            "{expected} should keep the size"
        );
    }
}

#[test]
fn rewrite_draft_sized_close_aliases_stay_literal() {
    let cases: &[(&str, usize)] = &[
        ("x made 2xx", 10),
        ("x 2xx", 5),
        ("= 2xx", 5),
        ("Plan 2xx", 8),
        ("2Xx", 3),
        ("0xs", 3),
        ("100xs", 5),
        ("2xS", 3),
        ("`2xx`", 4),
        ("[[2xx]]", 6),
    ];
    for &(raw, cursor) in cases {
        let rewrite = rewrite_draft(raw, Some(cursor));
        assert_eq!(rewrite.rule, None, "{raw:?} at {cursor}");
        assert_eq!(rewrite.text, raw);
    }
    for raw in ["2xx", "2x1s", "--1 2xs"] {
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None, "{raw} without cursor");
        assert_eq!(rewrite.text, raw);
    }
    let utf8 = "café\n\n2xx";
    let rewrite = rewrite_draft(utf8, Some(utf8.len()));
    assert_eq!(rewrite.text, "café\n\n2x!");
    assert_eq!(rewrite.edits[0].start, utf8.len() - 1);
    assert!(utf8.is_char_boundary(rewrite.edits[0].start));
    let crlf = "Plan\r\n\r\n2xx";
    let rewrite = rewrite_draft(crlf, Some(crlf.len()));
    assert_eq!(rewrite.text, "Plan\r\n\r\n2x!");
}

fn ranges(item: &EditorItemParse) -> Vec<(usize, usize, SpanKind)> {
    item.spans
        .iter()
        .map(|span| (span.start, span.end, span.kind))
        .collect()
}

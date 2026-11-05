//! `x=` past Pomodoro grammar: lexer, validation, stacks, and
//! editor/execution agreement.

use super::super::editor_model::*;
use super::super::model::*;
use super::*;

fn past_spec(parsed: &ParsedCaptureText) -> &PomodoroPastSpec {
    match &parsed.kind {
        CaptureKind::PomodoroPast { spec } => spec,
        other => panic!("expected past, got {other:?}"),
    }
}

fn editor_past(parse: &EditorParse) -> &PomodoroPastSpec {
    parse.pomodoro_past.as_ref().expect("expected past spec")
}

#[test]
fn past_token_table_reports_expected_specs() {
    // (typed, expected blocks, expected blocks_typed, expected name,
    // repair, close raw)
    let rows = vec![
        ("x=", 5, false, None, false, ""),
        ("x=3", 3, true, None, false, ""),
        ("x=3!", 3, true, None, true, ""),
        ("x=build", 5, false, Some("BUILD"), false, ""),
        ("x=build=", 5, false, Some("BUILD"), false, ""),
        ("x=fix3", 3, true, Some("FIX"), false, ""),
        ("x=3fix", 3, true, Some("FIX"), false, ""),
        ("x=build2", 2, true, Some("BUILD"), false, ""),
        ("x=2fa", 2, true, Some("FA"), false, ""),
        ("x=2#deep-work", 2, true, Some("DEEP-WORK"), false, ""),
        ("x=3fix=*3", 3, true, Some("FIX"), false, "*3"),
        ("x=fix3=*3", 3, true, Some("FIX"), false, "*3"),
        ("x=3fix!", 3, true, Some("FIX"), true, ""),
        ("x=fix3!=*3", 3, true, Some("FIX"), true, "*3"),
        ("x=#deep-work", 5, false, Some("DEEP-WORK"), false, ""),
        ("x=3#sase-v18", 3, true, Some("SASE-V18"), false, ""),
        ("x=3#\"DEEP WORK\"", 3, true, Some("DEEP WORK"), false, ""),
        ("x==1!2", 5, false, None, false, "1!2"),
        ("x==!", 5, false, None, false, "!"),
        ("x=!", 5, false, None, true, ""),
        ("x=!=!", 5, false, None, true, "!"),
        ("x=3!=1", 3, true, None, true, "1"),
        ("x==0", 5, false, None, false, "0"),
    ];
    for (typed, blocks, blocks_typed, name, repair, close_raw) in rows {
        let parsed = execute(typed).unwrap_or_else(|e| panic!("{typed}: {e}"));
        let spec = past_spec(&parsed);
        assert_eq!(spec.raw, typed, "{typed}");
        assert_eq!(spec.blocks, blocks, "{typed}");
        assert_eq!(spec.blocks_typed, blocks_typed, "{typed}");
        assert_eq!(spec.name.as_deref(), name, "{typed}");
        assert_eq!(spec.repair, repair, "{typed}");
        assert_eq!(spec.close.raw, close_raw, "{typed}");
        assert_eq!(
            spec.stack,
            PastStackPos {
                index: 1,
                size: 1,
                offset_blocks: 0,
                pin: None,
                lead_blocks: None,
            },
            "{typed}"
        );
        // Editor agrees.
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::PomodoroPast, "{typed}");
        assert!(edit.diagnostics.is_empty(), "{typed}");
        let espec = editor_past(&edit);
        assert_eq!(espec.blocks, blocks, "{typed}");
        assert_eq!(espec.name.as_deref(), name, "{typed}");
        assert_eq!(espec.repair, repair, "{typed}");
        assert_eq!(espec.close.raw, close_raw, "{typed}");
        assert_eq!(edit.section.as_deref(), name, "{typed}");
    }
}

#[test]
fn past_both_spellings_produce_equal_specs() {
    let a = execute("x=3fix=*3").expect("first");
    let b = execute("x=fix3=*3").expect("second");
    let sa = past_spec(&a);
    let sb = past_spec(&b);
    assert_ne!(sa.raw, sb.raw);
    assert_eq!(sa.blocks, sb.blocks);
    assert_eq!(sa.name, sb.name);
    assert_eq!(sa.repair, sb.repair);
    assert_eq!(sa.close.raw, sb.close.raw);
    // Minutes are blocks * 5; stacks are single.
    assert_eq!(sa.blocks * 5, 15);
    assert_eq!(sa.stack, sb.stack);
}

#[test]
fn past_teaching_errors_name_token_and_rewrite() {
    let rows = vec![
        ("x=3-1", "x=3"),
        ("x=-2", "x="),
        ("x=3-", "x=3"),
        ("x=fix-", "x=fix"),
        ("x=fix3-1", "x=fix3"),
        ("x=3-1fix", "x=3fix"),
        ("x=fix@3", "x=fix3"),
        ("x=build@3", "x=build3"),
        ("x=build@", "x=build3"),
        ("x=build@0", "x=build1"),
        ("x=@3", "x=3"),
        ("x=@", "x=3"),
        ("x=3@2", "x=3"),
        ("x=3#fix", "x=3fix"),
        ("x=#fix", "x=fix"),
        ("x=#build", "x=build"),
        ("x=2#build@4", "x=2build"),
        ("x=3#fix@2", "x=3fix"),
        ("x=2#\"DEEP WORK\"@4", "x=2#\"DEEP WORK\""),
        ("x=#\"DEEP WORK\"@3", "x=3#\"DEEP WORK\""),
        ("x=*3", "x==*3"),
        ("x=0", "x=1"),
        ("x=fix0", "x=fix1"),
        ("x=0fix", "x=1fix"),
        ("x=0!=1", "x=1!=1"),
        ("x=1!2", "x==1!2"),
        ("x=!2", "x==!2"),
        ("x=fix!2", "x=fix=!2"),
        ("x=3x", "x=3#x"),
        ("x=deep-work", "x=#deep-work"),
        ("x=3deep-work", "x=3#deep-work"),
        // Rewrites come from the typed parts, never from the table rows.
        ("x=*5", "x==*5"),
        ("x=build*5", "x=build=*5"),
        ("x=3!5", "x==3!5"),
        ("x=build!2", "x=build!=!2"),
        ("x=3fix!2", "x=3fix=!2"),
        ("x=build@3~2", "x=build3=~2"),
        ("x=fix3!2", "x=fix3=!2"),
        ("x=3fix2", "x=3fix"),
        ("x=3fix2", "x=fix2"),
        ("x=4-2=1", "x=4=1"),
    ];
    for (typed, rewrite) in rows {
        let err =
            execute(typed).expect_err(format!("{typed} should fail").as_str());
        assert!(err.contains(&format!("`{typed}`")), "{typed}: {err}");
        assert!(err.contains(&format!("`{rewrite}`")), "{typed}: {err}");
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::PomodoroPast, "{typed}");
        assert_eq!(edit.diagnostics.len(), 1, "{typed}");
        assert_eq!(
            edit.diagnostics[0].code, "invalid_pomodoro_past",
            "{typed}"
        );
        assert_eq!(edit.diagnostics[0].message, err, "{typed}");
    }
    // Merge/split shapes.
    for typed in ["x=/foo", "x=*build"] {
        let err = execute(typed).expect_err("should fail");
        assert!(err.contains(typed));
        assert!(
            err.contains("can't split")
                || err.contains("can't merge")
                || err.contains("split")
                || err.contains("merge"),
            "{err}"
        );
    }
    // Oversized reuses the start too-large wording.
    let big = format!("x={}", "9".repeat(30));
    let err = execute(&big).expect_err("oversized should fail");
    assert!(err.contains("too large"), "{err}");
}

#[test]
fn past_unfinished_uses_existing_needs() {
    for typed in ["x=#", "x=3#", "x=#\"DEEP"] {
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::Incomplete, "{typed}");
        assert_eq!(edit.needs, vec![Need::PomodoroName], "{typed}");
        assert!(execute(typed).is_err(), "{typed}");
    }
    // A dangling `@` is retired, not unfinished: timing glues to the name.
    for typed in ["x=build@", "x=@"] {
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::PomodoroPast, "{typed}");
        assert_eq!(edit.diagnostics.len(), 1, "{typed}");
        assert_eq!(
            edit.diagnostics[0].code, "invalid_pomodoro_past",
            "{typed}"
        );
        assert!(execute(typed).is_err(), "{typed}");
    }
    for typed in ["x=3=1,", "x==~"] {
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::Incomplete, "{typed}");
        assert_eq!(edit.needs, vec![Need::PomodoroCloseTask], "{typed}");
    }
    let edit = editor("x= 2");
    assert_eq!(edit.mode, EditorMode::Incomplete, "x= 2");
    assert_eq!(edit.needs, vec![Need::PomodoroCloseLogText]);
}

#[test]
fn past_prose_guards_stay_prose() {
    for typed in ["X=3", "Plan x=3"] {
        let parsed = execute(typed).expect("prose should parse");
        assert!(
            !matches!(parsed.kind, CaptureKind::PomodoroPast { .. }),
            "{typed}"
        );
        let edit = editor(typed);
        assert_ne!(edit.mode, EditorMode::PomodoroPast, "{typed}");
        assert!(edit.pomodoro_past.is_none(), "{typed}");
    }
}

#[test]
fn past_names_stop_at_operators_and_literals_hold_them() {
    // `!` and `=` end unquoted names.
    let parsed = execute("x=fix!2").expect_err("should be teaching error");
    assert!(parsed.contains("x=fix=!2"));
    // `!` and `=` inside a quoted literal stay name content (the error
    // names the whole literal), and then fail the `=#"…"` name rules.
    let err = execute("x=#\"A!B=C\"").expect_err("literal name rules");
    assert!(err.contains("Pomodoro name `A!B=C`"), "{err}");
    let parsed = execute("x=#\"A&B\"=1").expect("literal before close");
    let spec = past_spec(&parsed);
    assert_eq!(spec.name.as_deref(), Some("A&B"));
    assert_eq!(spec.close.raw, "1");
    // Literals canonicalize like `=#"…"`.
    let parsed = execute("x=#\"deep   work\"").expect("spaced literal");
    assert_eq!(past_spec(&parsed).name.as_deref(), Some("DEEP WORK"));
}

#[test]
fn past_names_follow_start_name_rules() {
    // A bare name cannot contain `-`: it teaches the `#` spelling, while
    // `#` names and literals keep their dashes.
    for (typed, name, blocks) in [
        ("x=#deep-work", "DEEP-WORK", 5),
        ("x=2#deep-work", "DEEP-WORK", 2),
        ("x=3#deep-work", "DEEP-WORK", 3),
        ("x=#\"A-B\"", "A-B", 5),
        ("x=3#\"DEEP WORK\"", "DEEP WORK", 3),
    ] {
        let parsed = execute(typed).unwrap_or_else(|e| panic!("{typed}: {e}"));
        let spec = past_spec(&parsed);
        assert_eq!(spec.name.as_deref(), Some(name), "{typed}");
        assert_eq!(spec.blocks, blocks, "{typed}");
        let edit = editor(typed);
        assert!(edit.diagnostics.is_empty(), "{typed}");
        assert_eq!(editor_past(&edit).name.as_deref(), Some(name), "{typed}");
    }
    // Bytes `=name` rejects are rejected with the start-family wording.
    for (typed, name) in [
        ("x=a$b", "a$b"),
        ("x=build#2", "build#2"),
        ("x=#b:x", "b:x"),
        ("x=#\"a$b\"", "a$b"),
    ] {
        let err = execute(typed).expect_err(typed);
        assert!(
            err.contains(&format!("Pomodoro name `{name}` in `{typed}`")),
            "{typed}: {err}"
        );
        let edit = editor(typed);
        assert_eq!(edit.diagnostics.len(), 1, "{typed}");
        assert_eq!(edit.diagnostics[0].code, "invalid_pomodoro_past");
    }
}

#[test]
fn past_compatibility_breaks_parse_as_chains() {
    // `x=3` was prose.
    assert!(matches!(
        execute("x=3").unwrap().kind,
        CaptureKind::PomodoroPast { .. }
    ));
    // `x x=3` was a close with Work Log entry `x=3`; now Close + Past.
    let draft = execute_draft("x x=3").expect("chain");
    assert_eq!(draft.items.len(), 2);
    assert!(matches!(
        draft.items[0].parsed.kind,
        CaptureKind::PomodoroClose { .. }
    ));
    assert!(matches!(
        draft.items[1].parsed.kind,
        CaptureKind::PomodoroPast { .. }
    ));
    // `=3 x=3` was a start shape error; now Start + Past.
    let draft = execute_draft("=3 x=3").expect("start+past");
    assert_eq!(draft.items.len(), 2);
    assert!(matches!(
        draft.items[0].parsed.kind,
        CaptureKind::PomodoroStart { .. }
    ));
    assert!(matches!(
        draft.items[1].parsed.kind,
        CaptureKind::PomodoroPast { .. }
    ));
}

#[test]
fn past_chain_ownership_and_stacks() {
    // Same-line chain tokens become consecutive items in one stack.
    let draft = execute_draft("x=build2 x=fix3").expect("stack");
    assert_eq!(draft.items.len(), 2);
    let a = match &draft.items[0].parsed.kind {
        CaptureKind::PomodoroPast { spec } => spec,
        other => panic!("first not past: {other:?}"),
    };
    let b = match &draft.items[1].parsed.kind {
        CaptureKind::PomodoroPast { spec } => spec,
        other => panic!("second not past: {other:?}"),
    };
    assert_eq!(
        (a.stack.index, a.stack.size, a.stack.offset_blocks),
        (1, 2, 3)
    );
    assert_eq!(
        (b.stack.index, b.stack.size, b.stack.offset_blocks),
        (2, 2, 0)
    );
    assert_eq!(a.blocks, 2);
    assert_eq!(b.blocks, 3);
    // Blank-line-separated items also stack.
    let draft = execute_draft("x=2\n\nx=3").expect("blank stack");
    assert_eq!(draft.items.len(), 2);
    // Another item breaks the stack.
    let draft = execute_draft("x=2\n\nbuy milk\n\nx=3").expect("broken");
    assert_eq!(draft.items.len(), 3);
    for item in &draft.items {
        if let CaptureKind::PomodoroPast { spec } = &item.parsed.kind {
            assert_eq!(spec.stack.size, 1);
        }
    }
    // Child bullets attach to the last close, which may be `x=`.
    let draft = execute_draft("x=3\n- 1 wired the lexer").expect("bullets");
    assert_eq!(draft.items.len(), 1);
    let spec = match &draft.items[0].parsed.kind {
        CaptureKind::PomodoroPast { spec } => spec,
        other => panic!("not past: {other:?}"),
    };
    assert_eq!(spec.close.log.len(), 1);
}

#[test]
fn past_segment_suffix_valid_shapes() {
    // (typed, expected raw suffix, kind, index, edge, blocks, name)
    let rows = vec![
        ("x=~", "~", "tail", None, "start", 5, None),
        ("x=3~", "~", "tail", None, "start", 3, None),
        ("x=fix~2", "~2", "index", Some(2), "start", 5, Some("FIX")),
        ("x=fix2~2+", "~2+", "index", Some(2), "end", 2, Some("FIX")),
        (
            "x=#\"DEEP WORK\"~1",
            "~1",
            "index",
            Some(1),
            "start",
            5,
            Some("DEEP WORK"),
        ),
        (
            "x=2#deep-work~1+",
            "~1+",
            "index",
            Some(1),
            "end",
            2,
            Some("DEEP-WORK"),
        ),
        ("x=~~", "~~", "head", None, "end", 5, None),
        ("x=2~1!=!1", "~1", "index", Some(1), "start", 2, None),
    ];
    for (typed, raw, kind, index, edge, blocks, name) in rows {
        let parsed = execute(typed).unwrap_or_else(|e| panic!("{typed}: {e}"));
        let spec = past_spec(&parsed);
        assert_eq!(spec.blocks, blocks, "{typed}");
        assert_eq!(spec.name.as_deref(), name, "{typed}");
        let segment = spec.segment.as_ref().expect("{typed} has segment");
        assert_eq!(segment.raw, raw, "{typed}");
        let kind_label = match segment.kind {
            PastSegmentKind::Tail => "tail",
            PastSegmentKind::Index => "index",
            PastSegmentKind::Head => "head",
        };
        assert_eq!(kind_label, kind, "{typed}");
        assert_eq!(segment.index, index, "{typed}");
        let edge_label = match segment.edge {
            PastEdge::Start => "start",
            PastEdge::End => "end",
        };
        assert_eq!(edge_label, edge, "{typed}");
        // Editor agrees and emits the segment span.
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::PomodoroPast, "{typed}");
        assert!(edit.diagnostics.is_empty(), "{typed}");
        let espec = editor_past(&edit);
        assert_eq!(
            espec.segment.as_ref().map(|s| s.raw.as_str()),
            Some(raw),
            "{typed}"
        );
        assert!(
            edit.spans
                .iter()
                .any(|span| span.kind == SpanKind::PomodoroPastSegment),
            "{typed} has segment span"
        );
    }
}

#[test]
fn past_segment_suffix_lexical_teaching_rows() {
    let rows = vec![
        ("x=~0", "x=~"),
        ("x=3~0", "x=3~"),
        ("x=~+", "x=~"),
        ("x=~0+", "x=~"),
        ("x=~~+", "x=~~"),
        ("x=~~2", "x=~~"),
        ("x=2~01", "x=2~1"),
        ("x=2~123", "future clock"),
        ("x=2~0800", "future clock"),
        ("x=~2fix", "x=fix~2"),
        ("x=~2#deep-work", "x=#deep-work~2"),
        ("x=2~1~2", "one segment suffix"),
        ("x=2~~1", "one segment suffix"),
        ("x=3~1!+", "x=3~1+!"),
        ("x=2~1,3", "x=2=~1,3"),
        ("x=2-1", "x=2~1"),
        ("x=-", "x=~"),
        ("x=3-", "x=3~"),
        ("x=deep-work", "x=#deep-work"),
        ("x=deep~work", "x=#deep-work"),
    ];
    for (typed, rewrite) in rows {
        let err =
            execute(typed).expect_err(format!("{typed} should fail").as_str());
        assert!(err.contains(&format!("`{typed}`")), "{typed}: {err}");
        assert!(err.contains(rewrite), "{typed}: {err}");
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::PomodoroPast, "{typed}");
        assert_eq!(edit.diagnostics.len(), 1, "{typed}");
        assert_eq!(
            edit.diagnostics[0].code, "invalid_pomodoro_past",
            "{typed}"
        );
        assert_eq!(edit.diagnostics[0].message, err, "{typed}");
    }
}

#[test]
fn past_run_pins_and_diagnostics() {
    // A start suffix on the first item pins the run forward.
    let draft = execute_draft("x=a~2 x=b").expect("pinned start");
    assert_eq!(draft.items.len(), 2);
    for item in &draft.items {
        if let CaptureKind::PomodoroPast { spec } = &item.parsed.kind {
            assert_eq!(spec.stack.pin, Some(PastEdge::Start));
        }
    }
    let (first_lead, second_lead) =
        match (&draft.items[0].parsed.kind, &draft.items[1].parsed.kind) {
            (
                CaptureKind::PomodoroPast { spec: a },
                CaptureKind::PomodoroPast { spec: b },
            ) => (a.stack.lead_blocks, b.stack.lead_blocks),
            _ => panic!("not past"),
        };
    assert_eq!((first_lead, second_lead), (Some(0), Some(5)));
    // An end suffix on the last item pins the run backward.
    let draft = execute_draft("x=email1 x=read2~2+").expect("pinned end");
    for item in &draft.items {
        if let CaptureKind::PomodoroPast { spec } = &item.parsed.kind {
            assert_eq!(spec.stack.pin, Some(PastEdge::End));
        }
    }
    // A start suffix anywhere else is misplaced.
    let err = execute_draft("x=a x=b~2").expect_err("misplaced start");
    assert!(err.contains("can't place this run"), "{err}");
    assert!(err.contains("x=a~2 x=b"), "{err}");
    let edit = editor("x=a\n\nx=b~2");
    let bad = edit
        .items
        .iter()
        .find(|it| !it.diagnostics.is_empty())
        .expect("misplaced start has a diagnostic");
    assert_eq!(
        bad.diagnostics[0].code, "invalid_pomodoro_past",
        "misplaced start code"
    );
    assert!(
        bad.diagnostics[0].message.contains("can't place this run"),
        "misplaced start message"
    );
    // An end suffix anywhere else is misplaced.
    let err = execute_draft("x=a~~ x=b").expect_err("misplaced end");
    assert!(err.contains("can't place this run"), "{err}");
    assert!(err.contains("x=a x=b~~"), "{err}");
    // Two suffixes in one run report the second place.
    let err = execute_draft("x=a~2 x=b~1").expect_err("double pin");
    assert!(err.contains("adds a second place"), "{err}");
}

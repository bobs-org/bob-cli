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
        ("x=#build", 5, false, Some("BUILD"), false, ""),
        ("x=build=", 5, false, Some("BUILD"), false, ""),
        ("x=2#fix", 2, true, Some("FIX"), false, ""),
        ("x=fix@2", 2, true, Some("FIX"), false, ""),
        ("x=2#fix=*3", 2, true, Some("FIX"), false, "*3"),
        ("x=fix@2=*3", 2, true, Some("FIX"), false, "*3"),
        ("x==1!2", 5, false, None, false, "1!2"),
        ("x==!", 5, false, None, false, "!"),
        ("x=!", 5, false, None, true, ""),
        ("x=!=!", 5, false, None, true, "!"),
        ("x=3!=1", 3, true, None, true, "1"),
        ("x==0", 5, false, None, false, "0"),
        ("x=#\"DEEP WORK\"@3", 3, true, Some("DEEP WORK"), false, ""),
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
        assert_eq!(spec.stack, PastStackPos { index: 1, size: 1, offset_blocks: 0 }, "{typed}");
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
    let a = execute("x=2#fix=*3").expect("first");
    let b = execute("x=fix@2=*3").expect("second");
    let sa = past_spec(&a);
    let sb = past_spec(&b);
    assert_ne!(sa.raw, sb.raw);
    assert_eq!(sa.blocks, sb.blocks);
    assert_eq!(sa.name, sb.name);
    assert_eq!(sa.repair, sb.repair);
    assert_eq!(sa.close.raw, sb.close.raw);
    // Minutes are blocks * 5; stacks are single.
    assert_eq!(sa.blocks * 5, 10);
    assert_eq!(sa.stack, sb.stack);
}

#[test]
fn past_teaching_errors_name_token_and_rewrite() {
    let rows = vec![
        ("x=3-1", "x=3"),
        ("x=-2", "x="),
        ("x=3-", "x=3"),
        ("x=build@3-1", "x=build@3"),
        ("x=3#fix@2", "x=3#fix"),
        ("x=~2", "x==~2"),
        ("x=3~2", "x=3=~2"),
        ("x=build~2", "x=build=~2"),
        ("x=*3", "x==*3"),
        ("x=0", "x=1"),
        ("x=build@0", "x=build@1"),
        ("x=1!2", "x==1!2"),
        ("x=!2", "x==!2"),
        ("x=fix!2", "x=fix=!2"),
        ("x=3x", "x=3#x"),
        ("x=3build", "x=3#build"),
    ];
    for (typed, rewrite) in rows {
        let err = execute(typed).expect_err(format!("{typed} should fail").as_str());
        assert!(err.contains(typed), "{typed}: {err}");
        assert!(err.contains(rewrite), "{typed}: {err}");
        let edit = editor(typed);
        assert_eq!(edit.mode, EditorMode::PomodoroPast, "{typed}");
        assert_eq!(edit.diagnostics.len(), 1, "{typed}");
        assert_eq!(edit.diagnostics[0].code, "invalid_pomodoro_past", "{typed}");
        assert!(edit.diagnostics[0].message.contains(typed), "{typed}");
        assert!(edit.diagnostics[0].message.contains(rewrite), "{typed}");
    }
    // Merge/split shapes.
    for typed in ["x=/foo", "x=*build"] {
        let err = execute(typed).expect_err("should fail");
        assert!(err.contains(typed));
        assert!(err.contains("can't split") || err.contains("can't merge") || err.contains("split") || err.contains("merge"), "{err}");
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
    let edit = editor("x=build@");
    assert_eq!(edit.mode, EditorMode::Incomplete);
    assert_eq!(edit.needs, vec![Need::PomodoroStartTiming]);
    assert!(execute("x=build@").is_err());
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
        assert!(!matches!(parsed.kind, CaptureKind::PomodoroPast { .. }), "{typed}");
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
    // Quoted literals may contain `!` and `=`.
    let parsed = execute("x=#\"A!B=C\"").expect("literal with operators");
    let spec = past_spec(&parsed);
    assert_eq!(spec.name.as_deref(), Some("A!B=C"));
}

#[test]
fn past_compatibility_breaks_parse_as_chains() {
    // `x=3` was prose.
    assert!(matches!(execute("x=3").unwrap().kind, CaptureKind::PomodoroPast { .. }));
    // `x x=3` was a close with Work Log entry `x=3`; now Close + Past.
    let draft = execute_draft("x x=3").expect("chain");
    assert_eq!(draft.items.len(), 2);
    assert!(matches!(draft.items[0].parsed.kind, CaptureKind::PomodoroClose { .. }));
    assert!(matches!(draft.items[1].parsed.kind, CaptureKind::PomodoroPast { .. }));
    // `=3 x=3` was a start shape error; now Start + Past.
    let draft = execute_draft("=3 x=3").expect("start+past");
    assert_eq!(draft.items.len(), 2);
    assert!(matches!(draft.items[0].parsed.kind, CaptureKind::PomodoroStart { .. }));
    assert!(matches!(draft.items[1].parsed.kind, CaptureKind::PomodoroPast { .. }));
}

#[test]
fn past_chain_ownership_and_stacks() {
    // Same-line chain tokens become consecutive items in one stack.
    let draft = execute_draft("x=build@2 x=fix@3").expect("stack");
    assert_eq!(draft.items.len(), 2);
    let a = match &draft.items[0].parsed.kind {
        CaptureKind::PomodoroPast { spec } => spec,
        other => panic!("first not past: {other:?}"),
    };
    let b = match &draft.items[1].parsed.kind {
        CaptureKind::PomodoroPast { spec } => spec,
        other => panic!("second not past: {other:?}"),
    };
    assert_eq!((a.stack.index, a.stack.size, a.stack.offset_blocks), (1, 2, 3));
    assert_eq!((b.stack.index, b.stack.size, b.stack.offset_blocks), (2, 2, 0));
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

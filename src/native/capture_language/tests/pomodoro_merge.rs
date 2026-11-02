//! Whole-item `=*` merge grammar, editor spans, and completion fields.

use super::super::editor_model::*;
use super::super::model::*;
use super::*;

#[test]
fn merge_token_parses_src_and_dests() {
    let parsed = execute("=*build#fix").expect("merge");
    let CaptureKind::PomodoroMerge { spec } = parsed.kind else {
        panic!("expected merge kind: {:?}", parsed.kind);
    };
    assert_eq!(spec.raw, "=*build#fix");
    assert_eq!(spec.src.as_deref(), Some("build"));
    assert_eq!(spec.dests.len(), 1);
    assert_eq!(spec.dests[0].text, "fix");
    assert!(!spec.dests[0].literal);
}

#[test]
fn merge_into_current_omits_src() {
    let parsed = execute("=*#fix#read").expect("merge current");
    let CaptureKind::PomodoroMerge { spec } = parsed.kind else {
        panic!("expected merge kind");
    };
    assert!(spec.src.is_none());
    assert_eq!(
        spec.dests
            .iter()
            .map(|dest| dest.text.as_str())
            .collect::<Vec<_>>(),
        vec!["fix", "read"]
    );
}

#[test]
fn dest_less_merge_is_complete() {
    for raw in ["=*", "=*build", "=*\"DEEP WORK\""] {
        let parsed = execute(raw).expect(raw);
        let CaptureKind::PomodoroMerge { spec } = parsed.kind else {
            panic!("expected merge kind for {raw}");
        };
        assert!(spec.dests.is_empty(), "{raw}");
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroMerge, "{raw}");
        assert!(parse.needs.is_empty(), "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
        assert_eq!(parse.spans[0].kind, SpanKind::PomodoroMerge, "{raw}");
    }
}

#[test]
fn dest_less_merge_with_extra_text_is_invalid() {
    let error = execute("=* extra").expect_err("extra text");
    assert!(error.contains("must be the whole capture item"), "{error}");
    let parse = editor("=* extra");
    assert_eq!(parse.mode, EditorMode::PomodoroMerge);
    assert_eq!(codes(&parse), vec!["invalid_pomodoro_merge"]);
}

#[test]
fn trailing_hash_needs_dest_in_the_editor() {
    let parse = editor("=*build#");
    assert_eq!(parse.mode, EditorMode::PomodoroMerge);
    assert_eq!(parse.needs, vec![Need::PomodoroMergeDest]);
    assert!(parse.diagnostics.is_empty());
    let error = execute("=*build#").expect_err("trailing hash");
    assert!(error.contains("incomplete"), "{error}");
}

#[test]
fn digit_and_marker_star_forms_stay_retired_closes() {
    for (raw, rewrite) in [
        ("=*1", "x*1"),
        ("=*!2", "x*!2"),
        ("=**", "x**"),
        ("=*~1", "x*~1"),
        ("=*@2", "x*@2"),
        ("=*,", "x*,"),
    ] {
        let error = execute(raw).expect_err(raw);
        assert!(error.contains("is retired"), "{raw}: {error}");
        assert!(error.contains(rewrite), "{raw}: {error}");
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroClose, "{raw}");
        assert_eq!(codes(&parse), vec!["invalid_pomodoro_close"], "{raw}");
    }
}

#[test]
fn double_star_close_lookalike_stays_not_a_close() {
    let error = execute("==*").expect_err("==*");
    assert_eq!(
        error,
        "`==*` is not a close: close the running Pomodoro with `x*`"
    );
}

#[test]
fn merge_editor_spans_cover_sigil_and_names() {
    let parse = editor("=*build#fix");
    assert_eq!(parse.mode, EditorMode::PomodoroMerge);
    assert!(parse.diagnostics.is_empty());
    assert_eq!(
        ranges(&parse),
        vec![
            (0, 2, SpanKind::PomodoroMerge),
            (2, 7, SpanKind::PomodoroName),
            (8, 11, SpanKind::PomodoroName),
        ]
    );
    assert_eq!(
        parse.pomodoro_merge.as_ref().unwrap().src.as_deref(),
        Some("build")
    );
}

#[test]
fn merge_completion_src_and_dest_fields() {
    let src = field("=*", 2).expect("src field");
    assert_eq!(src.context, CompletionContext::PomodoroMergeSrc);
    assert_eq!(src.replacement, (2, 2));

    let dest = field("=*build#", 8).expect("dest field");
    assert_eq!(dest.context, CompletionContext::PomodoroMergeDest);
    assert_eq!(dest.replacement, (8, 8));

    let dest_typed = field("=*build#fi", 10).expect("typed dest");
    assert_eq!(dest_typed.context, CompletionContext::PomodoroMergeDest);
    assert_eq!(dest_typed.query, "fi");
}

#[test]
fn extra_text_is_a_shape_error() {
    let error = execute("=*build#fix extra").expect_err("extra");
    assert!(error.contains("must be the whole capture item"), "{error}");
    let parse = editor("=*build#fix extra");
    assert_eq!(parse.mode, EditorMode::PomodoroMerge);
    assert_eq!(codes(&parse), vec!["invalid_pomodoro_merge"]);
}

//! Exact-name `#"NAME"` literal conformance: every capture name position
//! lexes straight `"…"` and curly `“…”` quotes through the closing quote,
//! resolves by key equality, and treats an unterminated literal as an
//! incomplete editing state, never a mistake. The bare `=name` spelling
//! never takes quotes.

use super::super::completion::*;
use super::super::editor_model::*;
use super::super::model::*;
use super::*;

fn start_kind(raw: &str) -> CaptureKind {
    execute(raw).expect("parses").kind
}

#[test]
fn exact_name_start_reports_canonical_name_and_literal_flag() {
    let CaptureKind::PomodoroStart {
        pomodoro_name,
        pomodoro_name_is_literal,
        ..
    } = start_kind(r#"=#"DEEP WORK""#)
    else {
        panic!("named start");
    };
    assert_eq!(pomodoro_name.as_deref(), Some("DEEP WORK"));
    assert!(pomodoro_name_is_literal);

    let CaptureKind::PomodoroStart {
        pomodoro_name,
        pomodoro_name_is_literal,
        ..
    } = start_kind("=#\u{201c}SASE V18\u{201d}")
    else {
        panic!("curly named start");
    };
    assert_eq!(pomodoro_name.as_deref(), Some("SASE V18"));
    assert!(pomodoro_name_is_literal);
}

#[test]
fn exact_name_accepts_round_trip_fixtures() {
    // Spaces, punctuation, digits, and `+` composites keep their spelling.
    for (raw, canonical) in [
        (r#"=#"DEEP WORK""#, "DEEP WORK"),
        (r#"=#"SASE.MD""#, "SASE.MD"),
        (r#"=#"FIX-APOLLO""#, "FIX-APOLLO"),
        (r#"=#"2FA""#, "2FA"),
        (r#"=#"BOB + DECKS""#, "BOB + DECKS"),
        ("=#\u{201c}DEEP WORK\u{201d}", "DEEP WORK"),
    ] {
        let CaptureKind::PomodoroStart {
            pomodoro_name,
            pomodoro_name_is_literal,
            ..
        } = start_kind(raw)
        else {
            panic!("named start for {raw}");
        };
        assert_eq!(pomodoro_name.as_deref(), Some(canonical), "{raw}");
        assert!(pomodoro_name_is_literal, "{raw}");
    }
}

#[test]
fn exact_name_link_reports_canonical_name() {
    let parsed = editor(r#"Do work @r:id#"SASE V18""#);
    assert_eq!(parsed.mode, EditorMode::PomodoroTask);
    assert_eq!(parsed.section.as_deref(), Some("SASE V18"));
    assert!(parsed.needs.is_empty());

    let parsed = editor(r#"Do work @cash^goog-exit@#"DEEP WORK""#);
    assert_eq!(parsed.mode, EditorMode::ProjectNote);
    assert_eq!(parsed.section.as_deref(), Some("DEEP WORK"));
    assert!(parsed.needs.is_empty());
}

#[test]
fn unterminated_literal_is_incomplete_never_a_mistake() {
    let parsed = editor(r#"=#"DEEP"#);
    assert_eq!(parsed.needs, vec![Need::PomodoroName]);
    assert!(parsed.diagnostics.is_empty());

    let error = execute(r#"=#"DEEP"#).expect_err("refuses");
    assert!(error.contains("incomplete"), "{error}");

    let parsed = editor(r#"Do work @r:id#"SASE V"#);
    assert_eq!(parsed.needs, vec![Need::PomodoroName]);
    assert!(parsed.diagnostics.is_empty());
}

#[test]
fn invalid_literal_names_error() {
    // `@` is outside the Pomodoro-name charset even inside a literal.
    let error = execute(r#"=#"A@B""#).expect_err("refuses");
    assert!(error.contains("may contain only"), "{error}");

    // An empty literal names nothing.
    let error = execute("=#\"\"").expect_err("refuses");
    assert!(
        error.contains("incomplete") || error.contains("may contain only"),
        "{error}"
    );
}

#[test]
fn bare_name_spelling_never_takes_quotes() {
    // `="BOB"` is ordinary prose, never a literal: quotes only follow `#`.
    let parsed = execute(r#"="BOB""#).expect("prose");
    assert_eq!(parsed.kind, CaptureKind::Task);
}

#[test]
fn close_hash_teaches_with_decoded_name() {
    let error = execute(r#"x#"DEEP WORK""#).expect_err("refuses");
    assert!(error.contains("DEEP WORK"), "{error}");
    assert!(!error.contains("\"DEEP"), "{error}");
}

#[test]
fn literal_spans_cover_the_quotes() {
    let parsed = editor(r#"=#"DEEP WORK""#);
    assert!(parsed.diagnostics.is_empty());
    assert!(
        ranges(&parsed).contains(&(2, 13, SpanKind::PomodoroName)),
        "{:?}",
        ranges(&parsed)
    );

    let parsed = editor(r#"Do work @r:id#"SASE V18""#);
    assert!(parsed.diagnostics.is_empty());
    assert!(
        ranges(&parsed).contains(&(14, 24, SpanKind::PomodoroName)),
        "{:?}",
        ranges(&parsed)
    );
}

#[test]
fn partial_literal_completes_on_decoded_text() {
    let start = field(r#"=#"DEEP"#, 7).expect("start field");
    assert_eq!(start.context, CompletionContext::PomodoroStartName);
    assert_eq!(start.query, "DEEP");
    assert_eq!(start.replacement, (2, 7));

    let link = field(r#"@r:id#"SASE V"#, 13).expect("link field");
    assert_eq!(link.context, CompletionContext::PomodoroName);
    assert_eq!(link.query, "SASE V");
    assert_eq!(link.replacement, (6, 13));
}

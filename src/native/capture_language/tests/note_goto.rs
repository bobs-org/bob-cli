//! Whole-draft `//query` Go To Notes grammar: claim, teaching error, rollback.

use super::super::completion::*;
use super::super::draft::*;
use super::super::editor_model::*;
use super::super::model::*;
use super::super::tokens::*;
use super::*;

const T4_BARE: &str = "`//` opens the Go To Notes picker; pick a note to open it in Obsidian — it never captures";

fn t4(token: &str) -> String {
    format!(
        "`{token}` opens the Go To Notes picker; pick a note to open it in Obsidian — it never captures"
    )
}

#[test]
fn note_goto_claim_covers_every_table_query_row() {
    for (raw, query) in [
        ("//", ""),
        ("//cash", "cash"),
        ("//prj/sase", "prj/sase"),
        ("//2026-11-14", "2026-11-14"),
        ("  //cash\n", "cash"),
    ] {
        let expected = if query.is_empty() && raw == "//" {
            T4_BARE.to_string()
        } else {
            t4(raw.trim())
        };
        let error = execute(raw).expect_err(&format!("{raw:?} is a query"));
        assert_eq!(error, expected, "{raw:?}");

        let parsed = editor(raw);
        assert_eq!(parsed.mode, EditorMode::Incomplete, "{raw:?}");
        assert_eq!(parsed.needs, vec![Need::NoteGoto], "{raw:?}");
        assert_eq!(parsed.body, "", "{raw:?}");
        assert!(parsed.diagnostics.is_empty(), "{raw:?}");
        assert_eq!(
            ranges(&parsed),
            vec![(
                parsed.spans[0].start,
                parsed.spans[0].end,
                SpanKind::InteractivePlaceholder
            )],
            "{raw:?}"
        );

        let end = if raw.ends_with('\n') {
            raw.len() - 1
        } else {
            raw.len()
        };
        let completion = field(raw, end).expect(&format!("{raw:?} completes"));
        assert_eq!(completion.context, CompletionContext::NoteGoto, "{raw:?}");
        assert_eq!(completion.query, query, "{raw:?}");
    }
    let forced = parse_capture_text_with_options(
        "//cash",
        Some("work"),
        None,
        &CaptureParseOptions::routing_off(),
    )
    .expect_err("forced query");
    assert!(forced.contains("opens the Go To Notes picker"), "{forced}");
}

#[test]
fn note_goto_completion_field_reports_sigil_relative_queries() {
    let before = field("//cash", 0).expect("before sigil");
    assert_eq!(before.context, CompletionContext::NoteGoto);
    assert_eq!(before.query, "");
    assert_eq!(before.replacement, (0, 6));

    let offset_one = field("//cash", 1).expect("offset 1");
    assert_eq!(offset_one.context, CompletionContext::NoteGoto);
    assert_eq!(offset_one.query, "");
    assert_eq!(offset_one.replacement, (0, 6));

    let after_sigil = field("//cash", 2).expect("after sigil");
    assert_eq!(after_sigil.context, CompletionContext::NoteGoto);
    assert_eq!(after_sigil.query, "");
    assert_eq!(after_sigil.replacement, (0, 6));

    let middle = field("//cash", 4).expect("middle");
    assert_eq!(middle.context, CompletionContext::NoteGoto);
    assert_eq!(middle.query, "ca");
    assert_eq!(middle.replacement, (0, 6));

    let bare_before = field("//", 0).expect("bare before sigil");
    assert_eq!(bare_before.context, CompletionContext::NoteGoto);
    assert_eq!(bare_before.query, "");
    assert_eq!(bare_before.replacement, (0, 2));

    let bare_mid = field("//", 1).expect("bare offset 1");
    assert_eq!(bare_mid.context, CompletionContext::NoteGoto);
    assert_eq!(bare_mid.query, "");
    assert_eq!(bare_mid.replacement, (0, 2));

    let bare_end = field("//", 2).expect("bare end");
    assert_eq!(bare_end.context, CompletionContext::NoteGoto);
    assert_eq!(bare_end.query, "");
    assert_eq!(bare_end.replacement, (0, 2));
}

#[test]
fn note_goto_and_task_goto_claims_are_disjoint() {
    for raw in ["//", "//cash", "//prj/sase", "//2026-11-14", "  //cash\n"] {
        let draft = split_capture_draft(raw);
        assert_eq!(draft.items.len(), 1, "{raw:?}");
        let item = &draft.items[0];
        assert!(note_goto_query_token(item).is_some(), "{raw:?}");
        assert_eq!(task_goto_query_token(item), None, "{raw:?}");
    }
    for raw in ["/", "/fix", "/sase:deep-fix", "  /fix\n"] {
        let draft = split_capture_draft(raw);
        assert_eq!(draft.items.len(), 1, "{raw:?}");
        let item = &draft.items[0];
        assert!(task_goto_query_token(item).is_some(), "{raw:?}");
        assert_eq!(note_goto_query_token(item), None, "{raw:?}");
    }
}

#[test]
fn note_goto_claim_leaves_prose_byte_identical() {
    for raw in ["///", "///x", "/Users/x", "/r/rust", "/a/b"] {
        execute(raw).expect(&format!("{raw} stays prose"));
        let parsed = editor(raw);
        assert_ne!(parsed.needs, vec![Need::NoteGoto], "{raw}");
        let completion = field(raw, raw.len());
        assert!(
            completion.is_none_or(
                |field| field.context != CompletionContext::NoteGoto
            ),
            "{raw}"
        );
    }
    for (raw, body) in [
        ("// cash", "// cash"),
        ("//a b", "//a b"),
        ("Buy //x", "Buy //x"),
    ] {
        let parsed = execute(raw).expect(&format!("{raw} stays prose"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.kind, CaptureKind::Task, "{raw}");
    }
    let parented = execute("//x\n- x").expect("query with a child");
    assert_eq!(parented.body, "//x");
    assert_eq!(sub_bullet_bodies(&parented.sub_bullets), vec!["x"]);
    let parsed = editor("//x\n- x");
    assert_ne!(parsed.needs, vec![Need::NoteGoto]);
    assert_eq!(field("//x\n- x", "//x\n- x".len()), None);

    let draft = split_capture_draft("//x\n\nBuy milk");
    assert_eq!(draft.items.len(), 2);
    for item in &draft.items {
        assert_eq!(note_goto_query_token(item), None);
    }
    let parsed = editor("//x\n\nBuy milk");
    assert_ne!(parsed.needs, vec![Need::NoteGoto]);

    let draft = split_capture_draft("@@work\n//x");
    for item in &draft.items {
        assert_eq!(note_goto_query_token(item), None);
    }
    let routed = execute("@@work\n//x").expect("declaration routes //x");
    assert_eq!(routed.route.as_deref(), Some("work"));
}

//! Whole-draft `@route^id` caret Go To grammar: claim, teaching error, rollback.

use super::super::completion::*;
use super::super::draft::*;
use super::super::editor_model::*;
use super::super::model::*;
use super::super::tokens::*;
use super::*;

fn t5(token: &str, route_label: &str, block_id: &str) -> String {
    format!(
        "`{token}` opens {route_label} at ^{block_id} in Obsidian — it never captures; add task text after the ID to create a task with this ID"
    )
}

fn assert_claimed(raw: &str, route: &str, block_id: &str) {
    let token = raw.trim();
    let error = execute(raw).expect_err(&format!("{raw:?} is a caret Go To"));
    assert_eq!(
        error,
        t5(token, &format!("{route}.md"), block_id),
        "{raw:?}"
    );

    let parsed = editor(raw);
    assert_eq!(parsed.mode, EditorMode::TaskGoto, "{raw:?}");
    assert!(parsed.needs.is_empty(), "{raw:?}");
    assert_eq!(parsed.body, "", "{raw:?}");
    assert_eq!(parsed.route.as_deref(), Some(route), "{raw:?}");
    assert_eq!(parsed.block_id.as_deref(), Some(block_id), "{raw:?}");
    assert!(parsed.diagnostics.is_empty(), "{raw:?}");
    assert_eq!(parsed.spans.len(), 2, "{raw:?}");
    assert_eq!(parsed.spans[0].kind, SpanKind::TaskBlockIdRoute, "{raw:?}");
    assert_eq!(parsed.spans[1].kind, SpanKind::TaskBlockId, "{raw:?}");
}

#[test]
fn caret_goto_claim_covers_every_table_row() {
    for (raw, route, block_id) in [
        ("@bob^intention-rollovers", "bob", "intention-rollovers"),
        ("@Bob^intention-rollovers", "bob", "intention-rollovers"),
        ("  @bob^intention-rollovers\n", "bob", "intention-rollovers"),
        ("@taxes^z-250414-15", "taxes", "z-250414-15"),
        ("@cash^id", "cash", "id"),
    ] {
        assert_claimed(raw, route, block_id);
        let draft = split_capture_draft(raw);
        assert_eq!(draft.items.len(), 1, "{raw:?}");
        let item = &draft.items[0];
        let claimed = caret_goto_token(item).expect(raw);
        assert_eq!(claimed.route.to_ascii_lowercase(), route, "{raw:?}");
        assert_eq!(claimed.block_id, block_id, "{raw:?}");
        assert_eq!(claimed.token.text, raw.trim(), "{raw:?}");
        assert_eq!(note_goto_query_token(item), None, "{raw:?}");
        assert_eq!(task_goto_query_token(item), None, "{raw:?}");
    }
    let forced = parse_capture_text_with_options(
        "@bob^intention-rollovers",
        Some("work"),
        None,
        &CaptureParseOptions::routing_off(),
    )
    .expect_err("forced address");
    assert!(
        forced.contains("opens bob.md at ^intention-rollovers"),
        "{forced}"
    );
}

#[test]
fn caret_address_empty_id_is_not_caret_goto() {
    for raw in ["@bob^", "  @bob^\n"] {
        let draft = split_capture_draft(raw);
        assert_eq!(draft.items.len(), 1, "{raw:?}");
        let item = &draft.items[0];
        let claimed = caret_address_token(item).expect(raw);
        assert_eq!(claimed.route, "bob", "{raw:?}");
        assert_eq!(claimed.block_id, "", "{raw:?}");
        assert_eq!(caret_goto_token(item), None, "{raw:?}");
        let parsed = editor(raw);
        assert_ne!(parsed.mode, EditorMode::TaskGoto, "{raw:?}");
        assert!(
            !execute(raw)
                .err()
                .unwrap_or_default()
                .contains("never captures"),
            "{raw:?} must not raise T5"
        );
    }
}

#[test]
fn caret_goto_completion_stays_task_block_id() {
    let raw = "@bob^intention-rollovers";
    let completion = field(raw, raw.len()).expect("completes at end");
    assert_eq!(completion.context, CompletionContext::TaskBlockId);
    assert_eq!(completion.query, "intention-rollovers");

    let empty = field("@bob^", "@bob^".len()).expect("empty id completes");
    assert_eq!(empty.context, CompletionContext::TaskBlockId);
    assert_eq!(empty.query, "");
}

#[test]
fn caret_goto_claim_leaves_prose_byte_identical() {
    for raw in [
        "@bob^x Fix it",
        "Fix it @bob^x",
        "@bob^x@",
        "@bob^x@ %n",
        "@bob^x#section",
        "@bob:x",
        "^bob:x",
        "@bob+x",
        "Discuss @bob^x later",
        "@@bob^x",
    ] {
        let draft = split_capture_draft(raw);
        for item in &draft.items {
            assert_eq!(caret_goto_token(item), None, "{raw}");
        }
        let parsed = editor(raw);
        assert_ne!(parsed.mode, EditorMode::TaskGoto, "{raw}");
        assert!(
            !execute(raw)
                .err()
                .unwrap_or_default()
                .contains("never captures"),
            "{raw} must not raise T5"
        );
    }

    let parented = execute("@bob^x\n- x");
    assert!(
        parented
            .err()
            .unwrap_or_default()
            .contains("task text is required"),
        "a child line keeps today's empty-body mint, not T5"
    );
    let parsed = editor("@bob^x\n- x");
    assert_ne!(parsed.mode, EditorMode::TaskGoto);
    let draft = split_capture_draft("@bob^x\n- x");
    assert_eq!(caret_goto_token(&draft.items[0]), None);

    let draft = split_capture_draft("@bob^x\n\nBuy milk");
    assert_eq!(draft.items.len(), 2);
    for item in &draft.items {
        assert_eq!(caret_goto_token(item), None);
    }
    let parsed = editor("@bob^x\n\nBuy milk");
    assert_ne!(parsed.mode, EditorMode::TaskGoto);

    let draft = split_capture_draft("@@work\n@bob^x");
    for item in &draft.items {
        assert_eq!(caret_goto_token(item), None);
    }
    let routed =
        execute("@@work\nFix it @bob^x").expect("declaration keeps mint");
    assert_eq!(routed.route.as_deref(), Some("bob"));
}

#[test]
fn caret_goto_and_slash_claims_are_disjoint() {
    for raw in ["@bob^id", "  @cash^z-1\n"] {
        let draft = split_capture_draft(raw);
        let item = &draft.items[0];
        assert!(caret_goto_token(item).is_some(), "{raw:?}");
        assert_eq!(task_goto_query_token(item), None, "{raw:?}");
        assert_eq!(note_goto_query_token(item), None, "{raw:?}");
    }
    for raw in ["/", "/fix", "//", "//cash"] {
        let draft = split_capture_draft(raw);
        let item = &draft.items[0];
        assert_eq!(caret_goto_token(item), None, "{raw:?}");
        assert_eq!(caret_address_token(item), None, "{raw:?}");
    }
}

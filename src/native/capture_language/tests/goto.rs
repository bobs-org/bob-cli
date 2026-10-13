//! Whole-draft `/query` Go To grammar: claim, teaching error, rollback.

use super::super::completion::*;
use super::super::draft::*;
use super::super::editor_model::*;
use super::super::model::*;
use super::super::tokens::*;
use super::*;

const T3_BARE: &str = "`/` opens the Go To picker; pick a task to open it in Obsidian — it never captures";

#[test]
fn goto_claim_covers_every_table_query_row() {
    // Every §1 claim row: execution refuses with T3, the editor needs
    // `task_goto`, and completion at the token end reports `task_goto`.
    for (raw, query) in [
        ("/", ""),
        ("/fix", "fix"),
        ("/sase:deep-fix", "sase:deep-fix"),
        ("  /fix\n", "fix"),
    ] {
        let expected = if query.is_empty() && raw == "/" {
            T3_BARE.to_string()
        } else {
            let token = raw.trim();
            format!(
                "`{token}` opens the Go To picker; pick a task to open it in Obsidian — it never captures"
            )
        };
        let error = execute(raw).expect_err(&format!("{raw:?} is a query"));
        assert_eq!(error, expected, "{raw:?}");

        let parsed = editor(raw);
        assert_eq!(parsed.mode, EditorMode::Incomplete, "{raw:?}");
        assert_eq!(parsed.needs, vec![Need::TaskGoto], "{raw:?}");
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

        // The padded draft completes at the token end (before `\n`).
        let end = if raw.ends_with('\n') {
            raw.len() - 1
        } else {
            raw.len()
        };
        let completion = field(raw, end).expect(&format!("{raw:?} completes"));
        assert_eq!(completion.context, CompletionContext::TaskGoto, "{raw:?}");
        assert_eq!(completion.query, query, "{raw:?}");
    }
    // Forced flags do not change the refusal.
    let forced = parse_capture_text_with_clip_control(
        "/fix",
        Some("work"),
        None,
        &CaptureParseOptions::routing_off(true),
    )
    .expect_err("forced query");
    assert!(forced.contains("opens the Go To picker"), "{forced}");
}

#[test]
fn goto_completion_field_reports_sigil_relative_queries() {
    // A cursor at the token start, just after the sigil, and mid-token.
    let before = field("/fix", 0).expect("before sigil");
    assert_eq!(before.context, CompletionContext::TaskGoto);
    assert_eq!(before.query, "");
    assert_eq!(before.replacement, (0, 4));

    let after_sigil = field("/fix", 1).expect("after sigil");
    assert_eq!(after_sigil.context, CompletionContext::TaskGoto);
    assert_eq!(after_sigil.query, "");
    assert_eq!(after_sigil.replacement, (0, 4));

    let middle = field("/fix", 2).expect("middle");
    assert_eq!(middle.context, CompletionContext::TaskGoto);
    assert_eq!(middle.query, "f");
    assert_eq!(middle.replacement, (0, 4));

    let bare_before = field("/", 0).expect("bare before sigil");
    assert_eq!(bare_before.context, CompletionContext::TaskGoto);
    assert_eq!(bare_before.query, "");
    assert_eq!(bare_before.replacement, (0, 1));

    let bare_end = field("/", 1).expect("bare end");
    assert_eq!(bare_end.context, CompletionContext::TaskGoto);
    assert_eq!(bare_end.query, "");
    assert_eq!(bare_end.replacement, (0, 1));
}

#[test]
fn goto_claim_leaves_prose_byte_identical() {
    // A second `/` stays prose.
    for raw in ["//", "/Users/x", "/r/rust"] {
        execute(raw).expect(&format!("{raw} stays prose"));
        let parsed = editor(raw);
        assert_ne!(parsed.needs, vec![Need::TaskGoto], "{raw}");
        let completion = field(raw, raw.len());
        assert!(
            completion.is_none_or(
                |field| field.context != CompletionContext::TaskGoto
            ),
            "{raw}"
        );
    }
    // Several tokens stay prose.
    for (raw, body) in [("/a b", "/a b"), ("Buy /x", "Buy /x")] {
        let parsed = execute(raw).expect(&format!("{raw} stays prose"));
        assert_eq!(parsed.body, body, "{raw}");
        assert_eq!(parsed.kind, CaptureKind::Task, "{raw}");
    }
    // A child line keeps the item prose.
    let parented = execute("/x\n- x").expect("query with a child");
    assert_eq!(parented.body, "/x");
    assert_eq!(sub_bullet_bodies(&parented.sub_bullets), vec!["x"]);
    let parsed = editor("/x\n- x");
    assert_ne!(parsed.needs, vec![Need::TaskGoto]);
    assert_eq!(field("/x\n- x", "/x\n- x".len()), None);

    // Two items are both captured as usual: neither item claims the draft.
    let draft = split_capture_draft("/x\n\nBuy milk");
    assert_eq!(draft.items.len(), 2);
    for item in &draft.items {
        assert_eq!(task_goto_query_token(item), None);
    }
    let parsed = editor("/x\n\nBuy milk");
    assert_ne!(parsed.needs, vec![Need::TaskGoto]);

    // A `@@` declaration line keeps `/x` a routed task: the whole draft is
    // no longer one token.
    let draft = split_capture_draft("@@work\n/x");
    for item in &draft.items {
        assert_eq!(task_goto_query_token(item), None);
    }
    let routed = execute("@@work\n/x").expect("declaration routes /x");
    assert_eq!(routed.route.as_deref(), Some("work"));
}

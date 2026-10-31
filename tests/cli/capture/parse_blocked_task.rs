//! `*note:block-id` capture-parse grammar.

use crate::support::*;
use serde_json::Value;

fn parse_json(draft: &str) -> Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-parse json");
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

#[test]
fn blocked_task_parses_separately_with_directional_spans() {
    let json = parse_json("Ask Kelly *job:cleanup");
    assert_eq!(json["body"], "Ask Kelly");
    assert_eq!(json["mode"], "task");
    assert!(json["dependencies"].is_null());
    assert_eq!(
        json["blocked_tasks"],
        serde_json::json!([{
            "raw": "*job:cleanup",
            "note": "job",
            "block_id": "cleanup",
            "quoted": false,
            "range": { "start": 10, "end": 22 },
        }])
    );
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({ "kind": "new_task" })
    );
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 10, "end": 11, "kind": "blocked_task_sigil" },
            { "start": 11, "end": 14, "kind": "blocked_task_note" },
            { "start": 15, "end": 22, "kind": "blocked_task_block_id" },
        ])
    );
}

#[test]
fn both_directions_keep_independent_lists_and_blocked_task_order() {
    let json = parse_json("Call registrar &school:transcript *job:apply-x");
    assert_eq!(json["body"], "Call registrar");
    assert_eq!(json["dependencies"][0]["raw"], "&school:transcript");
    assert_eq!(json["blocked_tasks"][0]["raw"], "*job:apply-x");
    assert_eq!(json["mode"], "task");

    let leading = parse_json("*job:cleanup Ask Kelly");
    assert_eq!(leading["body"], "Ask Kelly");
    assert_eq!(leading["blocked_tasks"][0]["raw"], "*job:cleanup");

    let existing = parse_json("*job:cleanup @body+work");
    assert_eq!(existing["mode"], "task_dependency");
    assert_eq!(
        existing["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": "body",
            "block_id": "work",
        })
    );

    let multiple = parse_json("Ask Kelly *job:cleanup *job:send-batch");
    assert_eq!(
        multiple["blocked_tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["raw"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["*job:cleanup", "*job:send-batch"]
    );

    let batch = parse_json("First *job:cleanup\n\nSecond *job:send-batch");
    assert_eq!(batch["items"][0]["blocked_tasks"][0]["raw"], "*job:cleanup");
    assert_eq!(
        batch["items"][1]["blocked_tasks"][0]["raw"],
        "*job:send-batch"
    );

    let inherited = parse_json("@@body+work\n*job:cleanup");
    assert_eq!(inherited["mode"], "task_dependency");
    assert_eq!(inherited["dependency_target"]["inherited"], true);
}

#[test]
fn blocked_task_picker_and_markdown_guards_are_distinct() {
    let partial = parse_json("Ask Kelly *");
    assert_eq!(partial["body"], "Ask Kelly");
    assert_eq!(partial["needs"], serde_json::json!(["blocked_task"]));
    assert_eq!(
        partial["spans"],
        serde_json::json!([
            { "start": 10, "end": 11, "kind": "interactive_placeholder" }
        ])
    );
    for text in ["Ask Kelly *jo", "Ask Kelly *job:"] {
        assert_eq!(
            parse_json(text)["needs"],
            serde_json::json!(["blocked_task"])
        );
    }
    assert!(parse_json("*")["blocked_tasks"].is_null());

    let ownerless = parse_json("*job:cleanup");
    assert_eq!(ownerless["mode"], "incomplete");
    assert_eq!(ownerless["needs"], serde_json::json!(["dependency_target"]));

    let temp = tempfile::tempdir().expect("temporary vault");
    let unsupported_query = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(temp.path())
        .arg("--dry-run")
        .arg("--")
        .arg("Support *nix")
        .output()
        .expect("run incomplete blocked-task capture");
    assert!(!unsupported_query.status.success());
    assert!(stderr(&unsupported_query).contains("escape a literal star"));

    for text in [
        "Fix *urgent* bug",
        "Read **Dune**",
        "* Buy milk",
        "5 * 3",
        "R*D",
        "Fix *foo:bar now",
    ] {
        let json = parse_json(text);
        assert!(json["blocked_tasks"].is_null(), "{text}: {json}");
        assert!(
            json["spans"]
                .as_array()
                .unwrap()
                .iter()
                .all(|span| !span["kind"]
                    .as_str()
                    .unwrap()
                    .starts_with("blocked_task_")),
            "{text}: {json}"
        );
    }

    let escaped = parse_json("Ask Kelly \\*job:cleanup");
    assert_eq!(escaped["body"], "Ask Kelly *job:cleanup");
    assert!(escaped["blocked_tasks"].is_null());

    let midline_escape = parse_json("Ask \\*job:cleanup later");
    assert_eq!(midline_escape["body"], "Ask \\*job:cleanup later");

    let trailing_escape = parse_json("Support \\*nix");
    assert_eq!(trailing_escape["body"], "Support *nix");
    assert!(trailing_escape["blocked_tasks"].is_null());

    for (text, mode) in [
        ("x*1", "pomodoro_close"),
        ("x*", "pomodoro_close"),
        ("==*", "pomodoro_start"),
    ] {
        let json = parse_json(text);
        assert!(json["blocked_tasks"].is_null(), "{text}: {json}");
        assert_eq!(json["mode"], mode, "{text}: {json}");
        assert_eq!(json["body"], text, "{text}: {json}");
    }
}

#[test]
fn blocked_task_note_star_is_invalid() {
    let malformed = parse_json("Ask Kelly *no*te:id");
    assert_eq!(malformed["diagnostics"][0]["code"], "invalid_dependency");
    assert!(malformed["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains('*'));
    let malformed_dependency = parse_json("Ask Kelly &no*te:id");
    assert_eq!(
        malformed_dependency["diagnostics"][0]["code"],
        "invalid_dependency"
    );
}

#[test]
fn blocked_task_parse_vectors_cover_leading_quoted_and_nested() {
    let leading = parse_json("@cash *job:cleanup Ask Kelly");
    assert_eq!(leading["body"], "Ask Kelly");
    assert_eq!(leading["route"], "cash");
    assert_eq!(leading["mode"], "task");
    assert_eq!(leading["blocked_tasks"][0]["raw"], "*job:cleanup");
    assert_eq!(
        leading["dependency_target"],
        serde_json::json!({ "kind": "new_task", "route": "cash" })
    );

    let colon = parse_json("@bob:ask-kelly *job:cleanup");
    assert_eq!(colon["mode"], "task_dependency");
    assert_eq!(
        colon["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": "bob",
            "block_id": "ask-kelly",
        })
    );

    let quoted = parse_json(r#"X *"Release Plan":publish"#);
    assert_eq!(quoted["body"], "X");
    assert_eq!(quoted["blocked_tasks"][0]["note"], "Release Plan");
    assert_eq!(quoted["blocked_tasks"][0]["quoted"], true);
    assert_eq!(quoted["blocked_tasks"][0]["block_id"], "publish");

    let linked = parse_json("Ask Kelly @mac_inbox:ask-kelly *job:cleanup");
    assert_eq!(linked["mode"], "pomodoro_task");
    assert_eq!(linked["block_id"], "ask-kelly");
    assert_eq!(linked["blocked_tasks"][0]["raw"], "*job:cleanup");

    let nested = parse_json("Order parts *projects/house/kitchen:cabinets");
    assert_eq!(nested["body"], "Order parts");
    assert_eq!(nested["blocked_tasks"][0]["note"], "projects/house/kitchen");
    assert_eq!(nested["blocked_tasks"][0]["block_id"], "cabinets");

    for draft in ["^bob:ask-kelly *job:cleanup", "*job:cleanup ^bob:ask-kelly"]
    {
        let json = parse_json(draft);
        assert_eq!(json["mode"], "task_dependency", "{draft}");
        assert_eq!(
            json["dependency_target"],
            serde_json::json!({
                "kind": "existing_task",
                "route": "bob",
                "block_id": "ask-kelly",
            }),
            "{draft}"
        );
        assert_eq!(json["blocked_tasks"][0]["raw"], "*job:cleanup", "{draft}");
        assert_eq!(json["diagnostics"], serde_json::json!([]), "{draft}");
    }

    let parent = parse_json("*job:cleanup +ex");
    assert_eq!(parent["mode"], "incomplete");
    assert_eq!(parent["needs"], serde_json::json!(["task_parent"]));
    assert_eq!(parent["blocked_tasks"][0]["raw"], "*job:cleanup");
    assert_eq!(parent["diagnostics"], serde_json::json!([]));
}

#[test]
fn bang_keeps_precedence_over_star() {
    let json = parse_json("!sase:x *foo");
    assert_eq!(json["mode"], "task_complete");
    let diagnostics = json["diagnostics"].as_array().expect("diagnostics");
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .unwrap_or_default()
                .contains('!')
        }),
        "bang teaching error stays: {json}"
    );
}

fn diagnostic_messages(json: &serde_json::Value) -> Vec<String> {
    json["diagnostics"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|diagnostic| {
            diagnostic["message"].as_str().map(str::to_string)
        })
        .collect()
}

fn assert_star_named_rejection(draft: &str) {
    let json = parse_json(draft);
    let messages = diagnostic_messages(&json);
    assert!(
        messages.iter().any(|message| message.contains('*')),
        "{draft}: expected a * diagnostic, got {json}"
    );
}

#[test]
fn star_twins_of_ampersand_rejections_name_the_sigil() {
    assert_star_named_rejection("x *job:cleanup");
    assert_star_named_rejection("++3 *job:cleanup");
    assert_star_named_rejection("+5 *job:cleanup");
    assert_star_named_rejection("-5 *job:cleanup");
    assert_star_named_rejection("Buy milk @notes#Ideas *job:cleanup");
    assert_star_named_rejection("*job:cleanup @body:excercise#name");
    assert_star_named_rejection("@job^cleanup@ *job:other");
    let close = parse_json("New thing @mac_inbox:new-thing=x *job:cleanup");
    let close_messages = diagnostic_messages(&close);
    assert!(
        close_messages.iter().any(|message| {
            message.contains('*') && message.contains("separate")
        }),
        "body-bearing close must refuse *: {close}"
    );
    assert_eq!(close["diagnostics"][0]["code"], "invalid_dependency_target");
}

#[test]
fn body_bearing_close_with_star_fails_capture() {
    let temp = tempfile::tempdir().expect("temporary vault");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(temp.path())
        .arg("--dry-run")
        .arg("--")
        .arg("New thing @mac_inbox:new-thing=x *job:cleanup")
        .output()
        .expect("run close+star capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    let message = stderr(&output);
    assert!(message.contains('*'), "{message}");
    assert!(
        message.contains("separate") || message.contains("blank-line"),
        "{message}"
    );
}

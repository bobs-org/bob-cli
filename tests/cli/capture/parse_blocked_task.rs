//! `*note:block-id` capture-parse grammar and temporary execution guard.

use crate::support::*;
use serde_json::Value;
use std::process::Output;

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

    for text in ["=x*1", "=*", "==*"] {
        let json = parse_json(text);
        assert!(json["blocked_tasks"].is_null(), "{text}: {json}");
    }
}

#[test]
fn blocked_task_note_star_is_invalid_and_capture_refuses_supported_parse() {
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

    let temp = tempfile::tempdir().expect("temporary vault");
    let output: Output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(temp.path())
        .arg("--dry-run")
        .arg("--")
        .arg("Ask Kelly *job:cleanup")
        .output()
        .expect("run unsupported capture preview");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("`*` blocked tasks are not supported yet"),
        "{}",
        format_output(&output)
    );
}

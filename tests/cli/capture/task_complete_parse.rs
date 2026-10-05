//! Whole-item `!note:block-id` grammar: queries, complete tokens,
//! claimed-invalid items, teaching refusals, and prose passthrough.

use crate::support::*;
use std::fs;

const BANG_QUERY: &str = "`!fix` opens the task picker; pick a task to insert its `!<note>:<block-id>` token, or write one yourself (for example `!sase:fix-flaky`)";
const BANG_PENDING: &str =
    "`!note:block-id` completion is not available in this build yet";

fn parse_json(draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-parse json");
    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn capture_output(
    vault: &std::path::Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", "2026-10-05 09:02:00")
        .output()
        .expect("run bob capture")
}

#[test]
fn task_complete_query_reports_incomplete_needing_task_complete() {
    for draft in ["!", "!fix"] {
        let json = parse_json(draft);
        assert_eq!(json["mode"], "incomplete", "{draft}");
        assert_eq!(json["body"], "", "{draft}");
        assert!(json["route"].is_null(), "{draft}");
        assert!(json["block_id"].is_null(), "{draft}");
        assert_eq!(json["needs"], serde_json::json!(["task_complete"]));
        assert_eq!(
            json["spans"],
            serde_json::json!([
                {
                    "start": 0,
                    "end": draft.len(),
                    "kind": "interactive_placeholder",
                },
            ]),
            "{draft}"
        );
        assert_eq!(json["diagnostics"], serde_json::json!([]));
    }
}

#[test]
fn task_complete_token_reports_mode_spans_and_object() {
    let json = parse_json("!projects/foo:bar");
    assert_eq!(json["mode"], "task_complete");
    assert_eq!(json["body"], "");
    assert_eq!(json["block_id"], "bar");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 0, "end": 1, "kind": "task_complete_sigil" },
            { "start": 1, "end": 13, "kind": "task_complete_note" },
            { "start": 14, "end": 17, "kind": "task_complete_block_id" },
        ])
    );

    let quoted = parse_json("!\"Shopping List\":milk");
    assert_eq!(quoted["mode"], "task_complete");
    assert_eq!(
        quoted["spans"],
        serde_json::json!([
            { "start": 0, "end": 1, "kind": "task_complete_sigil" },
            { "start": 1, "end": 16, "kind": "task_complete_note" },
            { "start": 17, "end": 21, "kind": "task_complete_block_id" },
        ])
    );

    // The per-item `task_complete` object appears on multi-item drafts.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("!\"Shopping List\":milk\n\nSecond @work")
        .output()
        .expect("run multi-item capture-parse");
    assert_success(&output);
    let batch: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    assert_eq!(
        batch["items"][0]["task_complete"],
        serde_json::json!({
            "raw": "!\"Shopping List\":milk",
            "note": "Shopping List",
            "block_id": "milk",
            "quoted": true,
            "range": { "start": 0, "end": 21 },
        })
    );
    assert!(batch["items"][1].get("task_complete").is_none());
}

#[test]
fn task_complete_padded_item_reports_invalid_task_complete() {
    let json = parse_json("!sase:fix-flaky more");
    assert_eq!(json["mode"], "task_complete");
    assert!(json["block_id"].is_null());
    let diagnostics = json["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "invalid_task_complete");
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("remove `more`")),
        "unexpected message: {}",
        diagnostics[0]["message"]
    );
}

#[test]
fn task_complete_query_refuses_with_the_picker_teaching_error() {
    let temp = TempDir::new("bob-cli-task-complete-query");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = capture_output(&vault, "!fix");
    assert_eq!(
        output.status.code(),
        Some(2),
        "a picker query is a usage error:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stderr(&output).trim(),
        format!("bob capture: capture item 1 starting on line 1: {BANG_QUERY}"),
        "unexpected teaching error:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "an unfinished query must not create a junk inbox task"
    );
}

#[test]
fn task_complete_token_refuses_until_execute_lands() {
    let temp = TempDir::new("bob-cli-task-complete-pending");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = capture_output(&vault, "!sase:fix-flaky");
    assert_eq!(
        output.status.code(),
        Some(2),
        "a complete token is a usage error until execute lands:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stderr(&output).trim(),
        format!(
            "bob capture: capture item 1 starting on line 1: {BANG_PENDING}"
        ),
        "unexpected refusal:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "a refused completion must not create a junk inbox task"
    );
}

#[test]
fn task_complete_padded_item_refuses_without_capturing() {
    let temp = TempDir::new("bob-cli-task-complete-invalid");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = capture_output(&vault, "!sase:fix-flaky more");
    assert_eq!(
        output.status.code(),
        Some(2),
        "a padded item is a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("must be the whole capture item"),
        "unexpected refusal:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "a refused completion must not create a junk inbox task"
    );
}

#[test]
fn bang_prose_rows_still_capture_as_tasks() {
    let temp = TempDir::new("bob-cli-task-complete-prose");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    for draft in ["!wow this works", "![[x#^y]]"] {
        let output = capture_output(&vault, draft);
        assert!(
            output.status.success(),
            "prose {draft} must still capture:\n{}",
            format_output(&output)
        );
    }
    let inbox = fs::read_to_string(vault.join("mac_inbox.md"))
        .expect("prose captures into the inbox");
    assert!(inbox.contains("!wow this works"), "{inbox}");
    assert!(inbox.contains("![[x#^y]]"), "{inbox}");
}

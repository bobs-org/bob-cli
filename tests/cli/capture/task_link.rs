//! Task-link picker query grammar: `:` claims, teaching errors, rollback.

use crate::support::*;
use std::fs;

const T1_DEE: &str = "`:dee` opens the task picker; pick a task to insert its `@<route>:<block-id>` link, or write the link yourself (for example `@sase:deep-fix`)";
const T2_START: &str = "`:sase:deep-fix=` opens the task picker; to link that task, write `@sase:deep-fix=`";

#[test]
fn capture_colon_query_fails_with_the_picker_teaching_error() {
    let temp = TempDir::new("bob-cli-capture-task-link");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg(":dee")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run colon query capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "a picker query is a usage error:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stderr(&output).trim(),
        format!("bob capture: capture item 1 starting on line 1: {T1_DEE}"),
        "unexpected teaching error:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "an unfinished query must not create a junk inbox task"
    );
}

#[test]
fn capture_colon_query_teaches_the_at_spelling_for_links() {
    let temp = TempDir::new("bob-cli-capture-task-link-at");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg(":sase:deep-fix=")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run link-shaped query capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "a picker query is a usage error:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stderr(&output).trim(),
        format!("bob capture: capture item 1 starting on line 1: {T2_START}"),
        "unexpected teaching error:\n{}",
        format_output(&output)
    );
}

#[test]
fn capture_colon_query_json_failure_reports_ok_false() {
    let temp = TempDir::new("bob-cli-capture-task-link-json");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(":dee")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run json query capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "a picker query is a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "json failure should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains(T1_DEE)),
        "unexpected json failure object: {json}"
    );
}

#[test]
fn capture_batch_with_a_colon_query_writes_nothing() {
    let temp = TempDir::new("bob-cli-capture-task-link-batch");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-15");
    let output = run_with_stdin(&mut command, "Buy milk\n\n:dee\n");

    assert_eq!(
        output.status.code(),
        Some(2),
        "a batch with a query rolls back:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("capture item 2 starting on line 3"),
        "the error names the query item:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains(T1_DEE),
        "the error teaches the picker:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "a batch rollback must not write the inbox"
    );
}

#[test]
fn capture_parse_colon_query_reports_incomplete_task_link() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(":dee")
        .output()
        .expect("run bob capture-parse json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["task_link"]));
    assert_eq!(json["body"], "");
    assert!(json["route"].is_null(), "{json}");
    assert!(json["section"].is_null(), "{json}");
    assert!(json["block_id"].is_null(), "{json}");
    assert_eq!(
        json["spans"],
        serde_json::json!([{"start": 0, "end": 4, "kind": "interactive_placeholder"}]),
        "{json}"
    );
    assert_eq!(json["diagnostics"], serde_json::json!([]), "{json}");
}

#[test]
fn capture_complete_colon_query_reports_task_link_with_empty_candidates() {
    let temp = TempDir::new("bob-cli-capture-task-link-complete");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("4")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(":dee")
        .output()
        .expect("run bob capture-complete json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["context"], "task_link");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 0, "end": 4}),
        "{json}"
    );
    assert_eq!(json["candidates"], serde_json::json!([]), "{json}");
}

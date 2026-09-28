//! Completion route, marker, section, task, global JSON.

use crate::support::*;
use std::fs;

#[test]
fn capture_complete_global_declaration_replaces_only_the_active_component() {
    let temp = TempDir::new("bob-cli-capture-complete-global-route");
    let vault = temp.path().join("vault");
    write_file(&vault.join("food.md"), "# Food\n");
    write_file(&vault.join("foo.md"), "# Foo\n");
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Existing trade [created::2026-06-01] ^goog-exit\n",
    );

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("4")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@@fo\nTask")
        .output()
        .expect("run global route completion");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "route");
    assert_eq!(
        json["replacement"],
        serde_json::json!({ "start": 2, "end": 4 })
    );

    let trailing = "Buy milk @@fo";
    let trailing_output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(trailing.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(trailing)
        .output()
        .expect("run trailing global route completion");
    assert_success(&trailing_output);
    let trailing_json: serde_json::Value =
        serde_json::from_str(stdout(&trailing_output).trim())
            .expect("complete JSON");
    assert_eq!(trailing_json["context"], "route");
    assert_eq!(
        trailing_json["replacement"],
        serde_json::json!({ "start": 11, "end": 13 })
    );

    let child = "Parent\n- child @@cash+go";
    let child_output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(child.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(child)
        .output()
        .expect("run child-line global task completion");
    assert_success(&child_output);
    let child_json: serde_json::Value =
        serde_json::from_str(stdout(&child_output).trim())
            .expect("complete JSON");
    assert_eq!(child_json["context"], "task");
    assert_eq!(child_json["candidates"][0]["route"], "cash");
    assert_eq!(
        child_json["replacement"],
        serde_json::json!({ "start": 22, "end": 24 })
    );
}

#[test]
fn capture_complete_all_tasks_works_on_a_global_sub_bullet_declaration() {
    let temp = TempDir::new("bob-cli-capture-complete-global-all-tasks");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("file.md"),
        concat!(
            "- [ ] #task identified [created::2026-06-01] ^has-id\n",
            "- [ ] #task needs id [created::2026-06-01]\n",
        ),
    );
    let draft = "@@file+\nnote";
    let cursor = draft.find('+').expect("plus") + 1;
    let output = bob_command()
        .arg("capture-complete")
        .arg("-a")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run global all-tasks completion");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "task");
    assert_eq!(
        json["replacement"],
        serde_json::json!({ "start": cursor, "end": cursor })
    );
    let ids = task_block_ids(&json);
    assert!(ids.contains(&Some("has-id")), "{json}");
    assert!(ids.contains(&None), "{json}");
}

#[test]
fn capture_complete_inherited_route_is_the_same_note_for_wikilinks() {
    let temp = TempDir::new("bob-cli-capture-complete-global-wikilink");
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "# Design\n# Decision Log\n");

    let draft = "@@sase\nSee [[#De";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run inherited wikilink completion");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "wikilink_heading");
    let candidates = json["candidates"].as_array().expect("candidates");
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate["heading"].as_str().expect("heading"))
            .collect::<Vec<_>>(),
        vec!["Design", "Decision Log"]
    );
    assert_eq!(candidates[0]["path"], "sase.md");
}

#[test]
fn capture_complete_completes_a_marker_on_a_child_line() {
    let temp = TempDir::new("bob-cli-capture-complete-child-line");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(&vault.join("cash-flow.md"), "---\ntype: [[area]]\n---\n");

    let draft = "parent line\n- context @ca";
    let cursor = draft.len().to_string();
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("--cursor")
        .arg(&cursor)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run child-line capture-complete");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "route");
    let names: Vec<&str> = json["candidates"]
        .as_array()
        .expect("route candidates")
        .iter()
        .map(|candidate| candidate["route"].as_str().expect("route"))
        .collect();
    assert_eq!(names, vec!["cash", "cash-flow"]);
}

#[test]
fn capture_complete_completes_a_marker_on_a_nested_child_line() {
    let temp = TempDir::new("bob-cli-capture-complete-nested-child-line");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(&vault.join("cash-flow.md"), "---\ntype: [[area]]\n---\n");

    let draft = "parent line\n- first child\n  - context @ca";
    let cursor = draft.len().to_string();
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("--cursor")
        .arg(&cursor)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run nested child-line capture-complete");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "route");
    let names: Vec<&str> = json["candidates"]
        .as_array()
        .expect("route candidates")
        .iter()
        .map(|candidate| candidate["route"].as_str().expect("route"))
        .collect();
    assert_eq!(names, vec!["cash", "cash-flow"]);
}

#[test]
fn capture_complete_scopes_to_later_batch_item_and_ignores_separator() {
    let temp = TempDir::new("bob-cli-capture-complete-batch-item");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(&vault.join("cash-flow.md"), "---\ntype: [[area]]\n---\n");
    write_file(&vault.join("work.md"), "---\ntype: [[area]]\n---\n");

    let draft = "first @work\n\nsecond @ca";
    let cursor = draft.len();
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("--cursor")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run batch item capture-complete");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "route");
    assert_eq!(
        json["replacement"],
        serde_json::json!({
            "start": draft.rfind('@').expect("at sign") + 1,
            "end": cursor,
        })
    );
    let names: Vec<&str> = json["candidates"]
        .as_array()
        .expect("route candidates")
        .iter()
        .map(|candidate| candidate["route"].as_str().expect("route"))
        .collect();
    assert_eq!(names, vec!["cash", "cash-flow"]);

    let separator_cursor = draft.find("\n\n").expect("separator") + 1;
    let separator_output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("--cursor")
        .arg(separator_cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run separator capture-complete");

    assert_success(&separator_output);
    let separator_json: serde_json::Value =
        serde_json::from_str(stdout(&separator_output).trim())
            .expect("capture-complete JSON");
    assert!(separator_json["context"].is_null(), "{separator_json}");
    assert_eq!(separator_json["candidates"], serde_json::json!([]));
    assert_eq!(
        separator_json["replacement"],
        serde_json::json!({
            "start": separator_cursor,
            "end": separator_cursor,
        })
    );
}

#[test]
fn capture_complete_all_tasks_uses_global_ranges_in_later_batch_item() {
    let temp = TempDir::new("bob-cli-capture-complete-batch-all-tasks");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(&vault.join("work.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("file.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task Plan the handoff\n",
            "- [*] #task Handoff ready ^hand-ready\n",
            "- [x] #task Done handoff\n",
            "- [ ] #task Later handoff\n",
        ),
    );

    let draft = "café first @work\n\nsecond @file+hand";
    let cursor = draft.len();
    let replacement_start =
        draft.find("@file+").expect("task marker") + "@file+".len();
    let output = bob_command()
        .arg("capture-complete")
        .arg("--all-tasks")
        .arg("-b")
        .arg(&vault)
        .arg("--cursor")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run later batch all-tasks completion");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["cursor"], cursor);
    assert_eq!(json["context"], "task");
    assert_eq!(
        json["replacement"],
        serde_json::json!({
            "start": replacement_start,
            "end": cursor,
        })
    );
    let candidates = json["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0]["block_id"], "hand-ready");
    assert_eq!(candidates[0]["requires_block_id"], false);
    assert_eq!(candidates[0]["replacement"], "hand-ready");
    assert_eq!(candidates[0]["route"], "file");
    assert_eq!(candidates[0]["text"], "Handoff ready");
    assert!(candidates[0]["ref"].as_str().expect("ref").contains(':'));
    assert!(candidates[1]["block_id"].is_null());
    assert_eq!(candidates[1]["requires_block_id"], true);
    assert_eq!(candidates[1]["replacement"], "");
    assert_eq!(candidates[1]["route"], "file");
    assert_eq!(candidates[1]["text"], "Plan the handoff");
    assert!(candidates[1]["ref"].as_str().expect("ref").contains(':'));
    assert_eq!(candidates[2]["text"], "Later handoff");

    let separator_cursor = draft.find("\n\n").expect("separator") + 1;
    let separator_output = bob_command()
        .arg("capture-complete")
        .arg("--all-tasks")
        .arg("-b")
        .arg(&vault)
        .arg("--cursor")
        .arg(separator_cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run separator all-tasks completion");

    assert_success(&separator_output);
    let separator_json: serde_json::Value =
        serde_json::from_str(stdout(&separator_output).trim())
            .expect("capture-complete JSON");
    assert!(separator_json["context"].is_null(), "{separator_json}");
    assert_eq!(separator_json["candidates"], serde_json::json!([]));
    assert_eq!(
        separator_json["replacement"],
        serde_json::json!({
            "start": separator_cursor,
            "end": separator_cursor,
        })
    );
}

#[test]
fn capture_complete_route_json_ranks_prefix_before_substring() {
    let temp = TempDir::new("bob-cli-capture-complete-route");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    fs::write(vault.join("cash.md"), "---\ntype: [[area]]\n---\n")
        .expect("write cash.md");
    fs::write(vault.join("petty-cash.md"), "---\ntype: [[area]]\n---\n")
        .expect("write petty-cash.md");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("3")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@ca")
        .output()
        .expect("run bob capture-complete route json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-complete stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["cursor"], 3);
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 1, "end": 3})
    );
    assert_eq!(json["context"], "route");
    let routes: Vec<&str> = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|candidate| candidate["route"].as_str().expect("route"))
        .collect();
    assert_eq!(routes, vec!["cash", "petty-cash"]);
    assert_eq!(json["candidates"][0]["replacement"], "cash");
    assert_eq!(json["candidates"][0]["kind"], "area");
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_complete_task_block_id_marker_completes_only_route_side() {
    let temp = TempDir::new("bob-cli-capture-complete-task-block-id");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    fs::write(vault.join("cash.md"), "---\ntype: [[area]]\n---\n")
        .expect("write cash.md");

    let route_side = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("6")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do @ca^new-id")
        .output()
        .expect("run route-side task block ID completion");

    assert_success(&route_side);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&route_side).trim())
            .expect("capture-complete JSON");
    assert_eq!(json["context"], "route");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 4, "end": 6})
    );
    assert_eq!(json["candidates"][0]["route"], "cash");

    let id_side = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("10")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do @ca^new-id")
        .output()
        .expect("run ID-side task block ID completion");

    assert_success(&id_side);
    let json: serde_json::Value = serde_json::from_str(stdout(&id_side).trim())
        .expect("capture-complete JSON");
    assert!(json["context"].is_null(), "{json}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 10, "end": 10})
    );
    assert_eq!(json["candidates"].as_array().expect("candidates").len(), 0);
}

#[test]
fn capture_complete_section_json_lists_headings_of_the_resolved_route() {
    let temp = TempDir::new("bob-cli-capture-complete-section");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    fs::write(vault.join("notes.md"), "# Ideas\n## Inbox Ideas\n")
        .expect("write notes.md");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("14")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Idea @notes#Id")
        .output()
        .expect("run bob capture-complete section json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "section");
    assert_eq!(
        json["candidates"]
            .as_array()
            .expect("candidates array")
            .iter()
            .map(|candidate| candidate["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        vec!["Ideas", "Inbox Ideas"]
    );
}

#[test]
fn capture_complete_task_json_only_offers_tasks_with_a_block_id() {
    let temp = TempDir::new("bob-cli-capture-complete-task");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"*","name":"Next","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task No block ID\n",
            "- [*] #task Finish Google Exit Packet! ^goog-exit\n",
        ),
    );

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("15")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @Cash+goog")
        .output()
        .expect("run bob capture-complete task json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "task");
    let candidates = json["candidates"].as_array().expect("candidates array");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["replacement"], "goog-exit");
    assert_eq!(candidates[0]["block_id"], "goog-exit");
    assert_eq!(candidates[0]["route"], "cash");
    assert_eq!(candidates[0]["requires_block_id"], false);
    assert_eq!(candidates[0]["text"], "Finish Google Exit Packet!");
    assert_eq!(candidates[0]["section"], "Tasks");
    assert_eq!(candidates[0]["status_symbol"], "*");
}

#[test]
fn capture_complete_all_tasks_is_opt_in_and_plus_context_only() {
    let temp = TempDir::new("bob-cli-capture-complete-all-tasks");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("file.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task First missing\n",
            "- [ ] #task Ready one ^ready-one\n",
            "- [x] #task Done missing\n",
            "- [*] #task Ready two ^ready-two\n",
            "- [ ] #task Second missing\n",
        ),
    );

    let default = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("6")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@file+")
        .output()
        .expect("run default task completion");
    assert_success(&default);
    let default_json: serde_json::Value =
        serde_json::from_str(stdout(&default).trim()).expect("json");
    assert_eq!(default_json["context"], "task");
    let default_ids = task_block_ids(&default_json);
    assert_eq!(default_ids, vec![Some("ready-one"), Some("ready-two")]);

    let all_tasks = bob_command()
        .arg("capture-complete")
        .arg("-a")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("6")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@file+")
        .output()
        .expect("run all-tasks completion");
    assert_success(&all_tasks);
    let all_json: serde_json::Value =
        serde_json::from_str(stdout(&all_tasks).trim()).expect("json");
    assert_eq!(all_json["context"], "task");
    let candidates = all_json["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 4);
    assert_eq!(candidates[0]["block_id"], "ready-one");
    assert_eq!(candidates[0]["requires_block_id"], false);
    assert_eq!(candidates[1]["block_id"], "ready-two");
    assert!(candidates[2]["block_id"].is_null());
    assert_eq!(candidates[2]["requires_block_id"], true);
    assert_eq!(candidates[2]["replacement"], "");
    assert_eq!(candidates[2]["route"], "file");
    assert_eq!(candidates[2]["text"], "First missing");
    assert!(candidates[2]["ref"].as_str().expect("ref").contains(':'));
    assert_eq!(candidates[3]["text"], "Second missing");

    let pomodoro = bob_command()
        .arg("capture-complete")
        .arg("--all-tasks")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("6")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@file:")
        .output()
        .expect("run pomodoro completion with all-tasks");
    assert_success(&pomodoro);
    let pomodoro_json: serde_json::Value =
        serde_json::from_str(stdout(&pomodoro).trim()).expect("json");
    assert_eq!(pomodoro_json["context"], "pomodoro_block_id");
    assert_eq!(
        task_block_ids(&pomodoro_json),
        vec![Some("ready-one"), Some("ready-two")]
    );
}

fn task_block_ids(json: &serde_json::Value) -> Vec<Option<&str>> {
    json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|candidate| candidate["block_id"].as_str())
        .collect()
}

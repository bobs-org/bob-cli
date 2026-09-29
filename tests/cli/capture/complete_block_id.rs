//! Block-ID completion contract: intent, used IDs, suggestions, link
//! candidates, and the project-note `+` range fix.

use crate::support::*;
use std::fs;

fn write_settings(vault: &std::path::Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
}

fn complete_json(
    vault: &std::path::Path,
    draft: &str,
    cursor: usize,
    day_file: Option<&std::path::Path>,
) -> serde_json::Value {
    let mut command = bob_command();
    command
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft);
    if let Some(day_file) = day_file {
        command.env("BOB_DAY_FILE", day_file);
    }
    let output = command.output().expect("run capture-complete");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("complete JSON")
}

#[test]
fn block_id_link_new_and_project_note_intents() {
    let temp = TempDir::new("bob-cli-complete-block-id-intent");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_settings(&vault);
    write_file(&vault.join("sase.md"), "- [ ] #task Existing ^tool\n");

    let link = complete_json(&vault, "@sase:", 6, None);
    assert_eq!(link["context"], "pomodoro_block_id");
    assert_eq!(link["block_id"]["intent"], "link");
    assert_eq!(link["block_id"]["marker"], ":");
    assert_eq!(link["block_id"]["route"], "sase");
    assert_eq!(link["block_id"]["allowed_character"], "[A-Za-z0-9-]");
    assert_eq!(
        link["block_id"]["allowed_description"],
        "A-Z, a-z, 0-9 or '-'"
    );

    let new_text = complete_json(&vault, "x @sase:", 8, None);
    assert_eq!(new_text["block_id"]["intent"], "new");
    assert_eq!(new_text["block_id"]["allowed_character"], "[A-Za-z0-9-]");
    assert_eq!(
        new_text["block_id"]["allowed_description"],
        "A-Z, a-z, 0-9 or '-'"
    );
    assert_eq!(
        new_text["candidates"].as_array().expect("candidates").len(),
        0
    );

    let new_spaced = complete_json(&vault, "@sase: x", 6, None);
    assert_eq!(new_spaced["block_id"]["intent"], "new");

    let project = complete_json(&vault, "@sase:x+", 7, None);
    assert_eq!(project["context"], "pomodoro_block_id");
    assert_eq!(project["block_id"]["intent"], "project_note");
    assert_eq!(
        project["replacement"],
        serde_json::json!({"start": 6, "end": 7})
    );
    assert_eq!(
        project["candidates"].as_array().expect("candidates").len(),
        0
    );

    let caret = complete_json(&vault, "Fix flaky test @sase^", 21, None);
    assert_eq!(caret["context"], "task_block_id");
    assert_eq!(caret["block_id"]["intent"], "new");

    let caret_note = complete_json(&vault, "@sase^x+", 7, None);
    assert_eq!(caret_note["block_id"]["intent"], "project_note");

    // Schedule and clipboard markers force new intent.
    let scheduled = complete_json(&vault, "@sase: s:1", 6, None);
    assert_eq!(scheduled["block_id"]["intent"], "new");
    let clipped = complete_json(&vault, "@sase: %", 6, None);
    assert_eq!(clipped["block_id"]["intent"], "new");

    // Trailing markers on authored child lines and batch items complete.
    let child = "Parent\n- child @sase:";
    let child_value = complete_json(&vault, child, child.len(), None);
    assert_eq!(child_value["context"], "pomodoro_block_id");
    assert_eq!(child_value["block_id"]["intent"], "new");

    let batch = "@work first\n\n@sase:";
    let batch_value = complete_json(&vault, batch, batch.len(), None);
    assert_eq!(batch_value["context"], "pomodoro_block_id");

    // A cursor after the project-note sigil is an empty success.
    let after_plus = complete_json(&vault, "@sase:x+", 8, None);
    assert!(after_plus["context"].is_null());
}

#[test]
fn block_id_used_covers_done_nontask_duplicates_and_order() {
    let temp = TempDir::new("bob-cli-complete-block-id-used");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_settings(&vault);
    write_file(
        &vault.join("sase.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task First ^bbb\n",
            "- [x] #task Done ^aaa\n",
            "a paragraph ^ccc\n",
            "- [ ] #task Again ^bbb\n",
        ),
    );

    let value = complete_json(&vault, "@sase:", 6, None);
    let used = value["block_id"]["used"].as_array().expect("used");
    let ids: Vec<&str> = used
        .iter()
        .map(|entry| entry["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["bbb", "aaa", "ccc"]);
    assert_eq!(used[0]["line"], 2);
    assert_eq!(used[0]["task"], true);
    assert_eq!(used[1]["line"], 3);
    assert_eq!(used[1]["task"], true);
    assert_eq!(used[2]["line"], 4);
    assert_eq!(used[2]["task"], false);
    assert_eq!(used[2]["text"], "a paragraph");
    assert!(used[2]["status_symbol"].is_null());
}

#[test]
fn block_id_missing_note_is_not_an_error() {
    let temp = TempDir::new("bob-cli-complete-block-id-missing");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let value = complete_json(&vault, "@nope:", 6, None);
    assert_eq!(value["context"], "pomodoro_block_id");
    assert_eq!(value["block_id"]["note_exists"], false);
    assert_eq!(value["block_id"]["used"].as_array().expect("used").len(), 0);
    assert_eq!(value["candidates"].as_array().expect("candidates").len(), 0);
}

#[test]
fn block_id_link_candidates_are_filtered_and_annotated() {
    let temp = TempDir::new("bob-cli-complete-block-id-link");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_settings(&vault);
    write_file(
        &vault.join("sase.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task Ready ^ready\n",
            "- [?] #task Blocked ^blocked\n",
            "- [*] #task Next ^next\n",
            "- [/] #task Doing ^doing\n",
            "- [x] #task Done ^done\n",
            "- [-] #task Gone ^gone\n",
        ),
    );
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (0900-0930) — BUGS\n  - [[sase#^next]]\n",
    );

    let value = complete_json(&vault, "@sase:", 6, Some(&day_file));
    let candidates =
        value["candidates"].as_array().expect("candidates").clone();
    let ids: Vec<&str> = candidates
        .iter()
        .map(|row| row["block_id"].as_str().expect("block id"))
        .collect();
    assert_eq!(ids, vec!["ready", "blocked", "next", "doing"]);
    for row in &candidates {
        assert!(row["line"].as_u64().expect("line") > 0);
    }
    let next = candidates
        .iter()
        .find(|row| row["block_id"] == "next")
        .expect("next row");
    assert_eq!(next["pomodoro"]["name"], "BUGS");
    let ready = candidates
        .iter()
        .find(|row| row["block_id"] == "ready")
        .expect("ready row");
    assert!(ready["pomodoro"].is_null());
}

#[test]
fn block_id_suggestions_follow_body_examples() {
    let temp = TempDir::new("bob-cli-complete-block-id-suggest");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_settings(&vault);
    write_file(&vault.join("sase.md"), "- [ ] #task Existing ^tool\n");

    let draft = "Fix flaky gkeep test @sase^";
    let value = complete_json(&vault, draft, draft.len(), None);
    assert_eq!(value["block_id"]["intent"], "new");
    let suggestions: Vec<&str> = value["block_id"]["suggestions"]
        .as_array()
        .expect("suggestions")
        .iter()
        .map(|item| item.as_str().expect("suggestion"))
        .collect();
    assert_eq!(suggestions, vec!["fix-flaky-gkeep", "flaky-gkeep-test"]);
}

#[test]
fn block_id_json_shape_and_human_line() {
    let temp = TempDir::new("bob-cli-complete-block-id-shape");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_settings(&vault);
    write_file(&vault.join("sase.md"), "- [ ] #task Existing ^tool\n");

    let draft = "Fix flaky gkeep test @sase^";
    let json = complete_json(&vault, draft, draft.len(), None);
    assert_eq!(json["schema_version"], 1);
    assert!(json["block_id"]["marker_range"]["start"].is_number());
    assert_eq!(json["block_id"]["allowed_character"], "[A-Za-z0-9-]");
    assert_eq!(
        json["block_id"]["allowed_description"],
        "A-Z, a-z, 0-9 or '-'"
    );

    // Other contexts carry no `block_id` object.
    let route = complete_json(&vault, "@", 1, None);
    assert_eq!(route["context"], "route");
    assert!(route.get("block_id").is_none(), "{route}");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("--")
        .arg(draft)
        .output()
        .expect("run human completion");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("task_block_id"), "{text}");
    assert!(text.contains("sase.md"), "{text}");
    assert!(text.contains("in use"), "{text}");
}

//! `blocked_task` (`*`) completion, candidate guards, and task-ID replacement.

use crate::support::*;
use std::{collections::HashMap, fs};

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn write_completion_fixture(vault: &std::path::Path) {
    write_toggle_task_settings(vault);
    write_file(
        &vault.join("mac_inbox.md"),
        "- [ ] #task Inbox owner task ^inbox-owner\n",
    );
    write_file(
        &vault.join("notes.md"),
        concat!(
            "- [ ] #task Running task ^running\n",
            "- [ ] #task Queued task ^queued\n",
            "- [/] #task Pending task ^pending\n",
            "- [*] #task Next task ^next\n",
            "- [x] #task Closed task ^closed\n",
            "```\n",
            "- [ ] #task Fenced task ^fenced\n",
            "```\n",
        ),
    );
    write_file(
        &vault.join("other.md"),
        "- [ ] #task Other open task ^other\n",
    );
    write_file(&vault.join("job.md"), "- [ ] #task Job task ^job-task\n");
    write_file(
        &vault.join("done/old.md"),
        "- [ ] #task Archived open task ^archived\n",
    );
    write_file(
        &vault.join("_templates/template.md"),
        "- [ ] #task Template task ^template\n",
    );
    write_file(
        &vault.join(".hidden/hidden.md"),
        "- [ ] #task Hidden note task ^hidden-note\n",
    );
    write_file(
        &vault.join("2026/20260930.md"),
        concat!(
            "# Today\n## Pomodoros\n",
            "- [ ] (0900-0930) — RUNNING\n",
            "  - [[notes#^running]]\n",
            "- [ ] () — PLANNED\n",
            "  - [[notes#^queued]]\n",
            "- [ ] #task Daily task ^daily-task\n",
        ),
    );
}

fn complete_json(
    vault: &std::path::Path,
    text: &str,
    cursor: usize,
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_NOW", BOB_NOW)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .output()
        .expect("run blocked-task completion");
    assert_success(&output);
    serde_json::from_slice(stdout(&output).as_bytes()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn candidate_by_text(
    json: &serde_json::Value,
    text: &str,
) -> serde_json::Value {
    json["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .find(|candidate| candidate["text"].as_str() == Some(text))
        .unwrap_or_else(|| panic!("missing candidate {text:?}: {json}"))
        .clone()
}

fn candidate_texts(json: &serde_json::Value) -> Vec<String> {
    json["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .map(|candidate| candidate["text"].as_str().unwrap().to_string())
        .collect()
}

fn snapshot(vault: &std::path::Path) -> HashMap<String, String> {
    let mut files = HashMap::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("read directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(vault) {
                files.insert(
                    relative.to_string_lossy().replace('\\', "/"),
                    fs::read_to_string(&path).expect("read fixture file"),
                );
            }
        }
    }
    files
}

#[test]
fn partial_star_fields_rank_today_running_then_lanes_and_owner_note() {
    let temp = TempDir::new("bob-cli-complete-blocked-task-ranking");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_completion_fixture(&vault);
    let before = snapshot(&vault);

    let text = "Create a report *";
    let json = complete_json(&vault, text, text.len());
    assert_eq!(json["context"], "blocked_task");
    assert_eq!(json["query"], "");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 16, "end": 17})
    );
    assert_eq!(json["owner"]["kind"], "new_task");
    let texts = candidate_texts(&json);
    assert_eq!(
        &texts[..5],
        &[
            "Running task",
            "Queued task",
            "Pending task",
            "Next task",
            "Inbox owner task",
        ]
    );
    for excluded in [
        "Closed task",
        "Daily task",
        "Archived open task",
        "Template task",
        "Hidden note task",
        "Fenced task",
    ] {
        assert!(!texts.contains(&excluded.to_string()), "{texts:?}");
    }

    let running = candidate_by_text(&json, "Running task");
    assert_eq!(running["replacement"], "*notes:running");
    assert_eq!(running["group"], "today");
    let pending = candidate_by_text(&json, "Pending task");
    assert_eq!(pending["group"], "in_progress");
    let next = candidate_by_text(&json, "Next task");
    assert_eq!(next["group"], "next");
    assert_eq!(snapshot(&vault), before, "completion is read-only");

    let text = "Create a report *no";
    let json = complete_json(&vault, text, text.len());
    assert_eq!(json["context"], "blocked_task", "{text}");
    assert!(json["replacement"]["start"].as_u64().is_some(), "{json}");

    let text = "X *job:";
    let json = complete_json(&vault, text, text.len());
    assert_eq!(json["context"], "blocked_task", "{text}");
    assert_eq!(json["query"], "job:");
    assert_eq!(candidate_texts(&json), vec!["Job task"]);
    assert_eq!(json["candidates"][0]["replacement"], "*job:job-task");
}

#[test]
fn existing_owner_reports_self_existing_edges_and_typed_blocked_rows() {
    let temp = TempDir::new("bob-cli-complete-blocked-task-owner");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("owner.md"),
        concat!("- [ ] #task Owner ^owner\n",),
    );
    write_file(
        &vault.join("target.md"),
        concat!(
            "- [ ] #task Already blocked ^already\n",
            "  - ⛓️ **DEPENDS ON:** [[owner#^owner]]\n",
            "- [ ] #task New target ^new-target\n",
        ),
    );

    let text = "@owner+owner *";
    let json = complete_json(&vault, text, text.len());
    assert_eq!(json["context"], "blocked_task");
    assert_eq!(json["owner"]["kind"], "existing_task");
    let already = candidate_by_text(&json, "Already blocked");
    assert_eq!(already["already_blocked"], true);
    assert_eq!(already["replacement"], "*target:already");
    let owner = candidate_by_text(&json, "Owner");
    assert_eq!(owner["disabled_reason"], "the owner task itself");
    assert_eq!(owner["replacement"], "");

    let text = "@owner+owner *target:already *";
    let json = complete_json(&vault, text, text.len());
    let already = candidate_by_text(&json, "Already blocked");
    assert_eq!(already["already_blocked"], true);
}

#[test]
fn disabled_rows_explain_duplicate_ids_cycles_and_internal_projects() {
    let temp = TempDir::new("bob-cli-complete-blocked-task-guards");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("owner.md"),
        "- [ ] #task Owner ^owner\n  - ⛓️ **DEPENDS ON:** [[bridge#^bridge]]\n",
    );
    write_file(&vault.join("bridge.md"), "- [ ] #task Bridge ^bridge\n  - ⛓️ **DEPENDS ON:** [[target#^cycle]]\n");
    write_file(
        &vault.join("target.md"),
        "- [ ] #task Cycle target ^cycle\n",
    );
    write_file(
        &vault.join("duplicates.md"),
        "- [ ] #task Duplicate one ^duplicate\n- [ ] #task Duplicate two ^duplicate\n",
    );
    write_file(
        &vault.join("project.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Project ^prj\n- [ ] #task Project owner ^project-owner\n",
    );

    let text = "@owner+owner *";
    let json = complete_json(&vault, text, text.len());
    let cycle = candidate_by_text(&json, "Cycle target");
    assert_eq!(cycle["disabled_reason"], "would create a dependency cycle");
    let duplicate = candidate_by_text(&json, "Duplicate one");
    assert!(duplicate["disabled_reason"]
        .as_str()
        .unwrap()
        .contains("duplicate block ID"));

    let text = "New task &bridge:bridge *";
    let json = complete_json(&vault, text, text.len());
    let cycle = candidate_by_text(&json, "Cycle target");
    assert_eq!(cycle["disabled_reason"], "would create a dependency cycle");

    let text = "@project+project-owner *";
    let json = complete_json(&vault, text, text.len());
    let prj = candidate_by_text(&json, "Project");
    assert_eq!(
        prj["disabled_reason"],
        "a project cannot depend on its own resident"
    );
}

#[test]
fn task_id_assignment_returns_star_replacement_with_quoted_locator() {
    let temp = TempDir::new("bob-cli-complete-blocked-task-id");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("Shopping List.md"), "- [ ] #task Buy cheese\n");
    let task_ref = capture_pomodoro_ref("- [ ] #task Buy cheese", 1);
    let output = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .args([
            "--note-path",
            "Shopping List",
            "--task-ref",
            &task_ref,
            "--block-id",
            "cheese-task",
            "--format",
            "json",
            "--dry-run",
        ])
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run capture-task-id");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_slice(stdout(&output).as_bytes())
            .expect("task-id JSON");
    assert_eq!(
        json["blocked_task_replacement"],
        "*\"Shopping List\":cheese-task"
    );
}

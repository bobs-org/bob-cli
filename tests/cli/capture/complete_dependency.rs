//! `task_dependency` (`&`) completion and the dependency `--note-path` /
//! `--allow-closed` ID-assignment flow.
//!
//! The fixture vault exercises the discovery contract: untyped root
//! notes, nested folders, quoted case-sensitive names, duplicate
//! basenames, duplicate block IDs, hidden tasks, closed history,
//! daily/previous-daily notes, archive history, fenced code, non-task
//! anchors, and excluded directories. Completion is read-only: every
//! test snapshots the vault and the focused tests assert it is
//! untouched.

use crate::support::*;
use std::collections::HashMap;
use std::fs;

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn write_fixture(vault: &std::path::Path) {
    write_toggle_task_settings(vault);
    write_file(
        &vault.join("home.md"),
        concat!(
            "# Home\n",
            "- [ ] #task Buy milk ^milk\n",
            "- [/] #task Cook dinner ^dinner\n",
            "- [ ] #task No id yet\n",
            "- [ ] #task Hidden chore #hide ^hidden-chore\n",
            "- [x] #task Done deed ^done-deed\n",
            "```\n",
            "- [ ] #task Fenced task ^fenced\n",
            "```\n",
            "Plain line ^plain-anchor\n",
        ),
    );
    write_file(
        &vault.join("body.md"),
        concat!(
            "- [ ] #task Exercise ^excercise\n",
            "\t- ⛓️ **DEPENDS ON:** [[cash#^budget]]\n",
        ),
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Confirm grocery budget ^budget\n",
            "- [*] #task Plan meals ^meals\n",
            "- [?] #task Blocked bill ^bill\n",
        ),
    );
    write_file(
        &vault.join("projects/alpha.md"),
        concat!(
            "- [ ] #task Alpha task ^alpha\n",
            "- [ ] #task Alpha clone\n",
        ),
    );
    write_file(
        &vault.join("Shopping List.md"),
        concat!("- [ ] #task Buy eggs ^eggs\n", "- [ ] #task Buy cheese\n",),
    );
    write_file(&vault.join("a/dup.md"), "- [ ] #task Dup A ^dup-a\n");
    write_file(&vault.join("b/dup.md"), "- [ ] #task Dup B ^dup-b\n");
    write_file(
        &vault.join("twin.md"),
        concat!(
            "- [ ] #task First twin ^twin\n",
            "- [ ] #task Second twin ^twin\n",
        ),
    );
    write_file(
        &vault.join("_templates/t.md"),
        "- [ ] #task Tmpl task ^tmpl\n",
    );
    write_file(
        &vault.join(".hidden/h.md"),
        "- [ ] #task Hidden note task ^hid\n",
    );
    write_file(
        &vault.join("done/old.md"),
        concat!("- [x] #task Old done\n", "- [x] #task Old kept ^kept\n",),
    );
    write_file(
        &vault.join("2026/20260930.md"),
        concat!(
            "- [ ] #task Today task ^today-task\n",
            "- [ ] #task Today plain\n",
        ),
    );
    write_file(
        &vault.join("2026/20260929.md"),
        "- [ ] #task Yesterday task\n",
    );
}

fn complete_json(
    vault: &std::path::Path,
    cursor: usize,
    text: &str,
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
        .output()
        .expect("run dependency completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn snapshot(vault: &std::path::Path) -> HashMap<String, String> {
    let mut files = HashMap::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut entries: Vec<_> =
            fs::read_dir(&directory).expect("read dir").collect();
        entries
            .sort_by_key(|entry| entry.as_ref().map(|entry| entry.path()).ok());
        for entry in entries {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if entry.file_type().expect("file type").is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(vault) {
                files.insert(
                    relative.to_string_lossy().replace('\\', "/"),
                    fs::read_to_string(&path).expect("read file"),
                );
            }
        }
    }
    files
}

fn candidate_texts(json: &serde_json::Value) -> Vec<String> {
    json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|candidate| candidate["text"].as_str().expect("text").to_string())
        .collect()
}

fn candidate_by_text(
    json: &serde_json::Value,
    text: &str,
) -> serde_json::Value {
    json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| candidate["text"].as_str() == Some(text))
        .unwrap_or_else(|| panic!("missing candidate {text:?}"))
        .clone()
}

#[test]
fn empty_query_orders_lanes_before_history_and_hides_last() {
    let temp = TempDir::new("bob-cli-complete-dependency-order");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);
    let before = snapshot(&vault);

    let text = "Buy Groceries! &";
    let json = complete_json(&vault, text.len(), text);
    assert_eq!(json["ok"], true);
    assert_eq!(json["context"], "task_dependency");
    assert_eq!(json["query"], "");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 15, "end": 16})
    );
    assert!(json["owner"].is_object(), "owner: {json}");

    let texts = candidate_texts(&json);
    // Lanes first: In Progress, then Next, then other open notes.
    assert_eq!(texts[0], "Cook dinner");
    assert_eq!(texts[1], "Plan meals");
    // Completed history trails every open task.
    let first_completed = texts
        .iter()
        .position(|text| {
            ["Done deed", "Old done", "Old kept"].contains(&text.as_str())
        })
        .expect("completed history is listed");
    assert!(
        texts[..first_completed].iter().all(|text| ![
            "Done deed",
            "Old done",
            "Old kept"
        ]
        .contains(&text.as_str())),
        "{texts:?}"
    );
    // Hidden tasks order last within their open section (`#hide`
    // stays in the display text, exactly like the `:` picker).
    let milk = texts.iter().position(|text| text == "Buy milk").unwrap();
    let hidden = texts
        .iter()
        .position(|text| text == "Hidden chore #hide")
        .unwrap();
    assert!(milk < hidden, "{texts:?}");
    // Excluded notes, fenced code, and non-task anchors never surface.
    for absent in ["Tmpl task", "Hidden note task", "Fenced task"] {
        assert!(!texts.contains(&absent.to_string()), "{texts:?}");
    }
    assert_eq!(snapshot(&vault), before, "completion writes nothing");
}

#[test]
fn rows_carry_exact_identities_and_id_flow_metadata() {
    let temp = TempDir::new("bob-cli-complete-dependency-rows");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let text = "Buy Groceries! &";
    let json = complete_json(&vault, text.len(), text);

    let budget = candidate_by_text(&json, "Confirm grocery budget");
    assert_eq!(budget["replacement"], "&cash:budget");
    assert_eq!(budget["note_path"], "cash.md");
    assert_eq!(budget["locator"], "cash");
    assert_eq!(budget["requires_block_id"], false);
    assert_eq!(budget["hidden"], false);

    let eggs = candidate_by_text(&json, "Buy eggs");
    assert_eq!(eggs["note_path"], "Shopping List.md");
    assert_eq!(eggs["locator"], "Shopping List");
    assert_eq!(eggs["replacement"], "&\"Shopping List\":eggs");

    let dup_a = candidate_by_text(&json, "Dup A");
    assert_eq!(dup_a["note_path"], "a/dup.md");
    assert_eq!(dup_a["locator"], "a/dup");
    assert_eq!(dup_a["replacement"], "&a/dup:dup-a");

    // ID-less rows carry no insertable replacement and suggest IDs.
    let clone = candidate_by_text(&json, "Alpha clone");
    assert_eq!(clone["replacement"], "");
    assert_eq!(clone["requires_block_id"], true);
    assert!(
        !clone["block_id_suggestions"]
            .as_array()
            .expect("suggestions")
            .is_empty(),
        "{clone}"
    );

    // Duplicate block IDs are guarded, not silently dropped.
    let twin = candidate_by_text(&json, "First twin");
    assert_eq!(twin["replacement"], "");
    assert!(
        twin["disabled_reason"]
            .as_str()
            .expect("reason")
            .contains("duplicate block ID"),
        "{twin}"
    );
}

#[test]
fn query_ranks_matches_and_keeps_history_findable() {
    let temp = TempDir::new("bob-cli-complete-dependency-rank");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let text = "Buy Groceries! &meals";
    let json = complete_json(&vault, text.len(), text);
    assert_eq!(json["query"], "meals");
    let texts = candidate_texts(&json);
    assert_eq!(texts, vec!["Plan meals"], "{texts:?}");

    // A locator-qualified query matches the `locator:block-id` field.
    let text = "Buy Groceries! &cash:bud";
    let json = complete_json(&vault, text.len(), text);
    assert_eq!(candidate_texts(&json), vec!["Confirm grocery budget"]);

    // Completed history stays findable through search.
    let text = "Buy Groceries! &kept";
    let json = complete_json(&vault, text.len(), text);
    let texts = candidate_texts(&json);
    assert_eq!(texts, vec!["Old kept"], "{texts:?}");
    assert_eq!(json["candidates"][0]["group"], "completed");
}

#[test]
fn refetch_at_replacement_start_returns_the_full_snapshot() {
    let temp = TempDir::new("bob-cli-complete-dependency-refetch");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let full = complete_json(&vault, 15, "Buy Groceries! &");
    let filtered = complete_json(&vault, 20, "Buy Groceries! &meals");
    assert!(
        full["candidates"].as_array().expect("array").len()
            > filtered["candidates"].as_array().expect("array").len()
    );
    assert_eq!(full["query"], "");
}

#[test]
fn existing_owner_marks_self_and_already_added_rows() {
    let temp = TempDir::new("bob-cli-complete-dependency-owner");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    // body.md ^excercise already depends on cash.md ^budget. The
    // trailing `&` opens the picker with an empty query, so the whole
    // snapshot (including the dependent itself) is listed.
    let text = "&cash:budget @body+excercise &";
    let json = complete_json(&vault, text.len(), text);
    assert_eq!(json["context"], "task_dependency");
    assert_eq!(json["query"], "");

    let budget = candidate_by_text(&json, "Confirm grocery budget");
    assert_eq!(budget["already_dependency"], true);

    let exercise = candidate_by_text(&json, "Exercise");
    assert_eq!(exercise["disabled_reason"], "the dependent task itself");
    assert_eq!(exercise["replacement"], "");
}

fn task_id_json(
    vault: &std::path::Path,
    args: &[&str],
    now: &str,
) -> (std::process::Output, serde_json::Value) {
    let output = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(vault)
        .args(args)
        .env("BOB_NOW", now)
        .output()
        .expect("run capture-task-id");
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!(
                "task-id stdout should be JSON: {error}\n{}",
                format_output(&output)
            )
        });
    (output, json)
}

#[test]
fn note_path_assigns_nested_case_sensitive_notes() {
    let temp = TempDir::new("bob-cli-dependency-task-id-path");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let task_ref = capture_pomodoro_ref("- [ ] #task Alpha clone", 2);
    let (output, json) = task_id_json(
        &vault,
        &[
            "-n",
            "projects/alpha.md",
            "-t",
            &task_ref,
            "-i",
            "alpha-clone",
            "-f",
            "json",
        ],
        BOB_NOW,
    );
    assert_success(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["note_path"], "projects/alpha.md");
    assert!(json.get("route").is_none(), "{json}");
    // The basename is unique, so the backend formats the shortest
    // unambiguous locator, exactly like the canonical Depends-On link.
    assert_eq!(json["dependency_replacement"], "&alpha:alpha-clone");
    assert_eq!(json["relative_target"], "projects/alpha.md");
    let updated =
        fs::read_to_string(vault.join("projects/alpha.md")).expect("read");
    assert!(
        updated.contains("- [ ] #task Alpha clone ^alpha-clone"),
        "{updated}"
    );
    // The sibling row is byte-identical.
    assert!(
        updated.contains("- [ ] #task Alpha task ^alpha\n"),
        "{updated}"
    );
}

#[test]
fn note_path_preserves_quoting_and_case_in_the_replacement() {
    let temp = TempDir::new("bob-cli-dependency-task-id-quoted");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    // Extensionless paths resolve like routes; the dry run writes
    // nothing to the quoted, case-sensitive note.
    let task_ref = capture_pomodoro_ref("- [ ] #task Buy cheese", 2);
    let (output, json) = task_id_json(
        &vault,
        &[
            "-n",
            "Shopping List",
            "-t",
            &task_ref,
            "-i",
            "new-eggs",
            "-f",
            "json",
            "-d",
        ],
        BOB_NOW,
    );
    assert_success(&output);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["note_path"], "Shopping List.md");
    assert_eq!(
        json["dependency_replacement"],
        "&\"Shopping List\":new-eggs"
    );
}

#[test]
fn closed_tasks_need_the_opt_in_and_stay_closed() {
    let temp = TempDir::new("bob-cli-dependency-task-id-closed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let task_ref = capture_pomodoro_ref("- [x] #task Old done", 1);
    let (output, json) = task_id_json(
        &vault,
        &[
            "-n",
            "done/old.md",
            "-t",
            &task_ref,
            "-i",
            "old-done",
            "-f",
            "json",
        ],
        BOB_NOW,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(json["ok"], false);
    assert!(json["error"]
        .as_str()
        .expect("error")
        .contains("no longer open"));

    let (output, json) = task_id_json(
        &vault,
        &[
            "-n",
            "done/old.md",
            "-t",
            &task_ref,
            "-i",
            "old-done",
            "-f",
            "json",
            "--allow-closed",
        ],
        BOB_NOW,
    );
    assert_success(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["dependency_replacement"], "&old:old-done");
    let updated = fs::read_to_string(vault.join("done/old.md")).expect("read");
    // The opt-in never reopens the task.
    assert!(
        updated.contains("- [x] #task Old done ^old-done"),
        "{updated}"
    );
}

#[test]
fn previous_daily_snapshot_is_read_only() {
    let temp = TempDir::new("bob-cli-dependency-task-id-daily");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let task_ref = capture_pomodoro_ref("- [ ] #task Yesterday task", 1);
    let before =
        fs::read_to_string(vault.join("2026/20260929.md")).expect("read");
    let (output, json) = task_id_json(
        &vault,
        &[
            "-n",
            "2026/20260929.md",
            "-t",
            &task_ref,
            "-i",
            "yesterday",
            "-f",
            "json",
        ],
        BOB_NOW,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .expect("error")
            .contains("previous daily snapshot"),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("2026/20260929.md")).expect("read"),
        before
    );

    // Today's daily note assigns normally.
    let today_plain_ref = capture_pomodoro_ref("- [ ] #task Today plain", 2);
    let (output, json) = task_id_json(
        &vault,
        &[
            "-n",
            "2026/20260930.md",
            "-t",
            &today_plain_ref,
            "-i",
            "today-plain",
            "-f",
            "json",
            "-d",
        ],
        BOB_NOW,
    );
    assert_success(&output);
    assert_eq!(json["ok"], true);
}

#[test]
fn traversal_and_ambiguous_paths_fail_without_writing() {
    let temp = TempDir::new("bob-cli-dependency-task-id-guards");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);
    let before = snapshot(&vault);

    let task_ref = capture_pomodoro_ref("- [ ] #task Dup A ^dup-a", 1);
    for (name, path) in [
        ("traversal", "../escape"),
        ("absolute", "/abs"),
        ("ambiguous", "dup"),
    ] {
        let (output, json) = task_id_json(
            &vault,
            &["-n", path, "-t", &task_ref, "-i", "x", "-f", "json"],
            BOB_NOW,
        );
        assert!(
            !output.status.success(),
            "{name}: {}",
            format_output(&output)
        );
        assert_eq!(json["ok"], false, "{name}: {json}");
        if name == "ambiguous" {
            assert!(
                json["error"].as_str().expect("error").contains("ambiguous"),
                "{json}"
            );
        }
    }
    assert_eq!(snapshot(&vault), before);
}

#[test]
fn protected_spans_never_open_the_dependency_picker() {
    let temp = TempDir::new("bob-cli-complete-dependency-literal");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    for text in ["See [[&foo:bar]]", "Note `&foo` here"] {
        let cursor = text.find("&").expect("sigil") + 2;
        let json = complete_json(&vault, cursor, text);
        assert_ne!(json["context"], "task_dependency", "{text}: {json}");
    }
}

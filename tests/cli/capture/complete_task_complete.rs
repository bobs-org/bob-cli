//! `task_complete` (`!`) completion: the vault-wide completable-task
//! catalog with today's Task Link annotations, the vault-scoped picker
//! descriptor, and the `complete_replacement` ID-assignment flow.
//!
//! The fixture vault pins `BOB_NOW` and `BOB_DAY_FILE` so today roles
//! and ordering are deterministic. Completion is read-only: every
//! test snapshots the vault and the focused tests assert it is
//! untouched.

use crate::support::*;
use std::fs;

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn write_fixture(vault: &std::path::Path) -> std::path::PathBuf {
    write_toggle_task_settings(vault);
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [*] #task Fix deep bug ^deep-fix\n",
            "- [/] #task Draft outline ^outline\n",
            "- [ ] #task No id yet\n",
            "- [ ] #task Water plants \u{1f501} ^water\n",
            "- [?] #task Blocked bill ^bill\n",
            "- [ ] #task Ship blog post ^blog\n",
        ),
    );
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Call the bank ^call-bank\n",
    );
    write_file(
        &vault.join("Shopping List.md"),
        concat!("- [ ] #task Buy eggs ^eggs\n", "- [ ] #task Buy cheese\n",),
    );
    let day_file = vault.join("2026/20260930.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**09:20 - 09:50** [t:: 30m]) \u{2014} CAPTURE\n",
            "\t- [[sase#^deep-fix]]\n",
            "\t- [[cash#^call-bank]]\n",
            "- [x] (0800-0830) \u{2014} PLAN\n",
            "\t- [[sase#^outline]]\n",
        ),
    );
    day_file
}

fn snapshot(vault: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    let mut files = Vec::new();
    for entry in walk(vault) {
        files.push((
            entry.clone(),
            fs::read_to_string(entry).unwrap_or_default(),
        ));
    }
    files
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(top) = stack.pop() {
        let entries = fs::read_dir(&top).expect("read dir");
        for entry in entries {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn complete_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
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
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run task-complete completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn complete_stdin_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    cursor: usize,
    text: &str,
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
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW);
    let output = run_with_stdin(&mut command, text);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn replacements(json: &serde_json::Value) -> Vec<&str> {
    json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|candidate| {
            candidate["replacement"]
                .as_str()
                .expect("replacement string")
        })
        .collect()
}

#[test]
fn complete_bare_bang_lists_today_first_with_picker() {
    let temp = TempDir::new("bob-cli-complete-task-complete-bare");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_fixture(&vault);
    let before = snapshot(&vault);

    let json = complete_json(&vault, &day_file, 1, "!");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["context"], "task_complete");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 0, "end": 1})
    );
    assert_eq!(json["query"], "");
    // Today rows first: running in ledger order, then worked, then
    // open by note path and line (byte order puts the quoted
    // `Shopping List.md` first), hidden and recurring sunk last.
    assert_eq!(
        replacements(&json),
        vec![
            "!sase:deep-fix",
            "!cash:call-bank",
            "!sase:outline",
            "!\"Shopping List\":eggs",
            "",
            "",
            "!sase:bill",
            "!sase:blog",
            "",
        ],
        "today-first order with quoted locators and ID-less tails"
    );
    let deep = &json["candidates"][0];
    assert_eq!(deep["note_path"], "sase.md");
    assert_eq!(deep["locator"], "sase");
    assert_eq!(deep["group"], "today");
    assert_eq!(deep["today"]["role"], "running");
    assert_eq!(deep["today"]["pomodoro"]["name"], "CAPTURE");
    assert_eq!(deep["block_id"], "deep-fix");
    // The bare `!` carries the vault picker plus continuation keys.
    let picker = &json["picker"];
    assert_eq!(picker["kind"], "task_complete");
    assert_eq!(picker["scope"], "vault");
    assert_eq!(picker["scope_token"], "!");
    assert_eq!(
        picker["marker_range"],
        serde_json::json!({"start": 0, "end": 1})
    );
    assert_eq!(
        picker["trigger_removal_range"],
        serde_json::json!({"start": 0, "end": 1})
    );
    assert_eq!(
        picker["action_continuation_keys"],
        serde_json::json!(["!", "["])
    );
    assert_eq!(snapshot(&vault), before, "completion writes nothing");
}

#[test]
fn complete_partial_query_decodes_and_drops_continuation_keys() {
    let temp = TempDir::new("bob-cli-complete-task-complete-query");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_fixture(&vault);

    let json = complete_json(&vault, &day_file, 4, "!fix");
    assert_eq!(json["context"], "task_complete");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 0, "end": 4})
    );
    assert_eq!(json["query"], "fix");
    // The descriptor stays; only the bare-`!` continuation keys go.
    assert_eq!(json["picker"]["kind"], "task_complete");
    assert_eq!(
        json["picker"]["marker_range"],
        serde_json::json!({"start": 0, "end": 4})
    );
    assert!(
        json["picker"].get("action_continuation_keys").is_none(),
        "only a bare ! continues: {}",
        json["picker"]
    );
    assert!(
        replacements(&json).contains(&"!sase:deep-fix"),
        "query matches: {}",
        json["candidates"]
    );

    // Quoted queries decode like `&`. The shared tiered matcher
    // also admits in-order subsequences, so the stranded `Shop`
    // substring still surfaces the quoted row first.
    let quoted = complete_json(&vault, &day_file, 6, "!\"Shop");
    assert_eq!(quoted["context"], "task_complete");
    assert_eq!(quoted["query"], "Shop");
    assert_eq!(
        replacements(&quoted),
        vec!["!\"Shopping List\":eggs", "", "!sase:blog"],
        "quoted locator match first"
    );
}

#[test]
fn complete_padded_line_offers_no_picker() {
    let temp = TempDir::new("bob-cli-complete-task-complete-padded");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_fixture(&vault);

    // A complete token with extra text is claimed invalid: teaching
    // error, never a picker.
    let json = complete_json(&vault, &day_file, 19, "!sase:deep-fix extra");
    assert_eq!(json["ok"], true);
    assert!(
        json.get("context").is_none() || json["context"].is_null(),
        "invalid items offer no context: {json}"
    );
    assert!(
        json.get("picker").is_none() || json["picker"].is_null(),
        "invalid items offer no picker: {json}"
    );
}

#[test]
fn complete_second_item_marks_already_selected() {
    let temp = TempDir::new("bob-cli-complete-task-complete-selected");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_fixture(&vault);

    // The second item's `sase:` query matches every sase task, so
    // the first item's task lists as already selected while the rest
    // stay enabled.
    let draft = "!sase:deep-fix\n\n!sase:";
    let json = complete_stdin_json(&vault, &day_file, draft.len(), draft);
    assert_eq!(json["context"], "task_complete");
    assert_eq!(json["query"], "sase:");
    let deep = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| candidate["block_id"] == "deep-fix")
        .expect("deep-fix lists");
    assert_eq!(deep["already_selected"], true);
    assert_eq!(deep["disabled_reason"], "Already in this draft");
    assert_eq!(deep["replacement"], "");
    // The other sase tasks stay enabled.
    let blog = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| candidate["block_id"] == "blog")
        .expect("blog lists");
    assert!(blog.get("already_selected").is_none(), "{blog}");
    assert_eq!(blog["replacement"], "!sase:blog");
}

#[test]
fn complete_id_less_and_recurring_rows_carry_guards() {
    let temp = TempDir::new("bob-cli-complete-task-complete-guards");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_fixture(&vault);

    let json = complete_json(&vault, &day_file, 1, "!");
    let candidates = json["candidates"].as_array().expect("candidates array");
    let no_id = candidates
        .iter()
        .find(|candidate| candidate["text"] == "No id yet")
        .expect("ID-less row lists");
    assert_eq!(no_id["requires_block_id"], true);
    assert_eq!(no_id["replacement"], "");
    assert!(
        !no_id["block_id_suggestions"]
            .as_array()
            .expect("suggestions")
            .is_empty(),
        "ID-less rows carry suggestions: {no_id}"
    );
    let recurring = candidates
        .iter()
        .find(|candidate| candidate["block_id"] == "water")
        .expect("recurring row lists");
    assert_eq!(recurring["recurring"], true);
    assert_eq!(
        recurring["disabled_reason"],
        "Recurring — complete it in Obsidian so Tasks writes the next occurrence"
    );
    assert_eq!(recurring["replacement"], "");
}

#[test]
fn capture_task_id_reports_complete_replacement_in_both_modes() {
    let temp = TempDir::new("bob-cli-complete-task-complete-task-id");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let _day_file = write_fixture(&vault);

    // The `!` picker discovers the ID-less row through
    // capture-complete, then assigns through capture-task-id.
    let json = complete_json(&vault, &vault.join("2026/20260930.md"), 1, "!");
    let no_id = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| candidate["text"] == "No id yet")
        .expect("ID-less row lists");
    let task_ref = no_id["ref"].as_str().expect("ref string");
    let suggestions = no_id["block_id_suggestions"]
        .as_array()
        .expect("suggestions array");
    assert!(!suggestions.is_empty(), "suggestions exist");
    let block_id = suggestions[0].as_str().expect("suggestion string");

    let output = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("sase")
        .arg("--task-ref")
        .arg(task_ref)
        .arg("--block-id")
        .arg(block_id)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("assign dry-run");
    assert_success(&output);
    let assigned: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!(
                "task-id stdout should be JSON: {error}\n{}",
                format_output(&output)
            )
        });
    assert_eq!(assigned["ok"], true);
    assert_eq!(
        assigned["complete_replacement"],
        format!("!sase:{block_id}"),
        "route mode splices the ! replacement"
    );

    // `--note-path` mode reports it too, quoted when needed.
    let cheese_ref = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| candidate["text"] == "Buy cheese")
        .expect("cheese lists")["ref"]
        .as_str()
        .expect("ref string");
    let cheese_id = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| candidate["text"] == "Buy cheese")
        .expect("cheese lists")["block_id_suggestions"][0]
        .as_str()
        .expect("suggestion string");
    let note_path_output = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .arg("--note-path")
        .arg("Shopping List.md")
        .arg("--task-ref")
        .arg(cheese_ref)
        .arg("--block-id")
        .arg(cheese_id)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("assign dry-run");
    assert_success(&note_path_output);
    let note_path_assigned: serde_json::Value = serde_json::from_str(
        stdout(&note_path_output).trim(),
    )
    .unwrap_or_else(|error| {
        panic!(
            "task-id stdout should be JSON: {error}\n{}",
            format_output(&note_path_output)
        )
    });
    assert_eq!(note_path_assigned["ok"], true);
    assert_eq!(
        note_path_assigned["complete_replacement"],
        format!("!\"Shopping List\":{cheese_id}"),
        "note-path mode quotes the locator"
    );
}

#[test]
fn shell_completion_serves_identified_enabled_bang_rows() {
    let temp = TempDir::new("bob-cli-complete-task-complete-shell");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let _day_file = write_fixture(&vault);

    let output = bob_command()
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .arg("bob")
        .arg("capture")
        .arg("!")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob __complete");
    assert_success(&output);
    let out = stdout(&output);
    // Values never carry the `!` sigil itself (it would parse as a
    // protocol directive): the presenter keeps it with `!prefix 1`
    // and the inserted text is still the full `!note:block-id`.
    assert!(
        out.lines().any(|line| line == "!prefix 1"),
        "sigil kept as prefix: {out}"
    );
    let values: Vec<&str> = out
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').next().unwrap_or_default())
        .collect();
    let groups: Vec<&str> = out
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').nth(2).unwrap_or_default())
        .collect();
    assert!(
        values.contains(&"sase:deep-fix"),
        "identified rows serve: {values:?}"
    );
    assert!(
        values.contains(&"\"Shopping List\":eggs"),
        "quoted rows serve verbatim: {values:?}"
    );
    assert!(groups.contains(&"today"), "today group first: {groups:?}");
    assert!(groups.contains(&"open"), "open group present: {groups:?}");
    assert!(
        !out.contains("No id yet"),
        "shell serves identified rows only: {out}"
    );
    assert!(
        !out.contains("Water plants"),
        "shell serves enabled rows only: {out}"
    );
}

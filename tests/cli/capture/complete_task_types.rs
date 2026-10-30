//! `task_type` capture-complete candidates and their zsh-facing values.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path};

const NOW: &str = "2026-10-30 09:00:00";

fn fixture(
    root: &Path,
    configured: bool,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("dev.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "task_type: BUILD\n",
            "---\n",
            "# Dev\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Current fix [type:: FIX] ^current\n",
            "- [ ] #task Inherited type ^inherited\n",
        ),
    );
    let day_file = vault.join("2026/20261030.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (0800-0830) — FIX\n",
            "- [x] (0830-0900) — BUILD\n",
        ),
    );
    write_file(
        &vault.join("2026/20261029.md"),
        "## Pomodoros\n- [x] () — DEEP WORK\n",
    );
    let mut config = String::from("plan:\n  default_type: BUILD\n");
    if configured {
        config.push_str(concat!(
            "types:\n",
            "  BUILD: yellow\n",
            "  FIX: blue\n",
            "  FIXUP: red\n",
            "  PREFIX: magenta\n",
            "  \"DEEP WORK\": violet\n",
        ));
    }
    let config_path = root.join("config.yml");
    write_file(&config_path, &config);
    (vault, config_path)
}

fn complete(
    root: &Path,
    vault: &Path,
    config: &Path,
    draft: &str,
    cursor: usize,
) -> Value {
    let output = bob_command()
        .args([
            "capture-complete",
            "-b",
            vault.to_str().expect("UTF-8 fixture path"),
            "-c",
            &cursor.to_string(),
            "-f",
            "json",
            "--",
            draft,
        ])
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", vault.join("2026/20261030.md"))
        .env("BOB_CONFIG_FILE", config)
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .expect("run task-type capture completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion output should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn complete_end(
    root: &Path,
    vault: &Path,
    config: &Path,
    draft: &str,
) -> Value {
    complete(root, vault, config, draft, draft.len())
}

fn names(value: &Value) -> Vec<&str> {
    value["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .map(|row| row["name"].as_str().expect("candidate name"))
        .collect()
}

#[test]
fn configured_menu_uses_config_order_and_carries_task_effects() {
    let temp = TempDir::new("bob-cli-complete-task-types-config");
    let (vault, config) = fixture(temp.path(), true);
    let draft = "@dev t:";
    let result = complete_end(temp.path(), &vault, &config, draft);

    assert_eq!(result["context"], "task_type");
    assert_eq!(result["subject"], "new");
    assert_eq!(result["query"], "");
    assert_eq!(
        result["replacement"],
        serde_json::json!({"start": 5, "end": 7})
    );
    assert_eq!(
        names(&result),
        vec!["BUILD", "FIX", "FIXUP", "PREFIX", "DEEP WORK"]
    );
    let build = &result["candidates"][0];
    assert_eq!(build["replacement"], "t:build ");
    assert_eq!(build["origin"], "configured");
    assert_eq!(build["current"], true);
    assert_eq!(build["inherited"], true);
    assert_eq!(build["effect"], "keep");
    assert_eq!(build["cursor_after"], 13);
    assert_eq!(build["suggestion_label"], "default type");
    assert_eq!(build["themes"][0]["configured"], true);
    assert_eq!(result["theme_suggestion"]["theme"]["key"], "build");
}

#[test]
fn query_orders_exact_prefix_substring_and_adds_typed_row() {
    let temp = TempDir::new("bob-cli-complete-task-types-rank");
    let (vault, config) = fixture(temp.path(), true);
    let result = complete_end(temp.path(), &vault, &config, "Fix it t:fix");

    assert_eq!(names(&result), vec!["FIX", "FIXUP", "PREFIX"]);
    assert_eq!(result["candidates"][0]["replacement"], "t:fix");
    assert_eq!(result["candidates"][0]["effect"], "set");

    let typed = complete_end(temp.path(), &vault, &config, "Fix it t:fixs");
    let row = typed["candidates"]
        .as_array()
        .unwrap()
        .last()
        .expect("typed row");
    assert_eq!(row["name"], "FIXS");
    assert_eq!(row["origin"], "typed");
    assert_eq!(row["themes"][0]["configured"], false);
    assert_eq!(row["similar_to"][0], "FIX");
    assert!(row.get("refused").is_none(), "unknown_types=warn");
}

#[test]
fn exact_hidden_type_remains_available_but_not_in_empty_menu() {
    let temp = TempDir::new("bob-cli-complete-task-types-hidden");
    let (vault, config) = fixture(temp.path(), true);
    write_file(
        &vault.join(".bob/themes/test.jsonl"),
        "{\"v\":1,\"ts\":\"2026-10-30T07:00:00Z\",\"host\":\"test\",\"kind\":\"theme.hide\",\"key\":\"fix\",\"name\":\"FIX\",\"via\":\"test\"}\n",
    );
    let journal = fs::read_to_string(vault.join(".bob/themes/test.jsonl"))
        .expect("theme journal");

    let empty = complete_end(temp.path(), &vault, &config, "t:");
    assert!(!names(&empty).contains(&"FIX"));
    let exact = complete_end(temp.path(), &vault, &config, "t:fix");
    let fix = exact["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "FIX")
        .expect("exact hidden row");
    assert_eq!(fix["hidden"], true);
    assert_eq!(
        fs::read_to_string(vault.join(".bob/themes/test.jsonl")).unwrap(),
        journal,
        "completion never changes theme preferences"
    );
}

#[test]
fn current_and_inherited_existing_types_drive_effects() {
    let temp = TempDir::new("bob-cli-complete-task-types-existing");
    let (vault, config) = fixture(temp.path(), true);

    let current = complete_end(temp.path(), &vault, &config, "@dev:current t:");
    assert_eq!(current["subject"], "existing");
    let fix = current["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "FIX")
        .unwrap();
    assert_eq!(fix["current"], true);
    assert_eq!(fix["effect"], "keep");
    let build = current["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "BUILD")
        .unwrap();
    assert_eq!(build["inherited"], true);
    assert_eq!(build["effect"], "clear");

    let inherited =
        complete_end(temp.path(), &vault, &config, "@dev:inherited t:");
    assert_eq!(inherited["subject"], "existing");
    let build = inherited["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "BUILD")
        .unwrap();
    assert_eq!(build["current"], true);
    assert_eq!(build["inherited"], true);
    assert_eq!(build["effect"], "keep");
}

#[test]
fn literal_replacement_child_and_second_line_triggers_work() {
    let temp = TempDir::new("bob-cli-complete-task-types-literal");
    let (vault, config) = fixture(temp.path(), true);
    let literal = complete_end(temp.path(), &vault, &config, "t:");
    let deep = literal["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "DEEP WORK")
        .unwrap();
    assert_eq!(deep["replacement"], "t:\"DEEP WORK\" ");

    let child = "Parent task\n- check the lexer t:fi";
    assert_eq!(
        complete_end(temp.path(), &vault, &config, child)["context"],
        "task_type"
    );
    let second_line = "Parent task\n t:fi";
    assert_eq!(
        complete_end(temp.path(), &vault, &config, second_line)["context"],
        "task_type"
    );
    let mid_line = "Ask about t:fix today";
    assert!(
        complete_end(temp.path(), &vault, &config, mid_line)["context"]
            .is_null()
    );
}

#[test]
fn route_completion_wins_at_its_token_and_unconfigured_menu_uses_history() {
    let temp = TempDir::new("bob-cli-complete-task-types-context");
    let (vault, config) = fixture(temp.path(), false);
    let route = complete(temp.path(), &vault, &config, "@de t:fix", 3);
    assert_eq!(route["context"], "route");

    let history = complete_end(temp.path(), &vault, &config, "t:");
    assert_eq!(history["context"], "task_type");
    assert_eq!(history["candidates"][0]["origin"], "history");
    assert_eq!(history["candidates"][0]["name"], "BUILD");
}

#[test]
fn type_marker_inside_task_wikilink_owns_the_caret() {
    let temp = TempDir::new("bob-cli-complete-task-types-wikilink");
    let (vault, config) = fixture(temp.path(), true);
    let draft = "Follow up ((foo t:";
    let at_type = complete(temp.path(), &vault, &config, draft, draft.len());
    assert_eq!(at_type["context"], "task_type", "{at_type}");
    assert_eq!(at_type["query"], "", "{at_type}");
    let start = draft.rfind("t:").expect("type marker");
    assert_eq!(
        at_type["replacement"],
        serde_json::json!({"start": start, "end": draft.len()}),
        "{at_type}"
    );

    let opener = draft.find("((").expect("wikilink opener");
    let at_opener = complete(temp.path(), &vault, &config, draft, opener);
    assert_eq!(at_opener["context"], "wikilink_task", "{at_opener}");
    assert_eq!(at_opener["query"], "", "{at_opener}");
    assert_eq!(
        at_opener["replacement"],
        serde_json::json!({"start": opener, "end": opener + 5}),
        "{at_opener}"
    );

    let inside_query =
        complete(temp.path(), &vault, &config, draft, opener + 4);
    assert_eq!(inside_query["context"], "wikilink_task", "{inside_query}");
    assert_eq!(inside_query["query"], "fo", "{inside_query}");
}

//! Requirement-chain lines at level 3 and deeper: `capture-complete`,
//! `capture-parse`, `capture-rewrite`, and `bob capture` read an admitted
//! deep line exactly like a level-2 line, and refused deep lines stay
//! inert.

use crate::support::*;
use serde_json::Value;
use std::path::Path;

const NOW: &str = "2026-11-22 09:00:00";

const LEVEL_2: &str = "@work T\n- [ ] r1\n  - [ ] r2 ";
const LEVEL_3: &str = "@work T\n- [ ] r1\n  - [ ] r2\n    - [ ] r3 ";

fn vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("work.md"),
        concat!(
            "# Work\n",
            "- [ ] #task Existing task ^ex\n",
            "- [ ] #task Other work ^wo\n",
        ),
    );
    vault
}

fn json(output: std::process::Output) -> Value {
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(&output))
    })
}

fn complete(vault: &Path, text: &str) -> Value {
    json(
        bob_command()
            .arg("capture-complete")
            .arg("-b")
            .arg(vault)
            .arg("-c")
            .arg(text.len().to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .env("BOB_NOW", NOW)
            .output()
            .expect("run capture-complete"),
    )
}

fn parse(text: &str) -> Value {
    json(
        bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-parse"),
    )
}

fn rewrite(text: &str) -> Value {
    json(
        bob_command()
            .arg("capture-rewrite")
            .arg("-c")
            .arg(text.len().to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-rewrite"),
    )
}

fn dry_run(vault: &Path, text: &str) -> Value {
    serde_json::from_str(
        stdout(
            &bob_command()
                .arg("capture")
                .arg("-b")
                .arg(vault)
                .arg("--dry-run")
                .arg("-f")
                .arg("json")
                .arg("--")
                .arg(text)
                .env("BOB_NOW", NOW)
                .output()
                .expect("run capture --dry-run"),
        )
        .trim(),
    )
    .expect("capture JSON")
}

fn candidate_ids(value: &Value) -> Vec<String> {
    value["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|candidate| candidate["block_id"].as_str())
        .map(str::to_string)
        .collect()
}

#[test]
fn deep_requirement_lines_open_the_same_pickers_as_level_two() {
    let temp = TempDir::new("bob-cli-capture-reqs-lexical-complete");
    let vault = vault(&temp);
    for (token, context) in [
        ("&wo", "task_dependency"),
        ("*wo", "blocked_task"),
        ("+Exi", "task_parent"),
        ("@wo", "route"),
        ("~wo", "relation_to"),
    ] {
        let level_2 = complete(&vault, &format!("{LEVEL_2}{token}"));
        assert_eq!(level_2["context"], context, "{level_2}");
        let raw = format!("{LEVEL_3}{token}");
        let level_3 = complete(&vault, &raw);
        assert_eq!(level_3["context"], context, "{raw:?}: {level_3}");
        let base = (raw.len() - token.len()) as u64;
        let shift = base - (LEVEL_2.len() as u64);
        assert_eq!(
            level_3["replacement"]["start"].as_u64(),
            level_2["replacement"]["start"]
                .as_u64()
                .map(|start| start + shift),
            "{raw:?}"
        );
        assert_eq!(candidate_ids(&level_3), candidate_ids(&level_2), "{raw:?}");
    }
    let dependency = complete(&vault, &format!("{LEVEL_3}&wo"));
    assert!(
        candidate_ids(&dependency).contains(&"wo".to_string()),
        "{dependency}"
    );
}

#[test]
fn parse_needs_agree_with_completion_on_deep_requirement_lines() {
    let value = parse(&format!("{LEVEL_3}+Exi"));
    assert_eq!(
        value["needs"],
        serde_json::json!(["task_parent"]),
        "{value}"
    );
    let kinds: Vec<&str> = value["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .filter_map(|span| span["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"interactive_placeholder"), "{value}");

    let value = parse(&format!("{LEVEL_3}&wo"));
    assert_eq!(
        value["needs"],
        serde_json::json!(["task_dependency"]),
        "{value}"
    );
}

#[test]
fn deep_requirement_lines_take_the_typing_aliases() {
    for (typed, expected) in [(",", "+"), ("@work .", "@work &")] {
        let raw = format!("{LEVEL_3}{typed}");
        let value = rewrite(&raw);
        assert_eq!(value["rule"], "expand_sigil_alias", "{value}");
        assert_eq!(value["text"], format!("{LEVEL_3}{expected}"), "{value}");
    }
    let value = rewrite(&format!("{LEVEL_3}@work+ex^"));
    assert_eq!(value["rule"], "switch_block_id_separator", "{value}");
    assert_eq!(value["text"], format!("{LEVEL_3}@work^ex"), "{value}");
}

#[test]
fn a_deep_requirement_prerequisite_writes_like_level_two() {
    let temp = TempDir::new("bob-cli-capture-reqs-lexical-dependency");
    let vault = vault(&temp);
    let level_2 = dry_run(&vault, &format!("{LEVEL_2}&work:wo"));
    let level_3 = dry_run(&vault, &format!("{LEVEL_3}&work:wo"));
    assert_eq!(level_2["ok"], true, "{level_2}");
    assert_eq!(level_3["ok"], true, "{level_3}");
    assert_eq!(level_3["task_line"], level_2["task_line"], "{level_3}");
    assert!(
        level_3["task_line"]
            .as_str()
            .is_some_and(|line| line.contains("[dependsOn:: work__wo]")),
        "{level_3}"
    );
    assert_eq!(
        level_3["sub_bullets"],
        serde_json::json!(["\t- [ ] r1", "\t\t- [ ] r2", "\t\t\t- [ ] r3"]),
        "{level_3}"
    );
}

#[test]
fn refused_deep_lines_stay_inert() {
    let temp = TempDir::new("bob-cli-capture-reqs-lexical-refused");
    let vault = vault(&temp);
    for raw in [
        "Task\n- note\n    - deep +abc",
        "Task\n- note\n    - deep &wo",
        "Task\n- [ ] r1\n      - jump @wo",
    ] {
        let value = complete(&vault, raw);
        assert_eq!(value["context"], Value::Null, "{raw:?}: {value}");
    }
    // `+abc` beneath a plain note keeps today's refusal: no picker and no
    // task-selection error.
    let raw = "Task\n- note\n    - deep +abc";
    let value = parse(raw);
    assert_eq!(value["needs"], serde_json::json!([]), "{value}");
    let codes: Vec<&str> = value["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect();
    assert_eq!(codes, vec!["invalid_child_line"], "{value}");
    let dry = dry_run(&vault, raw);
    assert_eq!(dry["ok"], false, "{dry}");
    assert!(
        dry["error"]
            .as_str()
            .is_some_and(|error| error.contains("capture line 3")),
        "{dry}"
    );
    for raw in [
        "Task\n- note\n    - deep ,",
        "Task\n- note\n    - deep @work .",
    ] {
        let value = rewrite(raw);
        assert_eq!(value["changed"], false, "{value}");
    }
}

#[test]
fn a_deep_marker_only_row_in_a_notes_list_is_still_refused() {
    let temp = TempDir::new("bob-cli-capture-reqs-lexical-notes-list");
    let vault = vault(&temp);
    let dry = dry_run(&vault, "@work#Tasks\n- [ ] a\n  - [ ] b\n    - #fix");
    assert_eq!(dry["ok"], false, "{dry}");
    assert_eq!(dry["code"], "invalid_child_line", "{dry}");
}

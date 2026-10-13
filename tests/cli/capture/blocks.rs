//! Capture `&` and dependency drops keep the BLOCKS mirror current
//! (BW13–BW16, `docs/task-dependencies.md` §14.7/§14.11).
//!
//! - `&` stages each added prerequisite's `🧱 **BLOCKS:**` entry (and
//!   the dependent's minted `^id`) in the same batch, reported
//!   additively as `blocks_line_updates` / `minted_dependent_block_id`;
//!   the prerequisite rides as a `dependency_target` task block.
//! - Dependency drops stage the dependent's removal the same way.
//! - `dependencies.blocks_lines: false` writes nothing; `--dry-run`
//!   reports rows without touching the vault.

use crate::support::*;
use std::fs;

fn write_vault(vault: &std::path::Path) {
    write_blocked_tasks_settings(vault);
    write_file(&vault.join("cash.md"), "- [ ] #task Seed work ^seed\n");
    write_file(
        &vault.join("day.md"),
        "## Pomodoros\n- [ ] (1330-1400 [t:: 30m]) Work\n",
    );
}

fn capture_json(
    vault: &std::path::Path,
    day: &std::path::Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> serde_json::Value {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    for arg in args {
        command.arg(arg);
    }
    command
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", "2026-07-10 13:40:00");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("run bob capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

// BW13: a new task with an explicit `^id` stages its prerequisite's
// BLOCKS entry in the same batch and reports the row; the
// prerequisite rides as a `dependency_target` task block.
#[test]
fn capture_new_task_with_explicit_id_stages_blocks_row() {
    let temp = TempDir::new("bob-cli-capture-blocks-new-explicit");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_vault(&vault);

    let json = capture_json(
        &vault,
        &day,
        &["Water fern", "@dev:fern", "&cash:seed"],
        &[],
    );
    assert_eq!(json["blocks_line_updates"].as_array().unwrap().len(), 1);
    assert_eq!(json["blocks_line_updates"][0]["path"], "cash.md");
    assert_eq!(json["blocks_line_updates"][0]["kind"], "inserted");
    assert_eq!(
        json["blocks_line_updates"][0]["added"],
        serde_json::json!(["[[dev#^fern]]"]),
        "{json}",
    );
    assert!(json.get("minted_dependent_block_id").is_none(), "{json}");
    let targets: Vec<(&str, Vec<&str>)> = json["task_blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| {
            (
                block["relative_target"].as_str().unwrap(),
                block["roles"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|role| role.as_str().unwrap())
                    .collect(),
            )
        })
        .collect();
    assert!(
        targets.contains(&("cash.md", vec!["dependency_target"])),
        "{json}",
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read prereq"),
        "- [*] #task Seed work [id:: cash__seed] ^seed\n\t- 🧱 **BLOCKS:** [[dev#^fern]]\n",
    );
    assert!(fs::read_to_string(vault.join("dev.md"))
        .expect("read dependent")
        .contains("- ⛓️ **DEPENDS ON:** [[cash#^seed]]\n"),);
}

// BW13: a new task with no `^id` is minted one (SB pipeline, no
// `[id::]`) and the prerequisite lists the minted id; the mint is
// reported with the dependent's inserted line.
#[test]
fn capture_new_task_without_id_mints_dependent_block_id() {
    let temp = TempDir::new("bob-cli-capture-blocks-new-mint");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_vault(&vault);

    let json = capture_json(&vault, &day, &["Plain chore &cash:seed"], &[]);
    let task_line = json["task_line"].as_str().unwrap_or_default();
    assert!(task_line.ends_with("^plain-chore"), "{json}");
    assert!(!task_line.contains("[id::"), "{json}");
    assert_eq!(
        json["minted_dependent_block_id"]["block_id"], "plain-chore",
        "{json}",
    );
    assert_eq!(json["minted_dependent_block_id"]["path"], "mac_inbox.md");
    assert_eq!(json["minted_dependent_block_id"]["line"], 1);
    assert_eq!(
        json["blocks_line_updates"][0]["added"],
        serde_json::json!(["[[mac_inbox#^plain-chore]]"]),
        "{json}",
    );
    assert_eq!(
        json["dependency_update"]["dependent_block_id"], "plain-chore",
        "{json}",
    );
    let inbox = fs::read_to_string(vault.join("mac_inbox.md"))
        .expect("read new dependent note");
    assert!(inbox.contains("^plain-chore\n"), "{inbox}");
    assert!(fs::read_to_string(vault.join("cash.md"))
        .expect("read prereq")
        .contains("🧱 **BLOCKS:** [[mac_inbox#^plain-chore]]\n"),);
}

// BW13: adding prerequisites to an explicitly selected existing task
// stages the mirror in the same batch.
#[test]
fn capture_dependency_only_add_stages_blocks_row() {
    let temp = TempDir::new("bob-cli-capture-blocks-existing");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_vault(&vault);
    write_file(&vault.join("dev.md"), "- [ ] #task Plain chore ^chore\n");

    let json = capture_json(&vault, &day, &["@dev:chore &cash:seed"], &[]);
    assert_eq!(json["kind"], "task_dependency", "{json}");
    assert_eq!(json["blocks_line_updates"][0]["kind"], "inserted");
    assert_eq!(
        json["blocks_line_updates"][0]["added"],
        serde_json::json!(["[[dev#^chore]]"]),
        "{json}",
    );
    assert!(json.get("minted_dependent_block_id").is_none(), "{json}");
    assert!(fs::read_to_string(vault.join("cash.md"))
        .expect("read prereq")
        .contains("🧱 **BLOCKS:** [[dev#^chore]]\n"),);
}

// BW14: a capture dependency drop removes the dependent from each
// dropped prerequisite's line in the same batch, deleting the line
// when it empties. Prerequisite task lines are never edited.
#[test]
fn capture_dependency_drop_removes_blocks_entry() {
    let temp = TempDir::new("bob-cli-capture-blocks-drop");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_vault(&vault);
    write_file(
        &vault.join("dev.md"),
        concat!(
            "- [?] #task Blocked chore [id:: dev__chore] [dependsOn:: cash__seed] ^chore\n",
            "\t- ⛓️ **DEPENDS ON:** [[cash#^seed]]\n",
        ),
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Seed work [id:: cash__seed] ^seed\n",
            "\t- 🧱 **BLOCKS:** [[dev#^chore]]\n",
        ),
    );

    let json = capture_json(&vault, &day, &["@dev:chore"], &[]);
    assert_eq!(
        json["dropped_dependencies"].as_array().unwrap().len(),
        1,
        "{json}",
    );
    assert_eq!(json["blocks_line_updates"][0]["kind"], "removed");
    assert_eq!(
        json["blocks_line_updates"][0]["removed"],
        serde_json::json!(["[[dev#^chore]]"]),
        "{json}",
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read prereq"),
        "- [ ] #task Seed work [id:: cash__seed] ^seed\n",
    );
}

// BW15: with `dependencies.blocks_lines: false`, `&` writes no BLOCKS
// edits and mints nothing; the additive keys stay omitted.
#[test]
fn capture_blocks_kill_switch_writes_nothing() {
    let temp = TempDir::new("bob-cli-capture-blocks-kill-switch");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_vault(&vault);
    let config = temp.path().join("config.yml");
    write_file(&config, "dependencies:\n  blocks_lines: false\n");

    let json = capture_json(
        &vault,
        &day,
        &["Plain chore &cash:seed"],
        &[("BOB_CONFIG_FILE", config.to_str().unwrap())],
    );
    assert!(json.get("blocks_line_updates").is_none(), "{json}");
    assert!(json.get("minted_dependent_block_id").is_none(), "{json}");
    assert!(!json["task_line"].as_str().unwrap_or_default().contains('^'));
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read prereq"),
        "- [ ] #task Seed work [id:: cash__seed] ^seed\n",
    );
}

// BW16: `--dry-run` reports the BLOCKS rows but writes nothing.
#[test]
fn capture_blocks_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-capture-blocks-dry-run");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_vault(&vault);

    let json = capture_json(
        &vault,
        &day,
        &["--dry-run", "Dry fern", "@dev:fern", "&cash:seed"],
        &[],
    );
    assert_eq!(json["dry_run"], true);
    assert_eq!(
        json["blocks_line_updates"][0]["added"],
        serde_json::json!(["[[dev#^fern]]"]),
        "{json}",
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read prereq"),
        "- [ ] #task Seed work ^seed\n",
    );
    assert!(!vault.join("dev.md").exists(), "dry run writes nothing");
}

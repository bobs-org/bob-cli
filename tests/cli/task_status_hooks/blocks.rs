//! BLOCKS projection pass on `bob task reconcile` (BR1–BR29,
//! `docs/task-dependencies.md` §14.6).

use super::dependency_lines::{
    assert_reconciled_clean, dry_run_json, warning_kinds, write_daily,
};
use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Output;

fn apply_reconcile(vault: &Path, daily: &Path) -> Output {
    let output = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .output()
        .expect("apply bob task reconcile");
    assert_success(&output);
    output
}

fn apply_json(vault: &Path, daily: &Path) -> Value {
    let output = bob_command()
        .args(["task", "reconcile", "--format", "json"])
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .output()
        .expect("apply json bob task reconcile");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("apply JSON")
}

fn dry_run_with_config(
    vault: &Path,
    daily: &Path,
    config: &Path,
) -> (Value, String) {
    let output = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_CONFIG_FILE", config)
        .output()
        .expect("dry-run with config");
    assert_success(&output);
    let json =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    (json, stderr(&output))
}

fn apply_with_config(vault: &Path, daily: &Path, config: &Path) -> Output {
    let output = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_CONFIG_FILE", config)
        .output()
        .expect("apply with config");
    assert_success(&output);
    output
}

fn touch_mtime(path: &Path, spec: &str) {
    let status = std::process::Command::new("touch")
        .arg("-d")
        .arg(spec)
        .arg(path)
        .status()
        .expect("touch mtime");
    assert!(status.success(), "touch {spec} {}", path.display());
}

fn blocks_updates(json: &Value) -> &[Value] {
    json["blocks_line_updates"].as_array().unwrap()
}

fn minted_ids(json: &Value) -> &[Value] {
    json["minted_dependent_block_ids"].as_array().unwrap()
}

fn has_blocks_entry(contents: &str, entry: &str) -> bool {
    contents.contains(&format!("  - 🧱 **BLOCKS:** {entry}\n"))
        || contents.contains(&format!("\t- 🧱 **BLOCKS:** {entry}\n"))
}

fn strip_blocks_lines(contents: &str) -> String {
    let kept: Vec<&str> = contents
        .lines()
        .filter(|line| {
            let body = line.trim_start();
            !body.contains("**BLOCKS:**") && !body.contains("**BLOCKING:**")
        })
        .collect();
    let mut out = kept.join("\n");
    if contents.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[test]
fn br1_same_note_insert() {
    let temp = TempDir::new("bob-cli-blocks-br1");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let updates = blocks_updates(&json);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["path"], "tasks.md");
    assert!(
        updates[0]["task"]
            .as_str()
            .unwrap()
            .starts_with("Prerequisite"),
        "{}",
        updates[0]["task"]
    );
    assert_eq!(updates[0]["kind"], "inserted");
    assert_eq!(updates[0]["added"], serde_json::json!(["[[#^d]]"]));
    assert!(updates[0]["removed"].as_array().unwrap().is_empty());
    assert!(minted_ids(&json).is_empty());
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
        )
    );

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        after.contains("- [?] #task Dependent [dependsOn:: tasks__t] ^d\n"),
        "{after}"
    );
    assert!(has_blocks_entry(&after, "[[#^d]]"), "{after}");
    assert_reconciled_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn br2_unique_and_ambiguous_basename_links() {
    let temp = TempDir::new("bob-cli-blocks-br2");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Cash dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("money/cash.md"),
        concat!(
            "- [ ] #task Money cash ^m\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^u]]\n",
        ),
    );
    write_file(&vault.join("other/cash.md"), "- [ ] #task Other cash ^o\n");
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Unique owner ^t\n",
            "- [ ] #task Ambiguous owner ^u\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let added: Vec<String> = blocks_updates(&json)
        .iter()
        .flat_map(|item| item["added"].as_array().unwrap())
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert!(added.contains(&"[[cash#^d]]".to_string()), "{added:?}");
    assert!(
        added.contains(&"[[money/cash#^m]]".to_string()),
        "{added:?}"
    );

    apply_reconcile(&vault, &daily);
    let tasks = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_blocks_entry(&tasks, "[[cash#^d]]"), "{tasks}");
    assert!(has_blocks_entry(&tasks, "[[money/cash#^m]]"), "{tasks}");
    assert_reconciled_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn br3_one_line_sorted_across_notes() {
    let temp = TempDir::new("bob-cli-blocks-br3");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("zeta.md"),
        concat!(
            "- [ ] #task Zeta ^e\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Cash ^c\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Same-note ^b\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Owner ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let tasks = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(
        has_blocks_entry(&tasks, "[[#^b]] • [[cash#^c]] • [[zeta#^e]]"),
        "{tasks}"
    );
    assert_reconciled_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn br4_mints_idless_dependent_without_id_or_fresh_fields() {
    let temp = TempDir::new("bob-cli-blocks-br4");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Call back\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let mints = minted_ids(&json);
    assert_eq!(mints.len(), 1);
    assert_eq!(mints[0]["path"], "tasks.md");
    assert_eq!(mints[0]["line"], 1);
    let minted = mints[0]["block_id"].as_str().unwrap();
    assert_eq!(minted, "call-back");
    let updates = blocks_updates(&json);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["kind"], "inserted");
    assert_eq!(
        updates[0]["added"],
        serde_json::json!([format!("[[#^{minted}]]")])
    );

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        after.contains(
            "- [?] #task Call back [dependsOn:: tasks__t] ^call-back\n"
        ),
        "{after}"
    );
    assert!(has_blocks_entry(&after, "[[#^call-back]]"), "{after}");
    let dependent = after
        .lines()
        .find(|line| line.contains("#task Call back"))
        .unwrap();
    assert!(!dependent.contains("[id::"), "{dependent}");
    assert!(!dependent.contains("[fresh::"), "{dependent}");
    assert_reconciled_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn br5_quiet_interval_skips_mint_until_next_run() {
    let temp = TempDir::new("bob-cli-blocks-br5");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let dep = vault.join("dep.md");
    write_file(
        &dep,
        concat!(
            "- [ ] #task Call back\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Prerequisite ^t\n");
    write_blocked_tasks_settings(&vault);
    touch_mtime(&dep, "+1 hour");

    let json = dry_run_json(&vault, &daily);
    assert!(minted_ids(&json).is_empty(), "{}", json);
    assert!(blocks_updates(&json).is_empty(), "{}", json);
    assert!(!fs::read_to_string(&dep).unwrap().contains(" ^"));

    touch_mtime(&dep, "-1 hour");
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&dep).unwrap();
    assert!(after.contains(" ^call-back\n"), "{after}");
    let tasks = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_blocks_entry(&tasks, "[[dep#^call-back]]"), "{tasks}");
    assert_reconciled_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn br6_inserts_after_depends_on_subtree() {
    let temp = TempDir::new("bob-cli-blocks-br6");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Owner ^t\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "    - nested note\n",
            "  - 🗓️ **SCHEDULE LOG**\n",
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Pre ^pre\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        after.contains(concat!(
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "    - nested note\n",
            "  - 🧱 **BLOCKS:** [[#^d]]\n",
            "  - 🗓️ **SCHEDULE LOG**\n",
        )),
        "{after}"
    );
    assert!(has_blocks_entry(&after, "[[#^t]]"), "{after}");
}

#[test]
fn br7_inserts_second_after_cancel_log() {
    let temp = TempDir::new("bob-cli-blocks-br7");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Owner ^t\n",
            "  - ❌ **CANCEL LOG**\n",
            "  - prose child\n",
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        after.contains(concat!(
            "- [ ] #task Owner [id:: tasks__t] ^t\n",
            "  - ❌ **CANCEL LOG**\n",
            "  - 🧱 **BLOCKS:** [[#^d]]\n",
            "  - prose child\n",
        )),
        "{after}"
    );
}

#[test]
fn br8_prj_order_then_projects_sync_is_noop() {
    let temp = TempDir::new("bob-cli-blocks-br8");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Parent #hide ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
            "\t- 🧩 **Sub-projects:** [[child]]\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "parent: \"[[parent]]\"\n",
            "---\n",
            "- [ ] #task #prj Child #hide ^prj\n",
            "- [ ] #task Child resident ^res\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Prerequisite ^pre\n",
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[parent#^prj]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let parent = fs::read_to_string(vault.join("parent.md")).unwrap();
    let depends = parent.find("**DEPENDS ON:**").expect("depends");
    let blocks = parent.find("**BLOCKS:**").expect("blocks");
    let subs = parent.find("**Sub-projects:**").expect("subs");
    assert!(depends < blocks && blocks < subs, "{parent}");
    assert!(has_blocks_entry(&parent, "[[tasks#^d]]"), "{parent}");

    let before_parent = fs::read_to_string(vault.join("parent.md")).unwrap();
    let before_child = fs::read_to_string(vault.join("child.md")).unwrap();
    let output = bob_command()
        .args(["projects", "sync", "--dry-run", "--bob-dir"])
        .arg(&vault)
        .output()
        .expect("projects sync dry-run");
    assert_success(&output);
    assert!(!stdout(&output).contains("would "), "{}", stdout(&output));
    assert_eq!(
        fs::read_to_string(vault.join("parent.md")).unwrap(),
        before_parent
    );
    assert_eq!(
        fs::read_to_string(vault.join("child.md")).unwrap(),
        before_child
    );
}

#[test]
fn br9_last_dependent_close_removes_line() {
    let temp = TempDir::new("bob-cli-blocks-br9");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_reconcile(&vault, &daily);
    let opened = fs::read_to_string(&tasks).unwrap();
    write_file(
        &tasks,
        &opened.replacen("- [?] #task Dependent", "- [x] #task Dependent", 1),
    );

    let json = dry_run_json(&vault, &daily);
    let updates = blocks_updates(&json);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["kind"], "removed");
    assert_eq!(updates[0]["removed"], serde_json::json!(["[[#^d]]"]));

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(!after.contains("**BLOCKS:**"), "{after}");
    assert_reconciled_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn br10_closed_owner_is_frozen() {
    let temp = TempDir::new("bob-cli-blocks-br10");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Dependent ^d\n",
        "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
        "- [x] #task Prerequisite ^t\n",
        "  - 🧱 **BLOCKS:** [[#^d]] • [[#^stale]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(blocks_updates(&json).is_empty(), "{}", json);
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        after.contains("  - 🧱 **BLOCKS:** [[#^d]] • [[#^stale]]\n"),
        "{after}"
    );
    assert!(after.contains("- [x] #task Prerequisite"), "{after}");
}

#[test]
fn br11_reopened_owner_is_reprojected() {
    let temp = TempDir::new("bob-cli-blocks-br11");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [x] #task Prerequisite ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_reconcile(&vault, &daily);
    let closed = fs::read_to_string(&tasks).unwrap();
    assert!(!closed.contains("**BLOCKS:**"), "{closed}");
    write_file(
        &tasks,
        &closed.replacen(
            "- [x] #task Prerequisite",
            "- [ ] #task Prerequisite",
            1,
        ),
    );

    let json = dry_run_json(&vault, &daily);
    assert_eq!(blocks_updates(&json)[0]["kind"], "inserted");
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(has_blocks_entry(&after, "[[#^d]]"), "{after}");
}

#[test]
fn br12_previous_daily_is_never_written_or_listed() {
    let temp = TempDir::new("bob-cli-blocks-br12");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let previous = vault.join("2026/20260715.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &previous,
        concat!(
            "- [ ] #task Old owner ^old\n",
            "- [ ] #task Old dependent ^od\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Live owner ^t\n",
            "- [ ] #task Live dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[20260715#^old]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    assert_eq!(
        fs::read_to_string(&previous).unwrap(),
        concat!(
            "- [ ] #task Old owner ^old\n",
            "- [ ] #task Old dependent ^od\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        )
    );
    let tasks = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(!tasks.contains("**BLOCKS:**"), "{tasks}");
    assert!(!tasks.contains("^od"), "{tasks}");
}

#[test]
fn br13_hand_added_entry_without_edge_is_removed() {
    let temp = TempDir::new("bob-cli-blocks-br13");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
            "  - 🧱 **BLOCKS:** [[#^d]] • [[#^ghost]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let updates = blocks_updates(&json);
    assert_eq!(updates[0]["kind"], "rewritten");
    assert_eq!(updates[0]["removed"], serde_json::json!(["[[#^ghost]]"]));

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(after.contains("  - 🧱 **BLOCKS:** [[#^d]]\n"), "{after}");
    assert!(!after.contains("ghost"), "{after}");
}

#[test]
fn br14_canonicalizes_blocking_label() {
    let temp = TempDir::new("bob-cli-blocks-br14");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
            "  - 🧱 **BLOCKING:** [[#^d]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(blocks_updates(&json)[0]["kind"], "canonicalized");
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(after.contains("  - 🧱 **BLOCKS:** [[#^d]]\n"), "{after}");
    assert!(!after.contains("**BLOCKING:**"), "{after}");
}

#[test]
fn br15_deduplicates_and_deletes_orphans() {
    let temp = TempDir::new("bob-cli-blocks-br15");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
            "  - 🧱 **BLOCKS:** [[#^d]] • [[#^extra]]\n",
            "  - 🧱 **BLOCKS:** [[#^d]]\n",
            "- [ ] #task Orphan ^orphan\n",
            "  - 🧱 **BLOCKS:** [[#^nobody]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let kinds: Vec<_> = blocks_updates(&json)
        .iter()
        .map(|item| item["kind"].as_str().unwrap().to_string())
        .collect();
    assert!(kinds.contains(&"deduplicated".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"removed".to_string()), "{kinds:?}");

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert_eq!(after.matches("**BLOCKS:**").count(), 1, "{after}");
    assert!(after.contains("  - 🧱 **BLOCKS:** [[#^d]]\n"), "{after}");
    assert!(!after.contains("Orphan ^orphan\n  - 🧱"), "{after}");
}

#[test]
fn br16_malformed_blocks_line_is_left_alone() {
    let temp = TempDir::new("bob-cli-blocks-br16");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Dependent ^d\n",
        "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
        "- [ ] #task Prerequisite ^t\n",
        "  - 🧱 **BLOCKS:** [[#^d]] until Friday\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(warning_kinds(&json), ["malformed_blocks_line"]);
    assert!(blocks_updates(&json).is_empty());
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(after.contains("until Friday"), "{after}");
    assert_eq!(after.matches("**BLOCKS:**").count(), 1, "{after}");
}

#[test]
fn br17_malformed_dependent_is_kept_not_newly_added() {
    let temp = TempDir::new("bob-cli-blocks-br17");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Messy ^messy\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]] leftover\n",
            "- [ ] #task Prerequisite ^pre\n",
            "  - 🧱 **BLOCKS:** [[#^messy]]\n",
            "- [ ] #task Other ^other\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(warning_kinds(&json).contains(&"malformed_dependency_line".into()));
    assert!(blocks_updates(&json).is_empty(), "{}", json);
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(after.contains("- [ ] #task Prerequisite"), "{after}");
    assert!(
        after.contains("  - 🧱 **BLOCKS:** [[#^messy]]\n"),
        "{after}"
    );
    assert!(!after.contains("Other ^other\n  - 🧱"), "{after}");
}

#[test]
fn br18_unresolved_self_and_non_task_are_not_listed() {
    let temp = TempDir::new("bob-cli-blocks-br18");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^missing]] • [[#^d]] • [[#^ref]]\n",
            "- [ ] #task Live ^t\n",
            "\n",
            "Heading only. ^ref\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(!after.contains("**BLOCKS:**"), "{after}");
}

#[test]
fn br19_prj_lists_only_the_project_task() {
    let temp = TempDir::new("bob-cli-blocks-br19");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Project ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
            "- [ ] #task Resident ^res\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Prerequisite ^t\n");
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let tasks = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_blocks_entry(&tasks, "[[proj#^prj]]"), "{tasks}");
    assert!(!tasks.contains("^res"), "{tasks}");
}

#[test]
fn br20_internal_prj_target_is_not_listed() {
    let temp = TempDir::new("bob-cli-blocks-br20");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Project ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[#^internal]]\n",
            "- [ ] #task Internal ^internal\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let proj = fs::read_to_string(vault.join("proj.md")).unwrap();
    assert!(!proj.contains("**BLOCKS:**"), "{proj}");
}

#[test]
fn br21_legacy_child_edge_is_listed() {
    let temp = TempDir::new("bob-cli-blocks-br21");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent [dependsOn:: tasks__t] ^d\n",
            "  - [[#^t]]\n",
            "- [ ] #task Target [id:: tasks__t] ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(has_blocks_entry(&after, "[[#^d]]"), "{after}");
}

#[test]
fn br22_cycle_each_lists_the_other() {
    let temp = TempDir::new("bob-cli-blocks-br22");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Alpha [dependsOn:: tasks__b] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
            "- [ ] #task Beta [dependsOn:: tasks__a] ^b\n",
            "  - ⛓️ **DEPENDS ON:** [[#^a]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        after.contains("- [?] #task Alpha")
            && after.contains("  - 🧱 **BLOCKS:** [[#^b]]\n"),
        "{after}"
    );
    assert!(
        after.contains("- [?] #task Beta")
            && after.contains("  - 🧱 **BLOCKS:** [[#^a]]\n"),
        "{after}"
    );
}

#[test]
fn br23_hide_ref_inbox_and_daily_are_listed() {
    let temp = TempDir::new("bob-cli-blocks-br23");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_file(
        &daily,
        concat!(
            "- [ ] #task Daily dep ^daily\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
            "\n",
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
        ),
    );
    write_file(
        &vault.join("inbox.md"),
        concat!(
            "- [ ] #task Inbox dep ^in\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("ref/note.md"),
        concat!(
            "- [ ] #task Ref dep ^rf\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task #hide Hidden dep ^h\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Owner ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let tasks = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(
        has_blocks_entry(
            &tasks,
            "[[#^h]] • [[20260716#^daily]] • [[inbox#^in]] • [[note#^rf]]"
        ),
        "{tasks}"
    );
}

#[test]
fn br24_kill_switch_removes_open_lines_and_skips_mints() {
    let temp = TempDir::new("bob-cli-blocks-br24");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_file(&config, "dependencies:\n  blocks_lines: false\n");
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Call back\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
            "  - 🧱 **BLOCKS:** [[#^ghost]]\n",
            "- [x] #task Closed ^c\n",
            "  - 🧱 **BLOCKS:** [[#^ghost]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let (json, _) = dry_run_with_config(&vault, &daily, &config);
    assert!(minted_ids(&json).is_empty(), "{}", json);
    assert_eq!(blocks_updates(&json)[0]["kind"], "removed");

    apply_with_config(&vault, &daily, &config);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(!after.contains("Prerequisite ^t\n  - 🧱"), "{after}");
    assert!(
        after.contains(
            "- [x] #task Closed ^c\n  - 🧱 **BLOCKS:** [[#^ghost]]\n"
        ),
        "{after}"
    );
    assert!(!after.contains(" ^call-back"), "{after}");
}

#[test]
fn br25_invalid_kill_switch_behaves_as_true() {
    let temp = TempDir::new("bob-cli-blocks-br25");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_file(&config, "dependencies:\n  blocks_lines: 1\n");
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let (json, err) = dry_run_with_config(&vault, &daily, &config);
    assert!(
        err.contains("invalid dependencies config")
            && err.contains("using dependencies.blocks_lines=true"),
        "{err}"
    );
    assert_eq!(blocks_updates(&json)[0]["kind"], "inserted");
}

#[test]
fn br26_rebuilds_identical_bytes_after_delete() {
    let temp = TempDir::new("bob-cli-blocks-br26");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
            "- [ ] #task Other ^o\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_reconcile(&vault, &daily);
    let snapshot = fs::read_to_string(vault.join("tasks.md")).unwrap();
    write_file(&vault.join("tasks.md"), &strip_blocks_lines(&snapshot));
    apply_reconcile(&vault, &daily);
    assert_eq!(
        fs::read_to_string(vault.join("tasks.md")).unwrap(),
        snapshot
    );
}

#[test]
fn br27_second_run_is_idempotent() {
    let temp = TempDir::new("bob-cli-blocks-br27");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Dependent ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "- [ ] #task Prerequisite ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_reconcile(&vault, &daily);
    let snapshot = fs::read_to_string(vault.join("tasks.md")).unwrap();
    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    apply_reconcile(&vault, &daily);
    assert_eq!(
        fs::read_to_string(vault.join("tasks.md")).unwrap(),
        snapshot
    );
}

#[test]
fn br28_owner_future_mtime_defers_structurally() {
    let temp = TempDir::new("bob-cli-blocks-br28");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Dependent ^d\n",
        "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
        "- [ ] #task Prerequisite ^t\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);
    touch_mtime(&tasks, "+1 hour");

    let output = bob_command()
        .args(["task", "reconcile", "--format", "json", "-r", "0"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("deferred BLOCKS write");
    assert_eq!(output.status.code(), Some(1));
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("deferral JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["reason"], "quiet_period");
    assert!(
        json["deferred_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str().unwrap().contains("tasks.md")),
        "{}",
        json["deferred_files"]
    );
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
}

#[test]
fn br29_dry_run_matches_apply_rows_without_writing() {
    let temp = TempDir::new("bob-cli-blocks-br29");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Call back\n",
        "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
        "- [ ] #task Prerequisite ^t\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let dry = dry_run_json(&vault, &daily);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    assert_eq!(minted_ids(&dry).len(), 1);
    assert_eq!(blocks_updates(&dry).len(), 1);

    let applied = apply_json(&vault, &daily);
    assert_eq!(applied["blocks_line_updates"], dry["blocks_line_updates"]);
    assert_eq!(
        applied["minted_dependent_block_ids"],
        dry["minted_dependent_block_ids"]
    );
    assert_ne!(fs::read_to_string(&tasks).unwrap(), before);
}

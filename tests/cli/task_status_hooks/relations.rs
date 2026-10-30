//! LINKED FROM projection pass on `bob task reconcile` (LR1–LR24,
//! `docs/task-relations.md` §7).

use super::dependency_lines::{
    assert_reconciled_clean, dry_run_json, write_daily,
};
use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
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

fn apply_json_with_config(vault: &Path, daily: &Path, config: &Path) -> Value {
    let output = bob_command()
        .args(["task", "reconcile", "--format", "json"])
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_CONFIG_FILE", config)
        .output()
        .expect("apply json with config");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("apply JSON")
}

fn write_mentions_config(path: &Path, enabled: bool) {
    write_file(
        path,
        &format!("relations:\n  linked_from_mentions: {enabled}\n"),
    );
}

fn mention_vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/task_mentions/vectors.json"
    ))
    .expect("mention vectors parse")
}

fn linked_from_updates(json: &Value) -> &[Value] {
    json["linked_from_line_updates"].as_array().unwrap()
}

fn minted_ids(json: &Value) -> &[Value] {
    json["minted_relation_source_block_ids"].as_array().unwrap()
}

fn relation_warning_kinds(json: &Value) -> Vec<String> {
    json["relation_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["kind"].as_str().unwrap().to_string())
        .collect()
}

fn has_linked_from(contents: &str, entry: &str) -> bool {
    contents.contains(&format!("  - 🪢 **LINKED FROM:** {entry}\n"))
        || contents.contains(&format!("\t- 🪢 **LINKED FROM:** {entry}\n"))
}

fn strip_linked_from_lines(contents: &str) -> String {
    let kept: Vec<&str> = contents
        .lines()
        .filter(|line| {
            let body = line.trim_start();
            !body.contains("**LINKED FROM:**")
                && !body.contains("**BACKLINKS:**")
        })
        .collect();
    let mut out = kept.join("\n");
    if contents.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn assert_relations_clean(json: &Value) {
    assert!(
        linked_from_updates(json).is_empty(),
        "unexpected LINKED FROM updates: {}",
        json["linked_from_line_updates"]
    );
    assert!(
        minted_ids(json).is_empty(),
        "unexpected minted relation source ids: {}",
        json["minted_relation_source_block_ids"]
    );
}

#[test]
fn lr1_same_note_insert() {
    // LR1: S → T in the same note; T has no mirror → inserted [[#^s]].
    let temp = TempDir::new("bob-cli-relations-lr1");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let updates = linked_from_updates(&json);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["path"], "tasks.md");
    assert!(
        updates[0]["task"].as_str().unwrap().starts_with("Target"),
        "{}",
        updates[0]["task"]
    );
    assert_eq!(updates[0]["kind"], "inserted");
    assert_eq!(updates[0]["added"], serde_json::json!(["[[#^s]]"]));
    assert!(updates[0]["removed"].as_array().unwrap().is_empty());
    assert!(minted_ids(&json).is_empty());
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        )
    );

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(has_linked_from(&after, "[[#^s]]"), "{after}");
    assert!(after.contains("- 🪢 **LINKS:** [[#^t]]"), "{after}");
    assert_relations_clean(&dry_run_json(&vault, &daily));
    assert_reconciled_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn lr2_labelled_edge() {
    // LR2: labelled edge → [[#^s]] (follow-up of).
    let temp = TempDir::new("bob-cli-relations-lr2");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** follow-up of [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(linked_from_updates(&json)[0]["kind"], "inserted");
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(has_linked_from(&after, "[[#^s]] (follow-up of)"), "{after}");
}

#[test]
fn lr3_sorted_sources_include_done() {
    // LR3: sources in the same note, another note, and done/; done/ as
    // [[done/…#^s]].
    let temp = TempDir::new("bob-cli-relations-lr3");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("done/bob_done.md"),
        concat!(
            "- [x] #task Archived source ^arch\n",
            "  - 🪢 **LINKS:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("other.md"),
        concat!(
            "- [ ] #task Other source ^o\n",
            "  - 🪢 **LINKS:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Same-note source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(
        has_linked_from(
            &after,
            "[[#^s]] • [[done/bob_done#^arch]] • [[other#^o]]"
        ),
        "{after}"
    );
    let archived = fs::read_to_string(vault.join("done/bob_done.md")).unwrap();
    assert!(!archived.contains("LINKED FROM"), "{archived}");
}

#[test]
fn lr4_mints_source_id() {
    // LR4: S has no ^id → minted (no [id::], no [fresh::]); T lists it.
    let temp = TempDir::new("bob-cli-relations-lr4");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Call back\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(minted_ids(&json).len(), 1);
    assert_eq!(minted_ids(&json)[0]["path"], "tasks.md");
    assert_eq!(minted_ids(&json)[0]["block_id"], "call-back");
    assert!(!fs::read_to_string(vault.join("tasks.md"))
        .unwrap()
        .contains(" ^call-back"));

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(
        after.contains("- [ ] #task Call back ^call-back\n"),
        "{after}"
    );
    assert!(!after.contains("[id::]"), "{after}");
    assert!(!after.contains("[fresh::]"), "{after}");
    assert!(has_linked_from(&after, "[[#^call-back]]"), "{after}");
}

#[test]
fn lr5_quiet_interval_skips_mint_until_next_run() {
    // LR5: S needs a mint and S's note is quiet-deferred → not listed
    // this run; the next run mints and lists it.
    let temp = TempDir::new("bob-cli-relations-lr5");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let source = vault.join("source.md");
    write_file(
        &source,
        concat!(
            "- [ ] #task Call back\n",
            "  - 🪢 **LINKS:** [[tasks#^t]]\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Target ^t\n");
    write_blocked_tasks_settings(&vault);
    touch_mtime_offset(&source, 3600);

    let json = dry_run_json(&vault, &daily);
    assert!(minted_ids(&json).is_empty(), "{}", json);
    assert!(linked_from_updates(&json).is_empty(), "{}", json);
    assert!(!fs::read_to_string(&source).unwrap().contains(" ^"));

    touch_mtime_offset(&source, -3600);
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&source).unwrap();
    assert!(after.contains(" ^call-back\n"), "{after}");
    let tasks = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_linked_from(&tasks, "[[source#^call-back]]"), "{tasks}");
    assert_relations_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn lr6_closed_target_still_mirrored() {
    // LR6: T closed in an active note → mirror maintained.
    let temp = TempDir::new("bob-cli-relations-lr6");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [x] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(after.contains("- [x] #task Target ^t\n"), "{after}");
    assert!(has_linked_from(&after, "[[#^s]]"), "{after}");
}

#[test]
fn lr7_done_and_previous_daily_targets_never_written() {
    // LR7: T in done/ or the previous daily note → never written.
    let temp = TempDir::new("bob-cli-relations-lr7");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let previous = vault.join("2026/20260715.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(&previous, "- [ ] #task Previous target ^pt\n");
    write_file(
        &vault.join("done/old_done.md"),
        "- [x] #task Archived target ^at\n",
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[20260715#^pt]] • [[done/old_done#^at]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    assert_eq!(
        fs::read_to_string(&previous).unwrap(),
        "- [ ] #task Previous target ^pt\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("done/old_done.md")).unwrap(),
        "- [x] #task Archived target ^at\n"
    );
}

#[test]
fn lr8_done_and_previous_daily_sources_are_listed() {
    // LR8: S in done/ / in the previous daily note → listed.
    let temp = TempDir::new("bob-cli-relations-lr8");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let previous = vault.join("2026/20260715.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &previous,
        concat!(
            "- [ ] #task Previous source ^ps\n",
            "  - 🪢 **LINKS:** [[tasks#^t]]\n",
        ),
    );
    write_file(
        &vault.join("done/bob_done.md"),
        concat!(
            "- [x] #task Archived source ^as\n",
            "  - 🪢 **LINKS:** [[tasks#^t]]\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Target ^t\n");
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(
        has_linked_from(&after, "[[20260715#^ps]] • [[done/bob_done#^as]]"),
        "{after}"
    );
    assert_eq!(
        fs::read_to_string(&previous).unwrap(),
        concat!(
            "- [ ] #task Previous source ^ps\n",
            "  - 🪢 **LINKS:** [[tasks#^t]]\n",
        )
    );
}

#[test]
fn lr9_hand_added_and_deleted_entries() {
    // LR9: hand-added mirror entry with no edge / hand-deleted entry →
    // removed / restored (rewritten).
    let temp = TempDir::new("bob-cli-relations-lr9");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "  - 🪢 **LINKED FROM:** [[#^ghost]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(linked_from_updates(&json)[0]["kind"], "rewritten");
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(has_linked_from(&after, "[[#^s]]"), "{after}");
    assert!(!after.contains("^ghost"), "{after}");
}

#[test]
fn lr10_noncanonical_line_is_canonicalized() {
    // LR10: renamed note leaves the mirror unsorted → canonicalized.
    let temp = TempDir::new("bob-cli-relations-lr10");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Zeta ^z\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Alpha ^a\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "  - 🔗 **LINKED FROM:** [[#^z]] • [[#^a]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(linked_from_updates(&json)[0]["kind"], "canonicalized");
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_linked_from(&after, "[[#^a]] • [[#^z]]"), "{after}");
}

#[test]
fn lr11_rebuilds_identical_bytes_after_delete() {
    // LR11: delete every LINKED FROM line; one run rebuilds identical
    // bytes; a second run makes zero changes.
    let temp = TempDir::new("bob-cli-relations-lr11");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Other ^o\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_reconcile(&vault, &daily);
    let snapshot = fs::read_to_string(vault.join("tasks.md")).unwrap();
    write_file(&vault.join("tasks.md"), &strip_linked_from_lines(&snapshot));
    apply_reconcile(&vault, &daily);
    assert_eq!(
        fs::read_to_string(vault.join("tasks.md")).unwrap(),
        snapshot
    );
    assert_relations_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn lr12_malformed_linked_from_left_alone() {
    // LR12: malformed LINKED FROM under T → every LINKED FROM child of T
    // untouched; warn.
    let temp = TempDir::new("bob-cli-relations-lr12");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let before = concat!(
        "- [ ] #task Source ^s\n",
        "  - 🪢 **LINKS:** [[#^t]]\n",
        "- [ ] #task Target ^t\n",
        "  - 🪢 **LINKED FROM:** [[#^s]] extra\n",
    );
    write_file(&vault.join("tasks.md"), before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(
        relation_warning_kinds(&json)
            .contains(&"malformed_linked_from_line".to_string()),
        "{}",
        json["relation_warnings"]
    );
    assert!(linked_from_updates(&json).is_empty(), "{}", json);
    apply_reconcile(&vault, &daily);
    assert_eq!(fs::read_to_string(vault.join("tasks.md")).unwrap(), before);
}

#[test]
fn lr13_malformed_links_never_newly_added() {
    // LR13: S's LINKS malformed → existing entries naming S kept; S never
    // newly added; warn.
    let temp = TempDir::new("bob-cli-relations-lr13");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Broken ^b\n",
            "  - 🪢 **LINKS:** [[#^t]] extra\n",
            "- [ ] #task Good ^g\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "  - 🪢 **LINKED FROM:** [[#^b]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(
        relation_warning_kinds(&json)
            .contains(&"malformed_links_line".to_string()),
        "{}",
        json["relation_warnings"]
    );
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_linked_from(&after, "[[#^b]] • [[#^g]]"), "{after}");
}

#[test]
fn lr14_unresolved_non_task_self_warned_and_omitted() {
    // LR14: unresolved / non-task / self → no entry; matching warning;
    // LINKS untouched.
    let temp = TempDir::new("bob-cli-relations-lr14");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let before = concat!(
        "- not a task ^nt\n",
        "- [ ] #task Source ^s\n",
        "  - 🪢 **LINKS:** [[#^gone]] • [[#^nt]] • [[#^s]]\n",
        "- [ ] #task Target ^t\n",
    );
    write_file(&vault.join("tasks.md"), before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let kinds = relation_warning_kinds(&json);
    assert!(
        kinds.contains(&"unresolved_relation_link".to_string()),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&"relation_target_not_task".to_string()),
        "{kinds:?}"
    );
    assert!(kinds.contains(&"self_relation".to_string()), "{kinds:?}");
    assert!(linked_from_updates(&json).is_empty(), "{}", json);
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(
        after.contains("  - 🪢 **LINKS:** [[#^gone]] • [[#^nt]] • [[#^s]]\n"),
        "{after}"
    );
    assert!(!after.contains("LINKED FROM"), "{after}");
}

#[test]
fn lr15_duplicate_linked_from_lines_deduplicated() {
    // LR15: two LINKED FROM lines → first rewritten, the rest deleted.
    let temp = TempDir::new("bob-cli-relations-lr15");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "  - 🪢 **LINKED FROM:** [[#^ghost]]\n",
            "  - 🪢 **LINKED FROM:** [[#^s]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(linked_from_updates(&json)[0]["kind"], "deduplicated");
    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert_eq!(after.matches("LINKED FROM").count(), 1, "{after}");
    assert!(has_linked_from(&after, "[[#^s]]"), "{after}");
}

#[test]
fn lr16_kill_switch_removes_writable_lines_and_skips_mints() {
    // LR16: relations.linked_from_lines: false → every LINKED FROM on
    // writable tasks removed, no mints; then true rebuilds.
    let temp = TempDir::new("bob-cli-relations-lr16");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_file(&config, "relations:\n  linked_from_lines: false\n");
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Call back\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "  - 🪢 **LINKED FROM:** [[#^ghost]]\n",
            "- [x] #task Closed ^c\n",
            "  - 🪢 **LINKED FROM:** [[#^ghost]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let (json, _) = dry_run_with_config(&vault, &daily, &config);
    assert!(minted_ids(&json).is_empty(), "{}", json);
    assert!(!linked_from_updates(&json).is_empty(), "{}", json);
    for update in linked_from_updates(&json) {
        assert_eq!(update["kind"], "removed");
    }

    apply_with_config(&vault, &daily, &config);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(!after.contains("LINKED FROM"), "{after}");
    assert!(after.contains("  - 🪢 **LINKS:** [[#^t]]\n"), "{after}");
    assert!(!after.contains(" ^call-back"), "{after}");

    write_file(&config, "relations:\n  linked_from_lines: true\n");
    apply_with_config(&vault, &daily, &config);
    let rebuilt = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(rebuilt.contains("LINKED FROM"), "{rebuilt}");
}

#[test]
fn lr17_invalid_kill_switch_behaves_as_true() {
    // LR17: invalid relations.linked_from_lines → behaves as true; stderr
    // warning.
    let temp = TempDir::new("bob-cli-relations-lr17");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_file(&config, "relations:\n  linked_from_lines: 1\n");
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let (json, err) = dry_run_with_config(&vault, &daily, &config);
    assert!(
        err.contains("invalid relations config")
            && err.contains("using relations.linked_from_lines=true"),
        "{err}"
    );
    assert_eq!(err.matches("invalid relations config").count(), 1, "{err}");
    assert_eq!(linked_from_updates(&json)[0]["kind"], "inserted");
}

#[test]
fn configured_cancel_label_stays_inert_during_reconcile() {
    let temp = TempDir::new("bob-cli-relations-cancel-inert");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    let tasks = vault.join("tasks.md");
    let expected = concat!(
        "- [ ] #task Source ^source\n",
        "  - 🪢 **LINKS:** duplicate of [[#^target]]\n",
        "- [ ] #task Target ^target\n",
    );
    write_file(
        &config,
        "relations:\n  linked_from_lines: false\n  cancel_labels:\n    - duplicate of\n",
    );
    write_file(&tasks, expected);
    write_blocked_tasks_settings(&vault);

    apply_with_config(&vault, &daily, &config);
    let first = fs::read_to_string(&tasks).expect("read after first reconcile");
    assert_eq!(first, expected);
    assert!(first.starts_with("- [ ] #task Source"), "{first}");

    apply_with_config(&vault, &daily, &config);
    let second =
        fs::read_to_string(&tasks).expect("read after second reconcile");
    assert_eq!(second, first);
}

#[test]
fn lr18_inert_statuses_and_fields() {
    // LR18: relation added / removed / relabelled between runs → only
    // LINKED FROM lines and minted ids differ.
    let temp = TempDir::new("bob-cli-relations-lr18");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Source ^s\n",
        "- [ ] #task Target ^t\n",
        "- [/] #task Pending ^p\n",
        "- [*] #task Next ^n\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);
    apply_reconcile(&vault, &daily);
    let baseline = fs::read_to_string(&tasks).unwrap();

    write_file(
        &tasks,
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** see [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "- [/] #task Pending ^p\n",
            "- [*] #task Next ^n\n",
        ),
    );
    apply_reconcile(&vault, &daily);
    let added = fs::read_to_string(&tasks).unwrap();
    assert!(has_linked_from(&added, "[[#^s]] (see)"), "{added}");
    assert!(added.contains("- [/] #task Pending ^p\n"), "{added}");
    assert!(added.contains("- [*] #task Next ^n\n"), "{added}");
    assert!(!added.contains("[fresh::]"), "{added}");

    write_file(
        &tasks,
        &added.replacen("see [[#^t]]", "follow-up of [[#^t]]", 1),
    );
    apply_reconcile(&vault, &daily);
    let relabelled = fs::read_to_string(&tasks).unwrap();
    assert!(
        has_linked_from(&relabelled, "[[#^s]] (follow-up of)"),
        "{relabelled}"
    );

    write_file(
        &tasks,
        concat!(
            "- [ ] #task Source ^s\n",
            "- [ ] #task Target ^t\n",
            "  - 🪢 **LINKED FROM:** [[#^s]] (follow-up of)\n",
            "- [/] #task Pending ^p\n",
            "- [*] #task Next ^n\n",
        ),
    );
    apply_reconcile(&vault, &daily);
    let removed = fs::read_to_string(&tasks).unwrap();
    assert!(!removed.contains("LINKED FROM"), "{removed}");
    assert_eq!(strip_linked_from_lines(&removed), baseline);
}

#[test]
fn lr19_symmetric_edges() {
    // LR19: A → B and B → A → each lists the other.
    let temp = TempDir::new("bob-cli-relations-lr19");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Alpha ^a\n",
            "  - 🪢 **LINKS:** [[#^b]]\n",
            "- [ ] #task Beta ^b\n",
            "  - 🪢 **LINKS:** [[#^a]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(
        after.contains("- [ ] #task Alpha ^a\n  - 🪢 **LINKS:** [[#^b]]\n  - 🪢 **LINKED FROM:** [[#^b]]\n"),
        "{after}"
    );
    assert!(
        after.contains("- [ ] #task Beta ^b\n  - 🪢 **LINKS:** [[#^a]]\n  - 🪢 **LINKED FROM:** [[#^a]]\n"),
        "{after}"
    );
}

#[test]
fn lr20_dry_run_matches_apply_rows_without_writing() {
    // LR20: --dry-run -f json → identical rows; nothing written; no mints
    // persisted.
    let temp = TempDir::new("bob-cli-relations-lr20");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Call back\n",
        "  - 🪢 **LINKS:** [[#^t]]\n",
        "- [ ] #task Target ^t\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let dry = dry_run_json(&vault, &daily);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    assert!(!minted_ids(&dry).is_empty(), "{dry}");

    let applied = apply_json(&vault, &daily);
    assert_eq!(
        applied["linked_from_line_updates"],
        dry["linked_from_line_updates"]
    );
    assert_eq!(
        applied["minted_relation_source_block_ids"],
        dry["minted_relation_source_block_ids"]
    );
    assert_ne!(fs::read_to_string(&tasks).unwrap(), before);
}

#[test]
fn lr21_owner_future_mtime_defers_structurally() {
    // LR21: owner's note modified less than 2 s ago / future mtime →
    // edits deferred.
    let temp = TempDir::new("bob-cli-relations-lr21");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Source ^s\n",
        "  - 🪢 **LINKS:** [[#^t]]\n",
        "- [ ] #task Target ^t\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);
    touch_mtime_offset(&tasks, 3600);

    let output = bob_command()
        .args(["task", "reconcile", "--format", "json", "-r", "0"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("deferred LINKED FROM write");
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
fn lr22_archived_source_keeps_done_form() {
    // LR22: S archived; T's entry already [[done/…#^s]]; reconcile keeps
    // it unchanged.
    let temp = TempDir::new("bob-cli-relations-lr22");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("done/old_done.md"),
        concat!(
            "- [x] #task Archived source ^s\n",
            "  - 🪢 **LINKS:** [[tasks#^t]]\n",
        ),
    );
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Target ^t\n",
        "  - 🪢 **LINKED FROM:** [[done/old_done#^s]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    assert_relations_clean(&dry_run_json(&vault, &daily));
}

#[test]
fn lr23_duplicate_source_id_not_listed() {
    // LR23: S's note has a duplicate ^s → S not listed; duplicate_block_id
    // warning.
    let temp = TempDir::new("bob-cli-relations-lr23");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Twin ^s\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(
        relation_warning_kinds(&json)
            .contains(&"duplicate_block_id".to_string()),
        "{}",
        json["relation_warnings"]
    );
    assert!(linked_from_updates(&json).is_empty(), "{}", json);
}

#[test]
fn lr24_prj_target_gets_the_mirror() {
    // LR24: target is a ^prj task → the ^prj line gets the mirror like
    // any task.
    let temp = TempDir::new("bob-cli-relations-lr24");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("project.md"),
        concat!(
            "- [ ] #task Project ^prj\n",
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^prj]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    apply_reconcile(&vault, &daily);
    let after = fs::read_to_string(vault.join("project.md")).unwrap();
    assert!(after.contains("- [ ] #task Project ^prj\n"), "{after}");
    assert!(has_linked_from(&after, "[[#^s]]"), "{after}");
}

#[test]
fn lr_human_output_includes_knot_group() {
    let temp = TempDir::new("bob-cli-relations-human");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let output = bob_command()
        .args(["task", "reconcile", "--dry-run"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("human dry-run");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("LINKED FROM"), "{text}");
    assert!(text.contains("🪢") || text.contains("Relations:"), "{text}");
}

fn setup_mention_vector(
    vector: &Value,
) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let id = vector["id"].as_str().unwrap();
    let temp = TempDir::new(&format!("bob-cli-mentions-{id}"));
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    for (path, contents) in vector["notes"].as_object().unwrap() {
        write_file(&vault.join(path), contents.as_str().unwrap());
    }
    let daily = if vector
        .get("previous_daily")
        .and_then(Value::as_str)
        .is_some()
    {
        let daily = vault.join("2026/20261029.md");
        write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
        daily
    } else {
        write_daily(&vault)
    };
    write_blocked_tasks_settings(&vault);
    (temp, vault, daily, config)
}

fn assert_fixture_mirrors(vault: &Path, vector: &Value) {
    let Some(mirrors) = vector.get("mirror").and_then(Value::as_object) else {
        return;
    };
    for (identity, expected) in mirrors {
        let (path, _) = identity.split_once("#^").expect("target identity");
        let after = fs::read_to_string(vault.join(path)).unwrap();
        let expected = expected.as_str().unwrap();
        assert!(
            after.contains(expected),
            "{}: missing {expected:?} in {path}: {after}",
            vector["id"]
        );
    }
}

#[test]
fn mn_fixture_reconcile_mirrors_and_mints() {
    // MN1–MN12, MN21, MN23, MN25 through the reconcile harness.
    for vector in mention_vectors()["vectors"].as_array().unwrap() {
        let id = vector["id"].as_str().unwrap();
        let (_temp, vault, daily, config) = setup_mention_vector(vector);
        let json = apply_json_with_config(&vault, &daily, &config);
        assert_fixture_mirrors(&vault, vector);
        if let Some(mints) = vector.get("mints").and_then(Value::as_array) {
            for expected in mints {
                let path = expected["path"].as_str().unwrap();
                let line = expected["line"].as_u64().unwrap();
                let reason = expected["reason"].as_str().unwrap();
                assert!(
                    minted_ids(&json).iter().any(|row| {
                        row["path"] == path
                            && row["line"] == line
                            && row["reason"] == reason
                    }),
                    "{id}: missing mint {expected} in {}",
                    json["minted_relation_source_block_ids"]
                );
            }
            if !mints.is_empty() {
                assert_eq!(
                    minted_ids(&json).len(),
                    mints.len(),
                    "{id}: {json}"
                );
            }
        }
        if id == "MN9" || id == "MN12" || id == "MN21" {
            for path in vector["notes"].as_object().unwrap().keys() {
                if path.starts_with("done/") {
                    continue;
                }
                let Ok(after) = fs::read_to_string(vault.join(path)) else {
                    continue;
                };
                assert!(
                    !after.contains("(💬 mentions)"),
                    "{id}: unexpected mention in {path}: {after}"
                );
            }
        }
        if id == "MN4" {
            let after = fs::read_to_string(vault.join("edges.md")).unwrap();
            for task in [
                "Depends target ^t",
                "Blocks target ^u",
                "Subproject target ^v",
                "Malformed-links target ^w",
            ] {
                assert!(
                    !after
                        .contains(&format!("{task}\n  - 🪢 **LINKED FROM:**")),
                    "{id}: edge-line target gained a mention: {after}"
                );
            }
        }
        if id == "MN5" || id == "MN6" {
            for path in vector["notes"].as_object().unwrap().keys() {
                let Ok(after) = fs::read_to_string(vault.join(path)) else {
                    continue;
                };
                assert!(
                    !after.contains("(💬 mentions)"),
                    "{id}: unexpected mention in {path}: {after}"
                );
            }
        }
        if id == "MN4" || id == "MN5" || id == "MN6" {
            let json_second = apply_json_with_config(&vault, &daily, &config);
            assert_relations_clean(&json_second);
        }
        if matches!(
            id,
            "MN1" | "MN2" | "MN3" | "MN7" | "MN10" | "MN22" | "MN25"
        ) {
            assert!(
                json["mention_edges"].as_u64().unwrap() >= 1,
                "{id}: mention_edges={}",
                json["mention_edges"]
            );
        }
        if matches!(
            id,
            "MN1" | "MN2" | "MN3" | "MN7" | "MN8" | "MN10" | "MN22" | "MN25"
        ) {
            let second = apply_json_with_config(&vault, &daily, &config);
            assert_relations_clean(&second);
        }
        if id == "MN23" {
            let keep = fs::read_to_string(vault.join("uncertain.md")).unwrap();
            assert!(
                keep.contains(
                    "- [ ] #task Keep target ^keep\n  - 🪢 **LINKED FROM:** [[#^s]] (💬 mentions)\n"
                ),
                "{keep}"
            );
            assert!(keep.contains("- [ ] #task New target ^new\n"), "{keep}");
            assert!(
                !keep.contains(
                    "- [ ] #task New target ^new\n  - 🪢 **LINKED FROM:"
                ),
                "uncertain source newly added: {keep}"
            );
        }
    }
}

#[test]
fn mn1_json_reports_mention_subset_and_edges() {
    let temp = TempDir::new("bob-cli-mentions-mn1-json");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(
        &vault.join("note.md"),
        concat!(
            "- [ ] #task Source mentions [[#^t]] in title ^s\n",
            "- [ ] #task Target task ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let (dry, _) = dry_run_with_config(&vault, &daily, &config);
    assert_eq!(dry["mention_edges"], 1);
    let updates = linked_from_updates(&dry);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["kind"], "inserted");
    assert_eq!(updates[0]["mentions_added"], serde_json::json!(["[[#^s]]"]));
    assert!(
        updates[0]
            .get("mentions_removed")
            .and_then(Value::as_array)
            .is_none_or(|removed| removed.is_empty()),
        "{}",
        updates[0]
    );
    let applied = apply_json_with_config(&vault, &daily, &config);
    assert_eq!(
        applied["linked_from_line_updates"],
        dry["linked_from_line_updates"]
    );
    assert_eq!(applied["mention_edges"], dry["mention_edges"]);
}

#[test]
fn mn8_relation_wins_over_mention() {
    let temp = TempDir::new("bob-cli-mentions-mn8");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(
        &vault.join("mix.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** duplicate of [[#^t]]\n",
            "  - duplicate of [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_with_config(&vault, &daily, &config);
    let after = fs::read_to_string(vault.join("mix.md")).unwrap();
    assert!(has_linked_from(&after, "[[#^s]] (duplicate of)"), "{after}");
    assert!(!after.contains("(💬 mentions)"), "{after}");
}

#[test]
fn mn9_dependency_suppresses_mention_but_not_relation() {
    let temp = TempDir::new("bob-cli-mentions-mn9");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [ ] #task Source depends ^s\n",
            "  - ⛓️ **DEPENDS ON:** [[#^t]]\n",
            "  - 🪢 **LINKS:** related to [[#^t]]\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Target depended ^t\n",
            "- [ ] #task Other source ^u\n",
            "  - mentions [[#^v]]\n",
            "- [ ] #task Reverse depended ^v\n",
            "  - ⛓️ **DEPENDS ON:** [[#^u]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_with_config(&vault, &daily, &config);
    let after = fs::read_to_string(vault.join("deps.md")).unwrap();
    assert!(has_linked_from(&after, "[[#^s]] (related to)"), "{after}");
    assert!(!after.contains("(💬 mentions)"), "{after}");
    assert!(
        !after.contains("Reverse depended ^v\n  - 🪢 **LINKED FROM:**"),
        "{after}"
    );
}

#[test]
fn mn11_mints_idless_source_with_mention_reason() {
    let temp = TempDir::new("bob-cli-mentions-mn11");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(&vault.join("src.md"), "- [ ] #task See this [[tgt#^t]]\n");
    write_file(&vault.join("tgt.md"), "- [ ] #task Target task ^t\n");
    write_blocked_tasks_settings(&vault);

    let json = apply_json_with_config(&vault, &daily, &config);
    assert_eq!(minted_ids(&json).len(), 1);
    assert_eq!(minted_ids(&json)[0]["path"], "src.md");
    assert_eq!(minted_ids(&json)[0]["line"], 1);
    assert_eq!(minted_ids(&json)[0]["reason"], "mention");
    let src = fs::read_to_string(vault.join("src.md")).unwrap();
    assert!(src.contains(" ^"), "{src}");
    assert!(!src.contains("[id::]"), "{src}");
    assert!(!src.contains("[fresh::]"), "{src}");
    let tgt = fs::read_to_string(vault.join("tgt.md")).unwrap();
    assert!(tgt.contains("(💬 mentions)"), "{tgt}");
}

#[test]
fn mn13_mentions_off_drops_only_mention_entries() {
    let temp = TempDir::new("bob-cli-mentions-mn13");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** see [[#^r]]\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Related ^r\n",
            "- [ ] #task Mentioned ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_with_config(&vault, &daily, &config);
    let on = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_linked_from(&on, "[[#^s]] (see)"), "{on}");
    assert!(has_linked_from(&on, "[[#^s]] (💬 mentions)"), "{on}");

    write_mentions_config(&config, false);
    apply_with_config(&vault, &daily, &config);
    let off = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(has_linked_from(&off, "[[#^s]] (see)"), "{off}");
    assert!(!off.contains("(💬 mentions)"), "{off}");
    assert!(off.contains("  - mentions [[#^t]]\n"), "{off}");

    write_mentions_config(&config, true);
    apply_with_config(&vault, &daily, &config);
    let rebuilt = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert_eq!(rebuilt, on);
}

#[test]
fn mn14_rebuilds_identical_bytes_after_deleting_mention_lines() {
    let temp = TempDir::new("bob-cli-mentions-mn14");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Other ^o\n",
            "  - also [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_with_config(&vault, &daily, &config);
    let snapshot = fs::read_to_string(vault.join("tasks.md")).unwrap();
    write_file(&vault.join("tasks.md"), &strip_linked_from_lines(&snapshot));
    apply_with_config(&vault, &daily, &config);
    assert_eq!(
        fs::read_to_string(vault.join("tasks.md")).unwrap(),
        snapshot
    );
    assert_relations_clean(&apply_json_with_config(&vault, &daily, &config));
}

#[test]
fn mn17_mention_writes_are_inert() {
    let temp = TempDir::new("bob-cli-mentions-mn17");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Source ^s\n",
        "- [ ] #task Target ^t\n",
        "- [/] #task Pending ^p\n",
        "- [*] #task Next ^n\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);
    apply_with_config(&vault, &daily, &config);
    let baseline = fs::read_to_string(&tasks).unwrap();

    write_file(
        &tasks,
        concat!(
            "- [ ] #task Source ^s\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "- [/] #task Pending ^p\n",
            "- [*] #task Next ^n\n",
        ),
    );
    apply_with_config(&vault, &daily, &config);
    let added = fs::read_to_string(&tasks).unwrap();
    assert!(has_linked_from(&added, "[[#^s]] (💬 mentions)"), "{added}");
    assert!(added.contains("- [/] #task Pending ^p\n"), "{added}");
    assert!(added.contains("- [*] #task Next ^n\n"), "{added}");
    assert!(!added.contains("[fresh::]"), "{added}");

    write_file(
        &tasks,
        concat!(
            "- [ ] #task Source ^s\n",
            "- [ ] #task Target ^t\n",
            "  - 🪢 **LINKED FROM:** [[#^s]] (💬 mentions)\n",
            "- [/] #task Pending ^p\n",
            "- [*] #task Next ^n\n",
        ),
    );
    apply_with_config(&vault, &daily, &config);
    let removed = fs::read_to_string(&tasks).unwrap();
    assert!(!removed.contains("LINKED FROM"), "{removed}");
    assert_eq!(strip_linked_from_lines(&removed), baseline);
}

#[test]
fn mention_only_owner_with_mentions_off_loses_only_mention_entries() {
    let temp = TempDir::new("bob-cli-mentions-off-owner");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, false);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - 🪢 **LINKS:** see [[#^r]]\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Target ^t\n",
            "  - 🪢 **LINKED FROM:** [[#^s]] (💬 mentions)\n",
            "- [ ] #task Related ^r\n",
            "  - 🪢 **LINKED FROM:** [[#^s]] (see)\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    apply_with_config(&vault, &daily, &config);
    let after = fs::read_to_string(vault.join("tasks.md")).unwrap();
    assert!(!after.contains("(💬 mentions)"), "{after}");
    assert!(has_linked_from(&after, "[[#^s]] (see)"), "{after}");
    assert!(after.contains("  - mentions [[#^t]]\n"), "{after}");
}

#[test]
fn mn_invalid_mentions_switch_behaves_as_false() {
    let temp = TempDir::new("bob-cli-mentions-invalid");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_file(&config, "relations:\n  linked_from_mentions: 1\n");
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    let (json, err) = dry_run_with_config(&vault, &daily, &config);
    assert!(
        err.contains("invalid relations config")
            && err.contains("using relations.linked_from_mentions=false"),
        "{err}"
    );
    assert_relations_clean(&json);
}

#[test]
fn mention_and_relation_mint_reports_relation_reason() {
    let temp = TempDir::new("bob-cli-mentions-mint-reason");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Call back\n",
            "  - 🪢 **LINKS:** [[#^r]]\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Related ^r\n",
            "- [ ] #task Mentioned ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    let json = apply_json_with_config(&vault, &daily, &config);
    assert_eq!(minted_ids(&json).len(), 1);
    assert_eq!(minted_ids(&json)[0]["reason"], "relation");
}

#[test]
fn mn_human_output_includes_mention_counts() {
    let temp = TempDir::new("bob-cli-mentions-human");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let config = temp.path().join("config.yml");
    write_mentions_config(&config, true);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Source ^s\n",
            "  - mentions [[#^t]]\n",
            "- [ ] #task Target ^t\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    let output = bob_command()
        .args(["task", "reconcile", "--dry-run"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("human dry-run");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("💬"), "{text}");
}

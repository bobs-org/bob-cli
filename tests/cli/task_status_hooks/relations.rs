//! LINKED FROM projection pass on `bob task reconcile` (LR1–LR24,
//! `docs/task-relations.md` §7).

use super::dependency_lines::{
    assert_reconciled_clean, dry_run_json, write_daily,
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

//! Capture relation-successor offers for managed Done (`!` and nested
//! complete-label commits). JSON shape, picks, refusal, omission, and
//! dry-run parity (`docs/task-dependencies.md` §12.5; SA4, SA14, SA15).

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-11-02 09:30:00";

const SUCCESSOR_CONFIG: &str = concat!(
    "relations:\n",
    "  successor_labels:\n",
    "    - starts after\n",
);

const COMPLETE_AND_SUCCESSOR_CONFIG: &str = concat!(
    "relations:\n",
    "  complete_labels:\n",
    "    - implemented by\n",
    "  successor_labels:\n",
    "    - starts after\n",
);

const CANCEL_AND_SUCCESSOR_CONFIG: &str = concat!(
    "relations:\n",
    "  cancel_labels:\n",
    "    - duplicate of\n",
    "  successor_labels:\n",
    "    - starts after\n",
);

fn vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261102.md");
    (temp, vault, day_file)
}

fn write_config(vault: &Path, body: &str) -> PathBuf {
    let config = vault.parent().expect("temp dir").join("config.yml");
    write_file(&config, body);
    config
}

fn agents_body() -> String {
    concat!(
        "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
        "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
    )
    .to_string()
}

fn sa4_vault(name: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("agents.md"), &agents_body());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    (temp, vault, day_file, config)
}

fn capture_output(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    flags: &[&str],
    text: &str,
    human: bool,
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if !human {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
    }
    command
        .args(flags)
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", config)
        .output()
        .expect("run bob capture")
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    flags: &[&str],
    text: &str,
) -> serde_json::Value {
    let output = capture_output(vault, day_file, config, flags, text, false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn vault_bytes(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read vault dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(vault)
                    .expect("relative")
                    .to_string_lossy()
                    .to_string();
                entries.push((relative, fs::read(&path).expect("read file")));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn relation_row(json: &serde_json::Value) -> &serde_json::Value {
    let rows = json["task_complete"]["relation_successors"]
        .as_array()
        .unwrap_or_else(|| panic!("relation_successors array: {json}"));
    assert_eq!(rows.len(), 1, "{rows:?}");
    &rows[0]
}

fn assert_sa4_row(row: &serde_json::Value) {
    assert_eq!(row["note_path"], "agents.md");
    assert_eq!(row["block_id"], "evaluate");
    assert_eq!(row["text"], "Prepare evaluation prompt");
    assert_eq!(row["trigger"], "relation");
    assert_eq!(row["offer"]["group"], "relation");
    assert_eq!(row["offer"]["after"], "agents.md#^implement");
    assert_eq!(row["relation"]["label"], "starts after");
    assert_eq!(row["relation"]["key"], "starts after");
    assert_eq!(row["relation"]["event"], "done");
    assert_eq!(
        row["relation"]["after"],
        serde_json::json!([{
            "note_path": "agents.md",
            "block_id": "implement",
            "text": "Send implementation prompt",
        }])
    );
    assert_eq!(
        row["unblocked_by"],
        serde_json::json!([{
            "note_path": "agents.md",
            "block_id": "implement",
            "text": "Send implementation prompt",
        }])
    );
}

// SA4: `!` completes implement and offers evaluate at the predecessor slot.
#[test]
fn sa4_bang_reports_relation_successor_at_slot() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa4-bang");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert_eq!(json["task_complete"]["unblocked"], serde_json::json!([]));
    assert_eq!(json["task_complete"]["relation_check"], "checked");
    let row = relation_row(&json);
    assert_sa4_row(row);
    assert_eq!(row["not_linked"], "not_selected");
    assert_eq!(row["offer"]["selected"], false);
    assert_eq!(row["offer"]["destination"]["entry_name"], "BUILD");
    assert_eq!(row["offer"]["destination"]["entry_role"], "slot");
    assert_eq!(json["pull_in"]["offer_count"], 1);
    assert_eq!(json["pull_in"]["picked_count"], 0);
}

// Nested complete-label Done uses the same `!` engine and relation JSON.
#[test]
fn nested_complete_label_done_offers_relation() {
    let (_temp, vault, day_file, _) = sa4_vault("bob-cli-sa4-nested");
    write_file(
        &vault.join("done_by.md"),
        "- [ ] #task Close the loop ^close-loop\n",
    );
    let config = write_config(&vault, COMPLETE_AND_SUCCESSOR_CONFIG);
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "@agents+implement ~done_by:close-loop|\"implemented by\"",
    );
    let nested = &json["relation_completes"][0]["task_complete"];
    assert_eq!(nested["action"], "completed");
    assert_eq!(nested["block_id"], "implement");
    assert_eq!(nested["relation_check"], "checked");
    let rows = nested["relation_successors"]
        .as_array()
        .expect("nested relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["trigger"], "relation");
    assert_eq!(rows[0]["offer"]["group"], "relation");
    assert_eq!(json["pull_in"]["offer_count"], 1);
}

// Cancel never raises a relation event.
#[test]
fn cancel_is_relation_silent() {
    let (_temp, vault, day_file, _) = sa4_vault("bob-cli-sa-cancel");
    write_file(
        &vault.join("dup.md"),
        "- [ ] #task Duplicate implement ^dup\n",
    );
    let config = write_config(&vault, CANCEL_AND_SUCCESSOR_CONFIG);
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "@agents+implement ~dup:dup|\"duplicate of\"",
    );
    let item = if json.get("relation_cancels").is_some() {
        &json
    } else {
        panic!("expected relation_cancels: {json}");
    };
    assert!(
        item.get("task_complete")
            .and_then(|value| value.get("relation_successors"))
            .is_none(),
        "{item}"
    );
    assert!(
        item["relation_cancels"][0]
            .get("task_complete")
            .and_then(|value| value.get("relation_successors"))
            .is_none(),
        "{item}"
    );
    assert_eq!(json["pull_in"]["offer_count"], 0);
}

// Held-only: a Pending source is JSON-only, with label/event/source.
#[test]
fn held_only_pending_source_keeps_label_event_and_source() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-held");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [/] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(
        json["task_complete"].get("relation_successors").is_none(),
        "{json}"
    );
    assert_eq!(json["task_complete"]["relation_check"], "checked");
    let held = json["task_complete"]["relation_held"]
        .as_array()
        .expect("relation_held");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["note_path"], "agents.md");
    assert_eq!(held[0]["block_id"], "evaluate");
    assert_eq!(held[0]["status_symbol"], "/");
    assert_eq!(held[0]["reason"], "already_started");
    assert_eq!(held[0]["relation"]["label"], "starts after");
    assert_eq!(held[0]["relation"]["event"], "done");
    assert_eq!(held[0]["line"], 1);
    assert_eq!(json["pull_in"]["offer_count"], 0);
}

// SA14: a direct dependent stays in unblocked[] with a relation second reason.
#[test]
fn sa14_dependent_keeps_hand_off_with_relation_reason() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa14");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [?] #task Prepare evaluation prompt [dependsOn:: implement] [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(
        json["task_complete"].get("relation_successors").is_none(),
        "{json}"
    );
    let unblocked = json["task_complete"]["unblocked"]
        .as_array()
        .expect("unblocked");
    assert_eq!(unblocked.len(), 1, "{unblocked:?}");
    assert_eq!(unblocked[0]["block_id"], "evaluate");
    assert_eq!(unblocked[0]["offer"]["group"], "hand_off");
    assert_ne!(unblocked[0]["trigger"], "relation");
    assert_eq!(unblocked[0]["relation"]["label"], "starts after");
    assert_eq!(unblocked[0]["relation"]["event"], "done");
}

// SA15: relation wins; project recovery still writes.
#[test]
fn sa15_relation_wins_with_project_recovery() {
    let (_temp, vault, day_file) = vault("bob-cli-sa15");
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    write_file(
        &vault.join("agents.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [?] #task #prj Agents [dependsOn:: implement] ^prj\n",
            "- [?] #task Prepare evaluation prompt #prj_is_blocked [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", "all"],
        "!agents:implement",
    );
    let row = relation_row(&json);
    assert_eq!(row["block_id"], "evaluate");
    assert_eq!(row["offer"]["group"], "relation");
    assert_eq!(row["offer"]["selected"], true);
    let note = fs::read_to_string(vault.join("agents.md")).unwrap();
    assert!(
        !note.contains("#prj_is_blocked"),
        "project recovery remains: {note}"
    );
    assert!(
        note.contains("- [*] #task Prepare evaluation prompt"),
        "{note}"
    );
}

// Id-less source still picks by its block-id key.
#[test]
fn id_less_source_pick_mints_and_links() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-idless");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", "all"],
        "!agents:implement",
    );
    let row = relation_row(&json);
    assert_eq!(row["block_id"], "evaluate");
    assert_eq!(row["offer"]["selected"], true);
    let note = fs::read_to_string(vault.join("agents.md")).unwrap();
    assert!(
        note.contains("- [*] #task Prepare evaluation prompt"),
        "{note}"
    );
    assert!(note.contains("[fresh:: 2026-11-02]"), "{note}");
}

// Future target note is read through the staged planner (cross-note source).
#[test]
fn future_target_note_is_read() {
    let (_temp, vault, day_file) = vault("bob-cli-sa-future");
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    write_file(
        &vault.join("agents.md"),
        "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
    );
    write_file(
        &vault.join("review.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    let row = relation_row(&json);
    assert_eq!(row["note_path"], "review.md");
    assert_eq!(row["block_id"], "evaluate");
    assert_eq!(row["offer"]["after"], "agents.md#^implement");
}

// Keys, numbers, all, and none share one number space.
#[test]
fn keys_numbers_all_and_none() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-keys");
    let dry = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    let n = dry["task_complete"]["relation_successors"][0]["offer"]["n"]
        .as_u64()
        .expect("n");
    let key = dry["task_complete"]["relation_successors"][0]["offer"]["key"]
        .as_str()
        .expect("key")
        .to_string();

    let by_n = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", &n.to_string()],
        "!agents:implement",
    );
    assert_eq!(
        by_n["task_complete"]["relation_successors"][0]["offer"]["selected"],
        true
    );

    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-key-pick");
    let by_key = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", &key],
        "!agents:implement",
    );
    assert_eq!(
        by_key["task_complete"]["relation_successors"][0]["offer"]["selected"],
        true
    );

    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-none");
    let none = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", "none"],
        "!agents:implement",
    );
    let row = relation_row(&none);
    assert_eq!(row["offer"]["selected"], false);
    assert_eq!(row["not_linked"], "not_selected");
    let note = fs::read_to_string(vault.join("agents.md")).unwrap();
    assert!(
        note.contains("- [ ] #task Prepare evaluation prompt"),
        "{note}"
    );
    assert!(!note.contains("[fresh::"), "{note}");
    let day = fs::read_to_string(&day_file).unwrap();
    assert!(!day.contains("agents#^evaluate"), "{day}");
}

// Unknown pick refuses; `--require-pull-in-choice` lists relation offers.
#[test]
fn refusal_and_require_choice_include_relation_rows() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-refuse");
    let output = capture_output(
        &vault,
        &day_file,
        &config,
        &["-p", "no-such-key"],
        "!agents:implement",
        false,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    let err = format!("{}{}", stdout(&output), stderr(&output));
    assert!(err.contains("no-such-key"), "{err}");

    let before = vault_bytes(&vault);
    let required = capture_output(
        &vault,
        &day_file,
        &config,
        &["--require-pull-in-choice"],
        "!agents:implement",
        false,
    );
    assert_eq!(
        required.status.code(),
        Some(1),
        "{}",
        format_output(&required)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&required).trim()).expect("json");
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "pull_in_choice_required");
    assert_eq!(json["pull_in"]["offer_count"], 1);
    let offers = json["offers"].as_array().expect("offers");
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0]["offer"]["group"], "relation");
    assert_eq!(vault_bytes(&vault), before);
}

// No-pick leaves the source byte-identical apart from completing T.
#[test]
fn no_pick_leaves_source_bytes() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-nopick");
    let before_source = fs::read_to_string(vault.join("agents.md")).unwrap();
    let json =
        capture_json(&vault, &day_file, &config, &[], "!agents:implement");
    let row = relation_row(&json);
    assert_eq!(row["offer"]["selected"], false);
    let after = fs::read_to_string(vault.join("agents.md")).unwrap();
    let evaluate = after
        .lines()
        .find(|line| line.contains("Prepare evaluation prompt"))
        .expect("evaluate line");
    assert!(
        evaluate.starts_with("- [ ] #task Prepare evaluation prompt"),
        "{after}"
    );
    assert!(!evaluate.contains("[fresh::"), "{after}");
    assert!(!evaluate.contains("- [*]"), "{after}");
    assert!(
        after.contains("- [x] #task Send implementation prompt"),
        "{after}"
    );
    let _ = before_source;
}

// Dry-run and real-run JSON match except dry_run and would-forms.
#[test]
fn dry_and_real_json_parity() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-parity");
    let dry = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run", "-p", "all"],
        "!agents:implement",
    );
    let before = vault_bytes(&vault);
    assert_eq!(dry["dry_run"], true);
    let real = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", "all"],
        "!agents:implement",
    );
    assert_eq!(real["dry_run"], false);
    assert_ne!(vault_bytes(&vault), before);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    assert_eq!(
        masked["task_complete"]["relation_successors"],
        real["task_complete"]["relation_successors"]
    );
    assert_eq!(
        masked["pull_in"]["picked_count"],
        real["pull_in"]["picked_count"]
    );
}

// Disabled labels and an edge-free close omit every relation field.
#[test]
fn disabled_and_edge_free_omit_relation_fields() {
    let (_temp, vault, day_file) = vault("bob-cli-sa-omit");
    write_file(&vault.join("agents.md"), &agents_body());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let missing = vault.parent().expect("temp").join("missing.yml");
    let json = capture_json(
        &vault,
        &day_file,
        &missing,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(
        json["task_complete"].get("relation_successors").is_none(),
        "{json}"
    );
    assert!(
        json["task_complete"].get("relation_held").is_none(),
        "{json}"
    );
    assert!(
        json["task_complete"].get("relation_check").is_none(),
        "{json}"
    );

    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa-edge-free");
    write_file(
        &vault.join("solo.md"),
        "- [ ] #task Lonely task [id:: lonely] ^lonely\n",
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!solo:lonely",
    );
    assert!(
        json["task_complete"].get("relation_successors").is_none(),
        "{json}"
    );
    assert!(
        json["task_complete"].get("relation_held").is_none(),
        "{json}"
    );
    assert!(
        json["task_complete"].get("relation_check").is_none(),
        "{json}"
    );
}

// `plan.link_unblocked: false` is the other kill switch.
#[test]
fn link_unblocked_false_omits_relation_fields() {
    let (_temp, vault, day_file, _) = sa4_vault("bob-cli-sa-kill");
    let config = write_config(
        &vault,
        concat!(
            "plan:\n",
            "  link_unblocked: false\n",
            "relations:\n",
            "  successor_labels:\n",
            "    - starts after\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(
        json["task_complete"].get("relation_successors").is_none(),
        "{json}"
    );
    assert!(
        json["task_complete"].get("relation_check").is_none(),
        "{json}"
    );
}

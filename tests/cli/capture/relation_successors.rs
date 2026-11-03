//! Capture relation-successor offers for managed Done (`!` and nested
//! complete-label commits), Pomodoro parked/close events (`x*`, `x!`,
//! embeds, `=x*`), and `@route+id!` unlink of a Pending task. JSON shape,
//! picks, refusal, omission, and dry-run parity
//! (`docs/task-dependencies.md` §§12.1, 12.5, 12.18; SA1–SA6, SA11–SA18,
//! SA31–SA32).

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

// SA22: normalized label keys match configured successor labels, while the
// inverse spelling remains inert on the CLI capture path.
#[test]
fn sa22_normalized_label_matches_and_inverse_is_inert() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-sa22-labels");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Normalized edge [id:: normalized] ^normalized\n",
            "  - 🪢 **LINKS:** Starts  After: [[agents#^implement]]\n",
            "- [ ] #task Inverse edge [id:: inverse] ^inverse\n",
            "  - 🪢 **LINKS:** starts before [[agents#^implement]]\n",
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
    let rows = json["task_complete"]["relation_successors"]
        .as_array()
        .expect("relation successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "normalized");
}

// SA23: an overlapping effect label has no capture-time relation effect.
#[test]
fn sa23_overlapping_successor_and_complete_label_is_inert() {
    let (_temp, vault, day_file, _) = sa4_vault("bob-cli-sa23-overlap");
    let config = write_config(
        &vault,
        concat!(
            "relations:\n",
            "  complete_labels: [starts after]\n",
            "  successor_labels: ['Starts  After:']\n",
        ),
    );
    let source_before = fs::read_to_string(vault.join("agents.md"))
        .expect("source before close");
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
        json["task_complete"].get("relation_completes").is_none(),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("agents.md"))
            .expect("source after close"),
        source_before
    );
}

// SA26: relation lookup uses LINKS and block identities without dependency
// IDs or generated mirrors.
#[test]
fn sa26_relations_work_without_task_ids_or_mirror_lines() {
    let (_temp, vault, day_file, _) = sa4_vault("bob-cli-sa26-no-ids");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt ^implement\n",
        ),
    );
    let config = write_config(
        &vault,
        concat!(
            "relations:\n",
            "  successor_labels: [starts after]\n",
            "  linked_from_lines: false\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    let row = relation_row(&json);
    assert_eq!(row["block_id"], "evaluate");
    assert_eq!(json["task_complete"]["relation_check"], "checked");
    assert_eq!(json["task_complete"]["unblocked"], serde_json::json!([]));
    let note = fs::read_to_string(vault.join("agents.md")).expect("note");
    assert!(!note.contains("[id::"), "{note}");
    assert!(!note.contains("[dependsOn::"), "{note}");
}

// SA28: closed, archived, and previous-daily sources stay out of the offer
// set even when their LINKS entries resolve to the just-completed target.
#[test]
fn sa28_closed_archived_and_previous_daily_sources_are_skipped() {
    let (_temp, vault, _) = vault("bob-cli-sa28-source-filters");
    let day_file = vault.join("2026/20261102.md");
    write_file(
        &vault.join("agents.md"),
        "- [/] #task Send implementation prompt ^implement\n",
    );
    let edge = |status: char, description: &str, id: &str| {
        format!(
            "- [{status}] #task {description} ^{id}\n  - 🪢 **LINKS:** starts after [[agents#^implement]]\n"
        )
    };
    write_file(
        &vault.join("closed.md"),
        &edge('x', "Closed source", "closed"),
    );
    write_file(
        &vault.join("done/archive.md"),
        &edge(' ', "Archived source", "archived"),
    );
    write_file(
        &vault.join("2026/20261101.md"),
        &edge(' ', "Previous daily source", "previous"),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
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
    assert!(json["task_complete"]["relation_held"].is_null(), "{json}");
}

// Parks: `x*` and close completions offer relation successors for parked
// and completed targets (`docs/task-dependencies.md` §12.18).

fn park_agents() -> String {
    concat!(
        "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
        "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
    )
    .to_string()
}

fn park_day() -> String {
    "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n  - [[agents#^implement]]\n"
        .to_string()
}

fn park_vault(name: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("agents.md"), &park_agents());
    write_file(&day_file, &park_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    (temp, vault, day_file, config)
}

fn close_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    flags: &[&str],
    text: &str,
) -> serde_json::Value {
    capture_json(vault, day_file, config, flags, text)["pomodoro_close"].clone()
}

fn assert_close_relation_silent(close: &serde_json::Value) {
    assert!(close.get("relation_successors").is_none(), "{close}");
    assert!(close.get("relation_held").is_none(), "{close}");
    assert!(close.get("relation_check").is_none(), "{close}");
}

// SA1: `x*` parks T; S is offered with a parked event at the closing
// anchor, and the park recovers none of T's dependents.
#[test]
fn sa1_park_reports_relation_successor_at_closing_anchor() {
    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa1-park");
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x*");
    assert_eq!(close["relation_check"], "checked");
    assert_eq!(close["unblocked"], serde_json::json!([]));
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["note_path"], "agents.md");
    assert_eq!(row["block_id"], "evaluate");
    assert_eq!(row["text"], "Prepare evaluation prompt");
    assert_eq!(row["trigger"], "relation");
    assert_eq!(row["offer"]["group"], "relation");
    assert_eq!(row["offer"]["after"], "agents.md#^implement");
    assert_eq!(row["offer"]["selected"], false);
    assert_eq!(row["not_linked"], "not_selected");
    assert_eq!(row["relation"]["label"], "starts after");
    assert_eq!(row["relation"]["key"], "starts after");
    assert_eq!(row["relation"]["event"], "parked");
    assert_eq!(
        row["relation"]["after"],
        serde_json::json!([{
            "note_path": "agents.md",
            "block_id": "implement",
            "text": "Send implementation prompt",
        }])
    );
    // A parked event never uses the struck-bullet slot anchor.
    assert_ne!(row["offer"]["destination"]["entry_role"], "slot");
    // The parked task row itself moved to Pending.
    let tasks = close["tasks"].as_array().expect("tasks");
    assert!(
        tasks.iter().any(|task| task["status_symbol"] == "/"
            && task["block_id"] == "implement"),
        "{tasks:?}"
    );
}

// SA2: T keeps a live link in a later open entry, so the park raises no
// event and the close stays relation-silent.
#[test]
fn sa2_second_live_link_voids_park_event() {
    let (_temp, vault, day_file) = vault("bob-cli-sa2-live");
    write_file(&vault.join("agents.md"), &park_agents());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n  - [[agents#^implement]]\n- [ ] () \u{2014} LATER\n  - [[agents#^implement]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x*");
    assert_close_relation_silent(&close);
    let tasks = close["tasks"].as_array().expect("tasks");
    assert!(
        tasks.iter().any(|task| task["status_symbol"] == "/"
            && task["block_id"] == "implement"),
        "{tasks:?}"
    );
}

// SA3: T is already `[/]`; parking it again still raises the event.
#[test]
fn sa3_already_pending_target_parks_again() {
    let (_temp, vault, day_file) = vault("bob-cli-sa3-pending");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(&day_file, &park_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x*");
    assert_eq!(close["relation_check"], "checked");
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "parked");
}

// SA5: closing completions raise done events: `x!1` on a linked task and
// plain `x` on an embedded task, both at the slot anchor.
#[test]
fn sa5_close_completion_reports_done_event() {
    let (_temp, vault1, day_file, config) = park_vault("bob-cli-sa5-bang");
    let close = close_json(&vault1, &day_file, &config, &["--dry-run"], "x!1");
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "done");
    assert_eq!(rows[0]["offer"]["after"], "agents.md#^implement");
    assert_eq!(rows[0]["offer"]["destination"]["entry_role"], "slot");

    let (_temp, vault, day_file) = vault("bob-cli-sa5-embed");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [ ] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n  - ![[agents#^implement]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x");
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "done");
}

// SA6: drops, carries, and defers raise no event.
#[test]
fn sa6_drop_carry_and_defer_stay_silent() {
    for text in ["x~1", "x"] {
        let (_temp, vault, day_file, config) =
            park_vault(&format!("bob-cli-sa6-{text}"));
        let close =
            close_json(&vault, &day_file, &config, &["--dry-run"], text);
        assert_close_relation_silent(&close);
    }
}

// SA11: S also starts after an unreleased T2, so it is held `waits_for`.
#[test]
fn sa11_unreleased_second_target_holds_source() {
    let (_temp, vault, day_file) = vault("bob-cli-sa11-waits");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]] • starts after [[agents#^deploy]]\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
            "- [ ] #task Deploy release [id:: deploy] ^deploy\n",
        ),
    );
    write_file(&day_file, &park_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x*");
    assert_eq!(close["relation_check"], "checked");
    assert!(close.get("relation_successors").is_none(), "{close}");
    let held = close["relation_held"].as_array().expect("relation_held");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["block_id"], "evaluate");
    assert_eq!(held[0]["reason"], "waits_for");
    assert_eq!(held[0]["relation"]["event"], "parked");
    let waits_for = held[0]["waits_for"].as_array().expect("waits_for");
    assert_eq!(waits_for.len(), 1, "{waits_for:?}");
    assert_eq!(waits_for[0]["block_id"], "deploy");
}

// SA12: one `x*` parks T and T2; S starts after both, so it appears once
// under the earlier target group naming both.
#[test]
fn sa12_two_parks_share_one_row() {
    let (_temp, vault, day_file) = vault("bob-cli-sa12-two");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]] • starts after [[agents#^deploy]]\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
            "- [*] #task Deploy release [id:: deploy] ^deploy\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n  - [[agents#^implement]]\n  - [[agents#^deploy]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x*");
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    let after = rows[0]["relation"]["after"].as_array().expect("after");
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0]["block_id"], "implement");
    assert_eq!(after[1]["block_id"], "deploy");
}

// SA13: S's other target does not resolve, so it is ignored and S offers.
#[test]
fn sa13_unresolved_second_target_ignored() {
    let (_temp, vault, day_file) = vault("bob-cli-sa13-missing");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]] • starts after [[missing#^unknown]]\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(&day_file, &park_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x*");
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "parked");
}

// A park never recovers T's dependents: S keeps its dependency hold and
// stays out of `unblocked`.
#[test]
fn sa14park_parked_target_keeps_dependent_blocked() {
    let (_temp, vault, day_file) = vault("bob-cli-sa14park");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [?] #task Prepare evaluation prompt [dependsOn:: implement] [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(&day_file, &park_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let close = close_json(&vault, &day_file, &config, &["--dry-run"], "x*");
    assert_eq!(close["unblocked"], serde_json::json!([]));
    assert!(close.get("relation_successors").is_none(), "{close}");
    let held = close["relation_held"].as_array().expect("relation_held");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["block_id"], "evaluate");
    assert_eq!(held[0]["reason"], "blocked");
    let waits_on = held[0]["waits_on"].as_array().expect("waits_on");
    assert_eq!(waits_on.len(), 1, "{waits_on:?}");
    assert_eq!(waits_on[0]["block_id"], "implement");
}

// SA16: skipping S at the park is stateless; completing T later offers S
// again with a done event.
#[test]
fn sa16_skipped_park_offers_again_on_done() {
    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa16-skip");
    let before = fs::read_to_string(vault.join("agents.md")).expect("agents");
    let first = close_json(&vault, &day_file, &config, &[], "x*");
    let rows = first["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["offer"]["selected"], false);
    let after_park =
        fs::read_to_string(vault.join("agents.md")).expect("agents");
    let evaluate_before = before
        .lines()
        .find(|line| line.contains("Prepare evaluation prompt"))
        .expect("evaluate line");
    let evaluate_after = after_park
        .lines()
        .find(|line| line.contains("Prepare evaluation prompt"))
        .expect("evaluate line");
    assert_eq!(evaluate_before, evaluate_after);
    let second =
        capture_json(&vault, &day_file, &config, &[], "!agents:implement");
    let rows = second["task_complete"]["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "done");
}

// SA17: picking S at the park links it; completing T later holds S as
// already planned.
#[test]
fn sa17_picked_park_holds_already_planned_on_done() {
    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa17-pick");
    let first = close_json(&vault, &day_file, &config, &["-p", "all"], "x*");
    let rows = first["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["offer"]["selected"], true);
    let note = fs::read_to_string(vault.join("agents.md")).expect("agents");
    assert!(
        note.contains("- [*] #task Prepare evaluation prompt"),
        "{note}"
    );
    assert!(note.contains("[fresh:: 2026-11-02]"), "{note}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(day.contains("[[agents#^evaluate]]"), "{day}");
    let second =
        capture_json(&vault, &day_file, &config, &[], "!agents:implement");
    assert!(
        second["task_complete"].get("relation_successors").is_none(),
        "{}",
        second["task_complete"]
    );
    let held = second["task_complete"]["relation_held"]
        .as_array()
        .expect("relation_held");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["block_id"], "evaluate");
    assert_eq!(held[0]["reason"], "already_planned");
}

// SA18: `x*!2` completes A (with dependents) and parks T in one picker:
// Hand-offs first, then the parked relation group.
#[test]
fn sa18_mixed_done_and_park_share_one_picker() {
    let (_temp, vault, day_file) = vault("bob-cli-sa18-mixed");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
            "- [ ] #task Ship schema change [id:: schema] ^schema\n",
            "- [?] #task Announce schema change [dependsOn:: schema] [id:: schema-change] ^schema-change\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n  - [[agents#^implement]]\n  - [[agents#^schema]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let json = capture_json(&vault, &day_file, &config, &["--dry-run"], "x*!2");
    let close = &json["pomodoro_close"];
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 1, "{unblocked:?}");
    assert_eq!(unblocked[0]["block_id"], "schema-change");
    assert_eq!(unblocked[0]["offer"]["group"], "hand_off");
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "parked");
    // One shared number space: the relation row follows the hand-off.
    let hand_off_n = unblocked[0]["offer"]["n"].as_u64().expect("n");
    let relation_n = rows[0]["offer"]["n"].as_u64().expect("n");
    assert!(relation_n > hand_off_n, "{hand_off_n} {relation_n}");
    assert_eq!(json["pull_in"]["offer_count"], 2);
}

// A parked pick writes Next, freshness, and the continuation link; dry and
// real JSON agree, and unpicked sources stay byte-identical.
#[test]
fn park_pick_links_into_continuation_with_parity() {
    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa1-dry");
    let dry = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run", "-p", "all"],
        "x*",
    );
    let before = vault_bytes(&vault);
    assert_eq!(dry["dry_run"], true);
    let real = capture_json(&vault, &day_file, &config, &["-p", "all"], "x*");
    assert_eq!(real["dry_run"], false);
    assert_ne!(vault_bytes(&vault), before);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    assert_eq!(
        masked["pomodoro_close"]["relation_successors"],
        real["pomodoro_close"]["relation_successors"]
    );
    assert_eq!(
        masked["pull_in"]["picked_count"],
        real["pull_in"]["picked_count"]
    );
    let row = &real["pomodoro_close"]["relation_successors"][0];
    assert_eq!(row["offer"]["selected"], true);
    let note = fs::read_to_string(vault.join("agents.md")).expect("agents");
    assert!(
        note.contains("- [*] #task Prepare evaluation prompt"),
        "{note}"
    );
    let task_blocks = real["task_blocks"].as_array().expect("task_blocks");
    assert!(
        task_blocks
            .iter()
            .any(|block| block["block_id"] == "evaluate"
                && block["roles"] == serde_json::json!(["relation_successor"])),
        "{task_blocks:?}"
    );

    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa1-nopick");
    let before_source =
        fs::read_to_string(vault.join("agents.md")).expect("agents");
    let json = capture_json(&vault, &day_file, &config, &[], "x*");
    let row = &json["pomodoro_close"]["relation_successors"][0];
    assert_eq!(row["offer"]["selected"], false);
    let after = fs::read_to_string(vault.join("agents.md")).expect("agents");
    let evaluate = after
        .lines()
        .find(|line| line.contains("Prepare evaluation prompt"))
        .expect("evaluate line");
    assert_eq!(
        evaluate,
        before_source
            .lines()
            .find(|line| line.contains("Prepare evaluation prompt"))
            .expect("evaluate line")
    );
    assert!(
        after.contains("- [/] #task Send implementation prompt"),
        "{after}"
    );
}

// The explicit-root `=x*` park form links T first, so the post-link day is
// the event baseline and the park still offers S.
#[test]
fn explicit_root_park_form_offers_relation() {
    let (_temp, vault, day_file) = vault("bob-cli-sa1-explicit");
    write_file(&vault.join("agents.md"), &park_agents());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "@agents:implement=x*",
    );
    let close = &json["pomodoro_close"];
    assert_eq!(close["relation_check"], "checked");
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "parked");
}

// SA19: a successor edge added by an earlier draft item is visible in
// staged text, so a later `x*` offers S through it.
#[test]
fn sa19_staged_edge_before_park_offers() {
    let (temp, vault, day_file) = vault("bob-cli-sa19-staged");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(&day_file, &park_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "@agents+evaluate ~agents:implement|\"starts after\"\n\nx*",
    );
    let _ = temp;
    let close = &json["captures"][1]["pomodoro_close"];
    let rows = close["relation_successors"]
        .as_array()
        .expect("relation_successors");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["block_id"], "evaluate");
    assert_eq!(rows[0]["relation"]["event"], "parked");
    assert_eq!(json["pull_in"]["offer_count"], 1);
}

// SA20: an edge added after its target was parked does not fire
// retroactively; authoring reports `relation_successor_already_released`.
#[test]
fn sa20_edge_after_park_warns_without_offer() {
    let (temp, vault, day_file) = vault("bob-cli-sa20-released");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(&day_file, &park_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "x*\n\n@agents+evaluate ~agents:implement|\"starts after\"",
    );
    let _ = temp;
    let close = &json["captures"][0]["pomodoro_close"];
    assert!(
        close.get("relation_successors").is_none(),
        "park before the edge offers nothing: {close}"
    );
    let warnings = json["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| {
            warning["code"] == "relation_successor_already_released"
        }),
        "{warnings:?}"
    );
    assert_eq!(json["pull_in"]["offer_count"], 0);
}

// SA21: a later item linking S drops S's row.
#[test]
fn sa21_later_link_of_source_drops_row() {
    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa21-source");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "x*\n\n@agents:evaluate",
    );
    let close = &json["captures"][0]["pomodoro_close"];
    assert!(
        close.get("relation_successors").is_none(),
        "later link drops S's row: {close}"
    );
    assert_eq!(json["pull_in"]["offer_count"], 0);
}

// SA21: a later item re-linking a parked T drops that T's parked group,
// even when the originating pick already linked S.
#[test]
fn sa21_later_relink_of_target_drops_parked_group() {
    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa21-target");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run", "-p", "all"],
        "x*\n\n@agents:implement",
    );
    let close = &json["captures"][0]["pomodoro_close"];
    assert!(
        close.get("relation_successors").is_none(),
        "relinked T drops its parked group: {close}"
    );
    assert_eq!(json["pull_in"]["offer_count"], 0);
}

// SA21: a picked S stays linked on disk when a later link drops its row
// from reporting; net reporting changes reporting, never files.
#[test]
fn sa21_dropped_pick_stays_linked_on_disk() {
    let (_temp, vault, day_file, config) = park_vault("bob-cli-sa21-disk");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", "all"],
        "x*\n\n@agents:evaluate",
    );
    let close = &json["captures"][0]["pomodoro_close"];
    assert!(
        close.get("relation_successors").is_none(),
        "picked row dropped from reporting: {close}"
    );
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(
        day.contains("[[agents#^evaluate]]"),
        "picked S stays linked: {day}"
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

fn unlink_agents(status: char) -> String {
    format!(
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [{status}] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
        status = status
    )
}

fn unlink_day() -> String {
    "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n"
        .to_string()
}

fn unlink_vault(
    name: &str,
    status: char,
) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("agents.md"), &unlink_agents(status));
    write_file(&day_file, &unlink_day());
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    (temp, vault, day_file, config)
}

fn item_relation_row(json: &serde_json::Value) -> &serde_json::Value {
    let rows = json["relation_successors"]
        .as_array()
        .unwrap_or_else(|| panic!("item relation_successors array: {json}"));
    assert_eq!(rows.len(), 1, "{rows:?}");
    &rows[0]
}

fn assert_item_relation_silent(json: &serde_json::Value) {
    assert!(json.get("relation_successors").is_none(), "{json}");
    assert!(json.get("relation_held").is_none(), "{json}");
    assert!(json.get("relation_check").is_none(), "{json}");
}

// SA31: `@agents+implement!` unlinks a `[/]` T and offers S at T's former
// slot on the capture item (no sub-object).
#[test]
fn sa31_pending_unlink_offers_relation_at_former_slot() {
    let (_temp, vault, day_file, config) =
        unlink_vault("bob-cli-sa31-unlink", '/');
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "@agents+implement!",
    );
    assert_eq!(json["kind"], "task_toggle");
    assert_eq!(json["toggle_direction"], "unlink");
    assert_eq!(json["status_symbol"], "/");
    assert_eq!(json["relation_check"], "checked");
    assert!(json.get("task_complete").is_none(), "{json}");
    let row = item_relation_row(&json);
    assert_eq!(row["note_path"], "agents.md");
    assert_eq!(row["block_id"], "evaluate");
    assert_eq!(row["text"], "Prepare evaluation prompt");
    assert_eq!(row["trigger"], "relation");
    assert_eq!(row["offer"]["group"], "relation");
    assert_eq!(row["offer"]["after"], "agents.md#^implement");
    assert_eq!(row["offer"]["selected"], false);
    assert_eq!(row["not_linked"], "not_selected");
    assert_eq!(row["offer"]["destination"]["entry_name"], "BUILD");
    assert_eq!(row["offer"]["destination"]["entry_role"], "slot");
    assert_eq!(row["relation"]["label"], "starts after");
    assert_eq!(row["relation"]["key"], "starts after");
    assert_eq!(row["relation"]["event"], "parked");
    assert_eq!(
        row["relation"]["after"],
        serde_json::json!([{
            "note_path": "agents.md",
            "block_id": "implement",
            "text": "Send implementation prompt",
        }])
    );
    assert_eq!(json["pull_in"]["offer_count"], 1);
    assert_eq!(json["pull_in"]["picked_count"], 0);
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["group"], "relation");
    assert_eq!(groups[0]["event"], "parked");
    assert_eq!(groups[0]["label"], "starts after");

    let human = capture_output(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "@agents+implement!",
        true,
    );
    let text = stdout(&human);
    assert!(
        text.contains(
            "Starts after \u{201c}Send implementation prompt\u{201d}"
        ),
        "{text}"
    );
    assert!(
        text.contains("it is parked (your \u{1faa2} links, not dependents)"),
        "{text}"
    );
}

// SA31 pick: `-p all` links S at T's former slot; dry-run JSON matches
// the real run aside from `dry_run`.
#[test]
fn sa31_pick_links_at_former_slot_with_dry_run_parity() {
    let (_temp, vault, day_file, config) =
        unlink_vault("bob-cli-sa31-pick", '/');
    let dry = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run", "-p", "all"],
        "@agents+implement!",
    );
    let before = vault_bytes(&vault);
    assert_eq!(dry["dry_run"], true);
    let row = item_relation_row(&dry);
    assert_eq!(row["offer"]["selected"], true);
    assert_eq!(row["offer"]["destination"]["entry_role"], "slot");
    let real = capture_json(
        &vault,
        &day_file,
        &config,
        &["-p", "all"],
        "@agents+implement!",
    );
    assert_eq!(real["dry_run"], false);
    assert_ne!(vault_bytes(&vault), before);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    assert_eq!(masked["relation_successors"], real["relation_successors"]);
    assert_eq!(
        masked["pull_in"]["picked_count"],
        real["pull_in"]["picked_count"]
    );
    let note = fs::read_to_string(vault.join("agents.md")).expect("agents");
    assert!(
        note.contains("- [*] #task Prepare evaluation prompt"),
        "{note}"
    );
    assert!(note.contains("[fresh:: 2026-11-02]"), "{note}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(day.contains("[[agents#^evaluate]]"), "{day}");
    assert!(!day.contains("[[agents#^implement]]"), "{day}");
}

// SA32: unlinking a Next or Ready T stays silent.
#[test]
fn sa32_next_or_ready_unlink_is_relation_silent() {
    for (status, name) in [('*', "next"), (' ', "ready")] {
        let (_temp, vault, day_file, config) =
            unlink_vault(&format!("bob-cli-sa32-{name}"), status);
        let json = capture_json(
            &vault,
            &day_file,
            &config,
            &["--dry-run"],
            "@agents+implement!",
        );
        assert_eq!(json["kind"], "task_toggle", "{name}");
        assert_eq!(json["toggle_direction"], "unlink", "{name}");
        assert_item_relation_silent(&json);
        assert_eq!(json["pull_in"]["offer_count"], 0, "{name}");
    }
}

// Later re-linking a parked T via `@route+id!` drops that T's parked group.
#[test]
fn unlink_then_relink_drops_parked_group() {
    let (_temp, vault, day_file, config) =
        unlink_vault("bob-cli-sa31-relink", '/');
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run", "-p", "all"],
        "@agents+implement!\n\n@agents+implement!",
    );
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2, "{captures:?}");
    assert!(
        captures[0].get("relation_successors").is_none(),
        "relinked T drops its parked group: {}",
        captures[0]
    );
    assert_eq!(captures[1]["toggle_direction"], "link");
    assert_eq!(json["pull_in"]["offer_count"], 0);
}

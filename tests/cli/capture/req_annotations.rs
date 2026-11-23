//! Additive `req: {depth, state}` on capture preview block lines and
//! idle-agenda lines, computed by the recognizer.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path};

const NOW: &str = "2026-06-15";
const AGENDA_NOW: &str = "2026-08-28 09:10:00";

fn capture_json(vault: &Path, args: &[&str], draft: &str) -> Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .args(args)
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn line_named<'a>(block: &'a Value, needle: &str) -> &'a Value {
    block["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .find(|row| {
            row["text"]
                .as_str()
                .is_some_and(|text| text.contains(needle))
        })
        .unwrap_or_else(|| panic!("missing line {needle}: {block}"))
}

fn assert_req(row: &Value, depth: u64, state: &str) {
    assert_eq!(row["req"]["depth"], depth, "{row}");
    assert_eq!(row["req"]["state"], state, "{row}");
}

fn assert_no_req(row: &Value) {
    assert!(row.get("req").is_none(), "unexpected req on {row}");
}

#[test]
fn new_task_preview_annotates_requirement_depths() {
    let temp = TempDir::new("bob-cli-req-annotations-new");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_capture_task_settings(&vault);
    write_file(&vault.join("work.md"), "");
    // A single new-task item writes children in-line and does not emit
    // `task_blocks`. Create the parent, then capture the reqs as sub-bullets
    // so the created block's lines carry the additive `req` object.
    let draft = concat!(
        "@work^export Fix export\n",
        "\n",
        "@work+export\n",
        "- [ ] Handles CRLF\n",
        "  - [ ] Windows\n",
        "    - [ ] Mixed files\n",
        "      - seen in March\n",
        "- see the ticket",
    );
    let dry = capture_json(&vault, &["--dry-run"], draft);
    let blocks = dry["task_blocks"].as_array().expect("task_blocks");
    assert_eq!(blocks.len(), 1, "{dry}");
    let block = &blocks[0];
    assert_eq!(block["created"], true, "{block}");
    assert_eq!(block["block_id"], "export", "{block}");
    assert_req(line_named(block, "Handles CRLF"), 1, "pending");
    assert_req(line_named(block, "Windows"), 2, "pending");
    assert_req(line_named(block, "Mixed files"), 3, "pending");
    assert_no_req(line_named(block, "seen in March"));
    assert_no_req(line_named(block, "NOTES"));
    assert_no_req(line_named(block, "see the ticket"));
    assert_no_req(line_named(block, "#task Fix export"));
}

#[test]
fn completing_a_task_meets_its_requirements() {
    let temp = TempDir::new("bob-cli-req-annotations-met");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("work.md"),
        concat!(
            "- [ ] #task Fix export [created:: 2026-01-01] ^ex\n",
            "\t- [ ] Handles CRLF\n",
            "\t\t- [ ] Windows\n",
            "\t\t\t- [ ] Mixed files\n",
        ),
    );
    let day = vault.join("20260615.md");
    write_file(
        &day,
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n  - [[work#^ex]]\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("!work:ex")
        .env("BOB_NOW", "2026-06-15 09:15:00")
        .env("BOB_DAY_FILE", &day)
        .output()
        .expect("run complete");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    let block = json["task_blocks"]
        .as_array()
        .expect("task_blocks")
        .iter()
        .find(|block| block["block_id"] == "ex")
        .expect("owner block");
    assert_req(line_named(block, "Handles CRLF"), 1, "met");
    assert_req(line_named(block, "Windows"), 2, "met");
    assert_req(line_named(block, "Mixed files"), 3, "met");
}

#[test]
fn existing_block_annotates_dropped_carried_excluded_and_notes() {
    let temp = TempDir::new("bob-cli-req-annotations-states");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("work.md"),
        concat!(
            "- [ ] #task Existing [created::2026-01-01] ^ex\n",
            "\t- [ ] pending req\n",
            "\t\t- attached note\n",
            "\t- [-] dropped req\n",
            "\t\t- [ ] excluded child\n",
            "\t- [>] [[#^own|carried req]]\n",
            "\t- NOTES\n",
            "\t\t- drawer note\n",
        ),
    );
    let json = capture_json(&vault, &["--dry-run"], "@work+ex\n- extra note");
    let block = &json["task_blocks"].as_array().expect("task_blocks")[0];
    assert_req(line_named(block, "pending req"), 1, "pending");
    assert_req(line_named(block, "dropped req"), 1, "dropped");
    assert_req(line_named(block, "excluded child"), 2, "excluded");
    assert_req(line_named(block, "carried req"), 1, "carried");
    assert_no_req(line_named(block, "attached note"));
    assert_no_req(line_named(block, "NOTES"));
    assert_no_req(line_named(block, "drawer note"));
    assert_no_req(line_named(block, "extra note"));
}

#[test]
fn idle_agenda_annotates_requirement_rows() {
    let temp = TempDir::new("bob-cli-req-annotations-agenda");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Deep fix [created:: 2026-08-20] ^deep-fix\n",
            "\t- [ ] Handles CRLF\n",
            "\t\t- [ ] Windows\n",
            "\t- NOTES\n",
            "\t\t- drawer note\n",
            "\t- a loose note\n",
        ),
    );
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] (**08:00 - 08:30** [t:: 30m]) — FIX\n",
            "\t- [[tasks#^deep-fix]]\n",
        ),
    );
    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-t")
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", AGENDA_NOW)
        .output()
        .expect("run agenda");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("agenda JSON");
    let item = &json["pomodoros"][0]["items"][0];
    assert_eq!(item["block_id"], "deep-fix", "{item}");
    let lines = item["lines"].as_array().expect("item lines");
    let req = lines
        .iter()
        .find(|row| {
            row["text"]
                .as_str()
                .is_some_and(|text| text.contains("Handles CRLF"))
        })
        .expect("depth-1 req");
    assert_eq!(req["kind"], "bullet", "{req}");
    assert_req(req, 1, "pending");
    let nested = lines
        .iter()
        .find(|row| {
            row["text"]
                .as_str()
                .is_some_and(|text| text.contains("Windows"))
        })
        .expect("depth-2 req");
    assert_req(nested, 2, "pending");
    let note = lines
        .iter()
        .find(|row| row["text"] == "a loose note")
        .expect("note bullet");
    assert_no_req(note);
    let drawer = lines
        .iter()
        .find(|row| {
            row["text"]
                .as_str()
                .is_some_and(|text| text.contains("NOTES"))
        })
        .expect("drawer");
    assert_no_req(drawer);
}

//! Several notes on one existing task (phase `note-bullets`).

use crate::support::*;
use std::fs;

fn capture_json(vault: &std::path::Path, draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn capture_error(vault: &std::path::Path, draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run refused capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    serde_json::from_str(stdout(&output).trim()).expect("error JSON")
}

fn write_foo(vault: &std::path::Path, contents: &str) {
    fs::create_dir_all(vault).expect("create vault");
    fs::write(vault.join("foo.md"), contents).expect("write foo.md");
}

fn write_cash(vault: &std::path::Path, contents: &str) {
    fs::create_dir_all(vault).expect("create vault");
    fs::write(vault.join("cash.md"), contents).expect("write cash.md");
}

#[test]
fn worked_example_files_notes_drawers_json_and_human() {
    let temp = TempDir::new("bob-cli-note-bullets-worked");
    let vault = temp.path().join("vault");
    write_foo(
        &vault,
        "- [ ] #task Upgrade Postgres [created::2026-07-31] ^bar\n  - REQUIREMENTS\n    - 2 GB RAM\n",
    );
    let draft = "@foo+bar\n- Ping Alice about the cutover\n- REQUIREMENTS\n  - Postgres 17 minimum\n- FUTURE WORK\n  - Try logical replication\n- Benchmarks look fine\n  - p99 under 40 ms";
    // Human dry-run first: the vault is fresh so FUTURE WORK is new.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("dry run");
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("· 4 notes"), "{text}");
    assert!(text.contains("Ping Alice about the cutover"), "{text}");
    assert!(text.contains("REQUIREMENTS"), "{text}");
    assert!(text.contains("FUTURE WORK"), "{text}");
    assert!(text.contains("new drawer"), "{text}");
    let json = capture_json(&vault, draft);
    assert_eq!(json["kind"], serde_json::json!("sub_bullet"), "{json}");
    assert_eq!(json["text"], serde_json::json!(""), "{json}");
    assert_eq!(json["task_line"], serde_json::json!(""), "{json}");
    assert!(json.get("sub_bullets").is_none(), "{json}");
    let notes = json["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 4, "{json}");
    assert_eq!(
        notes[0]["text"],
        serde_json::json!("Ping Alice about the cutover"),
        "{json}"
    );
    assert!(
        notes[0].get("drawer").expect("drawer null").is_null(),
        "{json}"
    );
    assert_eq!(
        notes[1]["text"],
        serde_json::json!("Postgres 17 minimum"),
        "{json}"
    );
    assert_eq!(
        notes[1]["drawer"]["title"],
        serde_json::json!("REQUIREMENTS"),
        "{json}"
    );
    assert_eq!(
        notes[1]["drawer"]["created"],
        serde_json::json!(false),
        "{json}"
    );
    assert_eq!(
        notes[2]["drawer"]["title"],
        serde_json::json!("FUTURE WORK"),
        "{json}"
    );
    assert_eq!(
        notes[2]["drawer"]["created"],
        serde_json::json!(true),
        "{json}"
    );
    assert_eq!(
        notes[3]["text"],
        serde_json::json!("Benchmarks look fine"),
        "{json}"
    );
    assert!(notes[3].get("sub_bullets").is_some(), "{json}");
    let drawers = json["drawers"].as_array().expect("drawers array");
    assert_eq!(drawers.len(), 2, "{json}");
    assert_eq!(
        drawers[0]["title"],
        serde_json::json!("REQUIREMENTS"),
        "{json}"
    );
    assert_eq!(
        drawers[1]["title"],
        serde_json::json!("FUTURE WORK"),
        "{json}"
    );
    assert!(json.get("drawer").expect("drawer null").is_null(), "{json}");
    let file = fs::read_to_string(vault.join("foo.md")).expect("foo.md");
    assert_eq!(
        file,
        "- [ ] #task Upgrade Postgres [created::2026-07-31] ^bar\n  - REQUIREMENTS\n    - 2 GB RAM\n    - Postgres 17 minimum\n  - Ping Alice about the cutover\n  - FUTURE WORK\n    - Try logical replication\n  - Benchmarks look fine\n    - p99 under 40 ms\n",
        "file mismatch"
    );
}

#[test]
fn inserts_before_managed_logs_in_draft_order() {
    let temp = TempDir::new("bob-cli-note-bullets-logs");
    let vault = temp.path().join("vault");
    write_cash(
        &vault,
        "- [ ] #task Parent ^parent\n\t- keep me\n\t- \u{1f5d3}\u{fe0f} **SCHEDULE LOG**\n\t\t- *2026-08-01* \u{2014} keep this entry\n",
    );
    let json = capture_json(&vault, "@cash+parent\n- first\n- second");
    assert_eq!(json["notes"].as_array().expect("notes").len(), 2, "{json}");
    let file = fs::read_to_string(vault.join("cash.md")).expect("cash.md");
    let first = file.find("- first").expect("first");
    let second = file.find("- second").expect("second");
    let log = file.find("SCHEDULE LOG").expect("log");
    assert!(first < second && second < log, "{file}");
}

#[test]
fn two_bullets_naming_one_missing_drawer_merge() {
    let temp = TempDir::new("bob-cli-note-bullets-merge");
    let vault = temp.path().join("vault");
    write_cash(&vault, "- [ ] #task Parent ^parent\n");
    let json = capture_json(
        &vault,
        "@cash+parent\n- #future-work\n  - first\n- #future-work\n  - second",
    );
    let drawers = json["drawers"].as_array().expect("drawers");
    assert_eq!(drawers.len(), 1, "{json}");
    assert_eq!(drawers[0]["created"], serde_json::json!(true), "{json}");
    let notes = json["notes"].as_array().expect("notes");
    assert_eq!(notes.len(), 2, "{json}");
    assert_eq!(
        notes[0]["drawer"]["created"],
        serde_json::json!(true),
        "{json}"
    );
    assert_eq!(
        notes[1]["drawer"]["created"],
        serde_json::json!(false),
        "{json}"
    );
    let file = fs::read_to_string(vault.join("cash.md")).expect("cash.md");
    assert_eq!(file.matches("FUTURE WORK").count(), 1, "{file}");
}

#[test]
fn childless_literal_matching_existing_drawer_is_refused() {
    let temp = TempDir::new("bob-cli-note-bullets-childless");
    let vault = temp.path().join("vault");
    write_cash(
        &vault,
        "- [ ] #task Parent ^parent\n\t- REQUIREMENTS\n\t\t- existing\n",
    );
    let err = capture_error(&vault, "@cash+parent\n- REQUIREMENTS");
    assert_eq!(
        err["code"],
        serde_json::json!("empty_drawer_bullet"),
        "{err}"
    );
}

#[test]
fn reserved_drawer_is_refused() {
    let temp = TempDir::new("bob-cli-note-bullets-reserved");
    let vault = temp.path().join("vault");
    write_cash(&vault, "- [ ] #task Parent ^parent\n");
    let err = capture_error(&vault, "@cash+parent\n- #schedule-log\n  - note");
    assert_eq!(err["code"], serde_json::json!("reserved_drawer"), "{err}");
}

#[test]
fn glued_drawer_files_every_bullet_inside() {
    let temp = TempDir::new("bob-cli-note-bullets-glued");
    let vault = temp.path().join("vault");
    write_cash(&vault, "- [ ] #task Parent ^parent\n");
    let json =
        capture_json(&vault, "@cash+parent#requirements\n- first\n- second");
    assert_eq!(json["notes"].as_array().expect("notes").len(), 2, "{json}");
    assert_eq!(
        json["drawer"]["title"],
        serde_json::json!("REQUIREMENTS"),
        "{json}"
    );
    let file = fs::read_to_string(vault.join("cash.md")).expect("cash.md");
    assert!(file.contains("REQUIREMENTS"), "{file}");
    assert!(
        file.contains("- first") && file.contains("- second"),
        "{file}"
    );
}

#[test]
fn same_line_note_still_nests() {
    let temp = TempDir::new("bob-cli-note-bullets-same-line");
    let vault = temp.path().join("vault");
    write_cash(&vault, "- [ ] #task Parent ^parent\n");
    let json = capture_json(
        &vault,
        "@cash+parent a same-line note\n- REQUIREMENTS\n  - nested",
    );
    assert_eq!(json["kind"], serde_json::json!("sub_bullet"), "{json}");
    assert_eq!(
        json["text"],
        serde_json::json!("a same-line note"),
        "{json}"
    );
    assert!(json.get("notes").is_none(), "{json}");
}

#[test]
fn same_line_shortcut_reports_drawer_position() {
    let temp = TempDir::new("bob-cli-note-bullets-same-line-pos");
    let vault = temp.path().join("vault");
    write_cash(&vault, "- [ ] #task Parent ^parent\n");
    let err = capture_error(
        &vault,
        "@cash+parent a same-line note\n- #future-work\n  - nested",
    );
    assert_eq!(
        err["code"],
        serde_json::json!("drawer_bullet_position"),
        "{err}"
    );
}

#[test]
fn retype_applies_once_for_all_notes() {
    let temp = TempDir::new("bob-cli-note-bullets-retype");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    let config = temp.path().join("config.yml");
    write_toggle_task_settings(&vault);
    write_cash(&vault, "- [ ] #task Parent ^parent\n");
    fs::write(&config, "plan:\nthemes:\n  FIX: blue\n").expect("config");
    fs::create_dir_all(day_file.parent().expect("day dir")).expect("day dir");
    fs::write(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} CURRENT\n  - context\n",
    )
    .expect("day");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+parent #fix\n- first\n- second")
        .env("BOB_NOW", "2026-06-15")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert!(json.get("task_theme").is_some(), "{json}");
    let file = fs::read_to_string(vault.join("cash.md")).expect("cash.md");
    assert!(file.contains("[type:: FIX]"), "{file}");
    assert!(
        file.contains("- first") && file.contains("- second"),
        "{file}"
    );
}

#[test]
fn amp_owner_keeps_target_as_owner() {
    let temp = TempDir::new("bob-cli-note-bullets-owner");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("vault");
    fs::write(
        vault.join("cash.md"),
        "- [ ] #task Parent ^parent\n- [ ] #task Dep ^dep\n",
    )
    .expect("cash");
    write_blocked_tasks_settings(&vault);
    let json =
        capture_json(&vault, "@cash+parent &cash:dep\n- first\n- second");
    assert_eq!(json["notes"].as_array().expect("notes").len(), 2, "{json}");
    let file = fs::read_to_string(vault.join("cash.md")).expect("cash.md");
    assert!(
        file.contains("- first") && file.contains("- second"),
        "{file}"
    );
}

#[test]
fn single_note_markers_are_refused() {
    for (marker, code) in [
        ("s:3", "multi_note_marker"),
        ("$2", "multi_note_marker"),
        ("%c", "multi_note_marker"),
    ] {
        let temp =
            TempDir::new(&format!("bob-cli-note-bullets-marker-{marker}"));
        let vault = temp.path().join("vault");
        write_cash(&vault, "- [ ] #task Parent ^parent\n");
        let draft = format!("@cash+parent {marker}\n- first");
        let err = capture_error(&vault, &draft);
        assert_eq!(err["code"], serde_json::json!(code), "{err} {marker}");
    }
}

#[test]
fn batch_rollback_keeps_atomicity() {
    let temp = TempDir::new("bob-cli-note-bullets-rollback");
    let vault = temp.path().join("vault");
    write_cash(&vault, "- [ ] #task Parent ^parent\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+parent\n- good note\n\nbad @cash+missing")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("batch");
    assert!(!output.status.success(), "{}", format_output(&output));
    let file = fs::read_to_string(vault.join("cash.md")).expect("cash.md");
    assert!(!file.contains("good note"), "{file}");
}

//! Link-guard pull-forward behavior for scheduled project residents.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-10-12 09:20:00";

fn capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    draft: &str,
) -> Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn assert_pulled_forward(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    json: &Value,
) {
    assert!(json.get("link_guard").is_none(), "{json}");
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["removed_scheduled"], "2026-10-20");
    let task = fs::read_to_string(vault.join("cash.md")).expect("read project");
    assert!(task.contains("[*] #task Scheduled resident"), "{task}");
    assert!(!task.contains("[scheduled:: 2026-10-20]"), "{task}");
    let day = fs::read_to_string(day_file).expect("read daily note");
    assert!(day.contains("[[cash#^resident]]"), "{day}");
}

#[test]
fn link_toggle_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-toggle-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);

    let json = capture_json(&vault, &day_file, "@cash+resident!");
    assert_eq!(json["toggle_direction"], "link", "{json}");
    assert_pulled_forward(&vault, &day_file, &json);
}

#[test]
fn pomodoro_link_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-link-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);

    let json = capture_json(&vault, &day_file, "@cash:resident");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(json["pomodoro_link_action"], "linked", "{json}");
    assert_pulled_forward(&vault, &day_file, &json);
}

#[test]
fn ensure_next_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-ensure-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^resident]]\n",
        ),
    );

    let json = capture_json(&vault, &day_file, "@cash+resident");
    assert_eq!(json["toggle_behavior"], "ensure_next", "{json}");
    assert_pulled_forward(&vault, &day_file, &json);
}

#[test]
fn close_link_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-close-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
        ),
    );

    let json = capture_json(&vault, &day_file, "@cash:resident=x");
    assert!(json.get("pomodoro_close").is_some(), "{json}");
    assert!(json.get("link_guard").is_none(), "{json}");
    let task = fs::read_to_string(vault.join("cash.md")).expect("read project");
    assert!(!task.contains("[scheduled:: 2026-10-20]"), "{task}");
    let day = fs::read_to_string(day_file).expect("read daily note");
    assert!(day.contains("[[cash#^resident]]"), "{day}");
}

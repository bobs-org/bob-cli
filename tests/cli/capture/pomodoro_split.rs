//! Capture `=/` split end to end.

use crate::support::*;
use std::fs;

fn write_vault(vault: &std::path::Path, day_name: &str, day_body: &str) {
    fs::create_dir_all(vault.join("2026")).expect("create vault year");
    write_file(
        &vault.join(format!("2026/{day_name}")),
        day_body,
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Build task [type:: BUILD] ^a1\n",
            "- [ ] #task Fix task [type:: FIX] ^a2\n",
            "- [ ] #task Read task [type:: READ] ^a3\n",
        ),
    );
}

fn day_body_three() -> &'static str {
    concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] (**0900-1000**) — BUILD + FIX + READ\n",
        "  - [[tasks#^a1]]\n",
        "  - [[tasks#^a2]]\n",
        "  - [[tasks#^a3]]\n",
    )
}

#[test]
fn capture_split_all_moves_each_type_to_new_entries() {
    let temp = TempDir::new("bob-cli-capture-split-all");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=/")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run split capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_split");
    assert_eq!(json["pomodoro_split"]["before"], "BUILD + FIX + READ");
    assert_eq!(json["pomodoro_split"]["after"], "BUILD");
    assert_eq!(json["pomodoro_split"]["stayed_links"], 1);
    let created = json["pomodoro_split"]["created"]
        .as_array()
        .expect("created array");
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["name"], "FIX");
    assert_eq!(created[0]["moved_links"], 1);
    assert_eq!(created[1]["name"], "READ");
    assert_eq!(created[1]["moved_links"], 1);
}

#[test]
fn capture_split_selected_moves_only_listed_type() {
    let temp = TempDir::new("bob-cli-capture-split-selected");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=/build#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run split capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["pomodoro_split"]["after"], "BUILD + READ");
    let created = json["pomodoro_split"]["created"]
        .as_array()
        .expect("created array");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["name"], "FIX");
    assert_eq!(created[0]["moved_links"], 1);
    assert_eq!(json["pomodoro_split"]["stayed_links"], 2);
    let day = fs::read_to_string(vault.join("2026/20261101.md"))
        .expect("read day file untouched by dry-run");
    assert!(day.contains("BUILD + FIX + READ"), "{day}");
}

#[test]
fn capture_split_writes_real_commit_and_human_output() {
    let temp = TempDir::new("bob-cli-capture-split-commit");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=/build#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run split capture");
    assert_success(&output);
    let day = fs::read_to_string(vault.join("2026/20261101.md"))
        .expect("read day file");
    assert!(day.contains("- [ ] (**0900-1000**) — BUILD + READ"), "{day}");
    assert!(day.contains("- [ ] () — FIX"), "{day}");
    assert!(day.contains("[[tasks#^a2]]"), "{day}");
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=/")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run split human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("split"), "{text}");
    assert!(text.contains("BUILD"), "{text}");
}

#[test]
fn capture_split_guards_teach_precisely() {
    let temp = TempDir::new("bob-cli-capture-split-guards");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-1000**) — BUILD + FIX\n",
            "  - [[tasks#^a1]]\n",
            "  - [[tasks#^a2]]\n",
        ),
    );
    let two_type = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=/build#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run two-type explicit split");
    assert!(!two_type.status.success(), "explicit type on two-type must fail");
    let err_json: serde_json::Value =
        serde_json::from_str(stdout(&two_type).trim())
            .expect("split guard JSON");
    assert_eq!(err_json["code"], "split_type_not_needed");
    let missing = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=/nope")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run missing base split");
    assert!(!missing.status.success());
    let missing_json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim())
            .expect("split missing JSON");
    assert_eq!(missing_json["code"], "split_target_missing");
}

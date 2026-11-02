//! Capture `=*` merge end to end.

use crate::support::*;
use std::fs;

fn write_vault(vault: &std::path::Path, day_name: &str, day_body: &str) {
    fs::create_dir_all(vault.join("2026")).expect("create vault year");
    write_file(&vault.join(format!("2026/{day_name}")), day_body);
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
        "- [ ] (**0900-1000**) — BUILD\n",
        "  - [[tasks#^a1]]\n",
        "- [ ] () — FIX\n",
        "  - [[tasks#^a2]]\n",
        "- [ ] () — READ\n",
        "  - [[tasks#^a3]]\n",
    )
}

#[test]
fn capture_merge_absorbs_planned_dests_into_src() {
    let temp = TempDir::new("bob-cli-capture-merge-all");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*build#fix#read")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run merge capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_merge");
    assert_eq!(json["pomodoro_merge"]["before"], "BUILD");
    assert_eq!(json["pomodoro_merge"]["after"], "BUILD + FIX + READ");
    let absorbed = json["pomodoro_merge"]["absorbed"]
        .as_array()
        .expect("absorbed array");
    assert_eq!(absorbed.len(), 2);
    assert_eq!(absorbed[0]["name"], "FIX");
    assert_eq!(absorbed[1]["name"], "READ");
}

#[test]
fn capture_merge_writes_and_drops_duplicate_links() {
    let temp = TempDir::new("bob-cli-capture-merge-commit");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "  - [[tasks#^a1]]\n",
            "  - [[tasks#^a2]]\n",
            "- [ ] () — FIX\n",
            "  - [[tasks#^a2]]\n",
            "  - [[tasks#^a3]]\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=*build#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run merge write");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["pomodoro_merge"]["after"], "BUILD + FIX");
    let absorbed = json["pomodoro_merge"]["absorbed"]
        .as_array()
        .expect("absorbed");
    assert_eq!(absorbed[0]["dropped_duplicates"], 1);
    let day = fs::read_to_string(vault.join("2026/20261101.md")).expect("day");
    assert!(day.contains("BUILD + FIX"), "{day}");
    assert!(!day.contains("- [ ] () — FIX\n"), "{day}");
    let a2_count = day.matches("[[tasks#^a2]]").count();
    assert_eq!(a2_count, 1, "{day}");
    assert!(day.contains("[[tasks#^a3]]"), "{day}");
}

#[test]
fn capture_merge_guards_teach_precisely() {
    let temp = TempDir::new("bob-cli-capture-merge-guards");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let same = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*build#build")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("same pomodoro");
    assert!(!same.status.success());
    let same_json: serde_json::Value =
        serde_json::from_str(stdout(&same).trim()).expect("json");
    assert_eq!(same_json["code"], "merge_same_pomodoro");

    let timed = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*fix#build")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("timed dest");
    assert!(!timed.status.success());
    let timed_json: serde_json::Value =
        serde_json::from_str(stdout(&timed).trim()).expect("json");
    assert_eq!(timed_json["code"], "merge_timed_pomodoro");
    let err = timed_json["error"].as_str().unwrap_or("");
    assert!(err.contains("=*build#fix"), "{err}");

    let missing = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*build#nope")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("missing dest");
    assert!(!missing.status.success());
    let missing_json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("json");
    assert_eq!(missing_json["code"], "merge_target_missing");
}

#[test]
fn capture_merge_human_output_names_absorbed() {
    let temp = TempDir::new("bob-cli-capture-merge-human");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=*build#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("human merge");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("would merge") || text.contains("merged"),
        "{text}"
    );
    assert!(text.contains("FIX"), "{text}");
    assert!(text.contains("BUILD + FIX"), "{text}");
}

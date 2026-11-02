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
    assert_eq!(json["pomodoro_merge"]["source_role"], "named");
    assert!(json["pomodoro_merge"].get("explicit").is_none());
    let absorbed = json["pomodoro_merge"]["absorbed"]
        .as_array()
        .expect("absorbed array");
    assert_eq!(absorbed.len(), 2);
    assert_eq!(absorbed[0]["name"], "FIX");
    assert_eq!(absorbed[0]["role"], "named");
    assert_eq!(absorbed[1]["name"], "READ");
    assert_eq!(absorbed[1]["role"], "named");
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
    assert!(
        !text.contains("same as"),
        "fully typed merge stays unchanged: {text}"
    );
}

#[test]
fn bare_merge_absorbs_the_next_planned_and_reports_its_spelling() {
    let temp = TempDir::new("bob-cli-capture-merge-next");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("bare merge capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["before"], "BUILD");
    assert_eq!(merge["after"], "BUILD + FIX");
    assert_eq!(merge["source_role"], "current");
    assert_eq!(merge["explicit"], "=*build#fix");
    assert_eq!(merge["absorbed"][0]["name"], "FIX");
    assert_eq!(merge["absorbed"][0]["role"], "next");

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("human bare merge");
    assert_success(&human);
    assert!(
        stdout(&human)
            .contains("same as =*build#fix · current BUILD, next FIX"),
        "{}",
        stdout(&human)
    );

    let named_destination = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("implied source merge");
    assert_success(&named_destination);
    let named_json: serde_json::Value =
        serde_json::from_str(stdout(&named_destination).trim()).expect("json");
    assert_eq!(named_json["pomodoro_merge"]["source_role"], "current");
    assert_eq!(named_json["pomodoro_merge"]["explicit"], "=*build#fix");
    assert_eq!(named_json["pomodoro_merge"]["absorbed"][0]["role"], "named");
}

#[test]
fn repeated_bare_merge_grows_one_planned_entry_at_a_time() {
    let temp = TempDir::new("bob-cli-capture-merge-next-twice");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    for expected in ["BUILD + FIX", "BUILD + FIX + READ"] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("=*")
            .env("BOB_NOW", "2026-11-01 09:30")
            .output()
            .expect("write bare merge");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["pomodoro_merge"]["after"], expected);
    }
}

#[test]
fn bare_merge_uses_next_up_without_a_running_session_and_skips_meet() {
    let temp = TempDir::new("bob-cli-capture-merge-next-up");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — PLAN\n",
            "- [ ] () — MEET\n",
            "- [ ] () — FIX\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("next-up merge");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["source_role"], "next_up");
    assert_eq!(merge["before"], "PLAN");
    assert_eq!(merge["after"], "PLAN + FIX");
    assert_eq!(merge["absorbed"][0]["role"], "next");
}

#[test]
fn explicit_source_uses_the_next_entry_below_it_without_wrapping() {
    let temp = TempDir::new("bob-cli-capture-merge-below-source");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — BUILD\n",
            "- [ ] () — FIX\n",
            "- [ ] () — READ\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("explicit source bare merge");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["before"], "FIX");
    assert_eq!(merge["after"], "FIX + READ");
    assert_eq!(merge["source_role"], "named");
    assert_eq!(merge["absorbed"][0]["name"], "READ");
    assert_eq!(merge["absorbed"][0]["role"], "next");
}

#[test]
fn bare_merge_teaches_missing_next_entry_and_omits_unsafe_spelling() {
    let temp = TempDir::new("bob-cli-capture-merge-next-missing");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!("## Pomodoros\n\n", "- [ ] () — FIX\n", "- [ ] () — READ\n",),
    );
    let missing = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*read")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("missing next entry");
    assert!(!missing.status.success());
    let error: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("error JSON");
    assert_eq!(error["code"], "merge_next_missing");
    assert!(error["error"].as_str().unwrap().contains("`=*read#fix`"));

    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — FIX\n",
            "- [ ] () — READ\n",
            "- [ ] () — MEET\n",
        ),
    );
    let meet_only_below = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*read")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("MEET passed during missing merge");
    assert!(!meet_only_below.status.success());
    let meet_json: serde_json::Value =
        serde_json::from_str(stdout(&meet_only_below).trim()).expect("json");
    assert!(meet_json["error"]
        .as_str()
        .unwrap()
        .contains("MEET never merges"));

    write_vault(&vault, "20261101.md", "## Pomodoros\n\n- [ ] () — PLAN\n");
    let no_other_planned = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("no next planned entry");
    assert!(!no_other_planned.status.success());
    let no_other_json: serde_json::Value =
        serde_json::from_str(stdout(&no_other_planned).trim()).expect("json");
    assert_eq!(no_other_json["code"], "merge_next_missing");
    assert!(no_other_json["error"]
        .as_str()
        .unwrap()
        .ends_with("Nothing planned below PLAN to merge"));

    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — FIX\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "- [ ] () — FIX\n",
        ),
    );
    let ambiguous_name = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("duplicate destination name");
    assert_success(&ambiguous_name);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&ambiguous_name).trim())
            .expect("capture JSON");
    assert!(json["pomodoro_merge"].get("explicit").is_none());
}

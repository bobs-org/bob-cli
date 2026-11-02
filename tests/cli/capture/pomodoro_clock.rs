//! Counted-links clock rule: starts, closes, and running lineup edits.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/pomodoro_clock/vectors.json"
    ))
    .expect("pomodoro clock vectors")
}

fn run_capture(
    vault: &Path,
    day_file: &Path,
    now: &str,
    token: &str,
    dry_run: bool,
) -> Value {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("--dry-run");
    }
    let output = command
        .arg("--")
        .arg(token)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", now)
        .output()
        .expect("run pomodoro clock capture");
    assert!(
        output.status.success(),
        "token {token:?} dry_run={dry_run}: {}",
        format_output(&output)
    );
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn session_off_clock(json: &Value) -> bool {
    json.get("pomodoro_start")
        .or_else(|| json.get("pomodoro_close"))
        .and_then(|session| session.get("off_clock"))
        .or_else(|| {
            json.get("pomodoro_blocks")
                .and_then(Value::as_array)
                .and_then(|blocks| blocks.first())
                .and_then(|block| block.get("off_clock"))
        })
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn assert_warning(json: &Value, id: &str, expected: Option<&str>) {
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    match expected {
        Some(code) => assert!(
            warnings.iter().any(|warning| warning["code"] == code),
            "{id}: missing {code}: {warnings:?}"
        ),
        None => assert!(
            warnings
                .iter()
                .all(|warning| warning["code"] != "pomodoro_clock_mixed"),
            "{id}: unexpected mixed warning: {warnings:?}"
        ),
    }
}

fn write_case(vault: &Path, case: &Value) {
    write_toggle_task_settings(vault);
    let day_file = vault.join("2026/20261101.md");
    write_file(&day_file, case["day"].as_str().expect("day"));
    if let Some(notes) = case.get("notes").and_then(Value::as_object) {
        for (relative, contents) in notes {
            write_file(
                &vault.join(relative),
                contents.as_str().expect("note contents"),
            );
        }
    }
}

#[test]
fn capture_matches_shared_pomodoro_clock_vectors() {
    for case in vectors()["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().expect("case id");
        let temp = TempDir::new(&format!("bob-cli-pomodoro-clock-{id}"));
        let vault = temp.path().join("vault");
        write_case(&vault, case);
        let day_file = vault.join("2026/20261101.md");
        let now = case["now"].as_str().expect("now");
        let token = case["token"].as_str().expect("token");
        let json = run_capture(&vault, &day_file, now, token, false);
        assert_eq!(
            fs::read_to_string(&day_file).expect("written day"),
            case["expected_day"].as_str().expect("expected day"),
            "{id}: {json}"
        );
        assert_eq!(
            session_off_clock(&json),
            case["expected_off_clock"].as_bool().unwrap_or(false),
            "{id}: {json}"
        );
        assert_warning(
            &json,
            id,
            case.get("expected_warning").and_then(Value::as_str),
        );
    }
}

#[test]
fn dry_run_matches_vector_json_and_writes_nothing() {
    for case in vectors()["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().expect("case id");
        let temp = TempDir::new(&format!("bob-cli-pomodoro-clock-dry-{id}"));
        let vault = temp.path().join("vault");
        write_case(&vault, case);
        let day_file = vault.join("2026/20261101.md");
        let before = fs::read_to_string(&day_file).expect("day before");
        let json = run_capture(
            &vault,
            &day_file,
            case["now"].as_str().expect("now"),
            case["token"].as_str().expect("token"),
            true,
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("day after dry-run"),
            before,
            "{id} dry-run wrote the daily note"
        );
        assert_eq!(
            session_off_clock(&json),
            case["expected_off_clock"].as_bool().unwrap_or(false),
            "{id} dry-run: {json}"
        );
        assert_warning(
            &json,
            id,
            case.get("expected_warning").and_then(Value::as_str),
        );
    }
}

#[test]
fn restart_keeps_an_all_off_clock_lineup_off_the_clock() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-restart");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n- [ ] #task Walk the dog #offclock ^walk\n",
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0800-0900** [t:: 60m]) — WALK\n",
            "  - [[bob#^walk]]\n",
        ),
    );
    let json =
        run_capture(&vault, &day_file, "2026-11-01 09:02:00", "=", false);
    let start = &json["pomodoro_start"];
    assert_eq!(start["off_clock"], true, "{json}");
    assert_eq!(start["time_range"], "(**0905-0930** [off:: 25m])", "{json}");
    let day = fs::read_to_string(&day_file).expect("restarted day");
    assert!(
        day.contains("- [ ] (**0905-0930** [off:: 25m]) — WALK\n"),
        "{day}"
    );
}

#[test]
fn link_start_stamps_an_off_clock_new_task() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-link-start");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n- [ ] #task Walk the dog #offclock ^walk\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — WALK\n");
    let json = run_capture(
        &vault,
        &day_file,
        "2026-11-01 09:02:00",
        "^bob:walk=",
        false,
    );
    let start = &json["pomodoro_start"];
    assert_eq!(start["off_clock"], true, "{json}");
    let day = fs::read_to_string(&day_file).expect("started day");
    assert!(
        day.contains("- [ ] (**0905-0930** [off:: 25m]) — WALK\n"),
        "{day}"
    );
    assert!(day.contains("[[bob#^walk]]"), "{day}");
}

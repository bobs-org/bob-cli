//! Counted-links clock rule: starts, closes, and running lineup edits.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

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

fn run_capture_with_config(
    vault: &Path,
    day_file: &Path,
    config: &Path,
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
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", now)
        .output()
        .expect("run configured pomodoro clock capture");
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
                .and_then(|blocks| {
                    blocks.iter().find(|block| block["removed"] != true)
                })
                .and_then(|block| block.get("off_clock"))
        })
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn session_clock_note(json: &Value) -> Option<&str> {
    json.get("pomodoro_start")
        .or_else(|| json.get("pomodoro_close"))
        .and_then(|session| session.get("clock_note"))
        .and_then(Value::as_str)
}

fn assert_clock_note(json: &Value, id: &str, expected: Option<&str>) {
    assert_eq!(session_clock_note(json), expected, "{id}: {json}");
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

/// The case's `config` member as a config file, when present. The JSON
/// object is written verbatim: JSON is valid YAML, so the bob config
/// loader reads it as the bob config document (for example
/// `{"plan": {"default_type": "CHORE"}}`). When `config` is absent no
/// config file is written.
fn write_case_config(temp: &Path, case: &Value) -> Option<PathBuf> {
    let config = case.get("config")?;
    let path = temp.join("config.yml");
    write_file(&path, &serde_json::to_string(config).expect("config JSON"));
    Some(path)
}

fn run_case_capture(
    vault: &Path,
    day_file: &Path,
    config: Option<&Path>,
    now: &str,
    token: &str,
    dry_run: bool,
) -> Value {
    match config {
        Some(path) => {
            run_capture_with_config(vault, day_file, path, now, token, dry_run)
        }
        None => run_capture(vault, day_file, now, token, dry_run),
    }
}

#[test]
fn capture_matches_shared_pomodoro_clock_vectors() {
    for case in vectors()["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().expect("case id");
        let temp = TempDir::new(&format!("bob-cli-pomodoro-clock-{id}"));
        let vault = temp.path().join("vault");
        write_case(&vault, case);
        let config = write_case_config(temp.path(), case);
        let day_file = vault.join("2026/20261101.md");
        let now = case["now"].as_str().expect("now");
        let token = case["token"].as_str().expect("token");
        let json = run_case_capture(
            &vault,
            &day_file,
            config.as_deref(),
            now,
            token,
            false,
        );
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
        assert_clock_note(
            &json,
            id,
            case.get("expected_clock_note").and_then(Value::as_str),
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
        let config = write_case_config(temp.path(), case);
        let day_file = vault.join("2026/20261101.md");
        let before = fs::read_to_string(&day_file).expect("day before");
        let json = run_case_capture(
            &vault,
            &day_file,
            config.as_deref(),
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
        assert_clock_note(
            &json,
            id,
            case.get("expected_clock_note").and_then(Value::as_str),
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

fn assert_dry_run_matches_write(
    vault: &Path,
    day_file: &Path,
    now: &str,
    token: &str,
    expected_line: &str,
    expected_off_clock: bool,
    expected_warning: Option<&str>,
) {
    let before = fs::read_to_string(day_file).expect("day before");
    let dry = run_capture(vault, day_file, now, token, true);
    assert_eq!(
        fs::read_to_string(day_file).expect("day after dry-run"),
        before,
        "{token} dry-run wrote the daily note"
    );
    let written = run_capture(vault, day_file, now, token, false);
    assert_eq!(
        dry["pomodoro_blocks"], written["pomodoro_blocks"],
        "{token}: dry-run and write pomodoro blocks differ\ndry: {dry}\nwrite: {written}"
    );
    assert_eq!(
        dry["warnings"], written["warnings"],
        "{token}: dry-run and write warnings differ\ndry: {dry}\nwrite: {written}"
    );
    assert_eq!(session_off_clock(&dry), expected_off_clock, "{dry}");
    assert_eq!(session_off_clock(&written), expected_off_clock, "{written}");
    assert_warning(&dry, token, expected_warning);
    assert_warning(&written, token, expected_warning);
    let day = fs::read_to_string(day_file).expect("written day");
    assert!(
        day.contains(expected_line),
        "expected {expected_line:?} in:\n{day}"
    );
}

fn write_merge_clock_vault(
    vault: &Path,
    running_line: &str,
    running_links: &str,
    planned_name: &str,
    planned_links: &str,
) -> PathBuf {
    write_toggle_task_settings(vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Walk the dog #offclock [type:: WALK] ^walk\n",
            "- [ ] #task Fix the parser [type:: FIX] ^fix\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        &format!(
            "## Pomodoros\n{running_line}{running_links}- [ ] () — {planned_name}\n{planned_links}"
        ),
    );
    day_file
}

#[test]
fn merge_recomputes_running_clock_both_directions_and_explicitly() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-merge");
    let vault = temp.path().join("vault");
    let day_file = write_merge_clock_vault(
        &vault,
        "- [ ] (**0905-0930** [t:: 25m]) — BOB\n",
        "",
        "WALK",
        "  - [[tasks#^walk]]\n",
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "=*",
        "(**0905-0930** [off:: 25m]) — BOB + WALK",
        true,
        None,
    );

    let temp = TempDir::new("bob-cli-pomodoro-clock-merge-mixed");
    let vault = temp.path().join("vault");
    let day_file = write_merge_clock_vault(
        &vault,
        "- [ ] (**0905-0930** [off:: 25m]) — WALK\n",
        "  - [[tasks#^walk]]\n",
        "FIX",
        "  - [[tasks#^fix]]\n",
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "=*#fix",
        "(**0905-0930** [t:: 25m]) — WALK + FIX",
        false,
        Some("pomodoro_clock_mixed"),
    );

    let temp = TempDir::new("bob-cli-pomodoro-clock-merge-explicit");
    let vault = temp.path().join("vault");
    let day_file = write_merge_clock_vault(
        &vault,
        "- [ ] (**0905-0930** [t:: 25m]) — BOB\n",
        "",
        "WALK",
        "  - [[tasks#^walk]]\n",
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "=*#walk",
        "(**0905-0930** [off:: 25m]) — BOB + WALK",
        true,
        None,
    );
}

#[test]
fn merging_planned_entries_keeps_the_session_untimed() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-merge-planned");
    let vault = temp.path().join("vault");
    let day_file = write_merge_clock_vault(
        &vault,
        "- [ ] () — BOB\n",
        "",
        "WALK",
        "  - [[tasks#^walk]]\n",
    );
    let before = fs::read_to_string(&day_file).expect("day before");
    let dry = run_capture(&vault, &day_file, "2026-11-01 09:30:00", "=*", true);
    assert!(!session_off_clock(&dry), "{dry}");
    assert!(dry["warnings"].as_array().is_none_or(Vec::is_empty));
    assert_eq!(fs::read_to_string(&day_file).unwrap(), before);
    let written =
        run_capture(&vault, &day_file, "2026-11-01 09:30:00", "=*", false);
    assert_eq!(dry["pomodoro_blocks"], written["pomodoro_blocks"]);
    assert_eq!(dry["warnings"], written["warnings"]);
    let day = fs::read_to_string(day_file).expect("written day");
    assert!(day.contains("- [ ] () — BOB + WALK"), "{day}");
    assert!(!day.contains("[t::"), "{day}");
    assert!(!day.contains("[off::"), "{day}");
}

#[test]
fn split_recomputes_the_remaining_timed_lineup_clock() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-split-off");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Walk the dog #offclock [type:: WALK] ^walk\n",
            "- [ ] #task Fix the parser [type:: FIX] ^fix\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [t:: 25m]) — WALK + FIX\n",
            "  - [[tasks#^walk]]\n",
            "  - [[tasks#^fix]]\n",
        ),
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "=/",
        "(**0905-0930** [off:: 25m]) — WALK",
        true,
        None,
    );

    let temp = TempDir::new("bob-cli-pomodoro-clock-split-worked");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Build it [type:: BUILD] ^build\n",
            "- [ ] #task Walk the dog #offclock [type:: WALK] ^walk\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [t:: 25m]) — BUILD + WALK\n",
            "  - [[tasks#^build]]\n",
            "  - [[tasks#^walk]]\n",
        ),
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "=/",
        "(**0905-0930** [t:: 25m]) — BUILD",
        false,
        None,
    );
}

#[test]
fn blocked_link_consumption_restamps_the_running_lineup() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-consume");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("bob.md"),
        "- [ ] #task Walk the dog #offclock ^walk\n",
    );
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Open prerequisite ^prep\n",
    );
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [?] #task Project #prj #hide ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
            "- [?] #task Blocked work ^blocked\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [t:: 25m]) — WALK + FIX\n",
            "  - [[bob#^walk]]\n",
            "  - [[proj#^blocked]]\n",
        ),
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "@proj+blocked",
        "(**0905-0930** [off:: 25m]) — WALK + FIX",
        true,
        None,
    );
}

#[test]
fn capture_unlink_restamps_the_running_lineup() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-unlink");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Walk the dog #offclock [type:: WALK] ^walk\n",
            "- [*] #task Fix the parser [type:: FIX] ^fix\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [t:: 25m]) — WALK + FIX\n",
            "  - [[tasks#^walk]]\n",
            "  - [[tasks#^fix]]\n",
        ),
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "@tasks+fix!",
        "(**0905-0930** [off:: 25m]) — WALK + FIX",
        true,
        None,
    );
}

#[test]
fn capture_link_restamps_the_running_lineup() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-link");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Walk the dog #offclock [type:: BOB] ^walk\n",
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0905-0930** [t:: 25m]) — BOB\n",
    );
    assert_dry_run_matches_write(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "@tasks+walk!",
        "(**0905-0930** [off:: 25m]) — BOB",
        true,
        None,
    );
}

#[test]
fn ensure_next_relocation_restamps_the_running_lineup() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-ensure-next");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_toggle_task_settings(&vault);
    write_file(&config, "types:\n  WALK: green\n");
    write_file(
        &vault.join("tasks.md"),
        "- [?] #task Walk the dog #offclock [scheduled:: 2026-11-05] ^walk\n",
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [off:: 25m]) — BOB\n",
            "  - [[tasks#^walk]]\n",
        ),
    );
    let before = fs::read_to_string(&day_file).expect("day before");
    let dry = run_capture_with_config(
        &vault,
        &day_file,
        &config,
        "2026-11-01 09:30:00",
        "@tasks+walk #walk",
        true,
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day after dry-run"),
        before
    );
    let written = run_capture_with_config(
        &vault,
        &day_file,
        &config,
        "2026-11-01 09:30:00",
        "@tasks+walk #walk",
        false,
    );
    assert_eq!(dry["pomodoro_blocks"], written["pomodoro_blocks"]);
    assert_eq!(dry["warnings"], written["warnings"]);
    let day = fs::read_to_string(day_file).expect("written day");
    assert!(
        day.contains("(**0905-0930** [t:: 25m]) — BOB\n"),
        "{day}; dry={dry}; written={written}"
    );
    assert!(
        day.contains("- [ ] () — WALK\n  - [[tasks#^walk]]"),
        "{day}; dry={dry}; written={written}"
    );
}

#[test]
fn completing_a_linked_task_restamps_the_running_lineup() {
    let temp = TempDir::new("bob-cli-pomodoro-clock-complete");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Fix the parser [type:: FIX] ^fix\n",
            "- [ ] #task Walk the dog #offclock [type:: WALK] ^walk\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [off:: 25m]) — WALK\n",
            "  - [[tasks#^walk]]\n",
            "- [ ] () — FIX\n",
            "  - [[tasks#^fix]]\n",
        ),
    );
    let before = fs::read_to_string(&day_file).expect("day before");
    let dry = run_capture(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "!tasks:fix",
        true,
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day after dry-run"),
        before
    );
    let written = run_capture(
        &vault,
        &day_file,
        "2026-11-01 09:30:00",
        "!tasks:fix",
        false,
    );
    assert_eq!(dry["pomodoro_blocks"], written["pomodoro_blocks"]);
    assert_eq!(dry["warnings"], written["warnings"]);
    let day = fs::read_to_string(day_file).expect("written day");
    assert!(
        day.contains("(**0905-0930** [t:: 25m]) — WALK"),
        "{day}; dry={dry}; written={written}"
    );
    assert!(
        day.contains(
            "- [ ] (**0905-0930** [t:: 25m]) — WALK\n  - [[tasks#^walk]]\n  - ~~[[tasks#^fix]]~~"
        ),
        "{day}"
    );
}

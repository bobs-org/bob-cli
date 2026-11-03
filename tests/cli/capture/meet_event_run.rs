//! MEET starts follow their next event run and the off-clock rule.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/meet_event_run/vectors.json"
    ))
    .expect("MEET event-run vectors")
}

fn run_start(vault: &Path, day_file: &Path, now: &str, token: &str) -> Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(token)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", now)
        .output()
        .expect("run MEET start");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("start JSON")
}

#[test]
fn capture_start_matches_shared_meet_event_run_vectors() {
    for case in vectors()["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().expect("case id");
        let temp = TempDir::new(&format!("bob-cli-meet-run-{id}"));
        let vault = temp.path().join("vault");
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
        let json = run_start(
            &vault,
            &day_file,
            case["now"].as_str().expect("now"),
            case["token"].as_str().expect("token"),
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("started day"),
            case["expected_day"].as_str().expect("expected day"),
            "{id}: {json}"
        );
        let start = &json["pomodoro_start"];
        assert_eq!(
            start
                .get("off_clock")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            case["expected_off_clock"],
            "{id}: {start}"
        );
        assert_eq!(
            format!(
                "{}-{}",
                start["start"].as_str().expect("start"),
                start["end"].as_str().expect("end")
            ),
            case["expected_span"].as_str().expect("expected span"),
            "{id}: {start}"
        );
        match case.get("expected_source").and_then(Value::as_str) {
            Some(source) => assert_eq!(
                start["event_run"]["source"], source,
                "{id}: {start}"
            ),
            None => assert!(
                start.get("event_run").is_none(),
                "{id} must not report an event run: {start}"
            ),
        }
        let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
        match case.get("expected_warning").and_then(Value::as_str) {
            Some(code) => assert!(
                warnings.iter().any(|warning| warning["code"] == code),
                "{id}: missing {code}: {warnings:?}"
            ),
            None => assert!(
                warnings.iter().all(
                    |warning| warning["code"] != "pomodoro_event_run_later"
                ),
                "{id}: unexpected run warning: {warnings:?}"
            ),
        }
        assert_eq!(
            start.get("clock_note").and_then(Value::as_str),
            case.get("expected_clock_note").and_then(Value::as_str),
            "{id}: {start}"
        );
    }
}

const LUNCH_DAY: &str = concat!(
    "## Events\n",
    "- [ ] #task **1200-1300** Lunch with Kelly #untracked [root:: love] ^lunch\n",
    "- [ ] #task **1300-1330** Standup ^standup\n",
    "\n",
    "## Pomodoros\n",
    "- [x] (**0900-0925** [t:: 25m]) — BOB\n",
    "- [ ] () — MEET\n",
    "  - [[#^lunch]]\n",
    "  - [[#^standup]]\n",
    "- [ ] () — BOB\n",
);

fn lunch_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&day_file, LUNCH_DAY);
    (temp, vault, day_file)
}

#[test]
fn start_json_reports_the_event_run_and_off_clock_block() {
    let (_temp, vault, day_file) = lunch_vault("bob-cli-meet-run-json");
    let json = run_start(&vault, &day_file, "2026-11-01 11:55:00", "=");
    let start = &json["pomodoro_start"];
    assert_eq!(start["time_range"], "(**1200-1300** [off:: 60m])");
    assert_eq!(start["duration_minutes"], 60);
    assert_eq!(start["offset_units"], 0);
    assert_eq!(start["tracked"], false);
    assert_eq!(start["off_clock"], true);
    assert_eq!(
        start["event_run"],
        serde_json::json!({
            "time_range": "1200-1300",
            "start": "1200",
            "end": "1300",
            "off_clock": true,
            "untracked": true,
            "source": "calendar",
            "events": [{
                "index": 1,
                "title": "Lunch with Kelly",
                "time_range": "1200-1300",
                "line": 2,
                "block_id": "lunch",
                "off_clock": true,
                "untracked": true
            }]
        })
    );
    let blocks = json["pomodoro_blocks"].as_array().expect("blocks");
    let meet = blocks
        .iter()
        .find(|block| block["name"] == "MEET")
        .expect("MEET block");
    assert_eq!(meet["tracked"], false, "{meet}");
    assert_eq!(meet["off_clock"], true, "{meet}");
    assert_eq!(meet["time_range"], "1200-1300");
}

#[test]
fn tracked_start_omits_tracked_and_event_run_without_events() {
    let temp = TempDir::new("bob-cli-meet-run-omitted");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — MEET\n  - [[sase#^other]]\n",
    );
    let json = run_start(&vault, &day_file, "2026-11-01 11:02:00", "=");
    let start = &json["pomodoro_start"];
    assert!(start.get("tracked").is_none(), "{start}");
    assert!(start.get("event_run").is_none(), "{start}");
    let block = &json["pomodoro_blocks"][0];
    assert!(block.get("tracked").is_none(), "{block}");
}

#[test]
fn human_start_names_off_clock_and_the_calendar_events() {
    let (_temp, vault, day_file) = lunch_vault("bob-cli-meet-run-human");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-01 11:55:00")
        .output()
        .expect("run human start");
    assert_success(&output);
    let rendered = stdout(&output);
    assert!(
        rendered.contains(
            "MEET 1200-1300 (60m · off-clock) at line 7 · from your calendar: Lunch with Kelly"
        ),
        "{rendered}"
    );
}

#[test]
fn dry_run_reports_the_same_span_and_writes_nothing() {
    let (_temp, vault, day_file) = lunch_vault("bob-cli-meet-run-dry");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-01 11:55:00")
        .output()
        .expect("run dry start");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry JSON");
    assert_eq!(
        json["pomodoro_start"]["time_range"],
        "(**1200-1300** [off:: 60m])"
    );
    assert_eq!(json["pomodoro_start"]["tracked"], false);
    assert_eq!(json["pomodoro_start"]["off_clock"], true);
    assert_eq!(fs::read_to_string(&day_file).expect("day"), LUNCH_DAY);
}

#[test]
fn far_run_warning_names_the_event_and_the_escape_hatch() {
    let (_temp, vault, day_file) = lunch_vault("bob-cli-meet-run-far");
    let json = run_start(&vault, &day_file, "2026-11-01 09:50:00", "=");
    let warning = json["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "pomodoro_event_run_later")
        .expect("far-run warning");
    assert_eq!(
        warning["message"],
        "MEET's next event, Lunch with Kelly, starts at 1200 (2h 10m from now) · capture `=5#meet` to start 25 minutes now"
    );
}

#[test]
fn link_start_into_an_off_clock_meet_stamps_the_clock() {
    let temp = TempDir::new("bob-cli-meet-run-link");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    write_file(&day_file, LUNCH_DAY);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:work-item#meet=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-01 11:02:00")
        .output()
        .expect("run link start");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("link JSON");
    assert_eq!(json["pomodoro_start"]["tracked"], false, "{json}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(
        day.contains("- [ ] (**1105-1130** [off:: 25m]) — MEET"),
        "{day}"
    );
    assert_eq!(json["pomodoro_start"]["off_clock"], true);
    assert!(!day.contains("[t:: 25m]) — MEET"), "{day}");
}

#[test]
fn closing_a_session_before_its_start_warns_about_x0() {
    let temp = TempDir::new("bob-cli-meet-run-close");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**1300-1330** [t:: 30m]) — MEET\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-01 12:50:00")
        .output()
        .expect("run early close");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("close JSON");
    assert_eq!(json["pomodoro_close"]["closed"]["end"], "1300", "{json}");
    assert_eq!(json["pomodoro_close"]["closed"]["duration_minutes"], 0);
    let warnings = json["warnings"].to_string();
    assert!(
        warnings.contains("capture `x0` to reset it instead"),
        "{warnings}"
    );
}

// --- Event buffers: the run follows what each event occupies. ---

const BUFFERED_LUNCH_DAY: &str = concat!(
    "## Events\n",
    "- [ ] #task **1200-1300** Lunch with Kelly #untracked [buffer:: -30m +15m] [root:: love] ^lunch\n",
    "- [ ] #task **1330-1400** Standup ^standup\n",
    "\n",
    "## Pomodoros\n",
    "- [ ] () — MEET\n",
    "  - [[#^lunch]]\n",
    "  - [[#^standup]]\n",
);

#[test]
fn start_json_reports_the_buffered_run_and_each_events_buffer() {
    let temp = TempDir::new("bob-cli-meet-run-buffer-json");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&day_file, BUFFERED_LUNCH_DAY);
    let json = run_start(&vault, &day_file, "2026-11-01 11:25:00", "=");
    let start = &json["pomodoro_start"];
    assert_eq!(start["time_range"], "(**1130-1315** [off:: 105m])");
    assert_eq!(start["duration_minutes"], 105);
    assert_eq!(start["tracked"], false);
    assert_eq!(
        start["event_run"],
        serde_json::json!({
            "time_range": "1130-1315",
            "start": "1130",
            "end": "1315",
            "off_clock": true,
            "untracked": true,
            "source": "calendar",
            "events": [{
                "index": 1,
                "title": "Lunch with Kelly",
                "time_range": "1200-1300",
                "line": 2,
                "block_id": "lunch",
                "off_clock": true,
                "untracked": true,
                "buffer": {
                    "before_minutes": 30,
                    "after_minutes": 15,
                    "time_range": "1130-1315"
                }
            }]
        })
    );
    assert!(json["warnings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .all(|warning| warning["code"] != "pomodoro_event_run_later"));
}

#[test]
fn human_start_labels_a_buffered_event_with_its_span_and_buffer() {
    let temp = TempDir::new("bob-cli-meet-run-buffer-human");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&day_file, BUFFERED_LUNCH_DAY);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-01 11:25:00")
        .output()
        .expect("run human start");
    assert_success(&output);
    let rendered = stdout(&output);
    assert!(
        rendered.contains(
            "MEET 1130-1315 (105m · off-clock) at line 6 · from your calendar: Lunch with Kelly (1200-1300 −30m +15m)"
        ),
        "{rendered}"
    );
}

#[test]
fn far_run_warning_measures_from_the_buffered_start() {
    let temp = TempDir::new("bob-cli-meet-run-buffer-far");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&day_file, BUFFERED_LUNCH_DAY);
    let json = run_start(&vault, &day_file, "2026-11-01 09:20:00", "=");
    let warning = json["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "pomodoro_event_run_later")
        .expect("far-run warning");
    assert_eq!(
        warning["message"],
        "MEET's next event, Lunch with Kelly (1200-1300 −30m +15m), starts at 1130 (2h 10m from now) · capture `=5#meet` to start 25 minutes now"
    );
}

#[test]
fn a_lead_out_in_progress_is_still_a_run_and_a_counted_start_keeps_its_span() {
    let temp = TempDir::new("bob-cli-meet-run-buffer-explicit");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&day_file, BUFFERED_LUNCH_DAY);
    // 13:10 is lead-out: the meeting ended at 1300, the run lasts to 1315.
    let json = run_start(&vault, &day_file, "2026-11-01 13:10:00", "=5#meet");
    let start = &json["pomodoro_start"];
    assert_eq!(start["tracked"], false, "{start}");
    assert_eq!(start["event_run"]["source"], "explicit", "{start}");
    assert_eq!(start["event_run"]["end"], "1315", "{start}");
}

#[test]
fn dry_run_reports_the_buffered_span_and_writes_nothing() {
    let temp = TempDir::new("bob-cli-meet-run-buffer-dry");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&day_file, BUFFERED_LUNCH_DAY);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-01 11:25:00")
        .output()
        .expect("run dry start");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry JSON");
    assert_eq!(
        json["pomodoro_start"]["time_range"],
        "(**1130-1315** [off:: 105m])"
    );
    assert_eq!(
        json["pomodoro_start"]["event_run"]["events"][0]["buffer"]
            ["before_minutes"],
        30
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day"),
        BUFFERED_LUNCH_DAY
    );
}

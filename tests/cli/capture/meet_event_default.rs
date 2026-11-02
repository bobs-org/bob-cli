//! An unnamed `=` joins the meeting in progress instead of the first
//! future placeholder.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const SCREENSHOT_DAY: &str = concat!(
    "## Events\n",
    "- [*] #task **0800-0815** Swings! #offclock ^swings\n",
    "- [ ] #task **1900-2000** Lincoln Park #offclock [buffer:: -30m] ^lincoln\n",
    "\n",
    "## Pomodoros\n",
    "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
    "- [ ] () — BUILD\n",
    "  - [[sase#^build]]\n",
    "- [ ] () — MEET\n",
    "  - [[#^lincoln]]\n",
    "  - [[#^swings]]\n",
    "- [ ] () — PLAN\n",
);

const STARTED_MEET: &str = concat!(
    "## Events\n",
    "- [*] #task **0800-0815** Swings! #offclock ^swings\n",
    "- [ ] #task **1900-2000** Lincoln Park #offclock [buffer:: -30m] ^lincoln\n",
    "\n",
    "## Pomodoros\n",
    "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
    "- [ ] (**0800-0815** [off:: 15m]) — MEET\n",
    "  - [[#^lincoln]]\n",
    "  - [[#^swings]]\n",
    "- [ ] () — BUILD\n",
    "  - [[sase#^build]]\n",
    "- [ ] () — PLAN\n",
);

fn write_day(
    name: &str,
    contents: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261102.md");
    write_file(&day_file, contents);
    (temp, vault, day_file)
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    now: &str,
    args: &[&str],
) -> Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", now)
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn during_warning(json: &Value) -> &Value {
    json["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "pomodoro_starts_during_event")
        .expect("starts-during-event warning")
}

fn swings_run() -> Value {
    serde_json::json!({
        "time_range": "0800-0815",
        "start": "0800",
        "end": "0815",
        "off_clock": true,
        "untracked": true,
        "source": "calendar",
        "events": [{
            "index": 2,
            "title": "Swings!",
            "time_range": "0800-0815",
            "line": 2,
            "block_id": "swings",
            "off_clock": true,
            "untracked": true
        }]
    })
}

#[test]
fn unnamed_equals_starts_the_meeting_in_progress() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-equals", SCREENSHOT_DAY);
    let json = capture_json(&vault, &day_file, "2026-11-02 08:04:00", &["="]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "MEET");
    assert_eq!(start["time_range"], "(**0800-0815** [off:: 15m])");
    assert_eq!(start["start"], "0800");
    assert_eq!(start["end"], "0815");
    assert_eq!(start["off_clock"], true);
    assert_eq!(start["event_run"], swings_run());
    assert!(start.get("event_in_progress").is_none(), "{start}");
    assert!(
        json["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|warning| warning["code"] != "pomodoro_starts_during_event"),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("started day"),
        STARTED_MEET
    );
}

#[test]
fn counted_suffix_starts_meet_with_an_explicit_span() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-counted", SCREENSHOT_DAY);
    let json = capture_json(&vault, &day_file, "2026-11-02 08:04:00", &["=3"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "MEET");
    assert_eq!(start["time_range"], "(**0805-0820** [off:: 15m])");
    assert_eq!(start["event_run"]["source"], "explicit");
    assert_eq!(start["event_run"]["events"][0]["title"], "Swings!");
    assert_eq!(
        fs::read_to_string(&day_file).expect("started day"),
        concat!(
            "## Events\n",
            "- [*] #task **0800-0815** Swings! #offclock ^swings\n",
            "- [ ] #task **1900-2000** Lincoln Park #offclock [buffer:: -30m] ^lincoln\n",
            "\n",
            "## Pomodoros\n",
            "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
            "- [ ] (**0805-0820** [off:: 15m]) — MEET\n",
            "  - [[#^lincoln]]\n",
            "  - [[#^swings]]\n",
            "- [ ] () — BUILD\n",
            "  - [[sase#^build]]\n",
            "- [ ] () — PLAN\n",
        )
    );
}

#[test]
fn named_build_starts_during_the_meeting_and_warns() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-build", SCREENSHOT_DAY);
    let json =
        capture_json(&vault, &day_file, "2026-11-02 08:04:00", &["=build"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "BUILD");
    assert_eq!(start["start"], "0805");
    assert_eq!(start["end"], "0830");
    assert_eq!(
        start["event_in_progress"],
        serde_json::json!({
            "event": {
                "title": "Swings!",
                "time_range": "0800-0815",
                "line": 2,
                "off_clock": true
            }
        })
    );
    assert_eq!(
        during_warning(&json)["message"],
        "⚠ BUILD 0805-0830 starts during Swings! (0800-0815)"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("started day"),
        concat!(
            "## Events\n",
            "- [*] #task **0800-0815** Swings! #offclock ^swings\n",
            "- [ ] #task **1900-2000** Lincoln Park #offclock [buffer:: -30m] ^lincoln\n",
            "\n",
            "## Pomodoros\n",
            "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
            "- [ ] (**0805-0830** [t:: 25m]) — BUILD\n",
            "  - [[sase#^build]]\n",
            "- [ ] () — MEET\n",
            "  - [[#^lincoln]]\n",
            "  - [[#^swings]]\n",
            "- [ ] () — PLAN\n",
        )
    );
}

#[test]
fn named_meet_still_follows_the_calendar() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-named", SCREENSHOT_DAY);
    let json =
        capture_json(&vault, &day_file, "2026-11-02 08:04:00", &["=meet"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "MEET");
    assert_eq!(start["time_range"], "(**0800-0815** [off:: 15m])");
    assert_eq!(start["event_run"], swings_run());
    assert_eq!(
        fs::read_to_string(&day_file).expect("started day"),
        STARTED_MEET
    );
}

#[test]
fn unnamed_equals_without_meet_starts_build_and_warns() {
    let day = concat!(
        "## Events\n",
        "- [*] #task **0800-0815** Swings! #offclock ^swings\n",
        "\n",
        "## Pomodoros\n",
        "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
        "- [ ] () — BUILD\n",
        "  - [[sase#^build]]\n",
        "- [ ] () — PLAN\n",
    );
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-no-meet", day);
    let json = capture_json(&vault, &day_file, "2026-11-02 08:04:00", &["="]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "BUILD");
    assert_eq!(start["start"], "0805");
    assert_eq!(start["end"], "0830");
    assert_eq!(
        during_warning(&json)["message"],
        "⚠ BUILD 0805-0830 starts during Swings! (0800-0815)"
    );
}

#[test]
fn event_in_the_started_lineup_does_not_warn() {
    let day = concat!(
        "## Events\n",
        "- [*] #task **0800-0815** Swings! #offclock ^swings\n",
        "\n",
        "## Pomodoros\n",
        "- [ ] () — BUILD\n",
        "  - [[#^swings]]\n",
        "- [ ] () — MEET\n",
        "  - [[#^swings]]\n",
    );
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-own-lineup", day);
    let json =
        capture_json(&vault, &day_file, "2026-11-02 08:04:00", &["=build"]);
    assert!(
        json["pomodoro_start"].get("event_in_progress").is_none(),
        "{json}"
    );
}

#[test]
fn restart_of_running_build_warns_and_is_not_redirected() {
    let day = concat!(
        "## Events\n",
        "- [*] #task **0800-0815** Swings! #offclock ^swings\n",
        "\n",
        "## Pomodoros\n",
        "- [ ] (**0755-0820** [t:: 25m]) — BUILD\n",
        "- [ ] () — MEET\n",
        "  - [[#^swings]]\n",
    );
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-restart", day);
    let json = capture_json(&vault, &day_file, "2026-11-02 08:04:00", &["="]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "BUILD");
    assert_eq!(start["start"], "0805");
    assert_eq!(start["end"], "0830");
    assert_eq!(start["override"]["action"], "restart");
    assert_eq!(
        during_warning(&json)["message"],
        "⚠ BUILD 0805-0830 starts during Swings! (0800-0815)"
    );
    let updated = fs::read_to_string(&day_file).expect("restarted day");
    assert!(updated.contains("— BUILD"), "{updated}");
    assert!(updated.contains("() — MEET"), "{updated}");
}

#[test]
fn dry_run_reports_the_same_json_and_writes_nothing() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-default-dry", SCREENSHOT_DAY);
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
        .env("BOB_NOW", "2026-11-02 08:04:00")
        .output()
        .expect("run dry start");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "MEET");
    assert_eq!(
        json["pomodoro_start"]["time_range"],
        "(**0800-0815** [off:: 15m])"
    );
    assert_eq!(json["pomodoro_start"]["event_run"], swings_run());
    assert_eq!(fs::read_to_string(&day_file).expect("day"), SCREENSHOT_DAY);
}

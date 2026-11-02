//! A MEET close carries its later meetings to the end of the ledger:
//! the created `- [ ] () — MEET` continuation lands after the last open
//! Pomodoro below the closed entry, and the close reports it.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const DAY: &str = concat!(
    "## Events\n",
    "- [ ] #task **0800-0815** Swings! #offclock ^swings\n",
    "- [ ] #task **1900-2000** Lincoln Park #offclock [buffer:: -30m] ^lincoln\n",
    "\n",
    "## Pomodoros\n",
    "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
    "- [ ] (**0800-0815** [off:: 15m]) — MEET\n",
    "  - [[#^swings]]\n",
    "  - [[#^lincoln]]\n",
    "- [ ] () — BUILD\n",
    "  - [[dev#^build]]\n",
    "- [ ] () — PLAN\n",
);

const CLOSED_DAY: &str = concat!(
    "## Events\n",
    "- [x] #task **0800-0815** Swings! #offclock  [completion:: 2026-11-02] ^swings\n",
    "- [ ] #task **1900-2000** Lincoln Park #offclock [buffer:: -30m] ^lincoln\n",
    "\n",
    "## Pomodoros\n",
    "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
    "- [x] (**0800-0815** [off:: 15m]) — MEET\n",
    "  - ~~[[#^swings]]~~\n",
    "- [ ] () — BUILD\n",
    "  - [[dev#^build]]\n",
    "- [ ] () — PLAN\n",
    "- [ ] () — MEET\n",
    "  - [[#^lincoln]]\n",
);

fn write_day(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("dev.md"), "---\ntype: [[area]]\n---\n");
    let day_file = vault.join("2026/20261102.md");
    write_file(&day_file, DAY);
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

#[test]
fn meet_close_places_the_carried_meet_last() {
    let (_temp, vault, day_file) = write_day("bob-cli-meet-continuation-last");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-11-02 08:15:00",
        &["x"],
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("closed day"),
        CLOSED_DAY
    );
    let close = &json["pomodoro_close"];
    assert_eq!(
        close["next_pomodoro"],
        serde_json::json!({
            "line": 12,
            "name": "MEET",
            "time_range": null,
            "created": true,
            "placed_last": true,
        })
    );
    let next_block = json["pomodoro_blocks"]
        .as_array()
        .expect("pomodoro blocks")
        .iter()
        .find(|block| block["roles"] == serde_json::json!(["next"]))
        .expect("next block");
    assert_eq!(next_block["name"], "MEET");
    assert_eq!(next_block["line"], 12);
    assert_eq!(next_block["created"], true);
}

#[test]
fn meet_close_reports_placed_last_in_human_output() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-continuation-last-human");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-02 08:15:00")
        .output()
        .expect("run human close");
    assert_success(&output);
    let rendered = stdout(&output);
    assert!(
        rendered.contains(
            "next: MEET (created) at line 12 · carries 1 link · placed last"
        ),
        "{rendered}"
    );
}

#[test]
fn close_then_start_chain_starts_build_while_meet_stays_last() {
    let (_temp, vault, day_file) = write_day("bob-cli-meet-continuation-chain");
    let json = capture_json(&vault, &day_file, "2026-11-02 08:15:00", &["x ="]);
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_start");
    assert_eq!(
        json["captures"][1]["pomodoro_start"]["pomodoro_name"],
        "BUILD"
    );
    let day = fs::read_to_string(&day_file).expect("chained day");
    assert!(
        day.contains("- [ ] (**0815-0840** [t:: 25m]) — BUILD"),
        "{day}"
    );
    assert_eq!(
        day.lines()
            .position(|line| line == "- [ ] () — MEET")
            .expect("moved MEET"),
        day.lines().count() - 2,
        "{day}"
    );
}

#[test]
fn back_to_back_meeting_starts_the_bottom_meet() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-meet-continuation-back-to-back");
    let back_to_back = DAY
        .replace(
            "- [ ] #task **1900-2000** Lincoln Park #offclock [buffer:: -30m] ^lincoln",
            "- [ ] #task **0815-0830** Standup #offclock ^standup",
        )
        .replace("[[#^lincoln]]", "[[#^standup]]");
    write_file(&day_file, &back_to_back);
    let json = capture_json(&vault, &day_file, "2026-11-02 08:15:00", &["x ="]);
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    let start = &json["captures"][1]["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "MEET");
    assert_eq!(start["time_range"], "(**0815-0830** [off:: 15m])");
}

#[test]
fn meet_close_without_a_placeholder_below_keeps_todays_bytes() {
    let temp = TempDir::new("bob-cli-meet-continuation-no-placeholder");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("2026/20261102.md");
    write_file(
        &day_file,
        concat!(
            "## Events\n",
            "- [ ] #task **1000-1030** Future meeting ^future\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] (**0800-0900** [t:: 60m]) — MEET\n",
            "  - [[#^future]]\n",
        ),
    );
    let json = capture_json(&vault, &day_file, "2026-11-02 09:00:00", &["x"]);
    assert_eq!(
        fs::read_to_string(&day_file).expect("closed day"),
        concat!(
            "## Events\n",
            "- [ ] #task **1000-1030** Future meeting ^future\n",
            "\n",
            "## Pomodoros\n",
            "- [x] (**0800-0900** [t:: 60m]) — MEET\n",
            "- [ ] () — MEET\n",
            "  - [[#^future]]\n",
        )
    );
    assert_eq!(
        json["pomodoro_close"]["next_pomodoro"],
        serde_json::json!({
            "line": 6,
            "name": "MEET",
            "time_range": null,
            "created": true,
        })
    );
}

#[test]
fn non_meet_close_carries_below_the_session() {
    let temp = TempDir::new("bob-cli-meet-continuation-control");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("2026/20261102.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0800-0900** [t:: 60m]) — BUILD\n",
            "  - [[dev#^build]]#\n",
            "- [ ] () — PLAN\n",
        ),
    );
    let json = capture_json(&vault, &day_file, "2026-11-02 09:00:00", &["x"]);
    assert_eq!(
        fs::read_to_string(&day_file).expect("closed day"),
        concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0900** [t:: 60m]) — BUILD\n",
            "- [ ] () — BUILD\n",
            "  - [[dev#^build]]\n",
            "- [ ] () — PLAN\n",
        )
    );
    assert_eq!(
        json["pomodoro_close"]["next_pomodoro"],
        serde_json::json!({
            "line": 3,
            "name": "BUILD",
            "time_range": null,
            "created": true,
        })
    );
}

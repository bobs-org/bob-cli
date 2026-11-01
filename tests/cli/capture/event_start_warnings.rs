//! Non-MEET starts, extends, and shifts warn when they run past an Event Task.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const STANDUP_DAY: &str = concat!(
    "## Events\n",
    "- [ ] #task **1200-1230** Standup ^standup\n",
    "\n",
    "## Pomodoros\n",
    "- [ ] () — BOB\n",
);

const UNTRACKED_DAY: &str = concat!(
    "## Events\n",
    "- [ ] #task **1200-1300** Lunch with Kelly #untracked [root:: love] ^lunch\n",
    "\n",
    "## Pomodoros\n",
    "- [ ] () — BOB\n",
);

fn write_day(
    name: &str,
    contents: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
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

fn past_event_warning(json: &Value) -> &Value {
    json["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == "pomodoro_runs_past_event")
        .expect("run-past-event warning")
}

#[test]
fn start_warns_with_fit_draft_and_event_conflict() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-event-warn-start", STANDUP_DAY);
    let json = capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["="]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["start"], "1150");
    assert_eq!(start["end"], "1215");
    assert_eq!(
        start["event_conflict"],
        serde_json::json!({
            "event": {
                "title": "Standup",
                "time_range": "1200-1230",
                "line": 2,
                "untracked": false
            },
            "overlap_minutes": 15,
            "fit": "=2"
        })
    );
    let warning = past_event_warning(&json);
    assert_eq!(
        warning["message"],
        "⚠ BOB 1150-1215 runs 15m into Standup (1200-1230) · =2 ends at 1200"
    );
}

#[test]
fn untracked_event_warns_like_any_other_open_event() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-event-warn-untracked", UNTRACKED_DAY);
    let json = capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["="]);
    let conflict = &json["pomodoro_start"]["event_conflict"];
    assert_eq!(conflict["event"]["title"], "Lunch with Kelly");
    assert_eq!(conflict["event"]["untracked"], true);
    assert_eq!(conflict["fit"], "=2");
}

#[test]
fn meet_sessions_do_not_warn() {
    let (_temp, vault, day_file) = write_day(
        "bob-cli-event-warn-meet",
        concat!(
            "## Events\n",
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] () — MEET\n",
        ),
    );
    let json =
        capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["=5#meet"]);
    assert!(json.get("event_conflict").is_none(), "{json}");
    assert!(
        json["pomodoro_start"].get("event_conflict").is_none(),
        "{json}"
    );
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .all(|warning| warning["code"] != "pomodoro_runs_past_event"),
        "{warnings:?}"
    );
}

#[test]
fn own_lineup_and_done_events_are_exempt() {
    let (_temp, vault, day_file) = write_day(
        "bob-cli-event-warn-lineup",
        concat!(
            "## Events\n",
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "- [x] #task **1205-1230** Done standup ^done\n",
            "- [-] #task **1210-1230** Cancelled ^cancelled\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] () — BOB\n",
            "  - [[#^standup]]\n",
        ),
    );
    let json = capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["="]);
    assert!(
        json["pomodoro_start"].get("event_conflict").is_none(),
        "{json}"
    );
}

#[test]
fn restart_does_not_rewarn_an_already_crossed_span() {
    let (_temp, vault, day_file) = write_day(
        "bob-cli-event-warn-restart",
        concat!(
            "## Events\n",
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] (**1150-1215** [t:: 25m]) — BOB\n",
        ),
    );
    let json = capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["="]);
    assert!(
        json["pomodoro_start"].get("event_conflict").is_none(),
        "{json}"
    );
}

#[test]
fn swap_with_fresh_timing_warns_on_a_new_crossing() {
    let (_temp, vault, day_file) = write_day(
        "bob-cli-event-warn-swap",
        concat!(
            "## Events\n",
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] (**1100-1125** [t:: 25m]) — BOB\n",
            "- [ ] () — BUGS\n",
        ),
    );
    let json =
        capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["=5#bugs"]);
    let conflict = &json["pomodoro_start"]["event_conflict"];
    assert_eq!(conflict["event"]["title"], "Standup");
    assert_eq!(conflict["fit"], "=2#bugs");
}

#[test]
fn named_start_fit_keeps_the_hash() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-event-warn-named", STANDUP_DAY);
    let json =
        capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["=#bob"]);
    // STANDUP_DAY already has a BOB placeholder; =#bob starts it.
    let conflict = &json["pomodoro_start"]["event_conflict"];
    assert_eq!(conflict["fit"], "=2#bob");
}

#[test]
fn link_start_warns() {
    let temp = TempDir::new("bob-cli-event-warn-link");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    write_file(&day_file, STANDUP_DAY);
    let json = capture_json(
        &vault,
        &day_file,
        "2026-11-01 11:50:00",
        &["Work", "@sase:work-item="],
    );
    let conflict = &json["pomodoro_start"]["event_conflict"];
    assert_eq!(conflict["event"]["title"], "Standup");
    assert_eq!(conflict["fit"], "=2");
}

#[test]
fn adjust_plus_warns_and_minus_does_not() {
    let (_temp, vault, day_file) = write_day(
        "bob-cli-event-warn-adjust",
        concat!(
            "## Events\n",
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] (**1150-1155** [t:: 5m]) — BOB\n",
        ),
    );
    let json = capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["+8"]);
    let conflict = &json["pomodoro_adjust"]["event_conflict"];
    assert_eq!(conflict["event"]["title"], "Standup");
    assert_eq!(conflict["fit"], "+1");
    assert_eq!(conflict["overlap_minutes"], 30);

    let minus = capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["-1"]);
    assert!(
        minus["pomodoro_adjust"].get("event_conflict").is_none(),
        "{minus}"
    );
    let warnings = minus["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .all(|warning| warning["code"] != "pomodoro_runs_past_event"),
        "{warnings:?}"
    );
}

#[test]
fn shift_later_and_earlier_warn_on_new_crossings() {
    let (_temp, vault, day_file) = write_day(
        "bob-cli-event-warn-shift-later",
        concat!(
            "## Events\n",
            "- [ ] #task **1210-1230** Review ^review\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] (**1140-1205** [t:: 25m]) — BOB\n",
        ),
    );
    let later =
        capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["++3"]);
    assert_eq!(later["pomodoro_shift"]["event_conflict"]["fit"], "++1");
    assert_eq!(
        later["pomodoro_shift"]["event_conflict"]["event"]["title"],
        "Review"
    );

    let (_temp, vault, day_file) = write_day(
        "bob-cli-event-warn-shift-earlier",
        concat!(
            "## Events\n",
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] (**1210-1235** [t:: 25m]) — BOB\n",
        ),
    );
    let earlier =
        capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["--3"]);
    assert_eq!(earlier["pomodoro_shift"]["event_conflict"]["fit"], "--1");
    assert_eq!(
        earlier["pomodoro_shift"]["event_conflict"]["event"]["title"],
        "Standup"
    );
}

#[test]
fn no_fit_when_the_session_starts_on_the_event() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-event-warn-nofit", STANDUP_DAY);
    let json = capture_json(&vault, &day_file, "2026-11-01 11:58:00", &["="]);
    let conflict = &json["pomodoro_start"]["event_conflict"];
    assert_eq!(conflict["event"]["title"], "Standup");
    assert!(conflict.get("fit").is_none(), "{conflict}");
    assert_eq!(
        past_event_warning(&json)["message"],
        "⚠ BOB 1200-1225 runs 25m into Standup (1200-1230)"
    );
}

#[test]
fn chain_warns_once_on_the_new_crossing() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-event-warn-chain", STANDUP_DAY);
    let json =
        capture_json(&vault, &day_file, "2026-11-01 11:50:00", &["=", "+2"]);
    let warnings = json["warnings"].as_array().expect("warnings");
    let past: Vec<_> = warnings
        .iter()
        .filter(|warning| warning["code"] == "pomodoro_runs_past_event")
        .collect();
    assert_eq!(past.len(), 1, "{warnings:?}");
    let captures = json["captures"].as_array().expect("captures");
    assert!(captures[0]["pomodoro_start"]
        .get("event_conflict")
        .is_some());
    assert!(captures[1]["pomodoro_adjust"]
        .get("event_conflict")
        .is_none());
}

#[test]
fn dry_run_reports_the_same_conflict_and_writes_nothing() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-event-warn-dry", STANDUP_DAY);
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
        .env("BOB_NOW", "2026-11-01 11:50:00")
        .output()
        .expect("run dry start");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry JSON");
    assert_eq!(json["pomodoro_start"]["event_conflict"]["fit"], "=2");
    assert_eq!(fs::read_to_string(&day_file).expect("day"), STANDUP_DAY);
}

#[test]
fn human_start_prints_the_banner() {
    let (_temp, vault, day_file) =
        write_day("bob-cli-event-warn-human", STANDUP_DAY);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-01 11:50:00")
        .output()
        .expect("run human start");
    assert_success(&output);
    let rendered = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        rendered.contains(
            "⚠ BOB 1150-1215 runs 15m into Standup (1200-1230) · =2 ends at 1200"
        ),
        "{rendered}"
    );
}

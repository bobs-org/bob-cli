//! `bob capture-pomodoros --tasks` goldens: the resolved agenda payload.
//!
//! Each case builds a synthetic vault, runs `--tasks -f json` with a
//! pinned clock, normalizes the temp root to `/Users/test/bob`, and
//! compares against `tests/fixtures/capture_pomodoros/`. Setting
//! `BOB_UPDATE_GOLDENS=1` regenerates the goldens instead of comparing.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const GOLDEN_ROOT: &str = "/Users/test/bob";
const PINNED_NOW: &str = "2026-08-28 09:10:00";

fn agenda_json(vault: &Path, day_file: &Path, extra: &[&str]) -> Value {
    agenda_json_with_config(vault, day_file, extra, None)
}

fn agenda_json_with_config(
    vault: &Path,
    day_file: &Path,
    extra: &[&str],
    config: Option<&Path>,
) -> Value {
    let mut command = bob_command();
    command
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(vault)
        .arg("-t")
        .arg("-f")
        .arg("json");
    for arg in extra {
        command.arg(arg);
    }
    if let Some(config) = config {
        command.env("BOB_CONFIG_FILE", config);
    }
    let output = command
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run bob capture-pomodoros --tasks json");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("agenda json")
}

fn normalize(value: Value, temp_root: &str) -> Value {
    match value {
        Value::String(text) => {
            Value::String(text.replace(temp_root, GOLDEN_ROOT))
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| normalize(item, temp_root))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| (key, normalize(item, temp_root)))
                .collect(),
        ),
        scalar => scalar,
    }
}

fn check_golden(name: &str, vault: &Path, day_file: &Path, extra: &[&str]) {
    let raw = agenda_json(vault, day_file, extra);
    let temp_root = vault
        .parent()
        .expect("temp root")
        .to_string_lossy()
        .to_string();
    let value = normalize(raw, &temp_root);
    let pretty = format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("pretty")
    );
    let golden = fixture(&format!("capture_pomodoros/{name}.json"));
    if std::env::var("BOB_UPDATE_GOLDENS").is_ok() {
        fs::create_dir_all(golden.parent().expect("golden parent"))
            .expect("create golden dir");
        fs::write(&golden, &pretty).expect("write golden");
        return;
    }
    let expected = fs::read_to_string(&golden).expect("read committed golden");
    assert_eq!(pretty, expected, "golden {name} changed");
}

fn write_tasks_vault(vault: &Path) {
    write_capture_task_settings(vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "## Tasks\n",
            "- [ ] #task Deep fix [created:: 2026-08-20] ^deep-fix\n",
            "\t- first child\n",
            "\t- \u{1F6E0}\u{FE0F} **Work log**\n",
            "\t\t- Aug 27 — running research\n",
            "- [ ] #task Second task [created:: 2026-08-21] ^second-task\n",
            "- [x] #task Closed task [created:: 2026-08-19] [completion:: 2026-08-28] ^closed-task\n",
        ),
    );
}

#[test]
fn agenda_current_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-current");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [x] (0700-0730 [t:: 30m]) — EARLY\n",
            "\t- [[tasks#^closed-task]]\n",
            "- [ ] (**08:00 - 08:30** [t:: 30m]) — FIX\n",
            "\t- [[tasks#^deep-fix]]\n",
            "\t- epic on test\n",
            "\t- ![[tasks#^second-task]]\n",
            "\t- ~~[[tasks#^closed-task]]~~\n",
            "\t- [[tasks#^deep-fix]]#\n",
            "- [ ] () — SASE\n",
            "\t- [[tasks#^second-task]]\n",
            "\t\t- [[tasks#^deep-fix]]\n",
            "\t- [[missing#^gone]]\n",
            "- [ ] () — LATER WORK\n",
            "\t- [[tasks#^deep-fix]]\n",
            "- [ ] ()\n",
        ),
    );
    check_golden("agenda-current", &vault, &day_file, &[]);
}

#[test]
fn agenda_nothing_running_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-idle");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] () — SASE\n",
            "\t- [[tasks#^deep-fix]]\n",
            "- [ ] () — BOB\n",
        ),
    );
    check_golden("agenda-nothing-running", &vault, &day_file, &[]);
}

#[test]
fn agenda_empty_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-empty");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(&day_file, "# Day\n## Pomodoros\n");
    check_golden("agenda-empty", &vault, &day_file, &[]);
}

#[test]
fn agenda_events_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-events");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &vault.join("work.md"),
        "---\ntype: [[area]]\ncolor: green\n---\n# Work\n",
    );
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[project]]\nparent: [[work]]\nroot: true\n---\n# Job\n",
    );
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Events\n",
            "\n",
            "*↻ synced 08:02 · [me@example.com](https://calendar.google.com/calendar/r/day/2026/8/28?authuser=me%40example.com)*\n",
            "- **all day** School holiday · *day 2 of 3* [📅](https://calendar.google.com/event?eid=holiday \"Open in Google Calendar\")\n",
            "- **1100-1130** Class meeting · *maybe* [📅](https://calendar.google.com/event?eid=class \"Open in Google Calendar\")\n",
            "- [*] #task **0930-1000** Standup [📅](https://calendar.google.com/event?eid=standup \"Open in Google Calendar\") [📹](https://meet.google.com/abc \"Join call\") [📍](https://www.google.com/maps/search/?api=1&query=Office \"Office\") [root:: job] ^standup\n",
            "- [ ] #task **1200-1300** Lunch with Kelly [📅](https://calendar.google.com/event?eid=lunch \"Open in Google Calendar\") #untracked ^lunch\n",
            "\n",
            "## Pomodoros\n",
            "- [x] (0730-0800) — UNTRACKED BREAK\n",
            "- [ ] (0900-0940 [t:: 40m]) — BOB\n",
            "- [ ] () — MEET\n",
            "\t- [[#^standup]]\n",
            "\t- [[#^lunch]]\n",
        ),
    );
    check_golden("agenda-events", &vault, &day_file, &["--all"]);
    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("--tasks")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run human capture-pomodoros agenda");
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("Calendar"), "{human}");
    assert!(human.contains("Standup"), "{human}");
    assert!(human.contains("Lunch with Kelly"), "{human}");
    assert!(human.contains("untracked"), "{human}");
}

#[test]
fn agenda_events_buffered_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-events-buffered");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Events\n",
            "- [x] #task **0830-0855** Past standup [buffer:: +30m] ^past-standup\n",
            "- [ ] #task **0940-1000** Lunch [buffer:: -30m +15m] #untracked ^lunch\n",
            "- [ ] #task **1030-1100** Standup [buffer:: +15m] ^standup\n",
            "## Pomodoros\n",
            "- [ ] (0905-0915 [t:: 10m]) — BOB\n",
            "- [ ] () — MEET\n",
            "\t- [[#^lunch]]\n",
            "\t- [[#^standup]]\n",
        ),
    );
    check_golden("agenda-events-buffered", &vault, &day_file, &[]);
    let json = agenda_json(&vault, &day_file, &[]);
    assert_eq!(json["events"]["rows"][0]["phase"], "ended");
    assert_eq!(
        json["events"]["rows"][0]["buffer"]["ends_at"],
        "2026-08-28T09:25"
    );
    assert_eq!(
        json["events"]["conflict"],
        serde_json::json!({
            "pomodoro_line": 7,
            "event": 1,
            "overlap_minutes": 5,
            "buffer_part": "before"
        })
    );
    assert_eq!(json["events"]["start_hint"], "x =meet");
    assert_eq!(json["events"]["refresh_at"], "2026-08-28T09:25");
    assert_eq!(
        json["pomodoros"][1]["items"][0]["event"]["buffer"]["time_range"],
        "0910-1015"
    );
    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("--tasks")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run human capture-pomodoros agenda with buffers");
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("Past standup"), "{human}");
    assert!(human.contains("+30m"), "{human}");
    assert!(human.contains("−30m +15m"), "{human}");
}

#[test]
fn agenda_gap_skips_a_running_event_lead_out() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-gap-buffer");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Events\n",
            "- [ ] #task **0830-0900** Post-event [buffer:: +30m] ^post-event\n",
            "- [ ] #task **1000-1030** Focus block ^focus\n",
            "## Pomodoros\n",
        ),
    );
    let json = agenda_json(&vault, &day_file, &[]);
    assert_eq!(
        json["events"]["gap"],
        serde_json::json!({
            "from": "09:30",
            "until": "10:00",
            "minutes": 30,
            "fits": "=6"
        })
    );
    assert_eq!(json["events"]["refresh_at"], "2026-08-28T09:30");
    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("--tasks")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run human capture-pomodoros agenda");
    assert_success(&output);
    assert!(stdout(&output).contains("Post-event"));
}

#[test]
fn agenda_events_idle_gap_reports_a_plain_fit() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-events-gap");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Events\n",
            "- [ ] #task **1000-1030** Focus block ^focus\n",
            "## Pomodoros\n",
        ),
    );
    let json = agenda_json(&vault, &day_file, &[]);
    assert_eq!(json["events"]["focus"], 0);
    assert_eq!(
        json["events"]["gap"],
        serde_json::json!({
            "from": "09:10",
            "until": "10:00",
            "minutes": 50,
            "fits": "=10"
        })
    );
    assert_eq!(json["events"]["refresh_at"], "2026-08-28T10:00");
}

#[test]
fn agenda_no_daily_note_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-no-note");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    check_golden("agenda-no-daily-note", &vault, &day_file, &[]);
}

#[test]
fn agenda_multiple_timed_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-multi");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] (0800-0830 [t:: 30m]) — ONE\n",
            "\t- [[tasks#^deep-fix]]\n",
            "- [ ] (0835-0905 [t:: 30m]) — TWO\n",
            "\t- [[tasks#^second-task]]\n",
            "- [ ] () — NEXT UP\n",
        ),
    );
    check_golden("agenda-multiple-timed", &vault, &day_file, &[]);
}

#[test]
fn agenda_heavy_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-heavy");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    for note in 0..6 {
        let mut contents = String::new();
        for task in 0..13 {
            contents.push_str(&format!(
                "- [ ] #task Task {note}-{task} [created:: 2026-08-01] ^n{note}-t{task}\n\t- detail {task}\n"
            ));
        }
        write_file(&vault.join(format!("n{note}.md")), &contents);
    }
    let day_file = vault.join("2026/20260828.md");
    let mut day = String::from("# Day\n## Pomodoros\n");
    day.push_str("- [ ] (0800-0830 [t:: 30m]) — FIRST\n");
    for link in 0..3 {
        day.push_str(&format!("\t- [[n{link}#^n{link}-t{link}]]\n"));
    }
    for entry in 1..24 {
        day.push_str(&format!("- [ ] () — E{entry}\n"));
        for link in 0..3 {
            let note = (entry + link) % 6;
            let task = (entry * 3 + link) % 13;
            day.push_str(&format!("\t- [[n{note}#^n{note}-t{task}]]\n"));
        }
    }
    day.push_str("- [ ] () — EXTRA\n");
    for extra in 0..6 {
        day.push_str(&format!("\t- [[n0#^n0-t{extra}]]\n"));
    }
    write_file(&day_file, &day);
    check_golden("agenda-heavy", &vault, &day_file, &[]);
}

#[test]
fn agenda_all_lists_completed_with_empty_payloads() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-all");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [x] (0700-0730 [t:: 30m]) — EARLY\n",
            "\t- [[tasks#^closed-task]]\n",
            "- [ ] () — SASE\n",
        ),
    );
    let json = agenda_json(&vault, &day_file, &["--all"]);
    let done = json["pomodoros"]
        .as_array()
        .expect("pomodoros array")
        .iter()
        .find(|entry| entry["name"] == "EARLY")
        .expect("completed entry")
        .clone();
    assert_eq!(done["role"], "completed");
    assert_eq!(done["starts_at"], "2026-08-28T07:00");
    assert_eq!(done["ends_at"], "2026-08-28T07:30");
    assert_eq!(done["notes"].as_array().expect("notes").len(), 0);
    assert_eq!(done["items"].as_array().expect("items").len(), 0);
    assert_eq!(done["retired_link_count"], 0);
    assert_eq!(
        json["completed_summary"],
        serde_json::json!({"count": 1, "minutes": 30})
    );
    assert_eq!(json["date"], "2026-08-28");
    assert_eq!(json["plan_budget"]["status"], "ok");
    // The completed EARLY entry and its link are visible under --all but
    // don't contribute to the saved open-ledger budget.
    assert!(json["plan_budget"].get("themes").is_none());
    assert_eq!(json["plan_budget"]["links"]["count"], 0);
    assert_eq!(json["plan_budget"]["links"]["cap"], 10);
}

#[test]
fn agenda_default_output_stays_byte_identical() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-default");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] (0800-0830) — FIX\n",
            "\t- [[tasks#^deep-fix]]\n",
        ),
    );
    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run default json");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("default json");
    assert!(json.get("date").is_none(), "{json}");
    assert!(json.get("completed_summary").is_none(), "{json}");
    assert!(json.get("plan_budget").is_none(), "{json}");
    for entry in json["pomodoros"].as_array().expect("array") {
        for key in [
            "role",
            "starts_at",
            "ends_at",
            "retired_link_count",
            "notes",
            "items",
        ] {
            assert!(entry.get(key).is_none(), "{key}: {json}");
        }
    }
}

#[test]
fn agenda_budget_uses_configured_link_cap() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-budget-caps");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    let config = temp.path().join("config.yml");
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] () — ALPHA\n",
            "\t- [[tasks#^a]]\n",
            "- [ ] () — BETA\n",
            "\t- [[tasks#^b]]\n",
            "- [ ] () — GAMMA\n",
            "\t- [[tasks#^c]]\n",
            "- [ ] () — DELTA\n",
            "\t- [[tasks#^d]]\n",
        ),
    );
    write_file(&config, "plan:\n  max_links: 4\n");

    let at_cap = agenda_json_with_config(&vault, &day_file, &[], Some(&config));
    assert_eq!(
        at_cap["plan_budget"],
        serde_json::json!({
            "status": "ok",
            "links": {"count": 4, "cap": 4, "over": false}
        })
    );

    write_file(&config, "plan:\n  max_links: 3\n");
    let over = agenda_json_with_config(&vault, &day_file, &[], Some(&config));
    assert_eq!(
        over["plan_budget"],
        serde_json::json!({
            "status": "over",
            "links": {"count": 4, "cap": 3, "over": true}
        })
    );
}

#[test]
fn agenda_budget_includes_real_zero_for_empty_and_completed_only_ledgers() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-budget-empty");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_file(&day_file, "# Day\n## Pomodoros\n");

    let empty = agenda_json(&vault, &day_file, &[]);
    assert!(empty["plan_budget"].get("themes").is_none());
    assert_eq!(empty["plan_budget"]["links"]["count"], 0);
    assert_eq!(empty["plan_budget"]["links"]["cap"], 10);

    write_file(
        &day_file,
        "# Day\n## Pomodoros\n- [x] (0700-0730 [t:: 30m]) — CLOSED\n\t- [[tasks#^closed]]\n",
    );
    let closed = agenda_json_with_config(&vault, &day_file, &["--all"], None);
    assert_eq!(closed["pomodoros"].as_array().unwrap().len(), 1);
    assert!(closed["plan_budget"].get("themes").is_none());
    assert_eq!(closed["plan_budget"]["links"]["count"], 0);
}

#[test]
fn agenda_budget_omits_missing_section_and_degrades_on_invalid_config() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-budget-failures");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    let config = temp.path().join("config.yml");
    write_file(&day_file, "# Day\n## Other\n- [ ] no ledger\n");
    let missing_section = agenda_json(&vault, &day_file, &[]);
    assert!(missing_section.get("plan_budget").is_none());

    write_file(&day_file, "# Day\n## Pomodoros\n- [ ] () — ALPHA\n");
    write_file(&config, "plan:\n  max_links: many\n");
    let invalid =
        agenda_json_with_config(&vault, &day_file, &[], Some(&config));
    assert!(invalid.get("plan_budget").is_none());
    let warnings = invalid["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0]
        .as_str()
        .unwrap()
        .starts_with("plan budget unavailable: "));

    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run no-tasks output");
    assert_success(&output);
    let no_tasks: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(no_tasks.get("plan_budget").is_none());
    assert!(no_tasks["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn agenda_budget_output_is_repeatable_and_read_only() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-budget-read-only");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        "# Day\n## Pomodoros\n- [ ] () — ALPHA\n\t- [[tasks#^a]]\n",
    );
    let before = fs::read(&day_file).unwrap();
    let run = || {
        let output = bob_command()
            .arg("capture-pomodoros")
            .arg("-b")
            .arg(&vault)
            .arg("-t")
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", PINNED_NOW)
            .output()
            .expect("run agenda");
        assert_success(&output);
        output.stdout
    };
    let first = run();
    let second = run();
    assert_eq!(first, second);
    assert_eq!(fs::read(&day_file).unwrap(), before);
    assert_eq!(fs::read_dir(&vault).unwrap().count(), 1);
}

#[test]
fn agenda_numbers_match_operator_lineups() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-parity");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] (**08:00 - 08:30** [t:: 30m]) — FIX\n",
            "\t- [[tasks#^deep-fix]]\n",
            "\t- ![[tasks#^second-task]]\n",
            "\t- quick note\n",
            "- [ ] () — SASE\n",
            "\t- [[tasks#^second-task]]\n",
            "\t- [[tasks#^deep-fix]]\n",
        ),
    );
    let agenda = agenda_json(&vault, &day_file, &[]);
    let saved_budget = &agenda["plan_budget"];
    let current = agenda["pomodoros"]
        .as_array()
        .expect("array")
        .iter()
        .find(|entry| entry["is_current"] == true)
        .expect("current")
        .clone();

    let close = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run x dry-run");
    assert_success(&close);
    let close_json: Value =
        serde_json::from_str(stdout(&close).trim()).expect("close json");
    assert_eq!(
        close_json["plan_budget"]["links"]["before"],
        saved_budget["links"]["count"]
    );
    let lineup = close_json["pomodoro_close"]["task_links"]
        .as_array()
        .expect("task_links");
    let items = current["items"].as_array().expect("items");
    assert_eq!(items.len(), lineup.len());
    for (item, row) in items.iter().zip(lineup.iter()) {
        assert_eq!(item["index"], row["index"]);
        assert_eq!(item["ledger_line"], row["ledger_line"]);
        assert_eq!(item["block_link"], row["block_link"]);
    }

    // `=` starts the next entry, which refuses while a session runs,
    // so the `=` parity case gets its own vault with no running entry.
    let idle = TempDir::new("bob-cli-pomodoros-agenda-start-parity");
    let idle_vault = idle.path().join("vault");
    let idle_day = idle_vault.join("2026/20260828.md");
    write_tasks_vault(&idle_vault);
    write_file(
        &idle_day,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] () — SASE\n",
            "\t- [[tasks#^second-task]]\n",
            "\t- [[tasks#^deep-fix]]\n",
            "- [ ] () — LATER\n",
        ),
    );
    let idle_agenda = agenda_json(&idle_vault, &idle_day, &[]);
    let next = idle_agenda["pomodoros"]
        .as_array()
        .expect("array")
        .iter()
        .find(|entry| entry["role"] == "next")
        .expect("next")
        .clone();
    let start = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&idle_vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &idle_day)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("run = dry-run");
    assert_success(&start);
    let start_json: Value =
        serde_json::from_str(stdout(&start).trim()).expect("start json");
    assert_eq!(
        start_json["plan_budget"]["links"]["before"],
        idle_agenda["plan_budget"]["links"]["count"]
    );
    for field in ["count", "cap", "over"] {
        assert_eq!(
            start_json["plan_budget"]["links"][field],
            idle_agenda["plan_budget"]["links"][field],
            "links.{field}"
        );
    }
    let rows = start_json["pomodoro_start"]["tasks"]
        .as_array()
        .expect("tasks");
    let next_items = next["items"].as_array().expect("items");
    assert_eq!(next_items.len(), rows.len());
    for (item, row) in next_items.iter().zip(rows.iter()) {
        assert_eq!(item["index"], row["index"]);
        assert_eq!(item["ledger_line"], row["ledger_line"]);
        assert_eq!(item["block_link"], row["block_link"]);
        assert_eq!(item["text"], row["text"]);
    }
}

#[test]
fn agenda_budget_matches_plan_for_merged_exempt_alias_and_prose_links() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-budget-parity");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] () — ALPHA / BETA\n",
            "  - [[#^self]]\n",
            "  - [[20260828#^self]]\n",
            "  - [[2026/20260828#^self]]\n",
            "  - note with [[tasks#^prose]]\n",
            "  - [[tasks#^shared]]\n",
            "  - ~~[[tasks#^struck]]~~\n",
            "  - `[[tasks#^code]]`\n",
            "  - ![[tasks#^embedded]]\n",
            "  - [[tasks#^deferred]]#\n",
            "  - ```markdown\n",
            "    [[tasks#^fenced]]\n",
            "    ```\n",
            "- [ ] () — BETA / ALPHA\n",
            "  - [[tasks#^shared]]\n",
            "  - [[tasks#^second]]\n",
            "- [ ] () — PLAN\n",
            "  - [[tasks#^exempt]]\n",
            "- [x] (0700-0730 [t:: 30m]) — CLOSED\n",
            "  - [[tasks#^closed]]\n",
            "- [ ] ()\n",
            "  - [[tasks#^unnamed]]\n",
        ),
    );
    let agenda = agenda_json(&vault, &day_file, &[]);
    let plan = bob_command()
        .arg("plan")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run bob plan json");
    assert_success(&plan);
    let plan: Value = serde_json::from_slice(&plan.stdout).expect("plan json");

    {
        let field = "status";
        assert_eq!(agenda["plan_budget"][field], plan[field]);
    }
    for field in ["count", "cap", "over"] {
        assert_eq!(
            agenda["plan_budget"]["links"][field], plan["links"][field],
            "links.{field}"
        );
    }
}

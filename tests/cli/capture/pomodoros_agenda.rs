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

fn agenda_config(temp: &TempDir) -> std::path::PathBuf {
    let path = temp.path().join("config.yml");
    write_file(&path, "timezone: America/New_York\n");
    path
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
    check_golden_with_config(name, vault, day_file, extra, None);
}

fn check_golden_with_config(
    name: &str,
    vault: &Path,
    day_file: &Path,
    extra: &[&str],
    config: Option<&Path>,
) {
    let raw = agenda_json_with_config(vault, day_file, extra, config);
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
    assert!(human.contains("off-clock"), "{human}");
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
fn agenda_inherited_clock_uses_note_default() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-inherited-clock");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &vault.join("love.md"),
        "---\ntype: [[area]]\noffclock: true\n---\n# Love\n",
    );
    write_file(
        &vault.join("abby.md"),
        "---\ntype: [[area]]\nparent: [[love]]\n---\n# Abby\n",
    );
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Events\n",
            "- [ ] #task **1000-1100** Love lunch [root:: abby] ^love-lunch\n",
            "- [ ] #task **1100-1200** Override #on [root:: abby] ^override\n",
            "- [ ] #task **1200-1300** Plain ^plain\n",
            "## Pomodoros\n",
            "- [ ] () — MEET\n",
            "\t- [[#^love-lunch]]\n",
            "\t- [[#^override]]\n",
            "\t- [[#^plain]]\n",
        ),
    );
    let json = agenda_json(&vault, &day_file, &[]);
    let rows = json["events"]["rows"].as_array().expect("event rows");
    assert_eq!(rows.len(), 3);
    let love = rows
        .iter()
        .find(|row| row["block_id"] == "love-lunch")
        .expect("love row");
    assert_eq!(love["off_clock"], true);
    assert_eq!(love["clock_source"], "note");
    assert_eq!(love["clock_note"], "love.md");
    let over = rows
        .iter()
        .find(|row| row["block_id"] == "override")
        .expect("override row");
    assert_eq!(over["off_clock"], false);
    assert_eq!(over["clock_source"], "on_tag");
    assert_eq!(over["clock_note"], serde_json::Value::Null);
    let plain = rows
        .iter()
        .find(|row| row["block_id"] == "plain")
        .expect("plain row");
    assert_eq!(plain["off_clock"], false);
    assert_eq!(plain["clock_source"], "default");
    assert_eq!(plain["clock_note"], serde_json::Value::Null);
    let items = json["pomodoros"][0]["items"].as_array().expect("items");
    let chip = items
        .iter()
        .find(|item| item["block_id"] == "love-lunch")
        .expect("love chip");
    assert_eq!(chip["event"]["off_clock"], true);
    assert_eq!(chip["event"]["clock_source"], "note");
    assert_eq!(chip["event"]["clock_note"], "love.md");
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
            "links": {"count": 4, "cap": 4, "over": false, "off_clock": 0}
        })
    );

    write_file(&config, "plan:\n  max_links: 3\n");
    let over = agenda_json_with_config(&vault, &day_file, &[], Some(&config));
    assert_eq!(
        over["plan_budget"],
        serde_json::json!({
            "status": "over",
            "links": {"count": 4, "cap": 3, "over": true, "off_clock": 0}
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
    for field in ["count", "cap", "over", "off_clock"] {
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
    for field in ["count", "cap", "over", "off_clock"] {
        assert_eq!(
            agenda["plan_budget"]["links"][field], plan["links"][field],
            "links.{field}"
        );
    }
}

#[test]
fn agenda_plan_budget_leaves_out_off_clock_links() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-off-clock");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Off one #off ^off01\n- [ ] #task On one ^on01\n",
    );
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] () — FOCUS\n",
            "\t- [[tasks#^off01]]\n",
            "\t- [[tasks#^on01]]\n",
        ),
    );
    let json = agenda_json(&vault, &day_file, &[]);
    assert_eq!(json["plan_budget"]["links"]["count"], 1);
    assert_eq!(json["plan_budget"]["links"]["off_clock"], 1);
    assert_eq!(json["plan_budget"]["status"], "ok");
}

#[test]
fn agenda_headline_is_display_null_or_omitted() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-headline");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_tasks_vault(&vault);
    write_file(
        &day_file,
        concat!(
            "# Day\n\n",
            "**Headline:** Ship [[bob#^x|the alias]] today\n\n",
            "## Pomodoros\n",
            "- [ ] () — LATER\n",
        ),
    );
    let json = agenda_json(&vault, &day_file, &[]);
    assert_eq!(json["headline"], "Ship the alias today");
    assert_eq!(json["schema_version"], 1);

    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-t")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", PINNED_NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("human capture-pomodoros --tasks");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("Ship the alias today"),
        "human prints the headline:\n{human}"
    );

    let without_tasks = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("capture-pomodoros json");
    assert_success(&without_tasks);
    let omitted: Value =
        serde_json::from_str(stdout(&without_tasks).trim()).expect("json");
    assert!(
        omitted.get("headline").is_none(),
        "without --tasks the key is absent: {omitted}"
    );
    assert!(
        omitted.get("snoozed").is_none(),
        "without --tasks the Snoozed block is absent: {omitted}"
    );

    write_file(&day_file, "# Day\n\n**Headline:**\n\n## Pomodoros\n");
    let empty = agenda_json(&vault, &day_file, &[]);
    assert!(empty["headline"].is_null(), "empty slot is null: {empty}");

    let missing_dir = temp.path().join("missing");
    let missing_day = missing_dir.join("2026/20260828.md");
    write_tasks_vault(&missing_dir);
    let missing = agenda_json(&missing_dir, &missing_day, &[]);
    assert!(
        missing["headline"].is_null(),
        "missing note is null: {missing}"
    );
}

#[test]
fn agenda_snoozed_rows_golden() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-snoozed");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    let config = agenda_config(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [*] #task Asleep task ^asleep\n",
            "- [*] #task Due task ^due\n",
            "- [?] #task Blocked task ^blocked\n",
        ),
    );
    write_file(
        &day_file,
        concat!(
            "# 2026-08-28\n\n",
            "## Pomodoros\n",
            "- [ ] () — FOCUS\n\n",
            "## Snoozed\n",
            "- **1530** [[tasks#^asleep]]\n",
            "    - Remember the first step.\n",
            "        - Open the notebook.\n",
            "- **0900** [[tasks#^due]]\n",
            "- **1900** [[tasks#^blocked]]\n",
            "- **bad** [[tasks#^asleep]]\n",
            "- **1700** [[missing#^gone]]\n",
        ),
    );

    let json = agenda_json_with_config(&vault, &day_file, &[], Some(&config));
    assert_eq!(json["snoozed"]["rows"].as_array().unwrap().len(), 3);
    assert_eq!(json["snoozed"]["rows"][0]["state"], "due");
    assert_eq!(json["snoozed"]["rows"][1]["state"], "asleep");
    assert_eq!(json["snoozed"]["rows"][2]["state"], "blocked");
    assert_eq!(
        json["snoozed"]["rows"][0]["wake_at"],
        "2026-08-28T09:00:00-04:00"
    );
    assert_eq!(json["snoozed"]["next_wake_at"], "2026-08-28T09:00:00-04:00");
    assert_eq!(
        json["snoozed"]["rows"][1]["notes"][0]["text"],
        "Remember the first step."
    );
    check_golden_with_config(
        "agenda-snoozed",
        &vault,
        &day_file,
        &[],
        Some(&config),
    );
}

#[test]
fn agenda_snoozed_history_golden_matches_latest_resolved_wake() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-snoozed-history");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    let config = agenda_config(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Deep fix ^deep-fix\n",
            "- [x] #task Closed task ^closed-task\n",
            "- [ ] #task First duplicate ^dupe\n",
            "- [ ] #task Second duplicate ^dupe\n",
        ),
    );
    write_file(
        &day_file,
        concat!(
            "# 2026-08-28\n\n",
            "## Pomodoros\n",
            "- [ ] () — WORK\n",
            "    - [[tasks#^deep-fix]]\n",
            "    - [[tasks#^closed-task]]\n",
            "    - [[tasks#^dupe]]\n\n",
            "## Snoozed\n",
            "- ~~**1900** [[tasks#^deep-fix]]~~ · woke 10:05\n",
            "- ~~**1530** [[../tasks#^deep-fix]]~~ · woke 10:15\n",
            "- ~~**1600** [[tasks#^deep-fix]]~~ · woke 10:15\n",
            "- ~~**1400** [[tasks#^closed-task]]~~ · woke 10:20\n",
            "- ~~**1400** [[tasks#^dupe]]~~ · woke 10:25\n",
            "- ~~**1200** [[missing#^gone]]~~ · woke 10:30\n",
            "- ~~**1200** [[tasks#^deep-fix]]~~ · woke noon\n",
        ),
    );

    let json = agenda_json_with_config(&vault, &day_file, &[], Some(&config));
    assert_eq!(json["snoozed"]["rows"], serde_json::json!([]));
    assert_eq!(json["pomodoros"][0]["items"][0]["woke_at"], "10:15");
    assert_eq!(json["pomodoros"][0]["items"][0]["snoozed_target"], "1600");
    assert!(json["pomodoros"][0]["items"][1].get("woke_at").is_none());
    assert_eq!(
        json["pomodoros"][0]["items"][2]["resolution"],
        "duplicate_block"
    );
    assert!(json["pomodoros"][0]["items"][2].get("woke_at").is_none());
    check_golden_with_config(
        "agenda-snoozed-history",
        &vault,
        &day_file,
        &[],
        Some(&config),
    );
}

#[test]
fn agenda_snoozed_without_pomodoros_and_past_day_are_empty_or_read_only() {
    let temp = TempDir::new("bob-cli-pomodoros-agenda-snoozed-only");
    let vault = temp.path().join("vault");
    let config = agenda_config(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("tasks.md"), "- [*] #task Later work ^later\n");
    let snoozed_only = vault.join("2026/20260828.md");
    write_file(
        &snoozed_only,
        "# 2026-08-28\n\n## Snoozed\n- **1530** [[tasks#^later]]\n",
    );
    let json =
        agenda_json_with_config(&vault, &snoozed_only, &[], Some(&config));
    assert_eq!(json["pomodoros"], serde_json::json!([]));
    assert_eq!(json["snoozed"]["rows"].as_array().unwrap().len(), 1);
    check_golden_with_config(
        "agenda-snoozed-only",
        &vault,
        &snoozed_only,
        &[],
        Some(&config),
    );

    let past_day = vault.join("2026/20260827.md");
    write_file(
        &past_day,
        concat!(
            "# 2026-08-27\n\n",
            "## Pomodoros\n",
            "- [ ] () — WORK\n",
            "    - [[tasks#^later]]\n\n",
            "## Snoozed\n",
            "- ~~**1530** [[tasks#^later]]~~ · woke 14:20\n",
            "- **1900** [[tasks#^later]]\n",
        ),
    );
    let past = agenda_json_with_config(&vault, &past_day, &[], Some(&config));
    assert_eq!(past["snoozed"]["rows"], serde_json::json!([]));
    assert!(past["snoozed"]["next_wake_at"].is_null());
    assert!(past["pomodoros"][0]["items"][0].get("woke_at").is_none());
    check_golden_with_config(
        "agenda-snoozed-past-day",
        &vault,
        &past_day,
        &[],
        Some(&config),
    );
}

#[test]
fn snoozed_wake_lifecycle_keeps_today_separate_and_provenance_stable() {
    let temp = TempDir::new("bob-cli-pomodoros-snooze-lifecycle");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        "timezone: America/New_York\nsnooze:\n  notify: off\n",
    );
    write_blocked_tasks_settings(&vault);
    let tasks_file = vault.join("tasks.md");
    write_file(
        &tasks_file,
        "- [*] #task Review agenda [fresh:: 2026-08-01] ^lifecycle\n",
    );
    write_file(
        &day_file,
        concat!(
            "# 2026-08-28\n\n",
            "## Pomodoros\n",
            "- [ ] () — FOCUS\n\n",
            "## Snoozed\n",
            "- **1000** [[tasks#^lifecycle]]\n",
        ),
    );
    let task_before = fs::read_to_string(&tasks_file).unwrap();

    let run_json = |args: &[&str], now: &str| -> Value {
        let mut command = bob_command();
        command
            .args(args)
            .env("BOB_DIR", &vault)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_NOW", now)
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("BOB_VAULT_SYNC_LOCK_FILE", temp.path().join("vault.lock"))
            .env("BOB_SNOOZE_NOTIFY_SINK", temp.path().join("notify.json"))
            .env("TZ", "UTC")
            .env("NO_COLOR", "1");
        let output = command.output().expect("run lifecycle command");
        assert_success(&output);
        serde_json::from_str::<Value>(stdout(&output).trim())
            .unwrap_or_else(|_| panic!("{}", format_output(&output)))
    };
    let before_snooze = run_json(&["snooze", "list", "-f", "json"], PINNED_NOW);
    let before_reconcile = run_json(
        &[
            "task",
            "reconcile",
            "--dry-run",
            "--format",
            "json",
            "-r",
            "0",
        ],
        PINNED_NOW,
    );
    let before_freshness =
        run_json(&["freshness", "list", "-f", "json"], PINNED_NOW);
    let before_walks = run_json(&["walks", "-f", "json"], PINNED_NOW);
    let before_plan = run_json(&["plan", "-f", "json"], PINNED_NOW);
    let before_agenda =
        run_json(&["capture-pomodoros", "--tasks", "-f", "json"], PINNED_NOW);
    assert_eq!(before_snooze["rows"][0]["state"], "asleep");
    assert_eq!(before_plan["today"]["count"], 0);
    assert_eq!(before_plan["snoozed"]["count"], 1);
    assert_eq!(before_plan["links"]["count"], 0);
    assert_eq!(
        before_agenda["snoozed"]["rows"].as_array().unwrap().len(),
        1
    );
    assert!(before_reconcile["ok"].as_bool().unwrap());
    assert!(before_freshness["queue"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["text"] != "Review agenda"));

    let wake_output = bob_command()
        .args(["snooze", "wake", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", "2026-08-28 10:01:00")
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_VAULT_SYNC_LOCK_FILE", temp.path().join("vault.lock"))
        .env("BOB_SNOOZE_NOTIFY_SINK", temp.path().join("notify.json"))
        .output()
        .expect("wake due lifecycle task");
    assert_success(&wake_output);
    let wake: Value =
        serde_json::from_str(stdout(&wake_output).trim()).unwrap();
    assert_eq!(wake["status"], "woke");
    assert_eq!(fs::read_to_string(&tasks_file).unwrap(), task_before);

    let after_snooze =
        run_json(&["snooze", "list", "-f", "json"], "2026-08-28 10:01:00");
    let after_reconcile = run_json(
        &[
            "task",
            "reconcile",
            "--dry-run",
            "--format",
            "json",
            "-r",
            "0",
        ],
        "2026-08-28 10:01:00",
    );
    let after_freshness =
        run_json(&["freshness", "list", "-f", "json"], "2026-08-28 10:01:00");
    let after_walks = run_json(&["walks", "-f", "json"], "2026-08-28 10:01:00");
    let after_plan = run_json(&["plan", "-f", "json"], "2026-08-28 10:01:00");
    let after_agenda = run_json(
        &["capture-pomodoros", "--tasks", "-f", "json"],
        "2026-08-28 10:01:00",
    );
    assert!(after_snooze["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["state"] == "woken"));
    assert_eq!(after_plan["today"]["count"], 1);
    assert_eq!(after_plan["snoozed"]["count"], 0);
    assert_eq!(after_plan["links"]["count"], 1);
    assert_eq!(after_agenda["snoozed"]["rows"], serde_json::json!([]));
    assert_eq!(after_agenda["pomodoros"][0]["items"][0]["woke_at"], "10:01");
    assert_eq!(
        after_agenda["pomodoros"][0]["items"][0]["snoozed_target"],
        "1000"
    );
    for family in ["s", "w"] {
        let before = before_walks["walks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["family"] == family)
            .expect("walk family");
        let after = after_walks["walks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["family"] == family)
            .expect("walk family");
        assert_eq!(before["counts"], after["counts"], "family {family}");
    }
    assert_eq!(before_freshness["queue"], after_freshness["queue"]);
    assert!(after_reconcile["ok"].as_bool().unwrap());
    assert_eq!(
        before_reconcile["marked_next"],
        after_reconcile["marked_next"]
    );
    assert_eq!(
        before_reconcile["marked_blocked"],
        after_reconcile["marked_blocked"]
    );
    assert_eq!(fs::read_to_string(&tasks_file).unwrap(), task_before);

    let repeat = bob_command()
        .args(["snooze", "wake", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", "2026-08-28 10:01:00")
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_VAULT_SYNC_LOCK_FILE", temp.path().join("vault.lock"))
        .env("BOB_SNOOZE_NOTIFY_SINK", temp.path().join("notify.json"))
        .output()
        .expect("repeat wake");
    assert_success(&repeat);
    let repeat_json: Value =
        serde_json::from_str(stdout(&repeat).trim()).unwrap();
    assert_eq!(repeat_json["status"], "nothing_due");
    assert_eq!(fs::read_to_string(&tasks_file).unwrap(), task_before);
}

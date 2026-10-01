//! `bob plan`: today's plan budget, Today's tasks, and the
//! NEXT/PENDING lanes.

use crate::support::*;
use serde_json::Value;
use std::fs;

const DAILY_NOTE: &str = "# 2026-09-30\n\
\n\
## Pomodoros\n\
\n\
- [ ] (0945-1015) — GOALS\n\
\x20   - [[sase#^fix-it]]\n\
\x20   - [[bob#^capture-stop]]\n\
\x20   - [[sase#^second]]\n\
- [ ] () — DECKS\n\
\x20   - [[decks#^d-one]]\n\
\x20   - [[decks#^d-two]]\n\
- [ ] () — BOB\n\
\x20   - [[sase#^fix-it]]\n\
\x20   - [[missing#^q]]\n\
- [ ] () — GTD\n\
\x20   - [[#^gtd]]\n\
\n\
## Tasks\n\
\n\
- [ ] #task Gtd chore ^gtd\n";

const SASE_TASKS: &str = "\
- [*] #task Fix it ^fix-it\n\
- [/] #task Second pass ^second\n";

const BOB_TASKS: &str = "- [/] #task Better capture stop ^capture-stop\n";

const DECKS_TASKS: &str = "\
- [ ] #task D one ^d-one\n\
- [?] #task D two ^d-two\n";

const LANE_EXCLUSIONS: &str = "\
- [*] #task Hidden next #hide ^hide-next\n\
- [*] #task Future next [scheduled:: 2026-10-05] ^future-next\n\
- [ ] #task Root [id:: lane-root]\n\
- [*] #task Waiting next [dependsOn:: lane-root] ^dep-next\n\
- [x] #task Done next ^done-next\n";

fn plan_vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("2026/20260930.md"), DAILY_NOTE);
    write_file(&vault.join("sase.md"), SASE_TASKS);
    write_file(&vault.join("bob.md"), BOB_TASKS);
    write_file(&vault.join("decks.md"), DECKS_TASKS);
    write_file(&vault.join("tasks.md"), LANE_EXCLUSIONS);
    write_file(
        &vault.join("_templates/tpl.md"),
        "- [*] #task Templated ^tpl-next\n",
    );
    write_file(
        &vault.join("_conflicts.md"),
        "- [*] #task Conflicted ^conf-next\n",
    );
    temp
}

fn vault_dir(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("vault")
}

fn plan_json(temp: &TempDir) -> (std::process::Output, Value) {
    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", vault_dir(temp))
        .env("BOB_DAY_FILE", vault_dir(temp).join("2026/20260930.md"))
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plan -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(&stdout(&output)).expect("plan JSON parses");
    (output, value)
}

#[test]
fn plan_reports_full_budget_as_json() {
    let temp = plan_vault("bob-cli-plan-json");
    let (_output, value) = plan_json(&temp);

    assert_eq!(value["ok"], Value::Bool(true));
    assert_eq!(value["schema_version"], Value::from(2));
    assert_eq!(value["date"], Value::from("2026-09-30"));
    assert_eq!(value["daily_file"], Value::from("2026/20260930.md"));
    assert_eq!(value["status"], Value::from("ok"));
    assert_eq!(value["themes"]["count"], Value::from(3));
    assert_eq!(value["themes"]["cap"], Value::from(3));
    assert_eq!(value["themes"]["over"], Value::Bool(false));
    assert_eq!(value["links"]["count"], Value::from(6));
    assert_eq!(value["links"]["cap"], Value::from(10));
    assert_eq!(value["links"]["over"], Value::Bool(false));
    assert!(value.get("now").is_none(), "schema 2 drops now");
    assert_eq!(
        value["caps"],
        serde_json::json!({
            "max_themes": 3,
            "max_links": 10,
            "max_next": 15,
            "max_pending": 10,
            "max_ready": 100,
            "strict": false,
        })
    );

    assert_eq!(value["today"]["count"], Value::from(6));
    assert_eq!(value["next"]["count"], Value::from(1));
    assert_eq!(value["next"]["cap"], Value::from(15));
    assert_eq!(value["next"]["over"], Value::Bool(false));
    assert_eq!(value["pending"]["count"], Value::from(2));
    assert_eq!(value["pending"]["cap"], Value::from(10));
    assert_eq!(value["pending"]["over"], Value::Bool(false));

    let keys: Vec<String> = value["today_tasks"]
        .as_array()
        .expect("today_tasks array")
        .iter()
        .map(|task| {
            format!(
                "{}#{}",
                task["path"].as_str().expect("task path"),
                task["block_id"].as_str().expect("block id")
            )
        })
        .collect();
    assert_eq!(
        keys,
        vec![
            "sase.md#fix-it",
            "bob.md#capture-stop",
            "sase.md#second",
            "decks.md#d-one",
            "decks.md#d-two",
            "2026/20260930.md#gtd",
        ]
    );
    let first = &value["today_tasks"][0];
    assert_eq!(first["status_symbol"], Value::from("*"));
    assert_eq!(first["status_name"], Value::from("Next"));
    assert_eq!(first["text"], Value::from("Fix it"));
    assert_eq!(first["entry_name"], Value::from("GOALS"));

    let names: Vec<&str> = value["theme_names"]
        .as_array()
        .expect("theme_names array")
        .iter()
        .map(|name| name.as_str().expect("theme name"))
        .collect();
    assert_eq!(names, vec!["GOALS", "DECKS", "BOB"]);

    let entries = value["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["name"], Value::from("GOALS"));
    assert_eq!(entries[0]["highlight"], Value::Bool(true));
    assert_eq!(entries[0]["running"], Value::Bool(true));
    assert_eq!(entries[0]["time_range"], Value::from("0945-1015"));
    assert_eq!(entries[0]["links"], Value::from(3));
    assert_eq!(entries[3]["name"], Value::from("GTD"));
    assert_eq!(entries[3]["exempt"], Value::Bool(true));

    let codes: Vec<&str> = value["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .map(|warning| warning["code"].as_str().expect("lint code"))
        .collect();
    assert_eq!(codes, vec!["today_link_unresolved"]);
}

#[test]
fn plan_counts_only_visible_lane_tasks() {
    // The JSON test above already pins next == 1 and pending == 2:
    // the #hide, future-scheduled, dependency-blocked, _templates,
    // _conflicts, and done `[*]` tasks all stay out, as do the
    // Ready and Blocked Today tasks.
    let temp = plan_vault("bob-cli-plan-lanes");
    let (_output, value) = plan_json(&temp);
    assert_eq!(value["next"]["count"], Value::from(1));
    assert_eq!(value["next"]["over"], Value::Bool(false));
    assert_eq!(value["pending"]["count"], Value::from(2));
    assert_eq!(value["pending"]["over"], Value::Bool(false));
}

#[test]
fn plan_reports_human_budget_without_ansi_when_piped() {
    let temp = plan_vault("bob-cli-plan-human");
    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_DAY_FILE", vault_dir(&temp).join("2026/20260930.md"))
        .output()
        .expect("run bob plan");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let human = stdout(&output);
    assert!(
        human.contains("bob plan")
            && human.contains("2026-09-30")
            && human.contains("2026/20260930.md"),
        "expected plan header:\n{human}"
    );
    assert!(
        human.contains("3/3 themes")
            && human.contains("6/10 links")
            && human.contains("TODAY 6")
            && human.contains("PENDING 2/10")
            && human.contains("NEXT 1/15"),
        "expected meters:\n{human}"
    );
    assert!(
        human.contains("★")
            && human.contains("GOALS")
            && human.contains("▶")
            && human.contains("0945-1015"),
        "expected highlight and running rows:\n{human}"
    );
    assert!(
        human.contains("exempt") && human.contains("GTD"),
        "expected exempt row:\n{human}"
    );
    assert!(
        human.contains("TODAY")
            && human.contains("[*] sase.md#^fix-it")
            && human.contains("Fix it")
            && human.contains("[/] bob.md#^capture-stop"),
        "expected TODAY section:\n{human}"
    );
}

#[test]
fn plan_without_daily_note_still_shows_today_zero_and_lanes() {
    let temp = TempDir::new("bob-cli-plan-no-note");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("tasks.md"), "- [*] #task Open ^open\n");

    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .output()
        .expect("run bob plan without a daily note");

    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("no daily note yet"),
        "expected no-note header:\n{human}"
    );
    assert!(human.contains("TODAY 0"), "expected TODAY meter:\n{human}");
    assert!(
        human.contains("NEXT 1/15"),
        "expected NEXT lane meter:\n{human}"
    );

    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plan -f json without a daily note");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(&stdout(&output)).expect("plan JSON parses");
    assert_eq!(value["ok"], Value::Bool(true));
    assert_eq!(value["today"]["count"], Value::from(0));
    assert!(value["today_tasks"]
        .as_array()
        .expect("today_tasks")
        .is_empty());
    assert_eq!(value["next"]["count"], Value::from(1));
    assert!(value["entries"].as_array().expect("entries").is_empty());
}

#[test]
fn plan_without_pomodoros_section_still_shows_lanes() {
    let temp = TempDir::new("bob-cli-plan-no-section");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("2026/20260930.md"),
        "# 2026-09-30\n\nNo ledger here.\n",
    );

    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .output()
        .expect("run bob plan without a section");

    assert_success(&output);
    assert!(
        stdout(&output).contains("no Pomodoros section"),
        "expected no-section header:\n{}",
        stdout(&output)
    );
}

#[test]
fn plan_placeholder_only_section_shows_daily_file() {
    let temp = TempDir::new("bob-cli-plan-placeholder");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("2026/20260930.md"),
        "## Pomodoros\n\n- [ ] ()\n- [x] () — DONE\n",
    );

    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .output()
        .expect("run bob plan with placeholder-only section");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("2026/20260930.md"),
        "placeholder-only section keeps the daily file header, not no-section:\n{out}"
    );
    assert!(
        !out.contains("no Pomodoros section"),
        "placeholder-only section must not print no-section:\n{out}"
    );
}

#[test]
fn plan_reports_over_cap_with_lints() {
    let temp = TempDir::new("bob-cli-plan-over");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("2026/20260930.md"),
        "## Pomodoros\n\
        \n\
        - [ ] () — ONE\n\
        - [ ] () — TWO\n\
        - [ ] () — THREE\n\
        - [ ] () — FOUR\n\
        - [ ] () — LATER\n\
        \x20   - [[task#^aaa]]\n",
    );

    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plan -f json over the cap");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(&stdout(&output)).expect("plan JSON parses");
    assert_eq!(value["status"], Value::from("over"));
    assert_eq!(value["themes"]["over"], Value::Bool(true));
    let codes: Vec<&str> = value["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .map(|warning| warning["code"].as_str().expect("lint code"))
        .collect();
    assert!(
        codes.contains(&"plan_theme_cap_exceeded")
            && codes.contains(&"inventory_label_open"),
        "expected cap and inventory lints: {codes:?}"
    );
}

#[test]
fn plan_reports_lane_over_cap_without_changing_status() {
    let temp = TempDir::new("bob-cli-plan-lane-over");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("2026/20260930.md"), "## Pomodoros\n");
    write_file(
        &vault.join("tasks.md"),
        "- [*] #task First ^first\n- [*] #task Second ^second\n",
    );
    let config = temp.path().join("config.yml");
    write_file(&config, "plan:\n  max_next: 1\n");

    let output = bob_command()
        .arg("plan")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plan -f json over the lane cap");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(&stdout(&output)).expect("plan JSON parses");
    assert_eq!(value["status"], Value::from("ok"));
    assert_eq!(value["next"]["count"], Value::from(2));
    assert_eq!(value["next"]["cap"], Value::from(1));
    assert_eq!(value["next"]["over"], Value::Bool(true));
    let codes: Vec<&str> = value["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .map(|warning| warning["code"].as_str().expect("lint code"))
        .collect();
    assert!(codes.contains(&"next_cap_exceeded"), "{codes:?}");
}

#[test]
fn plan_loads_a_config_that_still_has_max_now() {
    let temp = TempDir::new("bob-cli-plan-stale-max-now");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("2026/20260930.md"), "## Pomodoros\n");
    let config = temp.path().join("config.yml");
    write_file(&config, "plan:\n  max_now: 4\n");

    let output = bob_command()
        .arg("plan")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plan -f json with a stale max_now");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(&stdout(&output)).expect("plan JSON parses");
    assert_eq!(value["ok"], Value::Bool(true));
    assert_eq!(value["caps"]["max_next"], Value::from(15));
    assert_eq!(value["caps"]["max_pending"], Value::from(10));
    assert_eq!(value["caps"]["max_ready"], Value::from(100));
}

#[test]
fn plan_rejects_invalid_config_with_exit_2() {
    let temp = TempDir::new("bob-cli-plan-bad-config");
    let vault = vault_dir(&temp);
    fs::create_dir_all(&vault).expect("create vault dir");
    let config = temp.path().join("config.yml");
    write_file(&config, "plan:\n  max_themes: 0\n");

    let output = bob_command()
        .arg("plan")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .output()
        .expect("run bob plan with invalid config");

    assert_eq!(
        output.status.code(),
        Some(2),
        "invalid plan config exits 2: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("max_themes"),
        "expected config error naming the key:\n{}",
        stderr(&output)
    );

    let output = bob_command()
        .arg("plan")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260930.md"))
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plan -f json with invalid config");
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(&stdout(&output)).expect("error JSON parses");
    assert_eq!(value["ok"], Value::Bool(false));
}

#[test]
fn plan_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("plan")
        .arg("--help")
        .output()
        .expect("run bob plan --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Today's tasks with a dedicated Task Link")
            && help.contains("NEXT/PENDING lane counts"),
        "expected plan about:\n{help}"
    );
    assert_text_order(&help, &["-b, --bob-dir", "-f, --format", "-h, --help"]);
    assert!(
        help.contains("BOB_DIR")
            && help.contains("BOB_DAY_FILE")
            && help.contains("BOB_NOW")
            && help.contains("BOB_CONFIG_FILE")
            && help.contains("NO_COLOR")
            && help.contains("bob plan -f json"),
        "expected env and examples:\n{help}"
    );
    assert_stdout_has_no_ansi(&output);
}

//! `bob plan`: today's plan budget and this week's NOW count.

use crate::support::*;
use serde_json::Value;
use std::fs;

const DAILY_NOTE: &str = "# 2026-09-30\n\
\n\
## Pomodoros\n\
\n\
- [ ] (0945-1015) — GOALS\n\
\x20   - [[a#^one]] [[b#^two]] [[c#^three]]\n\
- [ ] () — DECKS\n\
\x20   - [[d#^four]] [[e#^five]]\n\
- [ ] () — BOB\n\
\x20   - [[f#^six]] [[g#^seven]]\n\
- [ ] () — GTD\n\
\x20   - [[#^gtd]]\n";

const NOW_TASKS: &str = "\
- [ ] #task Open now #now\n\
- [x] #task Done now #now\n\
- [-] #task Cancelled now #now\n\
- [ ] #task Hidden #now #hide\n\
- [ ] #task Future #now [scheduled:: 2026-10-05]\n\
- [ ] #task Today #now [scheduled:: 2026-09-30]\n\
- [ ] #task Root [id:: now-root]\n\
- [ ] #task Waiting #now [dependsOn:: now-root]\n\
- [ ] #task Nowadays #nowadays\n\
- [ ] #task Subtag #now/x\n";

fn plan_vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("2026/20260930.md"), DAILY_NOTE);
    write_file(&vault.join("tasks.md"), NOW_TASKS);
    write_file(
        &vault.join("_templates/tpl.md"),
        "- [ ] #task Templated #now\n",
    );
    write_file(
        &vault.join("_conflicts.md"),
        "- [ ] #task Conflicted #now\n",
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
    assert_eq!(value["schema_version"], Value::from(1));
    assert_eq!(value["date"], Value::from("2026-09-30"));
    assert_eq!(value["daily_file"], Value::from("2026/20260930.md"));
    assert_eq!(value["status"], Value::from("ok"));
    assert_eq!(value["themes"]["count"], Value::from(3));
    assert_eq!(value["themes"]["cap"], Value::from(3));
    assert_eq!(value["themes"]["over"], Value::Bool(false));
    assert_eq!(value["links"]["count"], Value::from(7));
    assert_eq!(value["links"]["cap"], Value::from(10));
    assert_eq!(value["links"]["over"], Value::Bool(false));
    assert_eq!(value["now"]["count"], Value::from(2));
    assert_eq!(value["now"]["cap"], Value::from(15));

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
    assert!(value["warnings"].as_array().expect("warnings").is_empty());
}

#[test]
fn plan_counts_only_visible_whole_token_now_tasks() {
    // The JSON test above already pins now == 2: the open task and
    // the one scheduled today. Done, cancelled, #hide, _templates,
    // future-scheduled, dependency-blocked, #nowadays, #now/x and
    // the _conflicts path all stay out.
    let temp = plan_vault("bob-cli-plan-now");
    let (_output, value) = plan_json(&temp);
    assert_eq!(value["now"]["count"], Value::from(2));
    assert_eq!(value["now"]["over"], Value::Bool(false));
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
            && human.contains("7/10 links")
            && human.contains("2/15"),
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
}

#[test]
fn plan_without_daily_note_still_shows_now() {
    let temp = TempDir::new("bob-cli-plan-no-note");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("tasks.md"), "- [ ] #task Open now #now\n");

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
    assert!(human.contains("1/15"), "expected NOW meter:\n{human}");

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
    assert_eq!(value["now"]["count"], Value::from(1));
    assert!(value["entries"].as_array().expect("entries").is_empty());
}

#[test]
fn plan_without_pomodoros_section_still_shows_now() {
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
        help.contains("Show today's Pomodoro plan budget")
            && help.contains("this week's NOW count"),
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

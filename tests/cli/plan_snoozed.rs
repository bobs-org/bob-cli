//! `bob plan` snoozed forecast: the additive `snoozed` meter, the
//! nonbinding wake forecast, and the `snooze_wake_would_exceed_cap` lint.

use crate::support::*;
use serde_json::Value;
use std::fs;

const DAY: &str = "2026/20261105.md";
const NOW: &str = "2026-11-05 12:00:00";

fn vault_with(
    prefix: &str,
    day: &str,
    notes: &[(&str, &str)],
    config: Option<&str>,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join(DAY), day);
    for (relative, contents) in notes {
        write_file(&vault.join(relative), contents);
    }
    let config_path = temp.path().join("config.yml");
    write_file(
        &config_path,
        config.unwrap_or("timezone: America/New_York\n"),
    );
    (temp, vault, config_path)
}

fn plan_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    config: &std::path::Path,
    now: &str,
) -> Value {
    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", now)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plan -f json");
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).expect("plan JSON parses")
}

fn plan_human(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    config: &std::path::Path,
    now: &str,
) -> String {
    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", now)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob plan");
    assert_success(&output);
    stdout(&output)
}

#[test]
fn plan_snoozed_empty_when_no_rows() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-empty",
        day,
        &[("tasks.md", "- [*] #task On one ^on-one\n")],
        None,
    );
    let day_file = vault.join(DAY);
    let value = plan_json(&vault, &day_file, &config, NOW);
    assert_eq!(value["schema_version"], Value::from(3));
    assert_eq!(value["snoozed"]["count"], Value::from(0));
    assert_eq!(value["snoozed"]["due"], Value::from(0));
    assert_eq!(value["snoozed"]["blocked"], Value::from(0));
    assert_eq!(value["snoozed"]["on_clock"], Value::from(0));
    assert!(value["snoozed"]["next_wake_at"].is_null());
    assert_eq!(
        value["snoozed"]["forecast"]["count"],
        value["links"]["count"]
    );
    assert!(!value["snoozed"]["forecast"]["over"].as_bool().unwrap());
    assert_eq!(value["status"], Value::from("ok"));
    assert_eq!(value["today"]["count"], Value::from(1));
    let human = plan_human(&vault, &day_file, &config, NOW);
    assert!(
        !human.contains("SNOOZED"),
        "no meter line at zero:\n{human}"
    );
    drop(temp);
}

#[test]
fn plan_snoozed_empty_when_no_daily_note() {
    let temp = TempDir::new("bob-cli-plan-snoozed-missing");
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("tasks.md"), "- [*] #task On one ^on-one\n");
    let config = temp.path().join("config.yml");
    write_file(&config, "timezone: America/New_York\n");
    let missing = vault.join("2026/20261106.md");
    let value = plan_json(&vault, &missing, &config, NOW);
    assert_eq!(value["snoozed"]["count"], Value::from(0));
    assert!(value["snoozed"]["next_wake_at"].is_null());
    assert_eq!(value["snoozed"]["forecast"]["count"], Value::from(0));
    let human = plan_human(&vault, &missing, &config, NOW);
    assert!(human.contains("no daily note yet"), "{human}");
    assert!(!human.contains("SNOOZED"), "{human}");
    drop(temp);
}

#[test]
fn plan_snoozed_only_note_reads_without_pomodoros() {
    let day =
        "# 2026-11-05\n\n## Snoozed\n\n- **1530** [[tasks#^snoozed-one]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-only",
        day,
        &[("tasks.md", "- [*] #task Snoozed one ^snoozed-one\n")],
        None,
    );
    let day_file = vault.join(DAY);
    let value = plan_json(&vault, &day_file, &config, NOW);
    assert_eq!(value["links"]["count"], Value::from(0));
    assert_eq!(value["today"]["count"], Value::from(0));
    assert_eq!(value["snoozed"]["count"], Value::from(1));
    assert_eq!(value["snoozed"]["on_clock"], Value::from(1));
    assert_eq!(
        value["snoozed"]["next_wake_at"],
        Value::from("2026-11-05T15:30:00-05:00")
    );
    assert_eq!(value["snoozed"]["forecast"]["count"], Value::from(1));
    let human = plan_human(&vault, &day_file, &config, NOW);
    assert!(human.contains("no Pomodoros section"), "{human}");
    assert!(
        human.contains("SNOOZED 1")
            && human.contains("next 15:30")
            && human.contains("TODAY 0/1 when all wake"),
        "{human}"
    );
    drop(temp);
}

#[test]
fn plan_snoozed_mixture_counts_due_blocked_and_forecast() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n\n## Snoozed\n\n- **1000** [[tasks#^due-one]]\n- **1530** [[tasks#^asleep-one]]\n- **1900** [[tasks#^blocked-one]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-mixture",
        day,
        &[(
            "tasks.md",
            "- [*] #task On one ^on-one\n- [*] #task Due one ^due-one\n- [*] #task Asleep one ^asleep-one\n- [?] #task Blocked one ^blocked-one\n",
        )],
        None,
    );
    let day_file = vault.join(DAY);
    let value = plan_json(&vault, &day_file, &config, NOW);
    assert_eq!(value["snoozed"]["count"], Value::from(3));
    assert_eq!(value["snoozed"]["due"], Value::from(1));
    assert_eq!(value["snoozed"]["blocked"], Value::from(1));
    assert_eq!(value["snoozed"]["on_clock"], Value::from(2));
    // Earliest live target arms the timer; the blocked row never does.
    assert_eq!(
        value["snoozed"]["next_wake_at"],
        Value::from("2026-11-05T10:00:00-05:00")
    );
    // Forecast is ledger links plus on-clock snoozed rows only.
    assert_eq!(value["links"]["count"], Value::from(1));
    assert_eq!(value["snoozed"]["forecast"]["count"], Value::from(3));
    assert!(!value["snoozed"]["forecast"]["over"].as_bool().unwrap());
    // Current budget, Today, and whole lanes never move for the forecast.
    assert_eq!(value["status"], Value::from("ok"));
    assert_eq!(value["today"]["count"], Value::from(1));
    assert_eq!(value["next"]["count"], value["next"]["count"]);
    let human = plan_human(&vault, &day_file, &config, NOW);
    assert!(
        human.contains("SNOOZED 3")
            && human.contains("next 10:00")
            && human.contains("1 due")
            && human.contains("TODAY 1/3 when all wake"),
        "{human}"
    );
    drop(temp);
}

#[test]
fn plan_snoozed_alias_and_duplicate_count_once() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n\n## Snoozed\n\n- **1530** [[tasks#^shared|Shared alias]]\n- **1900** [[tasks#^shared]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-alias-dupe",
        day,
        &[(
            "tasks.md",
            "- [*] #task On one ^on-one\n- [*] #task Shared ^shared\n",
        )],
        None,
    );
    let day_file = vault.join(DAY);
    let value = plan_json(&vault, &day_file, &config, NOW);
    // Two spellings, one identity; the earliest time wins and the rest is a
    // duplicate that never enters planned-today.
    assert_eq!(value["snoozed"]["count"], Value::from(1));
    assert_eq!(value["snoozed"]["on_clock"], Value::from(1));
    assert_eq!(
        value["snoozed"]["next_wake_at"],
        Value::from("2026-11-05T15:30:00-05:00")
    );
    assert_eq!(value["snoozed"]["forecast"]["count"], Value::from(2));
    drop(temp);
}

#[test]
fn plan_snoozed_already_linked_stays_out() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^shared]]\n\n## Snoozed\n\n- **1530** [[tasks#^shared]]\n- **1900** [[tasks#^other]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-linked",
        day,
        &[(
            "tasks.md",
            "- [*] #task Shared ^shared\n- [*] #task Other ^other\n",
        )],
        None,
    );
    let day_file = vault.join(DAY);
    let value = plan_json(&vault, &day_file, &config, NOW);
    // A live ledger link wins over snooze: only the unlinked row counts.
    assert_eq!(value["snoozed"]["count"], Value::from(1));
    assert_eq!(value["snoozed"]["on_clock"], Value::from(1));
    assert_eq!(
        value["snoozed"]["next_wake_at"],
        Value::from("2026-11-05T19:00:00-05:00")
    );
    drop(temp);
}

#[test]
fn plan_snoozed_explicit_off_clock_leaves_the_forecast() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n\n## Snoozed\n\n- **1530** [[tasks#^off-one]]\n- **1900** [[tasks#^on-two]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-off",
        day,
        &[(
            "tasks.md",
            "- [*] #task On one ^on-one\n- [*] #task Off one #off ^off-one\n- [*] #task On two ^on-two\n",
        )],
        None,
    );
    let day_file = vault.join(DAY);
    let value = plan_json(&vault, &day_file, &config, NOW);
    assert_eq!(value["snoozed"]["count"], Value::from(2));
    assert_eq!(value["snoozed"]["on_clock"], Value::from(1));
    assert_eq!(value["snoozed"]["forecast"]["count"], Value::from(2));
    let human = plan_human(&vault, &day_file, &config, NOW);
    assert!(human.contains("TODAY 1/2 when all wake"), "{human}");
    drop(temp);
}

#[test]
fn plan_snoozed_inherited_area_default_leaves_the_forecast() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n\n## Snoozed\n\n- **1530** [[rest#^nap]]\n- **1900** [[tasks#^on-two]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-inherited",
        day,
        &[
            ("tasks.md", "- [*] #task On one ^on-one\n- [*] #task On two ^on-two\n"),
            ("love.md", "---\ntype: [[area]]\noffclock: true\n---\n"),
            (
                "rest.md",
                "---\ntype: [[project]]\nparent: [[love]]\n---\n- [*] #task Nap ^nap\n",
            ),
        ],
        None,
    );
    let day_file = vault.join(DAY);
    let value = plan_json(&vault, &day_file, &config, NOW);
    // `rest#^nap` inherits the area off-clock default through its project
    // parent, so it stays visible in the meter but never joins the forecast.
    assert_eq!(value["snoozed"]["count"], Value::from(2));
    assert_eq!(value["snoozed"]["on_clock"], Value::from(1));
    assert_eq!(value["snoozed"]["forecast"]["count"], Value::from(2));
    drop(temp);
}

#[test]
fn plan_snoozed_cap_boundary_exact_then_over() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n\n## Snoozed\n\n- **1530** [[tasks#^snoozed-one]]\n";
    let config_exact = "timezone: America/New_York\nplan:\n  max_links: 2\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-cap",
        day,
        &[(
            "tasks.md",
            "- [*] #task On one ^on-one\n- [*] #task Snoozed one ^snoozed-one\n",
        )],
        Some(config_exact),
    );
    let day_file = vault.join(DAY);
    let exact = plan_json(&vault, &day_file, &config, NOW);
    assert_eq!(exact["links"]["count"], Value::from(1));
    assert_eq!(exact["snoozed"]["forecast"]["count"], Value::from(2));
    assert!(!exact["snoozed"]["forecast"]["over"].as_bool().unwrap());
    assert!(
        !exact["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "snooze_wake_would_exceed_cap"),
        "exact cap is allowed: {}",
        exact["warnings"]
    );
    assert_eq!(exact["status"], Value::from("ok"));

    let tight = temp.path().join("tight.yml");
    write_file(
        &tight,
        "timezone: America/New_York\nplan:\n  max_links: 1\n",
    );
    let over = plan_json(&vault, &day_file, &tight, NOW);
    assert_eq!(over["snoozed"]["forecast"]["count"], Value::from(2));
    assert!(over["snoozed"]["forecast"]["over"].as_bool().unwrap());
    let codes: Vec<&str> = over["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["code"].as_str())
        .collect();
    assert!(
        codes.contains(&"snooze_wake_would_exceed_cap"),
        "forecast lint fires over cap: {codes:?}"
    );
    // The lint never changes budget status, Today, or whole-lane meters.
    assert_eq!(over["status"], Value::from("ok"));
    assert_eq!(over["links"]["count"], Value::from(1));
    assert_eq!(over["today"]["count"], Value::from(1));
    let human = plan_human(&vault, &day_file, &tight, NOW);
    assert!(human.contains("SNOOZED 1"), "{human}");
    assert!(human.contains("snooze_wake_would_exceed_cap"), "{human}");
    drop(temp);
}

#[test]
fn plan_snoozed_human_line_names_wake_and_today_projection() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n\n## Snoozed\n\n- **1530** [[tasks#^snoozed-one]]\n- **1900** [[tasks#^snoozed-two]]\n";
    let (temp, vault, config) = vault_with(
        "bob-cli-plan-snoozed-human",
        day,
        &[(
            "tasks.md",
            "- [*] #task On one ^on-one\n- [*] #task Snoozed one ^snoozed-one\n- [*] #task Snoozed two ^snoozed-two\n",
        )],
        None,
    );
    let day_file = vault.join(DAY);
    let human = plan_human(&vault, &day_file, &config, NOW);
    assert!(
        human.contains("SNOOZED 2")
            && human.contains("next 15:30")
            && human.contains("TODAY 1/3 when all wake"),
        "{human}"
    );
    assert_stdout_has_no_ansi(
        &bob_command()
            .arg("plan")
            .env("BOB_DIR", &vault)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_NOW", NOW)
            .env("NO_COLOR", "1")
            .output()
            .expect("run bob plan"),
    );
    drop(temp);
}

#[test]
fn plan_snoozed_leaves_today_and_lanes_stable() {
    let day = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n\n## Snoozed\n\n- **1530** [[tasks#^snoozed-one]]\n";
    let bare = "# 2026-11-05\n\n## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^on-one]]\n";
    let notes: &[(&str, &str)] = &[(
        "tasks.md",
        "- [*] #task On one ^on-one\n- [*] #task Snoozed one ^snoozed-one\n",
    )];
    let (temp, vault, config) =
        vault_with("bob-cli-plan-snoozed-stable", day, notes, None);
    let day_file = vault.join(DAY);
    let with = plan_json(&vault, &day_file, &config, NOW);
    fs::write(&day_file, bare).expect("rewrite daily without Snoozed");
    let without = plan_json(&vault, &day_file, &config, NOW);
    assert_eq!(with["today"], without["today"]);
    assert_eq!(with["next"], without["next"]);
    assert_eq!(with["pending"], without["pending"]);
    assert_eq!(with["links"], without["links"]);
    assert_eq!(with["status"], without["status"]);
    assert_eq!(with["snoozed"]["forecast"]["count"], Value::from(2));
    assert_eq!(without["snoozed"]["count"], Value::from(0));
    drop(temp);
}

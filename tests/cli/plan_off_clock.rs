//! `bob plan` off-clock link budget: off-clock Task Links leave the TODAY
//! link budget while those tasks stay Today tasks.

use crate::support::*;
use serde_json::Value;
use std::fs;

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../fixtures/plan_budget_off_clock/vectors.json"
    ))
    .expect("plan budget off-clock vectors")
}

fn run_plan(temp: &TempDir, max_links: Option<i64>) -> Value {
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261105.md");
    let mut command = bob_command();
    command
        .arg("plan")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-11-05 12:00:00")
        .arg("-f")
        .arg("json");
    if let Some(cap) = max_links {
        let config = temp.path().join("config.yml");
        fs::write(&config, format!("plan:\n  max_links: {cap}\n"))
            .expect("write test config");
        command.env("BOB_CONFIG_FILE", &config);
    }
    let output = command.output().expect("run bob plan -f json");
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).expect("plan JSON parses")
}

#[test]
fn plan_leaves_off_clock_links_out_of_the_budget() {
    for case in vectors()["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().expect("case id");
        let temp = TempDir::new(&format!("bob-cli-plan-off-clock-{id}"));
        let vault = temp.path().join("vault");
        write_blocked_tasks_settings(&vault);
        write_file(
            &vault.join("2026/20261105.md"),
            case["day"].as_str().expect("day"),
        );
        if let Some(notes) = case.get("notes").and_then(Value::as_object) {
            for (relative, contents) in notes {
                write_file(
                    &vault.join(relative),
                    contents.as_str().expect("note contents"),
                );
            }
        }
        let max_links =
            case.get("max_links").and_then(serde_json::Value::as_i64);
        let value = run_plan(&temp, max_links);
        assert_eq!(
            value["links"]["count"], case["links_count"],
            "{id}: links.count"
        );
        assert_eq!(
            value["links"]["off_clock"], case["links_off_clock"],
            "{id}: links.off_clock"
        );
        assert_eq!(value["status"], case["status"], "{id}: status");
        let entries = value["entries"].as_array().expect("entries");
        let expected: Vec<i64> = case["entry_links"]
            .as_array()
            .expect("entry_links")
            .iter()
            .map(|count| count.as_i64().expect("entry link count"))
            .collect();
        assert_eq!(entries.len(), expected.len(), "{id}: entry count");
        for (entry, want) in entries.iter().zip(expected.iter()) {
            assert_eq!(
                entry["links"].as_i64().expect("entry links"),
                *want,
                "{id}: entry links"
            );
        }
        assert_eq!(
            value["today"]["count"], case["today_count"],
            "{id}: today.count"
        );
        if case["status"] == "over" {
            let codes: Vec<&str> = value["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .filter_map(|warning| warning["code"].as_str())
                .collect();
            assert!(
                codes.contains(&"plan_link_cap_exceeded"),
                "{id}: expected cap lint, got {codes:?}"
            );
        } else {
            let codes: Vec<&str> = value["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .filter_map(|warning| warning["code"].as_str())
                .collect();
            // Off-clock links never raise the cap lint on their own; an
            // over-cap fixture without the lint would be a contradiction.
            if value["links"]["count"].as_u64().unwrap_or(0)
                > value["links"]["cap"].as_u64().unwrap_or(u64::MAX)
            {
                assert!(
                    codes.contains(&"plan_link_cap_exceeded"),
                    "{id}: over cap without lint"
                );
            }
        }
    }
}

#[test]
fn plan_human_meter_names_off_clock_links() {
    let temp = TempDir::new("bob-cli-plan-off-clock-human");
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("2026/20261105.md"),
        "## Pomodoros\n\n- [ ] () — FOCUS\n    - [[tasks#^off-one]]\n    - [[tasks#^on-one]]\n",
    );
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Off one #off ^off-one\n- [ ] #task On one ^on-one\n",
    );
    let output = bob_command()
        .arg("plan")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20261105.md"))
        .env("BOB_NOW", "2026-11-05 12:00:00")
        .output()
        .expect("run bob plan");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let human = stdout(&output);
    assert!(
        human.contains("1/10 links") && human.contains("1 off-clock"),
        "expected off-clock suffix in the PLAN meter:\n{human}"
    );
}

//! `bob gcal list`: complete snapshots, classification, roots, and JSON.

mod gcal_support;

use gcal_support::{stderr, stdout, FakeGog, GcalEnv};
use serde_json::Value;
use std::fs;

fn prepare(env: &GcalEnv) {
    env.prepare_healthy_vault();
    for area in ["body", "job", "health"] {
        fs::write(
            env.vault().join(format!("{area}.md")),
            format!("---\ntype: area\n---\n# {area}\n"),
        )
        .expect("write area");
    }
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  colors:\n    tomato: job\n",
    );
}

#[test]
fn bare_gcal_lists_today_and_json_keeps_only_public_normalized_fields() {
    let env = GcalEnv::new("bob-cli-gcal-list");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let mut command = env.command();
    fake.install(&mut command).args(["gcal", "-f", "json"]);
    let output = command.output().expect("run bare bob gcal");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    assert_eq!(report["day"], "2026-10-30");
    assert_eq!(report["counts"]["tasks"], 5);
    assert_eq!(report["counts"]["context"], 5);
    assert_eq!(report["counts"]["omitted"], 3);
    assert_eq!(report["counts"]["awaiting_reply"], 1);
    assert_eq!(report["events"].as_array().unwrap().len(), 10);

    let rows = report["events"].as_array().unwrap();
    let interview = rows
        .iter()
        .find(|row| row["title"] == "Product interview")
        .expect("interview row");
    assert_eq!(interview["inclusion"], "task");
    assert_eq!(interview["root"]["value"], "job");
    assert_eq!(interview["root"]["source"], "colour tomato");
    let dentist = rows
        .iter()
        .find(|row| row["title"] == "Dentist")
        .expect("route token stripped from title");
    assert_eq!(dentist["root"]["value"], "body");
    assert_eq!(dentist["root"]["source"], "@body");
    assert!(rows.iter().any(|row| {
        row["title"] == "Late call" && row["time"] == "18:30-19:00"
    }));
    assert!(!stdout(&output).contains("Bring insurance card"));
    assert!(!stdout(&output).contains("attendees"));

    let events_call = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|pair| pair == ["calendar", "events"]))
        .expect("events call");
    assert!(events_call.contains(&"--all-pages".to_string()));
    assert!(events_call.windows(2).any(|pair| pair == ["--max", "250"]));
    assert!(events_call.contains(&"--fields".to_string()));
    assert!(events_call
        .iter()
        .any(|arg| arg.contains("organizer(self)")));
    assert!(events_call
        .iter()
        .any(|arg| arg == "2026-10-30T00:00:00-04:00"));
    assert!(events_call
        .iter()
        .any(|arg| arg == "2026-10-31T00:00:00-04:00"));
}

#[test]
fn all_flag_adds_omitted_events_with_their_reason() {
    let env = GcalEnv::new("bob-cli-gcal-list-all");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-a", "-f", "json"]);
    let output = command.output().expect("run gcal list -a");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    assert_eq!(report["events"].as_array().unwrap().len(), 13);
    assert!(report["events"].as_array().unwrap().iter().any(|row| {
        row["title"] == "Design review"
            && row["inclusion"] == "omitted"
            && row["reason"] == "reply in Calendar"
    }));
    assert!(report["events"].as_array().unwrap().iter().any(|row| {
        row["title"] == "Declined invitation" && row["reason"] == "declined"
    }));
}

#[test]
fn a_page_failure_or_remaining_token_fails_the_whole_snapshot() {
    let env = GcalEnv::new("bob-cli-gcal-list-incomplete");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog").scenario("events_failure");
    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let output = command.output().expect("run failing list");
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert!(stdout(&output).contains("gog"));

    let env = GcalEnv::new("bob-cli-gcal-list-next-page");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(r#"{"events":[],"nextPageToken":"still-more"}"#);
    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let output = command.output().expect("run partial list");
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert!(stdout(&output).contains("incomplete_snapshot"));
}

#[test]
fn bare_gcal_explains_how_to_connect_before_login() {
    let env = GcalEnv::new("bob-cli-gcal-not-connected");
    env.prepare_healthy_vault();
    env.write_config(
        "timezone: America/New_York\ngcal:\n  writer: test-host\n",
    );
    let fake = FakeGog::new(&env, "fake-gog").scenario("not_logged_in");
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .arg("gcal")
        .output()
        .expect("run bare bob gcal before login");

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
    let stderr = stderr(&output);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].contains("not connected yet"));
    assert!(lines[1].contains("bob gcal login"));
    assert!(lines[2].contains("bob gcal` again"));
}

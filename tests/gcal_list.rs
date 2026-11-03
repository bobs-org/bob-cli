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
    // The fixture dentist description yields one bullet: JSON carries
    // only the count, never the text.
    assert_eq!(dentist["description_lines"], 1);
    for row in rows {
        if row["title"] != "Dentist" {
            assert_eq!(row["description_lines"], 0, "{row}");
        }
    }
    assert!(rows.iter().any(|row| {
        row["title"] == "Late call" && row["time"] == "18:30-19:00"
    }));
    assert!(!stdout(&output).contains("Bring insurance card"));
    assert!(!stdout(&output).contains("attendees"));

    // The table shows a dim `≡ N` after the EVENT title.
    let mut table = env.command();
    fake.install(&mut table).args(["gcal"]);
    let table_out = table.output().expect("run table list");
    assert_eq!(table_out.status.code(), Some(0), "{}", stderr(&table_out));
    assert!(
        stdout(&table_out).contains("Dentist ≡ 1"),
        "{}",
        stdout(&table_out)
    );

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
    assert!(
        events_call
            .windows(2)
            .any(|pair| pair == ["--account", "you@example.com"]),
        "list must pass the resolved account: {events_call:?}"
    );
}

#[test]
fn explicit_on_directive_is_listed_on_clock_even_with_an_off_alias() {
    let env = GcalEnv::new("bob-cli-gcal-list-on-tag");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        r#"{
      "events": [{
        "id": "planning",
        "summary": "Planning",
        "description": "@body #on #offclock -30 +15",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T18:30:00-04:00"},
        "end": {"dateTime": "2026-10-30T19:00:00-04:00"},
        "attendees": [{"self": true, "responseStatus": "accepted"}]
      }],
      "nextPageToken": null
    }"#,
    );

    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let output = command.output().expect("run gcal list");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("list JSON");
    let planning = report["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|row| row["title"] == "Planning")
        .expect("planning event");
    assert_eq!(planning["root"]["value"], "body");
    assert_eq!(planning["off_clock"], false);
    assert_eq!(planning["untracked"], false);
    assert_eq!(planning["description_lines"], 0);
}

#[test]
fn list_uses_the_sole_bob_account_among_multiple_gog_accounts() {
    let env = GcalEnv::new("bob-cli-gcal-list-bob-account");
    env.prepare_healthy_vault();
    env.write_config(
        "timezone: America/New_York\ngcal:\n  writer: test-host\n",
    );
    let fake = FakeGog::new(&env, "fake-gog").scenario("two_default_accounts");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    let mut command = env.command();
    fake.install(&mut command).args(["gcal", "-f", "json"]);
    let output = command.output().expect("run list bob account");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let events_call = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|pair| pair == ["calendar", "events"]))
        .expect("events call");
    assert!(
        events_call
            .windows(2)
            .any(|pair| pair == ["--account", "you@example.com"]),
        "sole bob-gcal account must win: {events_call:?}"
    );
}

#[test]
fn list_uses_the_configured_account_among_multiple_bob_grants() {
    let env = GcalEnv::new("bob-cli-gcal-list-configured-account");
    env.prepare_healthy_vault();
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: work@example.com\n  writer: test-host\n",
    );
    let fake = FakeGog::new(&env, "fake-gog").scenario("two_bob_accounts");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    let mut command = env.command();
    fake.install(&mut command).args(["gcal", "-f", "json"]);
    let output = command.output().expect("run list configured account");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let events_call = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|pair| pair == ["calendar", "events"]))
        .expect("events call");
    assert!(
        events_call
            .windows(2)
            .any(|pair| pair == ["--account", "work@example.com"]),
        "configured account must win: {events_call:?}"
    );
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

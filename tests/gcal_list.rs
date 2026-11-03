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
fn list_shows_buffer_reasons_in_time_and_event_columns() {
    let env = GcalEnv::new("bob-cli-gcal-list-reasons");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        r#"{
      "events": [{
        "id": "dentist",
        "summary": "Dentist",
        "description": "Bring insurance card\n-30 Drive to the dentist\n+15 Drive home",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T18:30:00-04:00"},
        "end": {"dateTime": "2026-10-30T19:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=dentist",
        "attendees": [{"self": true, "responseStatus": "accepted"}]
      }],
      "nextPageToken": null
    }"#,
    );

    let mut table = env.command();
    fake.install(&mut table).args(["gcal", "list"]);
    let table_out = table.output().expect("run table list");
    assert_eq!(table_out.status.code(), Some(0), "{}", stderr(&table_out));
    let table_text = stdout(&table_out);
    assert!(
        table_text.contains("\u{2212}30m +15m"),
        "TIME carries buffers: {table_text}"
    );
    assert!(
        table_text.contains("Dentist \u{2261} 3"),
        "EVENT carries reason count: {table_text}"
    );

    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let output = command.output().expect("run gcal list json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("list JSON");
    let dentist = list_event(&report, "Dentist");
    assert_eq!(dentist["description_lines"], 3, "{dentist}");
    assert_eq!(dentist["buffer"]["before_minutes"], 30, "{dentist}");
    assert_eq!(dentist["buffer"]["after_minutes"], 15, "{dentist}");
    assert!(
        !stdout(&output).contains("dentist"),
        "no reason text in JSON: {}",
        stdout(&output)
    );
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

fn write_love_tree(env: &GcalEnv) {
    fs::write(
        env.vault().join("love.md"),
        "---\ntype: area\noffclock: true\n---\n# Love\n",
    )
    .expect("write love");
    fs::write(
        env.vault().join("abby.md"),
        "---\ntype: area\nparent: [[love]]\n---\n# Abby\n",
    )
    .expect("write abby");
    fs::write(
        env.vault().join("fun.md"),
        "---\ntype: area\nparent: [[love]]\noffclock: false\n---\n# Fun\n",
    )
    .expect("write fun");
}

fn list_event<'a>(report: &'a Value, title: &str) -> &'a Value {
    report["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|row| row["title"] == title)
        .unwrap_or_else(|| panic!("missing event {title}"))
}

#[test]
fn listed_clock_uses_google_root_and_via_note() {
    let env = GcalEnv::new("bob-cli-gcal-list-clock");
    prepare(&env);
    write_love_tree(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let events = serde_json::json!({
        "events": [
            {
                "id": "lunch",
                "summary": "Lunch @love",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T08:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T08:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "planning",
                "summary": "Planning @love #on",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T09:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T09:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "gym",
                "summary": "Gym @fun #off",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T10:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T10:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "swings",
                "summary": "Swings @abby",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T11:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T11:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "coffee",
                "summary": "Coffee #off",
                "status": "confirmed",
                "transparency": "transparent",
                "start": {"dateTime": "2026-10-30T13:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T13:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            }
        ],
        "nextPageToken": serde_json::Value::Null,
    });
    fake.events_json(&serde_json::to_string(&events).expect("events"));

    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let output = command.output().expect("run gcal list json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("list JSON");

    let lunch = list_event(&report, "Lunch");
    assert_eq!(lunch["inclusion"], "task");
    assert_eq!(lunch["root"]["value"], "love");
    assert_eq!(lunch["clock"], Value::Null);
    assert_eq!(lunch["clock_source"], "note");
    assert_eq!(lunch["clock_note"], "love.md");
    assert_eq!(lunch["off_clock"], true);
    assert_eq!(lunch["untracked"], true);
    assert!(
        lunch["vault"]
            .as_str()
            .unwrap_or_default()
            .contains("off-clock via love"),
        "{lunch}"
    );

    let planning = list_event(&report, "Planning");
    assert_eq!(planning["clock"], "on");
    assert_eq!(planning["clock_source"], "on_tag");
    assert_eq!(planning["clock_note"], Value::Null);
    assert_eq!(planning["off_clock"], false);

    let gym = list_event(&report, "Gym");
    assert_eq!(gym["clock"], "off");
    assert_eq!(gym["clock_source"], "off_tag");
    assert_eq!(gym["off_clock"], true);

    let swings = list_event(&report, "Swings");
    assert_eq!(swings["clock_source"], "note");
    assert_eq!(swings["clock_note"], "love.md");
    assert_eq!(swings["off_clock"], true);

    let coffee = list_event(&report, "Coffee");
    assert_eq!(coffee["inclusion"], "context");
    assert_eq!(coffee["clock"], "off");
    assert_eq!(coffee["clock_source"], "off_tag");
    assert_eq!(coffee["clock_note"], Value::Null);
    assert_eq!(coffee["off_clock"], true);
    assert_eq!(coffee["vault"], "context");

    let mut table = env.command();
    fake.install(&mut table).args(["gcal"]);
    let table_out = table.output().expect("run table list");
    assert_eq!(table_out.status.code(), Some(0), "{}", stderr(&table_out));
    let table_text = stdout(&table_out);
    assert!(table_text.contains("off-clock via love"), "{table_text}");
    assert!(
        table_text.lines().any(|line| {
            line.contains("Coffee")
                && line.contains("context")
                && !line.contains("via")
        }),
        "context rows stay tag-only:\n{table_text}"
    );
}

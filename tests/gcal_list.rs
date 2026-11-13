//! `bob gcal list`: complete snapshots, classification, roots, and JSON.

mod gcal_support;

use gcal_support::{stderr, stdout, FakeGog, GcalEnv, ACCOUNT, HOST};
use serde_json::{json, Value};
use std::{fs, fs::OpenOptions, io::Write};

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

fn sync_output(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
) -> std::process::Output {
    let mut command = env.command();
    let mut full = vec!["gcal", "sync"];
    full.extend(args);
    fake.install(&mut command)
        .args(full)
        .output()
        .expect("run sync")
}

fn today_note(env: &GcalEnv) -> String {
    fs::read_to_string(env.vault().join("2026/20261030.md"))
        .expect("read daily note")
}

fn timed_external(id: &str, summary: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id,
        "summary": summary,
        "status": "confirmed",
        "start": {"dateTime": start},
        "end": {"dateTime": end},
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    })
}

fn list_ack_envelope() -> String {
    serde_json::to_string(&json!({
        "events": [
            timed_external(
                "ext-ack",
                "Acked review",
                "2026-10-30T13:00:00-04:00",
                "2026-10-30T13:30:00-04:00",
            ),
            timed_external(
                "ext-note",
                "Noted review",
                "2026-10-30T14:00:00-04:00",
                "2026-10-30T14:30:00-04:00",
            ),
            timed_external(
                "ext-plain",
                "Plain review",
                "2026-10-30T15:00:00-04:00",
                "2026-10-30T15:30:00-04:00",
            ),
            {
                "id": "owned-open",
                "summary": "Owned sync",
                "status": "confirmed",
                "organizer": {
                    "self": true,
                    "displayName": "Bryan",
                    "email": "you@example.com",
                },
                "start": {"dateTime": "2026-10-30T16:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T16:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}],
            },
            {
                "id": "ext-day",
                "summary": "External holiday",
                "status": "confirmed",
                "start": {"date": "2026-10-30"},
                "end": {"date": "2026-10-31"},
                "attendees": [],
            },
        ],
        "nextPageToken": null,
    }))
    .expect("envelope")
}

fn append_journal_line(env: &GcalEnv, line: &Value) {
    fs::create_dir_all(env.vault().join(".bob/gcal")).expect("journal dir");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(env.vault().join(".bob/gcal/test-host.jsonl"))
        .expect("open journal");
    writeln!(file, "{}", serde_json::to_string(line).expect("line"))
        .expect("append journal");
}

fn list_output(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
) -> std::process::Output {
    let mut command = env.command();
    let mut full = vec!["gcal", "list"];
    full.extend(args);
    fake.install(&mut command)
        .args(full)
        .output()
        .expect("run list")
}

#[test]
fn list_shows_ack_states_and_counts_unacked() {
    let env = GcalEnv::new("bob-cli-gcal-list-ack");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&list_ack_envelope());
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    append_journal_line(
        &env,
        &json!({
            "v": 1,
            "ts": "2026-10-30T11:00:00-04:00",
            "host": "test-host",
            "kind": "gcal.ack",
            "account": "you@example.com",
            "key": {"calendar_id": "primary", "event_id": "ext-ack"},
            "scope": "event",
            "acked": true,
            "via": "ack",
            "seen": {
                "title": "Acked review",
                "start": "2026-10-30T13:00:00-04:00",
                "end": "2026-10-30T13:30:00-04:00",
                "original_start": null,
            },
        }),
    );
    append_journal_line(
        &env,
        &json!({
            "v": 1,
            "ts": "2026-10-30T11:00:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": "you@example.com",
            "key": {"calendar_id": "primary", "event_id": "ext-note"},
            "scope": "event",
            "text": "@family",
        }),
    );
    // Reconcile the new evidence so the lines match the books.
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let output = list_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(report["counts"]["unacked"], 1);
    let events = report["events"].as_array().expect("events");
    let acked = events
        .iter()
        .find(|event| event["event_id"] == "ext-ack")
        .expect("acked event");
    assert_eq!(acked["ack"]["state"], "acked");
    assert_eq!(acked["ack"]["scope"], "event");
    assert_eq!(acked["ack"]["via"], "ack");
    assert!(acked["ack"]["at"].is_string(), "{acked}");
    let noted = events
        .iter()
        .find(|event| event["event_id"] == "ext-note")
        .expect("noted event");
    assert_eq!(noted["ack"]["state"], "noted");
    assert_eq!(noted["ack"]["scope"], "event");
    assert_eq!(noted["ack"]["via"], "note");
    let plain = events
        .iter()
        .find(|event| event["event_id"] == "ext-plain")
        .expect("plain event");
    assert_eq!(plain["ack"]["state"], "unacked");
    assert!(plain["ack"]["scope"].is_null(), "{plain}");
    for id in ["owned-open", "ext-day"] {
        let event = events
            .iter()
            .find(|event| event["event_id"] == id)
            .unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(event["ack"]["state"], "not_required", "{event}");
    }

    // Note and ack text never travels: neither object carries a body,
    // only shapes and times (root warnings may still quote the route).
    for event in events {
        assert!(event["note"].get("text").is_none(), "{event}");
        assert!(event["ack"].get("text").is_none(), "{event}");
        assert!(event["ack"].get("seen").is_none(), "{event}");
    }

    let table = list_output(&env, &fake, &[]);
    assert_eq!(table.status.code(), Some(0), "{}", stderr(&table));
    let text = stdout(&table);
    assert!(text.contains("Plain review ⚠"), "{text}");
    assert!(!text.contains("Acked review ⚠"), "{text}");
    assert!(!text.contains("Noted review ⚠"), "{text}");
    assert!(
        text.contains("⚠ 1 unacked external event · bob gcal ack"),
        "{text}"
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
        stdout(&table_out).contains("Dentist ⚠ ≡ 1"),
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
        .any(|arg| { arg.contains("organizer(self,displayName,email)") }));
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
        table_text.contains("Dentist ⚠ \u{2261} 3"),
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
        !stdout(&output).contains("Drive to the dentist"),
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

#[test]
fn list_and_sync_agree_on_love_meets_with_type_filtered_clock_defaults() {
    for (case, inherits_meet, writes_type) in [
        ("inherits-meet", true, false),
        ("needs-meet-field", false, true),
    ] {
        let env = GcalEnv::new(&format!("bob-cli-gcal-list-sync-{case}"));
        prepare(&env);
        let type_default = if inherits_meet {
            "task_type: MEET\n"
        } else {
            ""
        };
        fs::write(
            env.vault().join("love.md"),
            format!(
                "---\ntype: area\noffclock_types:\n  - MEET\n{type_default}---\n# Love\n"
            ),
        )
        .expect("write love area");
        let fake = FakeGog::new(&env, "fake-gog");
        fake.events_json(
            r##"{
              "events": [{
                "id": "lunch",
                "summary": "Lunch @love",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T13:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T13:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}]
              }],
              "nextPageToken": null
            }"##,
        );

        let mut list = env.command();
        fake.install(&mut list).args(["gcal", "list", "-f", "json"]);
        let listed = list.output().expect("list before sync");
        assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
        let listed: Value =
            serde_json::from_str(&stdout(&listed)).expect("list JSON");
        let before = list_event(&listed, "Lunch");
        assert_eq!(before["off_clock"], true, "{before}");
        assert_eq!(before["clock_source"], "note", "{before}");
        assert_eq!(before["clock_note"], "love.md", "{before}");

        let mut sync = env.command();
        let synced = fake
            .install(&mut sync)
            .args(["gcal", "sync"])
            .output()
            .expect("sync after list");
        assert_eq!(synced.status.code(), Some(0), "{}", stderr(&synced));
        let day = fs::read_to_string(env.vault().join("2026/20261030.md"))
            .expect("read synced day");
        let event_row = day
            .lines()
            .find(|line| line.contains("^lunch"))
            .expect("synced Lunch Event Task");
        assert!(event_row.contains("[root:: love]"), "{event_row}");
        assert_eq!(
            event_row.contains("[type:: MEET]"),
            writes_type,
            "{event_row}"
        );

        let mut list = env.command();
        fake.install(&mut list).args(["gcal", "list", "-f", "json"]);
        let listed = list.output().expect("list after sync");
        assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
        let listed: Value =
            serde_json::from_str(&stdout(&listed)).expect("list JSON");
        let after = list_event(&listed, "Lunch");
        assert_eq!(after["off_clock"], true, "{after}");
        assert_eq!(after["clock_source"], "note", "{after}");
        assert_eq!(after["clock_note"], "love.md", "{after}");
        assert_eq!(after["off_clock"], before["off_clock"]);
    }
}

#[test]
fn list_shows_waiting_for_templater_on_an_unrendered_note() {
    let env = GcalEnv::new("bob-cli-gcal-list-unrendered");
    prepare(&env);
    fs::write(
        env.vault().join("2026/20261030.md"),
        "---\ndate: <% tp.date.now() %>\n---\n## Pomodoros\n",
    )
    .expect("write raw template note");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let mut command = env.command();
    fake.install(&mut command).args(["gcal"]);
    let output = command.output().expect("run bob gcal");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let table = stdout(&output);
    assert!(
        table
            .lines()
            .next()
            .unwrap_or("")
            .contains("waiting for Templater"),
        "{table}"
    );
    assert!(!table.contains("waiting for note"), "{table}");
    assert!(table.contains("waiting for Templater"), "{table}");

    let mut command = env.command();
    fake.install(&mut command).args(["gcal", "-f", "json"]);
    let output = command.output().expect("run bob gcal json");
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    let dentist = list_event(&report, "Dentist");
    assert_eq!(dentist["vault"], "waiting for Templater");
}

#[test]
fn list_reads_a_replaced_section_before_any_held_sync() {
    let env = GcalEnv::new("bob-cli-gcal-list-replaced-live");
    prepare(&env);
    fs::write(
        env.vault().join("meet.md"),
        "---\ntype: area\ncolor: violet\ntask_type: MEET\n---\n# Meet\n",
    )
    .expect("write meet area");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    let first = sync_output(&env, &fake, &[]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let original = today_note(&env);
    let events_start = original.find("## Events").unwrap();
    let pomodoros_start = original.find("## Pomodoros").unwrap();
    let replaced = format!(
        "{}## Notes\n\n{}",
        &original[..events_start],
        &original[pomodoros_start..]
    );
    fs::write(env.vault().join("2026/20261030.md"), &replaced)
        .expect("replace Events section");

    let mut listed = env.command();
    fake.install(&mut listed);
    let held = listed.args(["gcal", "list"]).output().expect("run list");
    assert_eq!(held.status.code(), Some(0), "{}", stderr(&held));
    let table = stdout(&held);
    assert!(table.contains("held · Events replaced"), "{table}");
    assert!(table.contains("lost · run sync -F"), "{table}");
    assert!(!table.contains("removed by you"), "{table}");

    fs::write(env.vault().join("2026/20261030.md"), &original)
        .expect("restore Events section");
    let mut listed = env.command();
    fake.install(&mut listed);
    let restored = listed.args(["gcal", "list"]).output().expect("run list");
    assert_eq!(restored.status.code(), Some(0), "{}", stderr(&restored));
    let table = stdout(&restored);
    assert!(!table.contains("held · Events replaced"), "{table}");
    assert!(!table.contains("lost · run sync -F"), "{table}");
}

fn list_events_json(events: Vec<Value>) -> String {
    serde_json::to_string(&json!({
        "events": events,
        "nextPageToken": Value::Null,
    }))
    .expect("events envelope")
}

fn guest_task(id: &str, title: &str, description: &str) -> Value {
    json!({
        "id": id,
        "summary": title,
        "description": description,
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T18:30:00-04:00"},
        "end": {"dateTime": "2026-10-30T19:00:00-04:00"},
        "organizer": {
            "displayName": "Kelly",
            "email": "kelly@example.com",
            "self": false
        },
        "attendees": [
            {"self": true, "responseStatus": "accepted"},
            {"email": "kelly@example.com", "responseStatus": "accepted"}
        ]
    })
}

fn run_list_json(env: &GcalEnv, fake: &FakeGog, extra: &[&str]) -> Value {
    let mut command = env.command();
    let mut args = vec!["gcal", "list", "-f", "json"];
    args.extend_from_slice(extra);
    fake.install(&mut command).args(args);
    let output = command.output().expect("run gcal list json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    serde_json::from_str(&stdout(&output)).expect("list JSON")
}

#[test]
fn list_marks_fragile_directives_only_for_guest_tasks_with_google_bob_lines() {
    let env = GcalEnv::new("bob-cli-gcal-list-fragile");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let fragile =
        guest_task("fragile-yes", "Planning", "@body\n-30 Drive there");
    let mut organized = guest_task("fragile-org", "Standup", "@body\n-30");
    organized["organizer"]["self"] = json!(true);
    let mut no_guests = guest_task("fragile-solo", "Focus", "@body\n-30");
    no_guests["attendees"] = json!([]);
    let mut declined = guest_task("fragile-no", "Skipped", "@body\n-30");
    declined["attendees"][0]["responseStatus"] = json!("declined");
    let noted = guest_task("fragile-noted", "Noted", "@body\n-30 Drive there");
    let prose = guest_task("fragile-prose", "Snacks", "Bring insurance card");
    fake.events_json(&list_events_json(vec![
        fragile, organized, no_guests, declined, noted, prose,
    ]));
    let dir = env.vault().join(".bob/gcal");
    fs::create_dir_all(&dir).expect("journal dir");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{HOST}.jsonl")))
        .expect("open journal");
    writeln!(
        file,
        "{}",
        serde_json::to_string(&json!({
            "v": 1,
            "ts": "2026-10-30T12:00:00-04:00",
            "host": HOST,
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "fragile-noted"},
            "scope": "event",
            "text": "@body",
        }))
        .expect("json")
    )
    .expect("append note");

    let report = run_list_json(&env, &fake, &["-a"]);
    let planning = list_event(&report, "Planning");
    assert_eq!(planning["inclusion"], "task", "{planning}");
    assert_eq!(planning["fragile_directives"], true, "{planning}");
    assert_eq!(planning["note"], Value::Null, "{planning}");
    assert_eq!(list_event(&report, "Standup")["fragile_directives"], false);
    assert_eq!(list_event(&report, "Focus")["fragile_directives"], false);
    assert_eq!(list_event(&report, "Skipped")["fragile_directives"], false);
    let noted_row = list_event(&report, "Noted");
    assert!(noted_row["note"].is_object(), "{noted_row}");
    assert_eq!(noted_row["fragile_directives"], false, "{noted_row}");
    assert_eq!(list_event(&report, "Snacks")["fragile_directives"], false);

    let mut table = env.command();
    fake.install(&mut table).args(["gcal", "list"]);
    let table_out = table.output().expect("run table list");
    assert_eq!(table_out.status.code(), Some(0), "{}", stderr(&table_out));
    let text = stdout(&table_out);
    let calendar = planning["calendar_id"].as_str().unwrap_or("primary");
    let expected = format!(
        "⚑ Planning: its Bob lines live in the organizer's description and may vanish on their next update · bob gcal note -A -i fragile-yes -C {calendar}"
    );
    assert!(text.contains(&expected), "{text}");
    assert!(!text.contains("⚑ Standup"), "{text}");
    assert!(!text.contains("⚑ Focus"), "{text}");
    assert!(!text.contains("⚑ Noted"), "{text}");
    assert!(!text.contains("⚑ Snacks"), "{text}");
}

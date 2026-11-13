//! `bob gcal ack`: queue and one-shot modes use only fake gog and temp vaults.

mod gcal_support;

use gcal_support::{
    json_stdout, run_gcal, stderr, stdout, FakeGog, GcalEnv, ACCOUNT,
};
use serde_json::{json, Value};
use std::{fs, fs::OpenOptions, io::Write};

fn prepare(env: &GcalEnv) {
    env.prepare_healthy_vault();
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n",
    );
}

fn timed_event(id: &str, title: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id,
        "summary": title,
        "status": "confirmed",
        "start": {"dateTime": start},
        "end": {"dateTime": end},
        "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
        "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    })
}

fn append_record(env: &GcalEnv, value: &Value) {
    let directory = env.vault().join(".bob/gcal");
    fs::create_dir_all(&directory).expect("create gcal journal");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("test-host.jsonl"))
        .expect("open gcal journal");
    writeln!(file, "{}", serde_json::to_string(value).unwrap())
        .expect("append gcal record");
}

fn ack_record(event_id: &str, start: &str, end: &str) -> Value {
    json!({
        "v": 1,
        "ts": "2026-10-30T12:00:00-04:00",
        "host": "test-host",
        "kind": "gcal.ack",
        "account": ACCOUNT,
        "key": {"calendar_id": "primary", "event_id": event_id},
        "scope": "event",
        "acked": true,
        "via": "ack",
        "seen": {
            "title": "PRIVATE_SEEN_TITLE",
            "start": start,
            "end": end,
            "original_start": null,
        },
    })
}

#[test]
fn ack_help_sorts_options_and_documents_mode_boundaries() {
    let env = GcalEnv::new("bob-cli-gcal-ack-help");
    let output = env
        .command()
        .args(["gcal", "ack", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    let ordered = [
        "-a, --all",
        "-b, --bob-dir",
        "-C, --calendar",
        "-c, --clear",
        "-D, --day",
        "-d, --dry-run",
        "-i, --event-id",
        "-f, --format",
        "-h, --help",
        "-l, --list",
        "-n, --next-n-days",
        "-q, --quiet",
        "-s, --scope",
    ];
    let mut last = 0;
    for option in ordered {
        let position = help
            .find(option)
            .unwrap_or_else(|| panic!("missing {option}:\n{help}"));
        assert!(position >= last, "{option} is out of order:\n{help}");
        last = position;
    }
    for phrase in [
        "Bob asks once about every event you can't describe in Google",
        "Look forward",
        "Exit codes",
        "BOB_DIR",
        "BOB_CONFIG_FILE",
        "BOB_GCAL_GOG",
        "BOB_GCAL_ASSUME_TTY",
        "BOB_NOW",
        "VISUAL",
        "EDITOR",
    ] {
        assert!(help.contains(phrase), "missing {phrase}:\n{help}");
    }
}

#[test]
fn list_json_has_task_and_waiting_lanes_without_writing() {
    let env = GcalEnv::new("bob-cli-gcal-ack-list");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": [
            timed_event("task-1", "Budget review", "2026-10-31T13:00:00-04:00", "2026-10-31T13:30:00-04:00"),
            {
                "id": "wait-1",
                "summary": "Unanswered invite",
                "status": "confirmed",
                "start": {"dateTime": "2026-11-01T13:00:00-05:00"},
                "end": {"dateTime": "2026-11-01T13:30:00-05:00"},
                "organizer": {"displayName": "Kelly", "self": false},
                "attendees": [{"self": true, "responseStatus": "needsAction"}],
            },
            {
                "id": "maybe-1",
                "summary": "Maybe invite",
                "status": "confirmed",
                "start": {"dateTime": "2026-11-02T13:00:00-05:00"},
                "end": {"dateTime": "2026-11-02T13:30:00-05:00"},
                "organizer": {"displayName": "Kelly", "self": false},
                "attendees": [{"self": true, "responseStatus": "tentative"}],
            }
        ], "nextPageToken": null}).to_string(),
    );
    let output = run_gcal(&env, &fake, &["ack", "-l", "-a", "-f", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["window"]["days"], 7);
    assert_eq!(value["queue"].as_array().unwrap().len(), 3);
    assert_eq!(value["waiting"]["count"], 1);
    assert_eq!(value["queue"][1]["lands"], "waiting_reply");
    assert!(value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["lands"] == "waiting_maybe"));
    assert_eq!(value["counts"]["queue"], 3);
    assert_eq!(value["complete"].as_bool(), Some(false));
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn ack_window_uses_contiguous_timezone_boundaries_across_dst() {
    let env = GcalEnv::new("bob-cli-gcal-ack-dst");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json("{\"events\":[],\"nextPageToken\":null}");
    let output = run_gcal(
        &env,
        &fake,
        &["ack", "-l", "-n", "2", "-D", "2026-11-01", "-f", "json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["window"]["days"], 2);
    let calls = fake.calls();
    let event_call = calls
        .iter()
        .find(|call| call.iter().any(|arg| arg == "events"))
        .expect("one contiguous events fetch");
    let from = event_call.iter().position(|arg| arg == "--from").unwrap();
    let to = event_call.iter().position(|arg| arg == "--to").unwrap();
    assert_eq!(event_call[from + 1], "2026-11-01T00:00:00-04:00");
    assert_eq!(event_call[to + 1], "2026-11-03T00:00:00-05:00");
}

#[test]
fn ack_off_leaves_the_queue_empty() {
    let env = GcalEnv::new("bob-cli-gcal-ack-off");
    env.prepare_healthy_vault();
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  ack: off\n",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({
            "events": [timed_event("off-1", "External task", "2026-10-31T13:00:00-04:00", "2026-10-31T13:30:00-04:00")],
            "nextPageToken": null
        })
        .to_string(),
    );
    let output = run_gcal(&env, &fake, &["ack", "-l", "-f", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value = json_stdout(&output);
    assert!(value["queue"].as_array().unwrap().is_empty());
    assert_eq!(value["counts"]["queue"], 0);
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn one_shot_ack_and_clear_preserve_historical_note_evidence() {
    let env = GcalEnv::new("bob-cli-gcal-ack-one-shot");
    prepare(&env);
    let event = timed_event(
        "far-event",
        "Travel briefing",
        "2026-12-13T10:00:00-05:00",
        "2026-12-13T10:30:00-05:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup("far-event", &json!({"event": event}).to_string());
    append_record(
        &env,
        &json!({
            "v": 1,
            "ts": "2026-10-30T10:00:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "far-event"},
            "scope": "event",
            "text": "PRIVATE_CANONICAL_NOTE",
        }),
    );

    let ack = run_gcal(&env, &fake, &["ack", "-i", "far-event"]);
    assert!(ack.status.success(), "{}", stderr(&ack));
    assert!(stdout(&ack).contains("✓ Acked"), "{}", stdout(&ack));
    assert!(
        stdout(&ack).contains("run `bob gcal sync`"),
        "{}",
        stdout(&ack)
    );
    let journal =
        fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
            .unwrap();
    assert!(journal.contains("\"acked\":true"));
    assert!(journal.contains("PRIVATE_CANONICAL_NOTE"));

    let clear = run_gcal(&env, &fake, &["ack", "-c", "-i", "far-event"]);
    assert!(clear.status.success(), "{}", stderr(&clear));
    assert!(
        stdout(&clear).contains("review remains noted"),
        "{}",
        stdout(&clear)
    );
    let final_journal =
        fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
            .unwrap();
    assert!(final_journal.contains("\"acked\":false"));
    assert!(!env.vault().join("2026/20261213.md").exists());
}

#[test]
fn json_preview_is_redacted_and_does_not_append() {
    let env = GcalEnv::new("bob-cli-gcal-ack-preview");
    prepare(&env);
    let event = timed_event(
        "preview-1",
        "Calendar preview",
        "2026-12-13T10:00:00-05:00",
        "2026-12-13T10:30:00-05:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup("preview-1", &json!({"event": event}).to_string());
    append_record(
        &env,
        &ack_record(
            "preview-1",
            "2026-12-13T10:00:00-05:00",
            "2026-12-13T10:30:00-05:00",
        ),
    );
    let before =
        fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
            .unwrap();
    let output =
        run_gcal(&env, &fake, &["ack", "-i", "preview-1", "-f", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["action"], "preview");
    assert_eq!(value["proposed_action"]["action"], "ack");
    assert_eq!(value["selected"]["ack"]["state"], "acked");
    assert!(!stdout(&output).contains("PRIVATE_SEEN_TITLE"));
    assert!(!stdout(&output).contains("PRIVATE_CANONICAL_NOTE"));
    let after =
        fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
            .unwrap();
    assert_eq!(before, after);
}

#[test]
fn ack_list_surfaces_moved_receipts_that_left_the_window() {
    let env = GcalEnv::new("bob-cli-gcal-ack-stale");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json("{\"events\":[],\"nextPageToken\":null}");
    let moved = timed_event(
        "moved-1",
        "Moved interview",
        "2026-11-15T13:00:00-05:00",
        "2026-11-15T13:30:00-05:00",
    );
    fake.event_lookup("moved-1", &json!({"event": moved}).to_string());
    append_record(
        &env,
        &ack_record(
            "moved-1",
            "2026-10-31T13:00:00-04:00",
            "2026-10-31T13:30:00-04:00",
        ),
    );
    let output = run_gcal(&env, &fake, &["ack", "-l", "-f", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value = json_stdout(&output);
    assert!(value["queue"].as_array().unwrap().is_empty());
    assert_eq!(value["counts"]["queue"], 0);
    assert_eq!(value["stale"].as_array().unwrap().len(), 1);
    assert_eq!(value["stale"][0]["event_id"], "moved-1");
    assert!(!stdout(&output).contains("PRIVATE_SEEN_TITLE"));

    let quiet = run_gcal(&env, &fake, &["ack", "-l", "-q"]);
    assert!(quiet.status.success(), "{}", stderr(&quiet));
    assert!(stdout(&quiet).is_empty());
    assert!(stderr(&quiet).contains("ack stale · moved since ack?"));
}

#[test]
fn non_tty_walk_refuses_but_bare_ack_lists_with_hint() {
    let env = GcalEnv::new("bob-cli-gcal-ack-nontty");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json("{\"events\":[],\"nextPageToken\":null}");
    let walk = run_gcal(&env, &fake, &["ack", "-n", "14"]);
    assert_eq!(walk.status.code(), Some(2));
    assert!(stderr(&walk).contains("explicit ack walk needs a TTY"));
    let bare = run_gcal(&env, &fake, &["ack"]);
    assert!(bare.status.success(), "{}", stderr(&bare));
    assert!(stdout(&bare).contains("read-only"));
}

#[test]
fn ack_selector_keeps_literal_note_query_and_scope_format_are_choices() {
    let env = GcalEnv::new("bob-cli-gcal-ack-selector");
    prepare(&env);
    let event = timed_event(
        "note-1",
        "A note about the screening",
        "2026-11-01T13:00:00-04:00",
        "2026-11-01T13:30:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": [event.clone()], "nextPageToken": null}).to_string(),
    );
    fake.event_lookup("note-1", &json!({"event": event}).to_string());
    let output = run_gcal(&env, &fake, &["ack", "note"]);
    assert!(
        output.status.success(),
        "{}\n{}",
        stderr(&output),
        stdout(&output)
    );
    assert!(stdout(&output).contains("Acked A note about the screening"));

    let bad_scope = run_gcal(&env, &fake, &["ack", "-s", "other"]);
    assert_eq!(bad_scope.status.code(), Some(2));
    let bad_format = run_gcal(&env, &fake, &["ack", "-f", "table"]);
    assert_eq!(bad_format.status.code(), Some(2));
    let contradictory =
        run_gcal(&env, &fake, &["ack", "-i", "note-1", "screening"]);
    assert_eq!(contradictory.status.code(), Some(2));
    assert!(stderr(&contradictory)
        .contains("cannot be combined with a title query"));
}

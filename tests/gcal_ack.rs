//! `bob gcal ack`: queue and one-shot modes use only fake gog and temp vaults.

mod gcal_support;

use gcal_support::{
    json_stdout, run_gcal, run_gcal_stdin, stderr, stdout, FakeGog, GcalEnv,
    ACCOUNT,
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

fn series_ack_record(series: &str, acked: bool) -> Value {
    json!({
        "v": 1,
        "ts": "2026-10-30T12:00:00-04:00",
        "host": "test-host",
        "kind": "gcal.ack",
        "account": ACCOUNT,
        "key": {"calendar_id": "primary", "series": series},
        "scope": "series",
        "acked": acked,
        "via": "ack",
        "seen": {
            "title": "PRIVATE_SEEN_TITLE",
            "start": "2026-10-30T13:00:00-04:00",
            "end": "2026-10-30T13:30:00-04:00",
            "original_start": null,
        },
    })
}

fn series_note_record(series: &str, text: &str) -> Value {
    json!({
        "v": 1,
        "ts": "2026-10-30T12:00:00-04:00",
        "host": "test-host",
        "kind": "gcal.note",
        "account": ACCOUNT,
        "key": {"calendar_id": "primary", "series": series},
        "scope": "series",
        "text": text,
    })
}

fn row_record(event_id: &str, day: &str) -> Value {
    json!({
        "v": 1,
        "ts": "2026-10-30T11:00:00-04:00",
        "host": "test-host",
        "kind": "gcal.row",
        "key": {"calendar_id": "primary", "event_id": event_id},
        "day": day,
        "path": format!("2026/{}.md", day.replace('-', "")),
        "block_id": event_id,
        "url": format!("https://calendar.google.com/event?eid={event_id}"),
        "row": "task",
    })
}

fn gone_record(event_id: &str, day: &str, reason: &str) -> Value {
    json!({
        "v": 1,
        "ts": "2026-10-30T11:05:00-04:00",
        "host": "test-host",
        "kind": "gcal.gone",
        "key": {"calendar_id": "primary", "event_id": event_id},
        "day": day,
        "reason": reason,
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
        "-f, --format",
        "-h, --help",
        "-i, --event-id",
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
fn persisted_line_removed_tombstone_stays_out_of_queue_and_waiting_lane() {
    let env = GcalEnv::new("bob-cli-gcal-ack-removed-tombstone");
    prepare(&env);
    let mut removed = timed_event(
        "removed-1",
        "Removed task",
        "2026-10-31T13:00:00-04:00",
        "2026-10-31T13:30:00-04:00",
    );
    removed["attendees"][0]["responseStatus"] = json!("needsAction");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": [removed], "nextPageToken": null}).to_string(),
    );
    append_record(&env, &row_record("removed-1", "2026-10-31"));
    append_record(
        &env,
        &gone_record("removed-1", "2026-10-31", "line_removed"),
    );

    let human = run_gcal(&env, &fake, &["ack", "-l"]);
    assert!(human.status.success(), "{}", stderr(&human));
    assert!(stdout(&human).contains("0 unacked external events"));
    let json_list = run_gcal(&env, &fake, &["ack", "-l", "-f", "json"]);
    assert!(json_list.status.success(), "{}", stderr(&json_list));
    assert_eq!(json_stdout(&json_list)["counts"]["queue"], 0);
    assert_eq!(json_stdout(&json_list)["counts"]["waiting"], 0);
    let all = run_gcal(&env, &fake, &["ack", "-l", "-a", "-f", "json"]);
    assert!(all.status.success(), "{}", stderr(&all));
    assert_eq!(json_stdout(&all)["counts"]["queue"], 0);
    assert_eq!(json_stdout(&all)["counts"]["waiting"], 0);
    assert!(
        !env.vault().join("2026/20261031.md").exists(),
        "queue reads must not create a daily note"
    );
}

#[test]
fn live_removed_row_is_suppressed_but_a_revived_row_returns() {
    let env = GcalEnv::new("bob-cli-gcal-ack-removed-live");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({
            "events": [timed_event("removed-1", "Removed task", "2026-10-31T13:00:00-04:00", "2026-10-31T13:30:00-04:00")],
            "nextPageToken": null
        })
        .to_string(),
    );
    append_record(&env, &row_record("removed-1", "2026-10-31"));
    let daily = env.vault().join("2026/20261031.md");
    fs::create_dir_all(daily.parent().unwrap()).expect("daily directory");
    fs::write(&daily, "## Events\n\nAn unrelated row.\n")
        .expect("write daily note without the event row");
    let removed = run_gcal(&env, &fake, &["ack", "-l", "-f", "json"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    assert_eq!(json_stdout(&removed)["counts"]["queue"], 0);

    append_record(
        &env,
        &gone_record("removed-1", "2026-10-31", "line_removed"),
    );
    let mut revived_row = row_record("removed-1", "2026-10-31");
    revived_row["ts"] = json!("2026-10-30T11:10:00-04:00");
    append_record(&env, &revived_row);
    fs::write(&daily, "## Events\n\n- [ ] #task Removed task ^removed-1\n")
        .expect("write revived Event Task");
    let revived = run_gcal(&env, &fake, &["ack", "-l", "-f", "json"]);
    assert!(revived.status.success(), "{}", stderr(&revived));
    let revived_json = json_stdout(&revived);
    assert_eq!(revived_json["counts"]["queue"], 1, "{revived_json}");
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
    let day_walk = run_gcal(&env, &fake, &["ack", "-D", "tomorrow"]);
    assert_eq!(day_walk.status.code(), Some(2));
    assert!(stderr(&day_walk).contains("explicit ack walk needs a TTY"));
    let bare = run_gcal(&env, &fake, &["ack"]);
    assert!(bare.status.success(), "{}", stderr(&bare));
    assert!(stdout(&bare).contains("read-only"));
}

#[test]
fn day_anchor_is_allowed_with_one_shot_query_and_id_selectors() {
    let env = GcalEnv::new("bob-cli-gcal-ack-day-selector-query");
    prepare(&env);
    let event = timed_event(
        "day-query-1",
        "Budget review",
        "2026-10-31T13:00:00-04:00",
        "2026-10-31T13:30:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": [event.clone()], "nextPageToken": null}).to_string(),
    );
    fake.event_lookup("day-query-1", &json!({"event": event}).to_string());

    let query = run_gcal(&env, &fake, &["ack", "-D", "tomorrow", "Budget"]);
    assert!(query.status.success(), "{}", stderr(&query));
    assert!(stdout(&query).contains("Acked Budget review"));
    assert_eq!(ack_receipts(&journal_text(&env)).len(), 1);

    let env = GcalEnv::new("bob-cli-gcal-ack-day-selector-id");
    prepare(&env);
    let event = timed_event(
        "day-id-1",
        "Budget review",
        "2026-10-31T13:00:00-04:00",
        "2026-10-31T13:30:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup("day-id-1", &json!({"event": event}).to_string());
    let id =
        run_gcal(&env, &fake, &["ack", "-i", "day-id-1", "-D", "tomorrow"]);
    assert!(id.status.success(), "{}", stderr(&id));
    assert!(stdout(&id).contains("Acked Budget review"));
    assert_eq!(ack_receipts(&journal_text(&env)).len(), 1);
}

#[test]
fn day_anchor_keeps_ambiguous_one_shot_queries_explicit() {
    let env = GcalEnv::new("bob-cli-gcal-ack-day-selector-ambiguous");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({
            "events": [
                timed_event("budget-1", "Budget review", "2026-10-31T13:00:00-04:00", "2026-10-31T13:30:00-04:00"),
                timed_event("budget-2", "Budget review", "2026-10-31T14:00:00-04:00", "2026-10-31T14:30:00-04:00")
            ],
            "nextPageToken": null
        })
        .to_string(),
    );
    let ambiguous = run_gcal(&env, &fake, &["ack", "-D", "tomorrow", "Budget"]);
    assert_eq!(ambiguous.status.code(), Some(2), "{}", stderr(&ambiguous));
    assert!(stderr(&ambiguous).contains("several events match"));
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
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

#[test]
fn doctor_ack_clear_fixes_run_for_missing_cancelled_and_series_receipts() {
    let env = GcalEnv::new("bob-cli-gcal-ack-doctor-clear");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "missing-1",
        r#"{"_fail":{"stderr":"404 Not Found","code":1}}"#,
    );
    fake.event_lookup(
        "cancelled-1",
        &json!({"event": {
            "id": "cancelled-1",
            "summary": "Cancelled event",
            "status": "cancelled",
            "start": {"dateTime": "2026-10-30T13:00:00-04:00"},
            "end": {"dateTime": "2026-10-30T13:30:00-04:00"},
        }})
        .to_string(),
    );
    fake.event_lookup(
        "series-master",
        r#"{"_fail":{"stderr":"404 Not Found","code":1}}"#,
    );
    append_record(
        &env,
        &ack_record(
            "missing-1",
            "2026-10-30T13:00:00-04:00",
            "2026-10-30T13:30:00-04:00",
        ),
    );
    let doctor = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert!(doctor.status.success(), "{}", stderr(&doctor));
    let doctor_json = json_stdout(&doctor);
    let fix = doctor_json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "acks")
        .unwrap()["fix"]
        .as_str()
        .unwrap();
    assert_eq!(fix, "bob gcal ack -c -i missing-1 -C primary -s event");
    run_doctor_ack_fix(&env, &fake, fix);
    assert_eq!(
        ack_receipts(&journal_text(&env)).last().unwrap()["acked"],
        false
    );

    append_record(
        &env,
        &ack_record(
            "cancelled-1",
            "2026-10-30T13:00:00-04:00",
            "2026-10-30T13:30:00-04:00",
        ),
    );
    let doctor = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    let doctor_json = json_stdout(&doctor);
    let fix = doctor_json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "acks")
        .unwrap()["fix"]
        .as_str()
        .unwrap();
    assert_eq!(fix, "bob gcal ack -c -i cancelled-1 -C primary -s event");
    run_doctor_ack_fix(&env, &fake, fix);

    append_record(&env, &series_ack_record("series-master", true));
    let doctor = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    let doctor_json = json_stdout(&doctor);
    let fix = doctor_json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "acks")
        .unwrap()["fix"]
        .as_str()
        .unwrap();
    assert_eq!(fix, "bob gcal ack -c -i series-master -C primary -s series");
    // The matching event id is a series master without recurringEventId;
    // the exact journal key still identifies the receipt to clear.
    fake.event_lookup(
        "series-master",
        &json!({"event": {
            "id": "series-master",
            "summary": "Series master",
            "status": "confirmed",
            "start": {"dateTime": "2026-10-30T13:00:00-04:00"},
            "end": {"dateTime": "2026-10-30T13:30:00-04:00"},
        }})
        .to_string(),
    );
    run_doctor_ack_fix(&env, &fake, fix);
    let receipts = ack_receipts(&journal_text(&env));
    assert!(
        receipts.iter().any(|receipt| {
            receipt["scope"] == "series"
                && receipt["key"]["series"] == "series-master"
                && receipt["acked"] == false
        }),
        "series receipt clear appended: {receipts:?}"
    );
}

fn run_doctor_ack_fix(env: &GcalEnv, fake: &FakeGog, fix: &str) {
    let words = shlex::split(fix).expect("doctor command shell words");
    assert_eq!(&words[..2], &["bob", "gcal"]);
    let args: Vec<&str> = words[2..].iter().map(String::as_str).collect();
    let output = run_gcal(env, fake, &args);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("Cleared"), "{}", stdout(&output));
}

#[test]
fn orphan_clear_preview_is_read_only_and_reports_same_scope_history() {
    let env = GcalEnv::new("bob-cli-gcal-ack-clear-preview");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    append_record(&env, &series_ack_record("gym-series", true));
    append_record(&env, &series_note_record("gym-series", "Bring snacks"));
    append_record(
        &env,
        &ack_record(
            "gym-1",
            "2026-10-30T13:00:00-04:00",
            "2026-10-30T13:30:00-04:00",
        ),
    );
    let before = journal_text(&env);
    let dry = run_gcal(
        &env,
        &fake,
        &[
            "ack",
            "-c",
            "-d",
            "-i",
            "gym-series",
            "-C",
            "primary",
            "-s",
            "series",
        ],
    );
    assert!(dry.status.success(), "{}", stderr(&dry));
    assert!(stdout(&dry).contains("same-scope evidence: acked"));
    assert_eq!(journal_text(&env), before);
    let json_preview = run_gcal(
        &env,
        &fake,
        &[
            "ack",
            "-c",
            "-f",
            "json",
            "-i",
            "gym-series",
            "-C",
            "primary",
            "-s",
            "series",
        ],
    );
    assert!(json_preview.status.success(), "{}", stderr(&json_preview));
    let value = json_stdout(&json_preview);
    assert_eq!(value["action"], "preview");
    assert_eq!(value["proposed_action"]["scope"], "series");
    assert_eq!(value["selected"]["scope_evidence"], "acked");
    assert!(!value.to_string().contains("Bring snacks"));
    assert!(!value.to_string().contains("PRIVATE_SEEN_TITLE"));
    assert_eq!(journal_text(&env), before);

    let clear = run_gcal(
        &env,
        &fake,
        &[
            "ack",
            "-c",
            "-i",
            "gym-series",
            "-C",
            "primary",
            "-s",
            "series",
        ],
    );
    assert!(clear.status.success(), "{}", stderr(&clear));
    let message = stdout(&clear);
    assert!(
        message.contains("same-scope note history remains"),
        "{message}"
    );
    assert!(
        message.contains("other-scope evidence was left unchanged"),
        "{message}"
    );
    let after = journal_text(&env);
    assert_eq!(after.matches("\"kind\":\"gcal.note\"").count(), 1);
    assert!(
        ack_receipts(&after).iter().any(|receipt| {
            receipt["scope"] == "event"
                && receipt["key"]["event_id"] == "gym-1"
                && receipt["acked"] == true
        }),
        "other-scope receipt remains unchanged"
    );
}

#[test]
fn orphan_clear_requires_an_exact_account_calendar_scope_key() {
    let env = GcalEnv::new("bob-cli-gcal-ack-clear-boundaries");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    for id in ["unknown", "foreign", "wrong-calendar", "transport"] {
        fake.event_lookup(
            id,
            if id == "transport" {
                r#"{"_fail":{"stderr":"connection reset","code":1}}"#
            } else {
                r#"{"_fail":{"stderr":"404 Not Found","code":1}}"#
            },
        );
    }
    let mut foreign = ack_record(
        "foreign",
        "2026-10-30T13:00:00-04:00",
        "2026-10-30T13:30:00-04:00",
    );
    foreign["account"] = json!("other@example.com");
    append_record(&env, &foreign);
    append_record(
        &env,
        &ack_record(
            "wrong-calendar",
            "2026-10-30T13:00:00-04:00",
            "2026-10-30T13:30:00-04:00",
        ),
    );
    let initial = ack_receipts(&journal_text(&env)).len();

    let wrong_account = run_gcal(
        &env,
        &fake,
        &["ack", "-c", "-i", "foreign", "-C", "primary", "-s", "event"],
    );
    assert_eq!(wrong_account.status.code(), Some(2));
    assert_eq!(ack_receipts(&journal_text(&env)).len(), initial);

    let unknown = run_gcal(
        &env,
        &fake,
        &["ack", "-c", "-i", "unknown", "-C", "primary", "-s", "event"],
    );
    assert_eq!(unknown.status.code(), Some(2));
    assert_eq!(ack_receipts(&journal_text(&env)).len(), initial);
    let wrong_calendar = run_gcal(
        &env,
        &fake,
        &[
            "ack",
            "-c",
            "-i",
            "wrong-calendar",
            "-C",
            "secondary",
            "-s",
            "event",
        ],
    );
    assert_ne!(wrong_calendar.status.code(), Some(0));
    assert_eq!(ack_receipts(&journal_text(&env)).len(), initial);
    let transport = run_gcal(
        &env,
        &fake,
        &[
            "ack",
            "-c",
            "-i",
            "transport",
            "-C",
            "primary",
            "-s",
            "event",
        ],
    );
    assert_eq!(transport.status.code(), Some(1), "{}", stderr(&transport));
    assert!(stderr(&transport).contains("connection reset"));
    assert_eq!(ack_receipts(&journal_text(&env)).len(), initial);
    assert!(
        ack_receipts(&journal_text(&env))
            .iter()
            .any(|receipt| receipt["account"] == "other@example.com"
                && receipt["acked"] == true),
        "foreign account receipt stays live"
    );
}

// ---------- terminal review session ----------
//
// `BOB_GCAL_ASSUME_TTY` treats piped stdin as the session's key stream; the
// session reads keys and cooked lines unbuffered from the same descriptor,
// so these tests drive it with exact bytes.

fn session_events() -> String {
    json!({"events": [
        timed_event("task-1", "Budget review", "2026-10-31T13:00:00-04:00", "2026-10-31T13:30:00-04:00"),
        timed_event("task-2", "Hiring sync", "2026-11-02T13:00:00-05:00", "2026-11-02T13:30:00-05:00"),
    ], "nextPageToken": null})
    .to_string()
}

fn recurring_pair() -> Vec<Value> {
    let event = |id: &str, start: &str, end: &str| {
        json!({
            "id": id,
            "summary": "Little Gym",
            "status": "confirmed",
            "recurringEventId": "gym-series",
            "start": {"dateTime": start},
            "end": {"dateTime": end},
            "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "accepted"}],
        })
    };
    vec![
        event(
            "gym-1",
            "2026-10-31T09:15:00-04:00",
            "2026-10-31T10:00:00-04:00",
        ),
        event(
            "gym-2",
            "2026-11-03T09:15:00-05:00",
            "2026-11-03T10:00:00-05:00",
        ),
    ]
}

fn journal_text(env: &GcalEnv) -> String {
    fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
        .expect("read session journal")
}

fn ack_receipts(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value["kind"] == "gcal.ack")
        .collect()
}

fn note_records(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value["kind"] == "gcal.note")
        .collect()
}

/// Spawn `bob gcal` with piped stdin, writing each chunk after its delay so
/// separately timed presses stay distinct from one held-key burst.
fn run_gcal_script(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    extra_env: &[(&str, &str)],
    writes: &[(&str, u64)],
) -> std::process::Output {
    use std::process::Stdio;
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gcal");
    for arg in args {
        command.arg(arg);
    }
    command.env("BOB_GCAL_ASSUME_TTY", "1");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gcal");
    let mut stdin = child.stdin.take().expect("piped stdin");
    for (chunk, delay_ms) in writes {
        std::thread::sleep(std::time::Duration::from_millis(*delay_ms));
        if let Err(error) = stdin.write_all(chunk.as_bytes()) {
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe,
                "write stdin"
            );
            break;
        }
    }
    drop(stdin);
    child.wait_with_output().expect("wait for bob gcal")
}

fn write_exec_script(env: &GcalEnv, name: &str, body: &str) -> String {
    let path = env.vault().join(name);
    fs::write(&path, body).expect("write helper script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions =
            fs::metadata(&path).expect("stat helper").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod helper");
    }
    path.to_string_lossy().into_owned()
}

#[test]
fn session_ack_then_quit_saves_first_answer() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-quit");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output =
        run_gcal_script(&env, &fake, &["ack"], &[], &[("a", 400), ("q", 600)]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ Acked Budget review · event scope"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ 1 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(
        errors.contains("→ 1 remaining · rerun `bob gcal ack`"),
        "{errors}"
    );
    let receipts = ack_receipts(&journal_text(&env));
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["key"]["event_id"], "task-1");
}

#[test]
fn session_quit_reports_nothing_saved() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-empty-quit");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_stdin(&env, &fake, &["ack"], "q");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(errors.contains("Budget review"), "{errors}");
    assert!(errors.contains("─ 1 of 2 ─"), "{errors}");
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped · 0 series-covered"),
        "{errors}"
    );
    assert!(errors.contains("→ 2 remaining"), "{errors}");
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_return_is_inert_and_eof_quits() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-return");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_stdin(&env, &fake, &["ack"], "\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(errors.contains("→ 2 remaining"), "{errors}");
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_ctrl_c_byte_exits_130_after_saving() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-int");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_script(
        &env,
        &fake,
        &["ack"],
        &[],
        &[("a", 400), ("\x03", 600)],
    );
    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ 1 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(errors.contains("→ 1 remaining"), "{errors}");
    assert_eq!(ack_receipts(&journal_text(&env)).len(), 1);
}

#[test]
fn session_skip_returns_next_run() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-skip");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_stdin(&env, &fake, &["ack"], "sq");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("→ Skipped Budget review · returns next run"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 1 skipped"),
        "{errors}"
    );
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
    let listed = run_gcal(&env, &fake, &["ack", "-l", "-f", "json"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert_eq!(json_stdout(&listed)["counts"]["queue"], 2);
}

#[test]
fn session_recurring_ack_defaults_to_all_events() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-series");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": recurring_pair(), "nextPageToken": null}).to_string(),
    );
    // Empty scope answer takes the All events default: the sibling instance
    // leaves the queue as series-covered, never as a manual ack.
    let output = run_gcal_stdin(&env, &fake, &["ack"], "a\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ Acked Little Gym · series scope"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ 1 acked · 0 noted · 0 skipped · 1 series-covered"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ queue complete for this window"),
        "{errors}"
    );
    let receipts = ack_receipts(&journal_text(&env));
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["scope"], "series");
    assert_eq!(receipts[0]["key"]["series"], "gym-series");
}

#[test]
fn session_reports_waiting_replies_that_are_not_in_the_walk() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-waiting-footer");
    prepare(&env);
    let mut waiting = timed_event(
        "wait-1",
        "Unanswered invite",
        "2026-11-01T13:00:00-05:00",
        "2026-11-01T13:30:00-05:00",
    );
    waiting["attendees"][0]["responseStatus"] = json!("needsAction");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({
            "events": [
                timed_event("task-1", "Budget review", "2026-10-31T13:00:00-04:00", "2026-10-31T13:30:00-04:00"),
                waiting,
            ],
            "nextPageToken": null
        })
        .to_string(),
    );
    let output = run_gcal_stdin(&env, &fake, &["ack"], "q");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(
            "1 awaiting your reply in Google Calendar · include with -a"
        ),
        "{}",
        stderr(&output)
    );
}

#[test]
fn series_answer_refolds_waiting_replies_and_skipped_instances() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-series-waiting");
    prepare(&env);
    let mut series = recurring_pair();
    series[1]["attendees"][0]["responseStatus"] = json!("needsAction");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": series, "nextPageToken": null}).to_string(),
    );
    let output = run_gcal_stdin(&env, &fake, &["ack"], "a\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains(
            "1 awaiting your reply in Google Calendar · include with -a"
        ),
        "{errors}"
    );
    assert!(
        errors.contains(
            "0 awaiting your reply in Google Calendar · include with -a"
        ),
        "{errors}"
    );

    let env = GcalEnv::new("bob-cli-gcal-ack-session-series-skip-ack");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": recurring_pair(), "nextPageToken": null}).to_string(),
    );
    let output = run_gcal_stdin(&env, &fake, &["ack"], "sa\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ 1 acked · 0 noted · 0 skipped · 1 series-covered"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ queue complete for this window"),
        "{errors}"
    );
    assert!(!errors.contains("remaining"), "{errors}");
}

#[test]
fn series_note_covers_a_previously_skipped_instance() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-series-skip-note");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": recurring_pair(), "nextPageToken": null}).to_string(),
    );
    let output = run_gcal_stdin(&env, &fake, &["ack"], "snBring snacks\n2\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ 0 acked · 1 noted · 0 skipped · 1 series-covered"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ queue complete for this window"),
        "{errors}"
    );
    let notes = note_records(&journal_text(&env));
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["scope"], "series");
}

#[test]
fn session_recurring_ack_event_scope_keeps_sibling() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-event-scope");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": recurring_pair(), "nextPageToken": null}).to_string(),
    );
    let output = run_gcal_stdin(&env, &fake, &["ack"], "a1\nq");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ Acked Little Gym · event scope"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ 1 acked · 0 noted · 0 skipped · 0 series-covered"),
        "{errors}"
    );
    assert!(errors.contains("→ 1 remaining"), "{errors}");
    let receipts = ack_receipts(&journal_text(&env));
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["scope"], "event");
    assert_eq!(receipts[0]["key"]["event_id"], "gym-1");
}

#[test]
fn session_recurring_scope_eof_quits_cleanly() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-scope-eof");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": recurring_pair(), "nextPageToken": null}).to_string(),
    );
    // EOF inside the This-event scope prompt quits the session without an
    // answer and without touching the journal.
    let output = run_gcal_stdin(&env, &fake, &["ack"], "n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(errors.contains("→ 2 remaining"), "{errors}");
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_note_line_saves_and_answers() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-note");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_stdin(&env, &fake, &["ack"], "nBring snacks\nq");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ Noted Budget review · event scope · acked"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ 0 acked · 1 noted · 0 skipped"),
        "{errors}"
    );
    let notes = note_records(&journal_text(&env));
    assert_eq!(notes.len(), 1);
    assert!(notes[0]["text"].as_str().unwrap().contains("Bring snacks"));
}

#[test]
fn session_empty_note_returns_to_card() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-empty-note");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_stdin(&env, &fake, &["ack"], "n\nq");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(errors.contains("unchanged · nothing was saved"), "{errors}");
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_invalid_note_offer_decline_stays() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-invalid");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let noop = write_exec_script(&env, "noop-editor.sh", "#!/bin/sh\nexit 0\n");
    let output = run_gcal_script(
        &env,
        &fake,
        &["ack"],
        &[("VISUAL", &noop)],
        &[("n@no-such-root-xyz\n", 400), ("n\n", 400), ("q", 400)],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("Open the editor with this draft? [Y/n]"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_invalid_note_decline_keeps_no_record() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-invalid-decline");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let noop = write_exec_script(&env, "noop-editor.sh", "#!/bin/sh\nexit 0\n");
    let output = run_gcal_script(
        &env,
        &fake,
        &["ack"],
        &[("VISUAL", &noop)],
        &[
            ("n@no-such-root-xyz\n", 400),
            ("\n", 400),
            ("n\n", 400),
            ("q", 400),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(errors.contains("Edit again? [Y/n]"), "{errors}");
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_editor_appends_valid_text() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-editor");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let append = write_exec_script(
        &env,
        "append-editor.sh",
        "#!/bin/sh\n{ printf 'Bring snacks\\n'; cat \"$1\"; } > \"$1.new\" && mv \"$1.new\" \"$1\"\n",
    );
    let output = run_gcal_script(
        &env,
        &fake,
        &["ack"],
        &[("VISUAL", &append)],
        &[("e", 400)],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ Noted Budget review · event scope · acked"),
        "{errors}"
    );
    let notes = note_records(&journal_text(&env));
    assert_eq!(notes.len(), 1);
    assert!(notes[0]["text"].as_str().unwrap().contains("Bring snacks"));
}

#[test]
fn session_editor_failure_stays_on_card() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-editor-fail");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_script(
        &env,
        &fake,
        &["ack"],
        &[("VISUAL", "/bin/false")],
        &[("e", 400)],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(errors.contains("could not open the editor"), "{errors}");
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_adopt_answers_from_google_description() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-adopt");
    prepare(&env);
    let mut event = timed_event(
        "drive-1",
        "Airport pickup",
        "2026-10-31T13:00:00-04:00",
        "2026-10-31T13:30:00-04:00",
    );
    event["description"] = json!("-30 Drive to the airport");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &json!({"events": [event], "nextPageToken": null}).to_string(),
    );
    let output = run_gcal_stdin(&env, &fake, &["ack"], "A");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("press A to adopt")
            || errors.contains("⚑ Google description"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ Noted Airport pickup · event scope · acked"),
        "{errors}"
    );
    let notes = note_records(&journal_text(&env));
    assert_eq!(notes.len(), 1);
    assert!(notes[0]["text"].as_str().unwrap().contains("-30"));
}

#[test]
fn session_adopt_unavailable_stays_on_card() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-no-adopt");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let output = run_gcal_stdin(&env, &fake, &["ack"], "Aq");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("no Google description lines to adopt"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
}

#[test]
fn session_help_and_open_preserve_state() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-open");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    let out_file = env.vault().join("browser.out");
    let browser = write_exec_script(
        &env,
        "record-browser.sh",
        "#!/bin/sh\necho \"$1\" >> \"$BOB_GCAL_FAKE_BROWSER_OUT\"\n",
    );
    let output = run_gcal_script(
        &env,
        &fake,
        &["ack"],
        &[
            ("BOB_GCAL_BROWSER", browser.as_str()),
            (
                "BOB_GCAL_FAKE_BROWSER_OUT",
                out_file.to_string_lossy().as_ref(),
            ),
        ],
        &[("?", 400), ("o", 500), ("q", 900)],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(errors.contains("a  ack this event"), "{errors}");
    assert!(errors.contains("↗ Opened https://calendar.google.com/event?eid=task-1 in a browser"), "{errors}");
    assert!(
        errors.contains("✓ 0 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(!env.vault().join(".bob/gcal/test-host.jsonl").exists());
    let mut seen = String::new();
    for _ in 0..50 {
        seen = fs::read_to_string(&out_file).unwrap_or_default();
        if seen.contains("task-1") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(seen.contains("task-1"), "browser saw no URL: {seen:?}");
}

#[test]
fn session_burst_repeats_answer_only_once() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-burst");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    // One held-key burst arrives all at once: only the visible card answers.
    let output = run_gcal_stdin(&env, &fake, &["ack"], "aaaq");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ 1 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(errors.contains("→ 1 remaining"), "{errors}");
    let receipts = ack_receipts(&journal_text(&env));
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["key"]["event_id"], "task-1");
}

#[test]
fn session_timed_presses_answer_each_card() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-timed");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    // The same key pressed with settling time between cards answers twice,
    // proving the debounce only drops in-flight repeats.
    let output =
        run_gcal_script(&env, &fake, &["ack"], &[], &[("a", 400), ("a", 800)]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(
        errors.contains("✓ 2 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(
        errors.contains("✓ queue complete for this window"),
        "{errors}"
    );
    assert_eq!(ack_receipts(&journal_text(&env)).len(), 2);
}

#[test]
fn session_write_failure_keeps_earlier_answers() {
    let env = GcalEnv::new("bob-cli-gcal-ack-session-failure");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&session_events());
    use std::process::Stdio;
    let mut command = env.command();
    fake.install(&mut command);
    command.args(["gcal", "ack"]);
    command.env("BOB_GCAL_ASSUME_TTY", "1");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gcal");
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin.write_all(b"a").expect("answer first card");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let journal_dir = env.vault().join(".bob/gcal");
    let journal_file = fs::read_dir(&journal_dir)
        .expect("read journal dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.is_file())
        .expect("one journal file after the first answer");
    let first_answer = fs::read(&journal_file).expect("retain first answer");
    fs::remove_file(&journal_file).expect("remove journal file");
    fs::create_dir(&journal_file).expect("make journal unreadable");
    stdin
        .write_all(b"nBring snacks\n")
        .expect("submit second card note");
    drop(stdin);
    let output = child.wait_with_output().expect("wait for bob gcal");
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let errors = stderr(&output);
    assert!(errors.contains("read private-note evidence"), "{errors}");
    assert!(
        errors.contains("✓ 1 acked · 0 noted · 0 skipped"),
        "{errors}"
    );
    assert!(!errors.contains("✓ Noted Hiring sync"), "{errors}");
    fs::remove_dir(&journal_file).expect("remove unreadable journal fixture");
    fs::write(&journal_file, first_answer).expect("restore first answer");
    let receipts = ack_receipts(&journal_text(&env));
    assert_eq!(receipts.len(), 1, "the failed answer was not appended");
    assert_eq!(receipts[0]["key"]["event_id"], "task-1");
    assert!(note_records(&journal_text(&env)).is_empty());
}

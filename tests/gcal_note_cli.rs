//! `bob gcal note`: command skeleton, selection, and read-only modes.

mod gcal_support;

use gcal_support::{
    json_stdout, run_gcal_stdin, stderr, stdout, FakeGog, GcalEnv, ACCOUNT,
};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
};

fn prepare(env: &GcalEnv) {
    env.prepare_healthy_vault();
    for area in ["body", "job", "health", "family", "abby"] {
        fs::write(
            env.vault().join(format!("{area}.md")),
            format!("---\ntype: area\n---\n# {area}\n"),
        )
        .expect("write area");
    }
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n",
    );
}

fn append_journal(env: &GcalEnv, host: &str, value: Value) {
    let dir = env.vault().join(".bob/gcal");
    fs::create_dir_all(&dir).expect("journal dir");
    let path = dir.join(format!("{host}.jsonl"));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("open journal");
    writeln!(file, "{}", serde_json::to_string(&value).expect("json"))
        .expect("append journal");
}

fn event_note(host: &str, ts: &str, event_id: &str, text: &str) -> Value {
    json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": "gcal.note",
        "account": ACCOUNT,
        "key": {"calendar_id": "primary", "event_id": event_id},
        "scope": "event",
        "text": text,
    })
}

fn guest_event(id: &str, summary: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id,
        "summary": summary,
        "status": "confirmed",
        "start": {"dateTime": start},
        "end": {"dateTime": end},
        "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
        "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    })
}

fn events_envelope(events: Vec<Value>) -> String {
    serde_json::to_string(&json!({
        "events": events,
        "nextPageToken": Value::Null,
    }))
    .expect("envelope")
}

fn assert_no_long_only_option_lines(label: &str, help: &str) {
    for line in help.lines() {
        let trimmed = line.trim_start();
        let starts_with_long_option = trimmed
            .strip_prefix("--")
            .and_then(|tail| tail.chars().next())
            .is_some_and(|first| first.is_ascii_alphabetic());
        if starts_with_long_option {
            panic!(
                "{label} exposes a long-only option line:\n{line}\n\n{help}"
            );
        }
    }
}

fn assert_text_order(text: &str, needles: &[&str]) {
    let mut last = 0;
    for needle in needles {
        let position = text
            .find(needle)
            .unwrap_or_else(|| panic!("expected `{needle}` in text:\n{text}"));
        assert!(position >= last, "`{needle}` is out of order:\n{text}");
        last = position;
    }
}

#[test]
fn note_help_pins_sorted_options_and_examples() {
    let env = GcalEnv::new("bob-cli-gcal-note-help");
    let output = env
        .command()
        .args(["gcal", "note", "--help"])
        .output()
        .expect("note help");
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    assert_text_order(
        &help,
        &[
            "-A, --adopt",
            "-a, --all",
            "-b, --bob-dir",
            "-C, --calendar",
            "-c, --clear",
            "-D, --day",
            "-d, --dry-run",
            "-e, --edit",
            "-i, --event-id",
            "-f, --format",
            "-h, --help",
            "-l, --list",
            "-s, --scope",
        ],
    );
    for needle in [
        "bob gcal note",
        "bob gcal note -a -f json",
        "BOB_GCAL_ASSUME_TTY",
        "QUERY",
        "TEXT",
    ] {
        assert!(help.contains(needle), "missing `{needle}`:\n{help}");
    }
    assert_no_long_only_option_lines("bob gcal note --help", &help);
}

#[test]
fn write_paths_exit_2_not_available_yet() {
    let env = GcalEnv::new("bob-cli-gcal-note-write-stub");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "e1",
        "Deerwood",
        "2026-12-13T10:00:00-05:00",
        "2026-12-13T12:00:00-05:00",
    )]));
    for args in [
        vec!["note", "deerwood", "@family"],
        vec!["note", "deerwood", "-c"],
        vec!["note", "deerwood", "-d"],
        vec!["note", "deerwood", "-e"],
        vec!["note", "deerwood", "-A"],
        vec!["note", "-i", "e1", "--", "@family"],
    ] {
        let mut command = env.command();
        let output = fake
            .install(&mut command)
            .args([vec!["gcal"], args.clone()].concat())
            .output()
            .expect("run note write stub");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("later phase"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
    // Buffer-like TEXT is never an option: `-30` becomes TEXT, then exits
    // 2 as not-available (not a clap usage error about `-3`).
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-30"])
        .output()
        .expect("run note -30");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("later phase"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn list_and_show_round_trip_with_notes() {
    let env = GcalEnv::new("bob-cli-gcal-note-list");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![
        guest_event(
            "deerwood",
            "Deerwood. Christmas breakfast",
            "2026-12-13T10:00:00-05:00",
            "2026-12-13T12:00:00-05:00",
        ),
        guest_event(
            "gym",
            "Little Gym",
            "2026-11-13T09:15:00-05:00",
            "2026-11-13T10:00:00-05:00",
        ),
    ]));
    append_journal(
        &env,
        "test-host",
        event_note(
            "test-host",
            "2026-10-30T12:00:00-04:00",
            "deerwood",
            "@family\n-30 Drive to Deerwood\n+15 Drive home",
        ),
    );
    // Bare list shows only noted events.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note"])
        .output()
        .expect("run note list");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("1 private note"), "{listing}");
    assert!(listing.contains("Deerwood"), "{listing}");
    assert!(!listing.contains("Little Gym"), "{listing}");
    // `-a` lists every upcoming event, noted or not.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-a"])
        .output()
        .expect("run note -a");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("Deerwood"), "{listing}");
    assert!(listing.contains("Little Gym"), "{listing}");
    // Show prints the note words plus the effective root/buffer/clock.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "deerwood"])
        .output()
        .expect("run note show");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let shown = stdout(&output);
    assert!(shown.contains("Deerwood. Christmas breakfast"), "{shown}");
    assert!(shown.contains("@family"), "{shown}");
    assert!(shown.contains("-30 Drive to Deerwood"), "{shown}");
    // List JSON carries no note text; show JSON does.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-f", "json"])
        .output()
        .expect("run note list json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["schema_version"], 1);
    let notes = document["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0]["note"]["text"].is_null()
            || notes[0]["note"].get("text").is_none(),
        "list JSON must not carry note text: {notes:?}"
    );
    assert_eq!(notes[0]["calendar_id"], "primary");
    assert_eq!(notes[0]["event_id"], "deerwood");
    assert_eq!(notes[0]["organizer"]["name"], "Kelly");
    assert_eq!(notes[0]["organizer"]["self"], false);
    assert!(
        !stdout(&output).contains("Drive to Deerwood"),
        "list JSON leaked note text"
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "deerwood", "-f", "json"])
        .output()
        .expect("run note show json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert_eq!(document["ok"], true);
    let text = document["event"]["note"]["text"].as_str().unwrap_or("");
    assert!(text.contains("@family"), "{document}");
    assert!(text.contains("-30 Drive to Deerwood"), "{document}");
}

#[test]
fn matching_requires_every_word_and_ranks_exact_first() {
    let env = GcalEnv::new("bob-cli-gcal-note-match");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![
        guest_event(
            "exact",
            "Deerwood",
            "2026-12-13T10:00:00-05:00",
            "2026-12-13T11:00:00-05:00",
        ),
        guest_event(
            "prefix",
            "Deerwood Christmas breakfast",
            "2026-12-14T10:00:00-05:00",
            "2026-12-14T11:00:00-05:00",
        ),
        guest_event(
            "contains",
            "Christmas Deerwood party",
            "2026-12-15T10:00:00-05:00",
            "2026-12-15T11:00:00-05:00",
        ),
    ]));
    // Every QUERY word must appear: "deerwood christmas" matches the two
    // multi-word titles, not the exact one-word title.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-a",
            "-f",
            "json",
            "-l",
            "deerwood",
            "christmas",
        ])
        .output()
        .expect("run filtered list");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    let events = document["events"].as_array().expect("events");
    assert_eq!(events.len(), 2, "{document}");
    // Exact title ranks first for a single-word query.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "deerwood", "-f", "json", "-s", "event"])
        .output()
        .expect("run ambiguous show json");
    // Three matches: exact + prefix + contains. JSON never prompts.
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["kind"], "ambiguous");
    let candidates = document["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0]["title"], "Deerwood");
}

#[test]
fn series_instances_group_and_scope_is_required_off_tty() {
    let env = GcalEnv::new("bob-cli-gcal-note-series");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let recurring = |id: &str, start: &str, end: &str| {
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
    fake.events_json(&events_envelope(vec![
        recurring(
            "gym-1",
            "2026-11-13T09:15:00-05:00",
            "2026-11-13T10:00:00-05:00",
        ),
        recurring(
            "gym-2",
            "2026-11-20T09:15:00-05:00",
            "2026-11-20T10:00:00-05:00",
        ),
    ]));
    // One grouped candidate, but recurring without `-s` off a TTY exits 2.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "little", "gym"])
        .output()
        .expect("run recurring without scope");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("This event")
            || stderr(&output).contains("needs_scope")
            || stderr(&output).contains("scope"),
        "{}",
        stderr(&output)
    );
    // `-s series` on the grouped series shows the soonest occurrence.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "little", "gym", "-s", "series", "-f", "json",
        ])
        .output()
        .expect("run recurring with series scope");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["event"]["title"], "Little Gym");
    assert_eq!(document["event"]["series_id"], "gym-series");
    assert_eq!(document["scope"], "series");
    // `-s series` on a one-off event exits 2.
    let env2 = GcalEnv::new("bob-cli-gcal-note-series-oneoff");
    prepare(&env2);
    let fake2 = FakeGog::new(&env2, "fake-gog");
    fake2.events_json(&events_envelope(vec![guest_event(
        "solo",
        "Solo Lunch",
        "2026-11-13T12:00:00-05:00",
        "2026-11-13T13:00:00-05:00",
    )]));
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args(["gcal", "note", "solo", "-s", "series"])
        .output()
        .expect("run series scope on one-off");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

#[test]
fn url_eid_decodes_and_verify_and_fallback_works() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let env = GcalEnv::new("bob-cli-gcal-note-url");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "evt123",
        "Board Meeting",
        "2026-11-15T10:00:00-05:00",
        "2026-11-15T11:00:00-05:00",
    )]));
    // Fake exact lookup: the event dir answers `evt123` for any calendar.
    fake.event_lookup(
        "evt123",
        &serde_json::to_string(&json!({
            "event": {
                "id": "evt123",
                "summary": "Board Meeting",
                "status": "confirmed",
                "start": {"dateTime": "2026-11-15T10:00:00-05:00"},
                "end": {"dateTime": "2026-11-15T11:00:00-05:00"},
                "htmlLink": "https://calendar.google.com/event?eid=evt123",
                "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
                "attendees": [{"self": true, "responseStatus": "accepted"}],
            }
        }))
        .expect("lookup body"),
    );
    let payload = URL_SAFE_NO_PAD.encode("evt123 extra-calendar-part");
    let url =
        format!("https://calendar.google.com/calendar/event?eid={payload}");
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", &url, "-f", "json"])
        .output()
        .expect("run note url");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert_eq!(document["event"]["event_id"], "evt123");
    // Unknown id falls back to a title search with a warning, then exits 2
    // because the URL carries no title words.
    let bad_payload = URL_SAFE_NO_PAD.encode("missing-id other-part");
    let bad_url =
        format!("https://calendar.google.com/calendar/event?eid={bad_payload}");
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", &bad_url])
        .output()
        .expect("run note bad url");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("falling back")
            || stderr(&output).contains("no events match"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn event_id_lookup_and_calendar_pin_and_ambiguous_ids() {
    let env = GcalEnv::new("bob-cli-gcal-note-id");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  calendars:\n    - id: primary\n    - id: work@group.calendar.google.com\n",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![]));
    fake.event_lookup(
        "dup",
        &serde_json::to_string(&json!({
            "event": {
                "id": "dup",
                "summary": "Dup Meeting",
                "status": "confirmed",
                "start": {"dateTime": "2026-11-16T10:00:00-05:00"},
                "end": {"dateTime": "2026-11-16T11:00:00-05:00"},
                "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
                "attendees": [{"self": true, "responseStatus": "accepted"}],
            }
        }))
        .expect("lookup body"),
    );
    // The fake answers the same id for both calendars: without `-C` the
    // lookup is ambiguous.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "dup"])
        .output()
        .expect("run ambiguous id");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("more than one calendar"),
        "{}",
        stderr(&output)
    );
    // Pinning the calendar resolves the ambiguity.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "dup", "-C", "primary", "-f", "json"])
        .output()
        .expect("run pinned id");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json_stdout(&output)["event"]["event_id"], "dup");
}

#[test]
fn tty_picker_and_scope_prompt_select_recurring() {
    let env = GcalEnv::new("bob-cli-gcal-note-picker");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![
        guest_event(
            "a1",
            "Alpha Planning",
            "2026-11-10T10:00:00-05:00",
            "2026-11-10T11:00:00-05:00",
        ),
        guest_event(
            "a2",
            "Alpha Review",
            "2026-11-11T10:00:00-05:00",
            "2026-11-11T11:00:00-05:00",
        ),
    ]));
    // Two different events match "alpha": off a TTY exits 2 with candidates.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "alpha"])
        .output()
        .expect("run ambiguous off tty");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("several events match"),
        "{}",
        stderr(&output)
    );
    // JSON off TTY carries the candidates array.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "alpha", "-f", "json"])
        .output()
        .expect("run ambiguous json");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert_eq!(document["error"]["kind"], "ambiguous");
    assert_eq!(
        document["candidates"].as_array().expect("candidates").len(),
        2
    );
    // On a TTY the picker chooses the second row.
    let output = run_gcal_stdin(&env, &fake, &["note", "alpha"], "2\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Alpha Review"),
        "{}",
        stdout(&output)
    );
    // Recurring scope prompt: first the picker is skipped (one group), then
    // the scope prompt picks All events (human mode reads the choice).
    let env2 = GcalEnv::new("bob-cli-gcal-note-scope-tty");
    prepare(&env2);
    let fake2 = FakeGog::new(&env2, "fake-gog");
    let recurring = |id: &str, start: &str, end: &str| {
        json!({
            "id": id,
            "summary": "Standup",
            "status": "confirmed",
            "recurringEventId": "standup-series",
            "start": {"dateTime": start},
            "end": {"dateTime": end},
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "accepted"}],
        })
    };
    fake2.events_json(&events_envelope(vec![
        recurring(
            "s1",
            "2026-11-13T09:00:00-05:00",
            "2026-11-13T09:30:00-05:00",
        ),
        recurring(
            "s2",
            "2026-11-20T09:00:00-05:00",
            "2026-11-20T09:30:00-05:00",
        ),
    ]));
    let output = run_gcal_stdin(&env2, &fake2, &["note", "standup"], "2\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("Standup"), "{}", stdout(&output));
    // JSON never prompts: `-s series` selects the scope without stdin.
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args(["gcal", "note", "standup", "-f", "json", "-s", "series"])
        .output()
        .expect("run series json with scope flag");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json_stdout(&output)["scope"], "series");
}

#[test]
fn declined_and_all_day_events_are_excluded() {
    let env = GcalEnv::new("bob-cli-gcal-note-excluded");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![
        json!({
            "id": "declined",
            "summary": "Declined Meeting",
            "status": "confirmed",
            "start": {"dateTime": "2026-11-12T10:00:00-05:00"},
            "end": {"dateTime": "2026-11-12T11:00:00-05:00"},
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "declined"}],
        }),
        json!({
            "id": "holiday",
            "summary": "Holiday",
            "status": "confirmed",
            "start": {"date": "2026-11-12"},
            "end": {"date": "2026-11-13"},
            "attendees": [],
        }),
        guest_event(
            "ok",
            "Okay Meeting",
            "2026-11-12T12:00:00-05:00",
            "2026-11-12T13:00:00-05:00",
        ),
    ]));
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-a", "-f", "json"])
        .output()
        .expect("run note -a");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let titles: Vec<String> = json_stdout(&output)["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["title"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(titles.contains(&"Okay Meeting".to_string()), "{titles:?}");
    assert!(
        !titles.iter().any(|title| title.contains("Declined")),
        "{titles:?}"
    );
    assert!(
        !titles.iter().any(|title| title.contains("Holiday")),
        "{titles:?}"
    );
    // A query matching only an excluded event exits 2 with no match.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "declined"])
        .output()
        .expect("run excluded query");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

#[test]
fn list_orphans_and_past_hiding() {
    let env = GcalEnv::new("bob-cli-gcal-note-orphans");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "live",
        "Live Meeting",
        "2026-11-14T10:00:00-05:00",
        "2026-11-14T11:00:00-05:00",
    )]));
    // A live note, a gone note (no lookup file), and a past note.
    append_journal(
        &env,
        "test-host",
        event_note("test-host", "2026-10-30T12:00:00-04:00", "live", "@family"),
    );
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-30T12:01:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "gone-1"},
            "scope": "event",
            "text": "@family",
            "seen": {"title": "Gone Meeting"},
        }),
    );
    fake.event_lookup(
        "past-1",
        &serde_json::to_string(&json!({
            "event": {
                "id": "past-1",
                "summary": "Past Meeting",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-01T10:00:00-04:00"},
                "end": {"dateTime": "2026-10-01T11:00:00-04:00"},
                "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
                "attendees": [{"self": true, "responseStatus": "accepted"}],
            }
        }))
        .expect("past lookup"),
    );
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-30T12:02:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "past-1"},
            "scope": "event",
            "text": "@family",
            "seen": {"title": "Past Meeting"},
        }),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note"])
        .output()
        .expect("run orphans");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("Live Meeting"), "{listing}");
    assert!(listing.contains("Gone Meeting"), "{listing}");
    assert!(listing.contains("no longer on your calendar"), "{listing}");
    assert!(listing.contains("bob gcal note -c -i gone-1"), "{listing}");
    assert!(!listing.contains("Past Meeting"), "{listing}");
}

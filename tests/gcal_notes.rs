//! Private event notes: hand-written `gcal.note` journal records layered
//! over Google descriptions in list and sync.

mod gcal_support;

use gcal_support::{json_stdout, stderr, stdout, FakeGog, GcalEnv, ACCOUNT};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    process::Output,
};

fn prepare(env: &GcalEnv) {
    env.prepare_healthy_vault();
    fs::write(
        env.vault().join("meet.md"),
        "---\ntype: area\ncolor: violet\ntask_type: MEET\n---\n# Meet\n",
    )
    .expect("write meet area");
    for area in ["body", "job", "health", "family"] {
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

fn sync_output(env: &GcalEnv, fake: &FakeGog, args: &[&str]) -> Output {
    let mut command = env.command();
    let mut full = vec!["gcal", "sync"];
    full.extend(args);
    fake.install(&mut command)
        .args(full)
        .output()
        .expect("run sync")
}

fn list_json(env: &GcalEnv, fake: &FakeGog, args: &[&str]) -> Output {
    let mut command = env.command();
    let mut full = vec!["gcal", "list", "-f", "json"];
    full.extend(args);
    fake.install(&mut command)
        .args(full)
        .output()
        .expect("run list")
}

fn today_note(env: &GcalEnv) -> String {
    fs::read_to_string(env.vault().join("2026/20261030.md"))
        .expect("read daily note")
}

fn write_today(env: &GcalEnv, note: &str) {
    fs::write(env.vault().join("2026/20261030.md"), note).expect("write note");
}

fn event_row(note: &str, needle: &str) -> String {
    note.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no {needle} row:\n{note}"))
        .to_string()
}

fn events_envelope(events: Vec<Value>) -> String {
    serde_json::to_string(&json!({
        "events": events,
        "nextPageToken": Value::Null,
    }))
    .expect("envelope")
}

fn guest_event(
    id: &str,
    summary: &str,
    description: Option<&str>,
    start: &str,
    end: &str,
) -> Value {
    let mut event = json!({
        "id": id,
        "summary": summary,
        "status": "confirmed",
        "start": {"dateTime": start},
        "end": {"dateTime": end},
        "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
        "organizer": {"email": "kelly@example.com", "self": false},
        "attendees": [
            {"self": true, "responseStatus": "accepted"},
            {
                "email": "kelly@example.com",
                "organizer": true,
                "responseStatus": "accepted"
            }
        ],
    });
    if let Some(description) = description {
        event["description"] = json!(description);
    }
    event
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

fn event_note(
    host: &str,
    ts: &str,
    event_id: &str,
    text: &str,
    account: &str,
) -> Value {
    json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": "gcal.note",
        "account": account,
        "key": {"calendar_id": "primary", "event_id": event_id},
        "scope": "event",
        "text": text,
    })
}

fn series_note(host: &str, ts: &str, series: &str, text: &str) -> Value {
    json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": "gcal.note",
        "account": ACCOUNT,
        "key": {"calendar_id": "primary", "series": series},
        "scope": "series",
        "text": text,
    })
}

fn json_report(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).expect("JSON report")
}

fn list_event<'a>(report: &'a Value, title: &str) -> &'a Value {
    report["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|row| row["title"] == title)
        .unwrap_or_else(|| panic!("no listed {title}: {report}"))
}

fn sync_row<'a>(report: &'a Value, title: &str) -> &'a Value {
    report["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["title"] == title)
        .unwrap_or_else(|| panic!("no sync row {title}: {report}"))
}

const SECRET: &str = "SECRET_NOTE_PHRASE_STROLLER";

#[test]
fn locked_guest_is_born_with_note_root_buffer_and_reasons() {
    let env = GcalEnv::new("bob-cli-gcal-notes-birth");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive to Deerwood\n+15 Drive home\nBring gifts",
            ACCOUNT,
        ),
    );

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = event_row(&note, "^deerwood");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(row.contains("[buffer:: -30m +15m]"), "{row}");
    assert!(
        note.contains("\t- \u{21a4} *30m before* \u{00b7} Drive to Deerwood"),
        "{note}"
    );
    assert!(
        note.contains("\t- \u{21a6} *15m after* \u{00b7} Drive home"),
        "{note}"
    );
    assert!(note.contains("\t- Bring gifts"), "{note}");
    let report = json_report(&output);
    let added = sync_row(&report, "Deerwood. Christmas breakfast");
    assert_eq!(added["root"], "family");
    assert_eq!(added["source"], "✎ @family");
    assert_eq!(added["note"]["scope"], "event");
    assert_eq!(added["note"]["bullet_lines"], 1);
    let human = sync_output(&env, &fake, &[]);
    assert!(
        stdout(&human).contains("via ✎ @family")
            || today_note(&env).contains("[root:: family]"),
        "{}",
        stdout(&human)
    );
}

#[test]
fn note_after_birth_updates_untouched_row_and_refiles_meet() {
    let env = GcalEnv::new("bob-cli-gcal-notes-refile");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "breakfast",
        "Breakfast",
        None,
        "2026-10-30T08:15:00-04:00",
        "2026-10-30T08:30:00-04:00",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let original = today_note(&env);
    assert!(original.contains(
        "- [x] (**0815-0830** [t:: 15m]) — MEET\n\t- 🍅 ~~[[#^breakfast]]~~"
    ), "{original}");
    assert!(!event_row(&original, "^breakfast").contains("[buffer::"));

    append_journal(
        &env,
        "test-host",
        event_note(
            "test-host",
            "2026-10-30T12:01:00-04:00",
            "breakfast",
            "@family\n-15 Walk over",
            ACCOUNT,
        ),
    );
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_report(&output);
    assert_eq!(report["pomodoro"]["links_refiled"], 1, "{report}");
    let updated = sync_row(&report, "Breakfast");
    assert_eq!(updated["action"], "updated");
    let note = today_note(&env);
    let row = event_row(&note, "^breakfast");
    assert!(row.contains("[buffer:: -15m]"), "{row}");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(note.contains(
        "- [x] (**0800-0815** [t:: 15m]) — MEET\n\t- 🍅 ~~[[#^breakfast]]~~"
    ) || note.contains("(**0800-0830**"), "{note}");
}

#[test]
fn row_edited_root_is_kept_and_reported_once() {
    let env = GcalEnv::new("bob-cli-gcal-notes-kept-root");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let edited = today_note(&env).replace("[root:: family]", "[root:: job]");
    write_today(&env, &edited);
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-30T12:02:00-04:00",
            "deerwood",
            "@body",
            ACCOUNT,
        ),
    );
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("kept your [root:: job]"),
        "kept_root: {combined}"
    );
    assert!(
        event_row(&today_note(&env), "^deerwood").contains("[root:: job]"),
        "{}",
        today_note(&env)
    );
    let again = sync_output(&env, &fake, &[]);
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(
        !combined.contains("kept your [root:: job]"),
        "reported once: {combined}"
    );
}

#[test]
fn occurrence_beats_series_beats_google_per_field() {
    let env = GcalEnv::new("bob-cli-gcal-notes-precedence");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![json!({
        "id": "park-1",
        "summary": "Park picnic @body",
        "description": "#off\n-30 Google before\n+15 Google after\nBring insurance card",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T18:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T19:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=park-1",
        "recurringEventId": "series-park",
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    })]));
    append_journal(
        &env,
        "athena",
        series_note(
            "athena",
            "2026-10-29T10:00:00-04:00",
            "series-park",
            "@job\n+20 Series after\nSeries snacks",
        ),
    );
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T11:00:00-04:00",
            "park-1",
            "@family\n#on\n-0\nBring the stroller",
            ACCOUNT,
        ),
    );

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = event_row(&note, "^park-picnic");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(!row.contains("@body"), "{row}");
    assert!(row.contains("#on") || !row.contains("#off"), "{row}");
    assert!(!row.contains("[buffer:: -"), "{row}");
    assert!(row.contains("[buffer:: +20m]"), "{row}");
    assert!(
        !note.contains("Google before"),
        "occurrence -0 drops Google's before reason:\n{note}"
    );
    assert!(
        !note.contains("Google after"),
        "series after drops Google's after reason:\n{note}"
    );
    assert!(
        note.contains("\t- \u{21a6} *20m after* \u{00b7} Series after"),
        "{note}"
    );
    assert!(note.contains("\t- Bring the stroller"), "{note}");
    assert!(note.contains("\t- Series snacks"), "{note}");
    assert!(note.contains("\t- Bring insurance card"), "{note}");
    let report = json_report(&output);
    let added = sync_row(&report, "Park picnic");
    assert_eq!(added["source"], "✎ @family");
    assert_eq!(added["note"]["scope"], "both");
}

#[test]
fn clearing_a_note_restores_google_on_an_untouched_row() {
    let env = GcalEnv::new("bob-cli-gcal-notes-clear");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "dentist",
        "Dentist @body",
        Some("Bring insurance card\n-30 Google before"),
        "2026-10-30T18:30:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:00:00-04:00",
            "dentist",
            "@family\n-45 Note before",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let first = event_row(&today_note(&env), "^dentist");
    assert!(first.contains("[root:: family]"), "{first}");
    assert!(first.contains("[buffer:: -45m]"), "{first}");

    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-30T12:01:00-04:00",
            "dentist",
            "",
            ACCOUNT,
        ),
    );
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let row = event_row(&today_note(&env), "^dentist");
    assert!(row.contains("[root:: body]"), "{row}");
    assert!(row.contains("[buffer:: -30m]"), "{row}");
    assert!(
        today_note(&env).contains("Google before"),
        "{}",
        today_note(&env)
    );
}

#[test]
fn other_host_journal_applies_and_last_writer_wins() {
    let env = GcalEnv::new("bob-cli-gcal-notes-hosts");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T10:00:00-04:00",
            "deerwood",
            "@family",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(
        event_row(&today_note(&env), "^deerwood").contains("[root:: family]"),
        "{}",
        today_note(&env)
    );

    append_journal(
        &env,
        "mac",
        event_note(
            "mac",
            "2026-10-29T11:00:00-04:00",
            "deerwood",
            "@job",
            ACCOUNT,
        ),
    );
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        event_row(&today_note(&env), "^deerwood").contains("[root:: job]"),
        "last writer wins:\n{}",
        today_note(&env)
    );
}

#[test]
fn force_reset_keeps_and_reapplies_notes() {
    let env = GcalEnv::new("bob-cli-gcal-notes-force");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let template = "# Friday 2026-10-30\n\n## Pomodoros\n";
    write_today(&env, template);
    let reset = sync_output(&env, &fake, &["-F", "-f", "json"]);
    assert_eq!(reset.status.code(), Some(0), "{}", stderr(&reset));
    let row = event_row(&today_note(&env), "^deerwood");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(row.contains("[buffer:: -30m]"), "{row}");
    let journal =
        fs::read_to_string(env.vault().join(".bob/gcal/athena.jsonl"))
            .expect("athena journal");
    assert!(journal.contains("gcal.note"), "{journal}");
}

#[test]
fn awaiting_reply_note_waits_applies_on_yes_and_skips_declined() {
    let env = GcalEnv::new("bob-cli-gcal-notes-rsvp");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let mut reply = guest_event(
        "review",
        "Design review",
        None,
        "2026-10-30T12:00:00-04:00",
        "2026-10-30T12:30:00-04:00",
    );
    reply["attendees"] =
        json!([{"self": true, "responseStatus": "needsAction"}]);
    let mut declined = guest_event(
        "party",
        "Declined invitation",
        None,
        "2026-10-30T13:00:00-04:00",
        "2026-10-30T13:30:00-04:00",
    );
    declined["attendees"] =
        json!([{"self": true, "responseStatus": "declined"}]);
    fake.events_json(&events_envelope(vec![reply.clone(), declined.clone()]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T09:00:00-04:00",
            "review",
            "@family\n-30 Prep",
            ACCOUNT,
        ),
    );
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T09:00:00-04:00",
            "party",
            "@family\n-30 Never",
            ACCOUNT,
        ),
    );

    let first = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let note = today_note(&env);
    assert!(!note.contains("Design review"), "{note}");
    assert!(!note.contains("Declined invitation"), "{note}");
    assert!(!note.contains("[root:: family]"), "{note}");

    reply["attendees"] = json!([{"self": true, "responseStatus": "accepted"}]);
    fake.events_json(&events_envelope(vec![reply, declined]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = event_row(&note, "Design review");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(row.contains("[buffer:: -30m]"), "{row}");
    assert!(!note.contains("Declined invitation"), "{note}");
}

#[test]
fn note_follows_an_event_moved_to_another_day() {
    let env = GcalEnv::new("bob-cli-gcal-notes-moved");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let today = guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    );
    fake.events_json(&events_envelope(vec![today.clone()]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(
        event_row(&today_note(&env), "^deerwood").contains("[root:: family]")
    );

    let mut moved = today;
    moved["start"] = json!({"dateTime": "2026-10-31T09:00:00-04:00"});
    moved["end"] = json!({"dateTime": "2026-10-31T10:00:00-04:00"});
    fake.events_json(&events_envelope(Vec::new()));
    fake.event_lookup(
        "deerwood",
        &serde_json::to_string(&json!({"event": moved.clone()}))
            .expect("lookup"),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(
        today_note(&env).contains("Moved to Sat 2026-10-31"),
        "{}",
        today_note(&env)
    );

    fs::write(
        env.vault().join("2026/20261031.md"),
        "# Saturday 2026-10-31\n\n## Pomodoros\n",
    )
    .expect("write tomorrow");
    fake.events_json(&events_envelope(vec![moved]));
    let output = sync_output(&env, &fake, &["-D", "2026-10-31"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let tomorrow = fs::read_to_string(env.vault().join("2026/20261031.md"))
        .expect("read tomorrow");
    let row = event_row(&tomorrow, "^deerwood");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(row.contains("[buffer:: -30m]"), "{row}");
}

#[test]
fn note_does_not_move_newest_or_count_as_malformed() {
    let env = GcalEnv::new("bob-cli-gcal-notes-newest");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "mac",
        json!({
            "v": 1,
            "ts": "2026-10-30T05:24:20-04:00",
            "host": "mac",
            "kind": "gcal.row",
            "key": {"calendar_id": "primary", "event_id": "e1"},
            "day": "2026-10-30",
            "block_id": "haircut",
            "row": "task",
            "spans": {"title": "abc"},
            "path": "2026/20261030.md",
        }),
    );
    append_journal(
        &env,
        "mac",
        event_note(
            "mac",
            "2026-10-30T06:00:00-04:00",
            "deerwood",
            "@family",
            ACCOUNT,
        ),
    );

    let listed = list_json(&env, &fake, &[]);
    assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
    let report = json_report(&listed);
    assert_eq!(report["last_sync"]["host"], "mac");
    assert_eq!(report["last_sync"]["ts"], "2026-10-30T05:24:20-04:00");

    let mut command = env.command();
    fake.install(&mut command);
    let doctor = command
        .args(["gcal", "doctor", "-f", "json"])
        .output()
        .expect("doctor");
    assert_eq!(doctor.status.code(), Some(0), "{}", stderr(&doctor));
    let doc = json_stdout(&doctor);
    let malformed = doc["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["id"] == "journal_malformed")
        .expect("malformed check");
    assert_eq!(malformed["status"], "ok", "{malformed}");
}

#[test]
fn list_and_sync_json_carry_no_note_text() {
    let env = GcalEnv::new("bob-cli-gcal-notes-privacy");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            &format!("@family\n{SECRET}"),
            ACCOUNT,
        ),
    );

    let sync = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(sync.status.code(), Some(0), "{}", stderr(&sync));
    let sync_text = stdout(&sync);
    assert!(!sync_text.contains(SECRET), "{sync_text}");
    let sync_report = json_report(&sync);
    let added = sync_row(&sync_report, "Deerwood. Christmas breakfast");
    assert_eq!(added["note"]["scope"], "event");
    assert!(added["note"]["fields"].as_array().is_some());

    let listed = list_json(&env, &fake, &[]);
    assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
    let list_text = stdout(&listed);
    assert!(!list_text.contains(SECRET), "{list_text}");
    let list_report = json_report(&listed);
    let event = list_event(&list_report, "Deerwood. Christmas breakfast");
    assert_eq!(event["calendar_id"], "primary");
    assert_eq!(event["event_id"], "deerwood");
    assert_eq!(event["root"]["kind"], "note");
    assert_eq!(event["note"]["scope"], "event");

    let mut table = env.command();
    fake.install(&mut table).args(["gcal", "list"]);
    let table_out = table.output().expect("table");
    assert_eq!(table_out.status.code(), Some(0), "{}", stderr(&table_out));
    let table_text = stdout(&table_out);
    assert!(table_text.contains("✎"), "{table_text}");
    assert!(!table_text.contains(SECRET), "{table_text}");
}

#[test]
fn note_prose_is_exempt_from_the_cap_and_off_hides_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-notes-cap");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  description_lines: 2\n",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        Some("one\ntwo\nthree"),
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\nNote snacks\nNote drinks",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let note = today_note(&env);
    assert!(note.contains("\t- Note snacks"), "{note}");
    assert!(note.contains("\t- Note drinks"), "{note}");
    assert!(note.contains("\t- one"), "{note}");
    assert!(
        note.contains("more lines in"),
        "Google lines are capped:\n{note}"
    );
    assert!(!note.contains("\t- two"), "{note}");

    let off = GcalEnv::new("bob-cli-gcal-notes-off");
    prepare(&off);
    off.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  description: off\n",
    );
    let fake_off = FakeGog::new(&off, "fake-gog");
    fake_off.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        Some("one\ntwo\nthree"),
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &off,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive\nNote snacks",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&off, &fake_off, &[]).status.code(), Some(0));
    let off_note = today_note(&off);
    let row = event_row(&off_note, "^deerwood");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(row.contains("[buffer:: -30m]"), "{row}");
    assert!(!off_note.contains("Note snacks"), "{off_note}");
    assert!(!off_note.contains("Drive"), "{off_note}");
}

#[test]
fn two_accounts_on_one_key_keep_separate_notes() {
    let env = GcalEnv::new("bob-cli-gcal-notes-two-accounts");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive",
            ACCOUNT,
        ),
    );
    append_journal(
        &env,
        "mac",
        event_note(
            "mac",
            "2026-10-29T19:02:11-04:00",
            "deerwood",
            "@job\n+15 Later",
            "other@example.com",
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let row = event_row(&today_note(&env), "^deerwood");
    assert!(row.contains("[root:: family]"), "{row}");
    assert!(row.contains("[buffer:: -30m]"), "{row}");
    assert!(!row.contains("[root:: job]"), "{row}");

    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-30T12:00:00-04:00",
            "deerwood",
            "",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let cleared = event_row(&today_note(&env), "^deerwood");
    assert!(cleared.contains("[root:: meet]"), "{cleared}");
    assert!(!cleared.contains("[buffer::"), "{cleared}");
    let journal = fs::read_to_string(env.vault().join(".bob/gcal/mac.jsonl"))
        .expect("foreign journal");
    assert!(journal.contains("@job"), "{journal}");
}

#[test]
fn foreign_account_note_is_ignored() {
    let env = GcalEnv::new("bob-cli-gcal-notes-account");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive",
            "other@example.com",
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let row = event_row(&today_note(&env), "^deerwood");
    assert!(row.contains("[root:: meet]"), "{row}");
    assert!(!row.contains("[buffer::"), "{row}");
    let listed = list_json(&env, &fake, &[]);
    let list_report = json_report(&listed);
    let event = list_event(&list_report, "Deerwood. Christmas breakfast");
    assert!(event["note"].is_null(), "{event}");
}

fn journal_rows(env: &GcalEnv) -> Vec<Value> {
    let dir = env.vault().join(".bob/gcal");
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return rows;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if value.get("kind").and_then(Value::as_str) == Some("gcal.row") {
                rows.push(value);
            }
        }
    }
    rows
}

fn field_names(note: &Value) -> Vec<&str> {
    note.get("fields")
        .and_then(Value::as_array)
        .expect("fields")
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

fn remove_note_lines(env: &GcalEnv) {
    let dir = env.vault().join(".bob/gcal");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let kept = text
            .lines()
            .filter(|line| !line.contains("\"gcal.note\""))
            .fold(String::new(), |mut out, line| {
                out.push_str(line);
                out.push('\n');
                out
            });
        fs::write(&path, kept).expect("strip note lines");
    }
}

fn truncate_first_note_line(env: &GcalEnv) {
    let path = env.vault().join(".bob/gcal/athena.jsonl");
    let text = fs::read_to_string(&path).expect("read note journal");
    let mut out = String::new();
    let mut truncated = false;
    for line in text.lines() {
        if !truncated && line.contains("\"gcal.note\"") {
            let cut = line.len().saturating_sub(20);
            out.push_str(&line[..cut]);
            out.push('\n');
            truncated = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(truncated, "no gcal.note line to truncate");
    fs::write(&path, out).expect("truncate note line");
}

#[test]
fn provenance_record_shape() {
    let env = GcalEnv::new("bob-cli-gcal-notes-provenance");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive to Deerwood\n+15 Drive home\nBring gifts",
            ACCOUNT,
        ),
    );
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let rows = journal_rows(&env);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let note = rows[0].get("note").expect("gcal.row carries note");
    let hash = note.get("hash").and_then(Value::as_str).expect("hash");
    assert_eq!(hash.len(), 16, "{note}");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{note}");
    let fields = note
        .get("fields")
        .and_then(Value::as_array)
        .expect("fields");
    let names: Vec<&str> = fields.iter().filter_map(Value::as_str).collect();
    assert!(names.contains(&"root"), "{names:?}");
    assert!(names.contains(&"before"), "{names:?}");
    assert!(names.contains(&"after"), "{names:?}");
    assert!(names.contains(&"bullets"), "{names:?}");
    // A second sync with nothing malformed stays clean.
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(!combined.contains("could not be read"), "{combined}");
}

#[test]
fn hold_on_truncated_note_line_and_clear_restores() {
    let env = GcalEnv::new("bob-cli-gcal-notes-hold");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive to Deerwood",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let before = today_note(&env);
    assert!(before.contains("[root:: family]"), "{before}");

    truncate_first_note_line(&env);
    let held = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(held.status.code(), Some(0), "{}", stderr(&held));
    let report = json_report(&held);
    assert_eq!(report["status"], "unchanged", "{report}");
    let warnings = report["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| warning.as_str().is_some_and(
            |text| text.contains("held Deerwood")
                && text.contains("could not be read")
                && text.contains("bob gcal doctor")
        )),
        "{warnings:?}"
    );
    let held_row = sync_row(&report, "Deerwood. Christmas breakfast");
    assert_eq!(held_row["action"], "held", "{held_row}");
    assert_eq!(held_row["buffer"]["before_minutes"], 30, "{held_row}");
    assert_eq!(today_note(&env), before, "held rows skip vault updates");
    assert_eq!(journal_rows(&env).len(), 1, "no duplicate gcal.row on hold");
    let human = sync_output(&env, &fake, &[]);
    assert_eq!(human.status.code(), Some(0), "{}", stderr(&human));
    let combined = format!("{}{}", stdout(&human), stderr(&human));
    assert!(!stdout(&human).contains('~'), "{combined}");
    assert!(combined.contains("held Deerwood"), "{combined}");
    assert_eq!(today_note(&env), before);
    assert_eq!(
        journal_rows(&env).len(),
        1,
        "a second hold does not append gcal.row"
    );

    // A deliberate clear is never held: it restores Google's values.
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-30T12:02:00-04:00",
            "deerwood",
            "",
            ACCOUNT,
        ),
    );
    let cleared = sync_output(&env, &fake, &[]);
    assert_eq!(cleared.status.code(), Some(0), "{}", stderr(&cleared));
    let row = event_row(&today_note(&env), "^deerwood");
    assert!(row.contains("[root:: meet]"), "{row}");
    assert!(!row.contains("[buffer::"), "{row}");
}

#[test]
fn note_fields_omit_invalid_root_and_off_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-notes-fields-root");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@no-such-area\n-30 Drive",
            ACCOUNT,
        ),
    );
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_report(&output);
    let added = sync_row(&report, "Deerwood. Christmas breakfast");
    let names = field_names(&added["note"]);
    assert!(names.contains(&"before"), "{names:?}");
    assert!(!names.contains(&"root"), "{names:?}");
    let rows = journal_rows(&env);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let journaled = field_names(&rows[0]["note"]);
    assert_eq!(journaled, names);
    let listed = list_json(&env, &fake, &[]);
    let listed_report = json_report(&listed);
    let event = list_event(&listed_report, "Deerwood. Christmas breakfast");
    assert_eq!(field_names(&event["note"]), names);

    let off = GcalEnv::new("bob-cli-gcal-notes-fields-off");
    prepare(&off);
    off.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  description: off\n",
    );
    let fake_off = FakeGog::new(&off, "fake-gog");
    fake_off.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &off,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive\nNote snacks",
            ACCOUNT,
        ),
    );
    let off_out = sync_output(&off, &fake_off, &["-f", "json"]);
    assert_eq!(off_out.status.code(), Some(0), "{}", stderr(&off_out));
    let off_report = json_report(&off_out);
    let off_added = sync_row(&off_report, "Deerwood. Christmas breakfast");
    let off_names = field_names(&off_added["note"]);
    assert!(off_names.contains(&"root"), "{off_names:?}");
    assert!(off_names.contains(&"before"), "{off_names:?}");
    assert!(!off_names.contains(&"bullets"), "{off_names:?}");
    let off_rows = journal_rows(&off);
    assert_eq!(field_names(&off_rows[0]["note"]), off_names);
}

#[test]
fn no_hold_when_nothing_is_malformed() {
    let env = GcalEnv::new("bob-cli-gcal-notes-no-hold");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\n-30 Drive to Deerwood",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(today_note(&env).contains("[root:: family]"));
    remove_note_lines(&env);
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_report(&output);
    let warnings = report["warnings"].as_array().expect("warnings");
    assert!(
        !warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("could not be read"))),
        "{warnings:?}"
    );
    let row = event_row(&today_note(&env), "^deerwood");
    assert!(row.contains("[root:: meet]"), "{row}");
    assert!(!row.contains("[buffer::"), "{row}");
}

#[test]
fn aliased_title_token_is_stripped_when_note_route_wins() {
    let env = GcalEnv::new("bob-cli-gcal-notes-alias-title");
    prepare(&env);
    fs::write(
        env.vault().join("family.md"),
        "---\ntype: area\nproject_name_aliases: [\"fam\"]\n---\n# family\n",
    )
    .expect("write family alias");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "breakfast",
        "Breakfast @fam",
        None,
        "2026-10-30T10:00:00-04:00",
        "2026-10-30T12:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "breakfast",
            "@family",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let row = event_row(&today_note(&env), "^breakfast");
    assert!(row.contains("Breakfast"), "{row}");
    assert!(!row.contains("@fam"), "{row}");
    assert!(row.contains("[root:: family]"), "{row}");
}

#[test]
fn kept_description_words_note_change() {
    let env = GcalEnv::new("bob-cli-gcal-notes-kept-note");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family\nBring gifts",
            ACCOUNT,
        ),
    );
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let edited = today_note(&env).replace("Bring gifts", "My edit");
    write_today(&env, &edited);
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-30T12:03:00-04:00",
            "deerwood",
            "@family\nBring snacks",
            ACCOUNT,
        ),
    );
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains(
            "kept your edited description bullets; your private note changed"
        ),
        "{combined}"
    );
}

#[test]
fn incomplete_snapshot_writes_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-notes-incomplete");
    prepare(&env);
    let before = today_note(&env);
    append_journal(
        &env,
        "athena",
        event_note(
            "athena",
            "2026-10-29T18:02:11-04:00",
            "deerwood",
            "@family",
            ACCOUNT,
        ),
    );
    let fake = FakeGog::new(&env, "fake-gog").scenario("events_failure");
    fake.events_json(&events_envelope(vec![guest_event(
        "deerwood",
        "Deerwood. Christmas breakfast",
        None,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(today_note(&env), before, "failed fetch writes nothing");
    let journal =
        fs::read_to_string(env.vault().join(".bob/gcal/athena.jsonl"))
            .expect("note journal");
    assert!(journal.contains("gcal.note"));
    assert!(!journal.contains("gcal.row"));
    assert!(
        !env.vault().join(".bob/gcal/test-host.jsonl").exists(),
        "failed fetch appends nothing to the writer journal"
    );
}

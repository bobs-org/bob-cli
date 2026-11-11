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

fn normalize_note_fixture(actual: &Value) -> Value {
    let mut normalized = actual.clone();
    if let Some(window) =
        normalized.get_mut("window").and_then(Value::as_object_mut)
    {
        // The integration harness pins BOB_NOW to 2026-10-30. Keep these
        // realistic, offset-aware values stable if the harness clock changes.
        window.insert(
            "from".to_string(),
            Value::String("2026-10-30T00:00:00-04:00".to_string()),
        );
        window.insert(
            "to".to_string(),
            Value::String("2027-01-28T00:00:00-05:00".to_string()),
        );
        window.insert(
            "start".to_string(),
            Value::String("2026-10-30".to_string()),
        );
        window
            .insert("end".to_string(), Value::String("2027-01-28".to_string()));
    }
    normalized
}

fn assert_golden(name: &str, actual: &Value) {
    let path = format!(
        "{}/tests/fixtures/gcal_note/json/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let actual = normalize_note_fixture(actual);
    if std::env::var_os("BOB_UPDATE_GCAL_NOTE_FIXTURES").is_some() {
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&actual).unwrap()),
        )
        .unwrap_or_else(|error| panic!("write fixture {path}: {error}"));
        return;
    }
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {path}: {error}"));
    let expected: Value = serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("parse golden {path}: {error}"));
    assert_eq!(actual, expected, "full JSON fixture {name}");
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
fn preview_save_unchanged_clear_and_invalid_json() {
    let env = GcalEnv::new("bob-cli-gcal-note-write");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "e1",
        &serde_json::to_string(&json!({
            "event": guest_event(
                "e1",
                "Deerwood",
                "2026-12-13T10:00:00-05:00",
                "2026-12-13T12:00:00-05:00",
            )
        }))
        .expect("lookup body"),
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "e1", "-d", "-f", "json", "--", "@family",
            "-30", "Drive", "there", "+15", "Drive", "home",
        ])
        .output()
        .expect("preview private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let preview = json_stdout(&output);
    assert_eq!(preview["action"], "preview");
    assert_eq!(
        preview["note"]["text"],
        "@family\n-30 Drive there\n+15 Drive home"
    );
    assert_eq!(preview["preview"]["occupied"]["start"], "09:30");
    assert_eq!(preview["preview"]["occupied"]["end"], "12:15");
    assert_eq!(preview["effective"]["buffer"]["before_minutes"], 30);
    assert_eq!(
        preview["effective"]["buffer"]["before_reason"],
        "Drive there"
    );
    assert_eq!(preview["effective"]["buffer"]["after_reason"], "Drive home");
    assert_eq!(preview["preview"]["bullets"].as_array().unwrap().len(), 2);
    assert!(preview["preview"]["rail"]
        .as_str()
        .unwrap()
        .contains("● family"));
    assert!(preview["preview"]["row"]
        .as_str()
        .unwrap()
        .contains("[root:: family]"));
    assert_golden("preview", &preview);
    assert!(
        !env.vault().join(".bob/gcal/test-host.jsonl").exists(),
        "preview must not create the note journal"
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "e1", "-f", "json", "--", "@family", "-30",
            "Drive", "there", "+15", "Drive", "home",
        ])
        .output()
        .expect("save private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = json_stdout(&output);
    assert_eq!(saved["action"], "saved");
    assert_eq!(saved["status"]["state"], "waiting_note");
    assert_golden("saved", &saved);
    assert_eq!(saved["preview"]["row"], preview["preview"]["row"]);
    assert_eq!(saved["preview"]["bullets"], preview["preview"]["bullets"]);
    assert!(saved["spans"]
        .as_array()
        .unwrap()
        .iter()
        .any(|span| span["kind"] == "reason"));

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "e1", "-f", "json", "--", "@family", "-30",
            "Drive", "there", "+15", "Drive", "home",
        ])
        .output()
        .expect("repeat private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let unchanged = json_stdout(&output);
    assert_eq!(unchanged["action"], "unchanged");
    assert_golden("unchanged", &unchanged);
    let journal_path = env.vault().join(".bob/gcal/test-host.jsonl");
    assert_eq!(
        fs::read_to_string(&journal_path).unwrap().lines().count(),
        1,
        "an unchanged canonical note must not append another journal record"
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "e1", "-c", "-f", "json"])
        .output()
        .expect("clear private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let cleared = json_stdout(&output);
    assert_eq!(cleared["action"], "cleared");
    assert_eq!(cleared["note"]["text"], "");
    // The cleared preview rebuilds from the pre-note Google event: e1 has
    // no Google description, so no buffers survive the clear.
    assert_eq!(cleared["effective"]["buffer"]["before_minutes"], 0);
    assert_eq!(cleared["effective"]["buffer"]["after_minutes"], 0);
    assert_eq!(cleared["preview"]["occupied"]["start"], "10:00");
    assert_eq!(cleared["preview"]["occupied"]["end"], "12:00");
    assert_eq!(
        cleared["preview"]["bullets"]
            .as_array()
            .expect("bullets")
            .len(),
        0
    );
    assert_golden("cleared", &cleared);
    assert_eq!(
        fs::read_to_string(&journal_path).unwrap().lines().count(),
        2,
        "clear appends a tombstone after the original note"
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "e1", "-f", "json", "--", "@famly"])
        .output()
        .expect("reject unknown route");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let invalid = json_stdout(&output);
    assert_eq!(invalid["ok"], false);
    assert_eq!(invalid["action"], "invalid");
    assert_eq!(invalid["errors"][0]["kind"], "route_unknown");
    assert_eq!(invalid["errors"][0]["suggestions"][0], "family");
    assert!(invalid["spans"].as_array().unwrap().iter().any(|span| {
        span["kind"] == "invalid" && span["start"] == 0 && span["end"] == 6
    }));
    assert_golden("invalid", &invalid);
    assert_eq!(
        fs::read_to_string(&journal_path).unwrap().lines().count(),
        2,
        "invalid input must not write"
    );

    fake.event_lookup(
        "midnight",
        &serde_json::to_string(&json!({
            "event": guest_event(
                "midnight",
                "Early appointment",
                "2026-12-14T00:15:00-05:00",
                "2026-12-14T00:45:00-05:00",
            )
        }))
        .expect("midnight lookup"),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "midnight", "-d", "-f", "json", "--",
            "@family", "-30", "Drive",
        ])
        .output()
        .expect("preview clipped time rail");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let rail_doc = json_stdout(&output);
    let rail = rail_doc["preview"]["rail"].as_str().expect("time rail");
    assert!(rail.contains("(clipped at 00:00)"), "{rail}");
    assert!(rail.chars().count() <= 48, "{rail}");

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "e1",
            "-d",
            "-f",
            "json",
            "--",
            "@family",
            "#offclock",
            "#foo",
            "-d",
        ])
        .output()
        .expect("preview warning-bearing note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let warnings = json_stdout(&output);
    assert_eq!(warnings["note"]["text"], "@family #off\n#foo -d");
    assert!(warnings["spans"]
        .as_array()
        .unwrap()
        .iter()
        .any(|span| { span["kind"] == "clock" }));
    let warning_messages = warnings["warnings"].as_array().unwrap();
    assert!(warning_messages.iter().any(|warning| {
        warning.as_str().unwrap_or_default().contains("deprecated")
    }));
    assert!(warning_messages.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("not a Bob directive")
    }));
    assert!(warning_messages.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("put options before")
    }));

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "e1", "-d", "-f", "json", "--", "-30",
            "Drive", "-d",
        ])
        .output()
        .expect("preview option-like text inside a reason");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let reason_warning = json_stdout(&output);
    assert!(reason_warning["warnings"].as_array().unwrap().iter().any(
        |warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .contains("put options before")
        }
    ));
}

#[test]
fn invalid_routes_and_empty_notes_have_ranged_errors() {
    let env = GcalEnv::new("bob-cli-gcal-note-write-errors");
    prepare(&env);
    fs::write(
        env.vault().join("family.md"),
        "---\ntype: project\nparent: [[body]]\nroot: true\n---\n# family\n",
    )
    .expect("write root project");
    fs::write(
        env.vault().join("famil.md"),
        "---\ntype: project\nparent: [[family]]\n---\n# famil\n",
    )
    .expect("write nearby nested project");
    let event = guest_event(
        "route-invalid",
        "Lunch with Kelly",
        "2026-12-13T13:00:00-05:00",
        "2026-12-13T14:00:00-05:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "route-invalid",
        &serde_json::to_string(&json!({"event": event})).expect("lookup body"),
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "route-invalid",
            "-f",
            "json",
            "--",
            "@famil",
        ])
        .output()
        .expect("reject invalid route");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let invalid = json_stdout(&output);
    assert_eq!(invalid["errors"][0]["kind"], "route_invalid");
    assert_eq!(invalid["errors"][0]["suggestions"][0], "family");
    assert!(invalid["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("@famil"));
    assert!(invalid["spans"].as_array().unwrap().iter().any(|span| {
        span["kind"] == "invalid" && span["start"] == 0 && span["end"] == 6
    }));

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "route-invalid",
            "-d",
            "-f",
            "json",
            "--",
        ])
        .output()
        .expect("reject empty note");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let empty = json_stdout(&output);
    assert_eq!(empty["errors"][0]["kind"], "empty");
    assert!(empty["spans"]
        .as_array()
        .unwrap()
        .iter()
        .any(|span| { span["kind"] == "invalid" }));
}

#[test]
fn human_write_and_clear_show_plain_preview_output() {
    let env = GcalEnv::new("bob-cli-gcal-note-human-write");
    prepare(&env);
    let event = guest_event(
        "human-write",
        "Lunch with Kelly",
        "2026-12-13T13:00:00-05:00",
        "2026-12-13T14:00:00-05:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "human-write",
        &serde_json::to_string(&json!({"event": event})).expect("lookup body"),
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "human-write", "-d", "@family"])
        .output()
        .expect("preview private note in human mode");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let preview = stdout(&output);
    assert!(preview.contains("Preview · nothing was saved"), "{preview}");
    assert!(
        !preview.contains("nothing saved · nothing was saved"),
        "{preview}"
    );
    assert!(preview.contains("● family"), "{preview}");

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "human-write",
            "@family",
            "-30",
            "Drive",
            "there",
        ])
        .output()
        .expect("write private note in human mode");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = stdout(&output);
    assert!(saved.contains("✓ Noted · only in Bob"), "{saved}");
    assert!(saved.contains("lands as"), "{saved}");
    assert!(saved.contains("[root:: family]"), "{saved}");
    assert!(
        !saved.contains("\u{1b}["),
        "non-TTY output must be plain: {saved:?}"
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "human-write", "-c"])
        .output()
        .expect("clear private note in human mode");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let cleared = stdout(&output);
    assert!(
        cleared.contains("✓ Cleared · Google's description decides again"),
        "{cleared}"
    );
    assert!(cleared.contains("lands as"), "{cleared}");
    assert!(
        !cleared.contains("\u{1b}["),
        "non-TTY output must be plain: {cleared:?}"
    );
}

#[test]
fn saved_note_show_and_preview_share_highlight_input() {
    let env = GcalEnv::new("bob-cli-gcal-note-show-preview-match");
    prepare(&env);
    let event = guest_event(
        "highlight-note",
        "Little Gym",
        "2026-11-13T09:00:00-05:00",
        "2026-11-13T10:00:00-05:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "highlight-note",
        &serde_json::to_string(&json!({"event": event})).expect("lookup body"),
    );
    append_journal(
        &env,
        "test-host",
        event_note(
            "test-host",
            "2026-10-30T11:00:00-04:00",
            "highlight-note",
            "@family\n-10 Drive there",
        ),
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "highlight-note", "-f", "json"])
        .output()
        .expect("show saved note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let show = json_stdout(&output);
    let note_text = show["event"]["note"]["text"]
        .as_str()
        .expect("saved note text");
    assert_eq!(note_text, "@family\n-10 Drive there");
    assert_golden("show_preview_match", &show);

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "highlight-note",
            "-d",
            "-f",
            "json",
            "--",
        ])
        .arg(note_text)
        .output()
        .expect("preview saved note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let preview = json_stdout(&output);
    assert_eq!(preview["action"], "preview");
    assert_eq!(preview["input"]["text"], note_text);
    assert!(!preview["spans"]
        .as_array()
        .expect("preview spans")
        .is_empty());
    assert_golden("preview_match", &preview);
}

#[test]
fn writer_host_applies_note_and_preview_matches_sync() {
    let env = GcalEnv::new("bob-cli-gcal-note-immediate-sync");
    prepare(&env);
    let event = guest_event(
        "today-event",
        "Lunch with Kelly",
        "2026-10-30T13:00:00-04:00",
        "2026-10-30T14:00:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![event.clone()]));
    fake.event_lookup(
        "today-event",
        &serde_json::to_string(&json!({"event": event})).expect("lookup body"),
    );

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "today-event",
            "-f",
            "json",
            "--",
            "@family",
            "-10",
            "Drive",
            "there",
        ])
        .output()
        .expect("save and immediately sync note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let result = json_stdout(&output);
    assert_eq!(result["action"], "saved");
    assert_eq!(result["status"]["state"], "applied");
    assert_golden("applied", &result);

    let daily = fs::read_to_string(env.vault().join("2026/20261030.md"))
        .expect("read synced daily note");
    let row = result["preview"]["row"].as_str().expect("preview row");
    assert!(
        daily.contains(row),
        "sync row differs from preview:\n{daily}\n---\n{row}"
    );
    for bullet in result["preview"]["bullets"].as_array().unwrap() {
        let bullet = bullet.as_str().unwrap();
        assert!(
            daily.contains(bullet),
            "sync bullet differs from preview: {bullet}\n{daily}"
        );
    }
}

#[test]
fn off_writer_and_sync_failure_keep_the_saved_note() {
    let env = GcalEnv::new("bob-cli-gcal-note-other-writer");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: the-mac\n",
    );
    let event = guest_event(
        "off-writer",
        "Lunch with Kelly",
        "2026-10-30T13:00:00-04:00",
        "2026-10-30T14:00:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "off-writer",
        &serde_json::to_string(&json!({"event": event})).expect("lookup body"),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "off-writer",
            "-f",
            "json",
            "--",
            "@family",
        ])
        .output()
        .expect("save on non-writer host");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let result = json_stdout(&output);
    assert_eq!(result["action"], "saved");
    assert_eq!(result["status"]["state"], "saved_elsewhere");
    assert_golden("saved_elsewhere", &result);
    assert!(result["status"]["detail"]
        .as_str()
        .unwrap()
        .contains("the-mac"));
    assert!(env.vault().join(".bob/gcal/test-host.jsonl").is_file());

    let env = GcalEnv::new("bob-cli-gcal-note-sync-failure");
    prepare(&env);
    let event = guest_event(
        "sync-fails",
        "Lunch with Kelly",
        "2026-10-30T13:00:00-04:00",
        "2026-10-30T14:00:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog").scenario("events_failure");
    fake.event_lookup(
        "sync-fails",
        &serde_json::to_string(&json!({"event": event})).expect("lookup body"),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "sync-fails",
            "-f",
            "json",
            "--",
            "@family",
        ])
        .output()
        .expect("save while immediate sync fails");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let result = json_stdout(&output);
    assert_eq!(result["action"], "saved");
    assert_eq!(result["status"]["state"], "sync_failed");
    assert_golden("saved_sync_failed", &result);
    assert!(result["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .contains("immediate sync failed")
        }));
    assert!(env.vault().join(".bob/gcal/test-host.jsonl").is_file());
}

#[test]
fn series_note_immediately_syncs_each_existing_occurrence_day() {
    let env = GcalEnv::new("bob-cli-gcal-note-series-sync");
    prepare(&env);
    for day in ["20261113", "20261120"] {
        fs::write(
            env.vault().join("2026").join(format!("{day}.md")),
            format!("# {day}\n\n## Pomodoros\n"),
        )
        .expect("write occurrence daily note");
    }
    let occurrence = |id: &str, start: &str, end: &str| {
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
    let first = occurrence(
        "gym-1",
        "2026-11-13T09:00:00-05:00",
        "2026-11-13T10:00:00-05:00",
    );
    let events = vec![
        first.clone(),
        occurrence(
            "gym-2",
            "2026-11-20T09:00:00-05:00",
            "2026-11-20T10:00:00-05:00",
        ),
    ];
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(events));
    fake.event_lookup(
        "gym-1",
        &serde_json::to_string(&json!({"event": first})).expect("lookup body"),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "gym-1", "-s", "series", "-f", "json", "--",
            "@family", "Bring", "socks",
        ])
        .output()
        .expect("save series note and sync occurrence days");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let result = json_stdout(&output);
    assert_eq!(result["action"], "saved");
    assert_eq!(result["scope"], "series");
    assert_eq!(result["status"]["state"], "applied");
    assert!(result["status"]["detail"]
        .as_str()
        .unwrap()
        .contains("applies to every Little Gym"));
    for day in ["20261113", "20261120"] {
        let daily = fs::read_to_string(
            env.vault().join("2026").join(format!("{day}.md")),
        )
        .expect("read synced occurrence note");
        assert!(daily.contains("[root:: family]"), "{day}: {daily}");
        assert!(daily.contains("Bring socks"), "{day}: {daily}");
    }
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
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-a", "-f", "json"])
        .output()
        .expect("run note -a json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_golden("list_all", &json_stdout(&output));
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
    assert_golden("show", &document);
}

#[test]
fn list_json_key_names_the_mode_not_the_rows() {
    // Empty window: the key still follows the mode.
    let env = GcalEnv::new("bob-cli-gcal-note-list-mode-empty");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![]));
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-f", "json"])
        .output()
        .expect("bare list empty window");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert!(document["notes"].is_array(), "{document}");
    assert!(document.get("events").is_none(), "{document}");
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-l", "-a", "-f", "json"])
        .output()
        .expect("all list empty window");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert!(document["events"].is_array(), "{document}");
    assert!(document.get("notes").is_none(), "{document}");
    // All-noted window: `-l -a` still uses `events`, bare still `notes`.
    let env2 = GcalEnv::new("bob-cli-gcal-note-list-mode-noted");
    prepare(&env2);
    let fake2 = FakeGog::new(&env2, "fake-gog");
    fake2.events_json(&events_envelope(vec![guest_event(
        "noted",
        "Noted Meeting",
        "2026-11-14T10:00:00-05:00",
        "2026-11-14T11:00:00-05:00",
    )]));
    append_journal(
        &env2,
        "test-host",
        event_note(
            "test-host",
            "2026-10-29T12:01:00-04:00",
            "noted",
            "@family",
        ),
    );
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args(["gcal", "note", "-l", "-a", "-f", "json"])
        .output()
        .expect("all list all noted");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert!(document["events"].is_array(), "{document}");
    assert!(document.get("notes").is_none(), "{document}");
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args(["gcal", "note", "-f", "json"])
        .output()
        .expect("bare list all noted");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert!(document["notes"].is_array(), "{document}");
    assert!(document.get("events").is_none(), "{document}");
}

#[test]
fn show_hides_foreign_account_notes_with_a_warning() {
    let env = GcalEnv::new("bob-cli-gcal-note-foreign-account");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![]));
    fake.event_lookup(
        "foreign-1",
        &serde_json::to_string(&json!({
            "event": guest_event(
                "foreign-1",
                "Foreign Meeting",
                "2026-11-14T10:00:00-05:00",
                "2026-11-14T11:00:00-05:00",
            )
        }))
        .expect("lookup body"),
    );
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:01:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": "someone-else@example.com",
            "key": {"calendar_id": "primary", "event_id": "foreign-1"},
            "scope": "event",
            "text": "@elsewhere",
            "seen": {"title": "Foreign Meeting"},
        }),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "foreign-1"])
        .output()
        .expect("show foreign note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    // Never the foreign text; always the warning.
    assert!(
        !stdout(&output).contains("@elsewhere"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("no private note yet"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("saved under another account"),
        "{}",
        stderr(&output)
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "foreign-1", "-f", "json"])
        .output()
        .expect("show foreign note json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert!(
        document["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap_or("")
                .contains("another account")),
        "{document}"
    );
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
    assert_golden("ambiguous", &document);
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
    assert_golden("series_scope", &document);
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
    // because the URL carries no title words. The warning prints with the
    // error, in both human output and JSON.
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
        stderr(&output).contains("falling back"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("no events match"),
        "{}",
        stderr(&output)
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", &bad_url, "-f", "json"])
        .output()
        .expect("run note bad url json");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let document = json_stdout(&output);
    assert_eq!(document["ok"], false);
    assert!(
        document["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap_or("")
                .contains("falling back")),
        "{document}"
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
        guest_event(
            "alpha",
            "Alpha Sync",
            "2026-11-10T10:00:00-05:00",
            "2026-11-10T11:00:00-05:00",
        ),
    ]));
    append_journal(
        &env2,
        "test-host",
        event_note("test-host", "2026-10-29T12:01:00-04:00", "s1", "@family"),
    );
    // A query matching a grouped series and a one-off shows the series
    // group with its next occurrence and the applied note summary. The
    // picker chooses row 2 (Standup); the scope prompt then reads EOF and
    // keeps This event.
    let output = run_gcal_stdin(&env2, &fake2, &["note", "a"], "2\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("Standup"), "{}", stdout(&output));
    let picker = stderr(&output);
    assert!(picker.contains("↻ next Fri Nov 20"), "{picker}");
    assert!(picker.contains("✎ this event"), "{picker}");
    assert!(picker.contains("● family"), "{picker}");
    assert!(
        picker.contains("Standup repeats · Fridays 09:00-09:30 · guest · Yes"),
        "{picker}"
    );
    let output = run_gcal_stdin(&env2, &fake2, &["note", "standup"], "2\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("Standup"), "{}", stdout(&output));
    // The single-group run skips the picker but still shows the prompt.
    assert!(
        stderr(&output)
            .contains("Standup repeats · Fridays 09:00-09:30 · guest · Yes"),
        "{}",
        stderr(&output)
    );
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

#[test]
fn orphan_clear_commands_remove_the_note_by_key() {
    let env = GcalEnv::new("bob-cli-gcal-note-orphan-clear");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![]));
    // An event-scope orphan: no lookup file, so the lookup is not found.
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:01:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "gone-1"},
            "scope": "event",
            "text": "@family",
            "seen": {"title": "Gone Meeting"},
        }),
    );
    // A series-scope orphan: the master is gone too.
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:02:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "series": "gone-series"},
            "scope": "series",
            "text": "@family",
            "seen": {"title": "Gone Series"},
        }),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note"])
        .output()
        .expect("list orphans");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("bob gcal note -c -i gone-1"), "{listing}");
    assert!(
        listing.contains("bob gcal note -c -s series -i gone-series"),
        "{listing}"
    );
    // Run the exact printed event-scope clear command.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-c", "-i", "gone-1"])
        .output()
        .expect("clear event orphan");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("✓ Cleared"), "{}", stdout(&output));
    // Run the exact printed series-scope clear command.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-c", "-s", "series", "-i", "gone-series"])
        .output()
        .expect("clear series orphan");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("✓ Cleared"), "{}", stdout(&output));
    // Both orphans are gone from the next listing.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note"])
        .output()
        .expect("list after clear");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(!listing.contains("Gone Meeting"), "{listing}");
    assert!(!listing.contains("Gone Series"), "{listing}");
    // Clearing again reports no note, not a lookup failure.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-c", "-i", "gone-1"])
        .output()
        .expect("clear twice");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("no private note"),
        "{}",
        stderr(&output)
    );
    // A JSON by-key clear carries the key and an empty note.
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:04:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "gone-2"},
            "scope": "event",
            "text": "@family",
            "seen": {"title": "Gone Two"},
        }),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-c", "-i", "gone-2", "-f", "json"])
        .output()
        .expect("clear orphan json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let cleared = json_stdout(&output);
    assert_eq!(cleared["action"], "cleared");
    assert_eq!(cleared["scope"], "event");
    assert_eq!(cleared["event_id"], "gone-2");
    assert_eq!(cleared["note"]["text"], "");

    // A bare series master (no recurringEventId) still clears by key.
    let env2 = GcalEnv::new("bob-cli-gcal-note-bare-master-clear");
    prepare(&env2);
    let fake2 = FakeGog::new(&env2, "fake-gog");
    fake2.events_json(&events_envelope(vec![]));
    fake2.event_lookup(
        "bare-master",
        &serde_json::to_string(&json!({
            "event": guest_event(
                "bare-master",
                "Bare Series",
                "2026-11-14T10:00:00-05:00",
                "2026-11-14T11:00:00-05:00",
            )
        }))
        .expect("lookup body"),
    );
    append_journal(
        &env2,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:03:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "series": "bare-master"},
            "scope": "series",
            "text": "@family",
            "seen": {"title": "Bare Series"},
        }),
    );
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args(["gcal", "note", "-c", "-s", "series", "-i", "bare-master"])
        .output()
        .expect("clear bare master series");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("✓ Cleared"), "{}", stdout(&output));
}

#[test]
fn orphans_check_gone_before_past() {
    let env = GcalEnv::new("bob-cli-gcal-note-gone-before-past");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![]));
    let mut cancelled_past = guest_event(
        "cancelled-past",
        "Cancelled Past",
        "2026-01-05T10:00:00-05:00",
        "2026-01-05T11:00:00-05:00",
    );
    cancelled_past["status"] = json!("cancelled");
    let mut declined_past = guest_event(
        "declined-past",
        "Declined Past",
        "2026-01-06T10:00:00-05:00",
        "2026-01-06T11:00:00-05:00",
    );
    declined_past["attendees"][0]["responseStatus"] = json!("declined");
    // A live series master whose first occurrence is in the past: judged
    // by status, so the series note is neither orphan nor hidden.
    let live_master = guest_event(
        "old-series",
        "Old Series",
        "2026-01-01T10:00:00-05:00",
        "2026-01-01T11:00:00-05:00",
    );
    let mut cancelled_master = guest_event(
        "dead-series",
        "Dead Series",
        "2026-01-02T10:00:00-05:00",
        "2026-01-02T11:00:00-05:00",
    );
    cancelled_master["status"] = json!("cancelled");
    for event in [
        &cancelled_past,
        &declined_past,
        &live_master,
        &cancelled_master,
    ] {
        fake.event_lookup(
            event["id"].as_str().unwrap(),
            &serde_json::to_string(&json!({"event": event}))
                .expect("lookup body"),
        );
    }
    for (title, key) in [
        ("Cancelled Past", "cancelled-past"),
        ("Declined Past", "declined-past"),
    ] {
        append_journal(
            &env,
            "test-host",
            json!({
                "v": 1,
                "ts": "2026-10-29T12:01:00-04:00",
                "host": "test-host",
                "kind": "gcal.note",
                "account": ACCOUNT,
                "key": {"calendar_id": "primary", "event_id": key},
                "scope": "event",
                "text": "@family",
                "seen": {"title": title},
            }),
        );
    }
    for (series, title) in
        [("old-series", "Old Series"), ("dead-series", "Dead Series")]
    {
        append_journal(
            &env,
            "test-host",
            json!({
                "v": 1,
                "ts": "2026-10-29T12:02:00-04:00",
                "host": "test-host",
                "kind": "gcal.note",
                "account": ACCOUNT,
                "key": {"calendar_id": "primary", "series": series},
                "scope": "series",
                "text": "@family",
                "seen": {"title": title},
            }),
        );
    }
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note"])
        .output()
        .expect("list gone before past");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    // Cancelled and declined beat past-hiding: both stay orphans.
    assert!(listing.contains("Cancelled Past"), "{listing}");
    assert!(listing.contains("Declined Past"), "{listing}");
    assert!(listing.contains("Dead Series"), "{listing}");
    // The live series is judged by its master's status, never its start.
    assert!(!listing.contains("Old Series"), "{listing}");
}

#[test]
fn lookup_outages_are_warnings_not_orphans() {
    let env = GcalEnv::new("bob-cli-gcal-note-lookup-outage");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![]));
    // A genuinely gone note: no lookup file, so the lookup is not found.
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:01:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "gone-9"},
            "scope": "event",
            "text": "@family",
            "seen": {"title": "Gone Nine"},
        }),
    );
    // A note whose lookup fails outright: an outage, not an orphan.
    fake.event_lookup(
        "flaky-1",
        &serde_json::to_string(&json!({
            "_fail": {"stderr": "boom: connection reset", "code": 1}
        }))
        .expect("failure body"),
    );
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:02:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "event_id": "flaky-1"},
            "scope": "event",
            "text": "@family",
            "seen": {"title": "Flaky One"},
        }),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note"])
        .output()
        .expect("list with outage");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("Gone Nine"), "{listing}");
    assert!(!listing.contains("Flaky One"), "{listing}");
    assert!(
        stderr(&output).contains("could not check 1 note"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn effective_buffer_reasons_follow_occurrence_series_and_google_precedence() {
    let env = GcalEnv::new("bob-cli-gcal-note-effective-reasons");
    prepare(&env);
    let occurrence = |id: &str, start: &str, end: &str| {
        let event = json!({
            "id": id,
            "summary": "Little Gym",
            "status": "confirmed",
            "recurringEventId": "gym-series",
            "start": {"dateTime": start},
            "end": {"dateTime": end},
            "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "accepted"}],
            "description": "-10 Google before\n+20 Google after",
        });
        event
    };
    let first = occurrence(
        "gym-1",
        "2026-11-13T09:00:00-05:00",
        "2026-11-13T10:00:00-05:00",
    );
    let second = occurrence(
        "gym-2",
        "2026-11-20T09:00:00-05:00",
        "2026-11-20T10:00:00-05:00",
    );
    let mut solo = guest_event(
        "solo-reasons",
        "Solo Lunch",
        "2026-11-21T12:00:00-05:00",
        "2026-11-21T13:00:00-05:00",
    );
    solo["description"] =
        json!("-12 Google drive there\n+18 Google drive home");

    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![
        first.clone(),
        second.clone(),
        solo.clone(),
    ]));
    for event in [&first, &second, &solo] {
        fake.event_lookup(
            event["id"].as_str().unwrap(),
            &serde_json::to_string(&json!({"event": event}))
                .expect("lookup body"),
        );
    }
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-30T11:00:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "series": "gym-series"},
            "scope": "series",
            "text": "-30 Series before\n+40 Series after",
        }),
    );
    append_journal(
        &env,
        "test-host",
        event_note(
            "test-host",
            "2026-10-30T11:30:00-04:00",
            "gym-1",
            "-0\n+5 Event after",
        ),
    );

    let show = |event_id: &str| {
        let mut command = env.command();
        let output = fake
            .install(&mut command)
            .args(["gcal", "note", "-i", event_id, "-s", "event", "-f", "json"])
            .output()
            .expect("show event note");
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        json_stdout(&output)
    };

    let both_scopes = show("gym-1");
    assert_eq!(both_scopes["effective"]["buffer"]["before_minutes"], 0);
    assert_eq!(
        both_scopes["effective"]["buffer"]["before_reason"],
        Value::Null
    );
    assert_eq!(both_scopes["effective"]["buffer"]["after_minutes"], 5);
    assert_eq!(
        both_scopes["effective"]["buffer"]["after_reason"],
        "Event after"
    );
    assert_golden("both_scopes", &both_scopes);

    let inherited = show("gym-2");
    assert_eq!(
        inherited["effective"]["buffer"]["before_reason"],
        "Series before"
    );
    assert_eq!(
        inherited["effective"]["buffer"]["after_reason"],
        "Series after"
    );
    assert_golden("inherited_reasons", &inherited);

    let google = show("solo-reasons");
    assert_eq!(
        google["effective"]["buffer"]["before_reason"],
        "Google drive there"
    );
    assert_eq!(
        google["effective"]["buffer"]["after_reason"],
        "Google drive home"
    );
    assert_golden("google_reasons", &google);
}

#[test]
fn list_json_exposes_waiting_reply_and_maybe_events() {
    let env = GcalEnv::new("bob-cli-gcal-note-waiting-events");
    prepare(&env);
    let mut reply = guest_event(
        "waiting-reply",
        "Review proposal",
        "2026-11-11T09:00:00-05:00",
        "2026-11-11T10:00:00-05:00",
    );
    reply["attendees"][0]["responseStatus"] = json!("needsAction");
    let mut maybe = guest_event(
        "waiting-maybe",
        "Lunch invitation",
        "2026-11-12T12:00:00-05:00",
        "2026-11-12T13:00:00-05:00",
    );
    maybe["attendees"][0]["responseStatus"] = json!("tentative");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![reply, maybe]));

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-a", "-f", "json"])
        .output()
        .expect("list waiting events");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    let events = document["events"].as_array().expect("event list");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["lands"], "waiting_reply");
    assert_eq!(events[1]["lands"], "waiting_maybe");
    assert_golden("waiting_states", &document);
}

#[test]
fn write_json_reports_waiting_reply_and_maybe_statuses() {
    let env = GcalEnv::new("bob-cli-gcal-note-waiting-write");
    prepare(&env);
    let mut reply = guest_event(
        "waiting-reply-write",
        "Review proposal",
        "2026-11-11T09:00:00-05:00",
        "2026-11-11T10:00:00-05:00",
    );
    reply["attendees"][0]["responseStatus"] = json!("needsAction");
    let mut maybe = guest_event(
        "waiting-maybe-write",
        "Lunch invitation",
        "2026-11-12T12:00:00-05:00",
        "2026-11-12T13:00:00-05:00",
    );
    maybe["attendees"][0]["responseStatus"] = json!("tentative");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "waiting-reply-write",
        &serde_json::to_string(&json!({"event": reply})).expect("reply lookup"),
    );
    fake.event_lookup(
        "waiting-maybe-write",
        &serde_json::to_string(&json!({"event": maybe})).expect("maybe lookup"),
    );

    for (event_id, expected) in [
        ("waiting-reply-write", "waiting_reply"),
        ("waiting-maybe-write", "waiting_maybe"),
    ] {
        let mut command = env.command();
        let output = fake
            .install(&mut command)
            .args([
                "gcal", "note", "-i", event_id, "-f", "json", "--", "@family",
            ])
            .output()
            .expect("write note for unanswered event");
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let document = json_stdout(&output);
        assert_eq!(document["status"]["state"], expected);
    }
}

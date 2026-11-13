//! `bob gcal note`: command skeleton, selection, and read-only modes.

mod gcal_support;

use gcal_support::{
    json_stdout, run_gcal_stdin, stderr, stdout, FakeGog, GcalEnv, ACCOUNT,
};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Stdio,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
        "bob gcal note deerwood -e",
        "bob gcal note deerwood -A",
        "BOB_GCAL_ASSUME_TTY",
        "VISUAL",
        "git-style scissors",
        "Edit again? [Y/n]",
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
    // The dry run saves nothing, so its row keeps the warning tag; the
    // save acks the event, so its preview already omits it. Otherwise
    // the two previews agree.
    let dry_row = preview["preview"]["row"].as_str().expect("dry row");
    let saved_row = saved["preview"]["row"].as_str().expect("saved row");
    assert!(dry_row.contains(" #unacked "), "dry row:\n{dry_row}");
    assert!(!saved_row.contains("#unacked"), "saved row:\n{saved_row}");
    assert_eq!(saved_row, dry_row.replace(" #unacked", ""));
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
fn saved_note_appends_acked_only_for_new_external_review() {
    let env = GcalEnv::new("bob-cli-gcal-note-acked");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "acked-new",
        &serde_json::to_string(&json!({
            "event": guest_event(
                "acked-new",
                "Deerwood",
                "2026-12-13T10:00:00-05:00",
                "2026-12-13T12:00:00-05:00",
            )
        }))
        .expect("lookup body"),
    );

    // Dry-run previews pre-save truth and never claims an ack.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "acked-new",
            "-d",
            "-f",
            "json",
            "--",
            "@family",
        ])
        .output()
        .expect("preview private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let preview = json_stdout(&output);
    assert_eq!(preview["action"], "preview");
    assert!(
        !preview["status"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("· acked"),
        "dry-run must not claim an ack: {}",
        preview["status"]["detail"]
    );

    // First save on a previously-unacked external event appends `· acked`.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "acked-new",
            "-f",
            "json",
            "--",
            "@family",
        ])
        .output()
        .expect("save private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = json_stdout(&output);
    assert_eq!(saved["action"], "saved");
    assert!(
        saved["status"]["detail"]
            .as_str()
            .unwrap_or_default()
            .ends_with("· acked"),
        "first save must append · acked: {}",
        saved["status"]["detail"]
    );

    // Human confirmation carries the same suffix (fresh event, first save).
    fake.event_lookup(
        "acked-human",
        &serde_json::to_string(&json!({
            "event": guest_event(
                "acked-human",
                "Deerwood",
                "2026-12-13T10:00:00-05:00",
                "2026-12-13T12:00:00-05:00",
            )
        }))
        .expect("human lookup body"),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "acked-human", "--", "@family"])
        .output()
        .expect("save with human output");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let human = stdout(&output);
    assert!(human.contains("· acked"), "{human}");

    // An already-reviewed save (unchanged canonical text) never re-claims.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "acked-new",
            "-f",
            "json",
            "--",
            "@family",
        ])
        .output()
        .expect("repeat private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let unchanged = json_stdout(&output);
    assert_eq!(unchanged["action"], "unchanged");
    assert!(
        !unchanged["status"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("· acked"),
        "already-reviewed save must not claim a new ack: {}",
        unchanged["status"]["detail"]
    );

    // Invalid input never claims an ack.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "acked-new",
            "-f",
            "json",
            "--",
            "@nope",
        ])
        .output()
        .expect("reject invalid note");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let invalid = json_stdout(&output);
    assert_eq!(invalid["action"], "invalid");
    assert!(
        !serde_json::to_string(&invalid)
            .unwrap_or_default()
            .contains("· acked"),
        "invalid must not claim an ack: {invalid}"
    );

    // Clearing the note never claims an ack.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "acked-new", "-c", "-f", "json"])
        .output()
        .expect("clear private note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let cleared = json_stdout(&output);
    assert_eq!(cleared["action"], "cleared");
    assert!(
        !cleared["status"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("· acked"),
        "clear must not claim an ack: {}",
        cleared["status"]["detail"]
    );
}

#[test]
fn saved_note_never_acks_owned_or_config_off_events() {
    // Owned (non-external) events never claim `· acked`, even on first save.
    let env = GcalEnv::new("bob-cli-gcal-note-acked-owned");
    prepare(&env);
    let owned = json!({
        "id": "owned-new",
        "summary": "Team standup",
        "status": "confirmed",
        "start": {"dateTime": "2026-12-13T10:00:00-05:00"},
        "end": {"dateTime": "2026-12-13T12:00:00-05:00"},
        "htmlLink": "https://calendar.google.com/event?eid=owned-new",
        "organizer": {"displayName": "Bryan", "email": "you@example.com", "self": true},
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    });
    let fake = FakeGog::new(&env, "fake-gog");
    fake.event_lookup(
        "owned-new",
        &serde_json::to_string(&json!({"event": owned})).expect("lookup body"),
    );
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-i",
            "owned-new",
            "-f",
            "json",
            "--",
            "@family",
        ])
        .output()
        .expect("save owned note");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = json_stdout(&output);
    assert_eq!(saved["action"], "saved");
    assert!(
        !saved["status"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("· acked"),
        "owned events must not claim an ack: {}",
        saved["status"]["detail"]
    );

    // Config `ack: off` never claims `· acked`, even for external events.
    let off_env = GcalEnv::new("bob-cli-gcal-note-acked-off");
    off_env.prepare_healthy_vault();
    for area in ["body", "job", "health", "family", "abby"] {
        fs::write(
            off_env.vault().join(format!("{area}.md")),
            format!("---\ntype: area\n---\n# {area}\n"),
        )
        .expect("write area");
    }
    off_env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  ack: off\n",
    );
    let off_fake = FakeGog::new(&off_env, "fake-gog");
    off_fake.event_lookup(
        "off-new",
        &serde_json::to_string(&json!({
            "event": guest_event(
                "off-new",
                "Deerwood",
                "2026-12-13T10:00:00-05:00",
                "2026-12-13T12:00:00-05:00",
            )
        }))
        .expect("lookup body"),
    );
    let mut command = off_env.command();
    let output = off_fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "off-new", "-f", "json", "--", "@family",
        ])
        .output()
        .expect("save with ack off");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = json_stdout(&output);
    assert_eq!(saved["action"], "saved");
    assert!(
        !saved["status"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("· acked"),
        "ack off must not claim an ack: {}",
        saved["status"]["detail"]
    );
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

fn chmod_exec(path: &Path) {
    #[cfg(unix)]
    {
        let mut permissions =
            fs::metadata(path).expect("stat script").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod script");
    }
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    chmod_exec(path);
}

fn install_lookup(fake: &FakeGog, event: &Value) {
    let id = event["id"].as_str().expect("event id");
    fake.event_lookup(
        id,
        &serde_json::to_string(&json!({ "event": event }))
            .expect("lookup body"),
    );
}

fn run_note_edit(
    env: &GcalEnv,
    fake: &FakeGog,
    visual: &Path,
    args: &[&str],
    stdin_text: &str,
) -> std::process::Output {
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gcal");
    for arg in args {
        command.arg(arg);
    }
    command
        .env("BOB_GCAL_ASSUME_TTY", "1")
        .env("VISUAL", visual)
        .env_remove("EDITOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gcal note -e");
    let write_result = child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin_text.as_bytes());
    if let Err(error) = write_result {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "write stdin");
    }
    child.wait_with_output().expect("wait for bob gcal note -e")
}

fn journal_note_count(env: &GcalEnv) -> usize {
    let path = env.vault().join(".bob/gcal/test-host.jsonl");
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("\"gcal.note\""))
        .count()
}

#[test]
fn edit_requires_a_tty_and_hints_to_pass_text() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-tty");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let event = guest_event(
        "edit-tty",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    install_lookup(&fake, &event);

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-e", "-i", "edit-tty"])
        .output()
        .expect("edit off a TTY");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let err = stderr(&output);
    assert!(
        err.contains("editing a private event note needs a TTY"),
        "{err}"
    );
    assert!(
        err.contains("pass the note as TEXT instead of `-e`"),
        "{err}"
    );
}

#[test]
fn edit_uses_visual_over_editor_and_drops_the_scissors_block() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-scissors");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let event = guest_event(
        "edit-scissors",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    install_lookup(&fake, &event);

    let dump = env.root().join("editor-input");
    let visual = env.root().join("visual.sh");
    let editor = env.root().join("editor.sh");
    write_script(
        &visual,
        &format!(
            "#!/bin/sh\ncp \"$1\" {dump}\nprintf '%s\\n' '@family' '-30 Drive there' > \"$1\"\n",
            dump = dump.display()
        ),
    );
    write_script(&editor, "#!/bin/sh\nprintf '%s\\n' '@body' > \"$1\"\n");

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .env("BOB_GCAL_ASSUME_TTY", "1")
        .env("VISUAL", &visual)
        .env("EDITOR", &editor)
        .args(["gcal", "note", "-e", "-i", "edit-scissors"])
        .output()
        .expect("edit with VISUAL");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = stdout(&output);
    assert!(saved.contains("✓ Noted · only in Bob"), "{saved}");
    assert!(saved.contains("@family"), "{saved}");
    assert!(saved.contains("-30 Drive there"), "{saved}");
    assert!(!saved.contains("@body"), "{saved}");

    let input = fs::read_to_string(&dump).expect("editor dump");
    assert!(
        input.contains("------------------------ >8 ------------------------"),
        "{input}"
    );
    assert!(
        input.contains("Do not modify or remove the line above; everything below it is ignored."),
        "{input}"
    );
    assert!(
        input.contains("Private note for Fri Oct 30 09:00–10:00 Deerwood (from Kelly · Yes)"),
        "{input}"
    );
    assert!(input.contains("An empty note clears it."), "{input}");
    assert!(
        !input.contains("# --- Bob event note help"),
        "retired comment marker must not appear:\n{input}"
    );
}

#[test]
fn edit_empty_clears_or_reports_unchanged_when_there_is_no_note() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-empty");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let event = guest_event(
        "edit-empty",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    install_lookup(&fake, &event);
    let visual = env.root().join("visual.sh");
    write_script(&visual, "#!/bin/sh\n: > \"$1\"\n");

    let empty = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-i", "edit-empty"],
        "",
    );
    assert_eq!(empty.status.code(), Some(0), "{}", stderr(&empty));
    let unchanged = stdout(&empty);
    assert!(
        unchanged.contains("✓ Unchanged · there is no private note"),
        "{unchanged}"
    );
    assert_eq!(journal_note_count(&env), 0);

    let mut command = env.command();
    let saved = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "edit-empty", "--", "@family"])
        .output()
        .expect("seed a note");
    assert_eq!(saved.status.code(), Some(0), "{}", stderr(&saved));
    assert_eq!(journal_note_count(&env), 1);

    let cleared = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-i", "edit-empty"],
        "",
    );
    assert_eq!(cleared.status.code(), Some(0), "{}", stderr(&cleared));
    assert!(
        stdout(&cleared)
            .contains("✓ Cleared · Google's description decides again"),
        "{}",
        stdout(&cleared)
    );
    assert_eq!(journal_note_count(&env), 2);
}

#[test]
fn edit_unchanged_text_appends_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-unchanged");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let event = guest_event(
        "edit-unchanged",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    install_lookup(&fake, &event);
    let mut command = env.command();
    let saved = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "edit-unchanged", "--", "@family"])
        .output()
        .expect("seed a note");
    assert_eq!(saved.status.code(), Some(0), "{}", stderr(&saved));
    assert_eq!(journal_note_count(&env), 1);

    let visual = env.root().join("visual.sh");
    write_script(&visual, "#!/bin/sh\nprintf '%s\\n' '@family' > \"$1\"\n");
    let output = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-i", "edit-unchanged"],
        "",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output)
            .contains("✓ Unchanged · the canonical note is already saved"),
        "{}",
        stdout(&output)
    );
    assert_eq!(journal_note_count(&env), 1);
}

#[test]
fn edit_again_yes_reopens_and_no_saves_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-again");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let event = guest_event(
        "edit-again",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    install_lookup(&fake, &event);

    let count = env.root().join("edit-count");
    let visual = env.root().join("visual.sh");
    write_script(
        &visual,
        &format!(
            "#!/bin/sh
n=0
if [ -f {count} ]; then n=$(cat {count}); fi
n=$((n+1))
echo \"$n\" > {count}
if [ \"$n\" -eq 1 ]; then
  printf '%s\\n' '@no-such-area' > \"$1\"
else
  printf '%s\\n' '@family' > \"$1\"
fi
",
            count = count.display()
        ),
    );

    let refused = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-i", "edit-again"],
        "n\n",
    );
    assert_eq!(refused.status.code(), Some(2), "{}", stderr(&refused));
    let err = stderr(&refused);
    assert!(err.contains("Edit again? [Y/n]"), "{err}");
    assert!(
        err.contains("@no-such-area")
            || err.to_ascii_lowercase().contains("route"),
        "{err}"
    );
    assert_eq!(journal_note_count(&env), 0);
    fs::remove_file(&count).ok();

    let accepted = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-i", "edit-again"],
        "Y\n",
    );
    assert_eq!(accepted.status.code(), Some(0), "{}", stderr(&accepted));
    assert!(
        stdout(&accepted).contains("✓ Noted · only in Bob"),
        "{}",
        stdout(&accepted)
    );
    assert_eq!(journal_note_count(&env), 1);
    let rounds = fs::read_to_string(&count).expect("edit count");
    assert_eq!(rounds.trim(), "2", "{rounds}");
}

#[test]
fn adopt_copies_bare_buffers_and_says_they_live_in_bob() {
    let env = GcalEnv::new("bob-cli-gcal-note-adopt");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let mut event = guest_event(
        "adopt-bare",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    event["description"] =
        json!("@family\n#off\n-30 Drive there\n+15\nPlease bring the form");
    install_lookup(&fake, &event);

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-A", "-i", "adopt-bare"])
        .output()
        .expect("adopt Google lines");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = stdout(&output);
    assert!(
        saved.contains(
            "✓ Noted · now live in Bob too: they survive the organizer's next update"
        ),
        "{saved}"
    );
    assert!(saved.contains("@family"), "{saved}");
    assert!(saved.contains("#off"), "{saved}");
    assert!(saved.contains("-30 Drive there"), "{saved}");
    assert!(saved.contains("+15"), "{saved}");

    let mut command = env.command();
    let shown = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "adopt-bare", "-f", "json"])
        .output()
        .expect("show adopted note");
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    let shown_doc = json_stdout(&shown);
    assert_eq!(
        shown_doc["event"]["note"]["text"],
        "@family #off +15\n-30 Drive there"
    );
    assert!(
        !shown_doc["event"]["note"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Please bring the form"),
        "{shown_doc}"
    );

    let mut zeros = guest_event(
        "adopt-zeros",
        "Zeros",
        "2026-10-30T11:00:00-04:00",
        "2026-10-30T12:00:00-04:00",
    );
    zeros["description"] = json!("-0\n+0");
    install_lookup(&fake, &zeros);
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-A", "-i", "adopt-zeros", "-f", "json"])
        .output()
        .expect("adopt explicit zeros");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let document = json_stdout(&output);
    let zero_text = document["note"]["text"]
        .as_str()
        .or_else(|| document["event"]["note"]["text"].as_str())
        .unwrap_or_default();
    assert!(
        zero_text.contains("-0") && zero_text.contains("+0"),
        "{document}"
    );
}

#[test]
fn adopt_refuses_an_existing_note_and_nothing_to_copy() {
    let env = GcalEnv::new("bob-cli-gcal-note-adopt-refuse");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let mut event = guest_event(
        "adopt-exists",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    event["description"] = json!("@family -30");
    install_lookup(&fake, &event);
    let mut command = env.command();
    let seeded = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "adopt-exists", "--", "@family"])
        .output()
        .expect("seed a note");
    assert_eq!(seeded.status.code(), Some(0), "{}", stderr(&seeded));

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-A", "-i", "adopt-exists"])
        .output()
        .expect("adopt over an existing note");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let err = stderr(&output);
    assert!(
        err.contains(
            "this event already has a private note · edit it with `-e`"
        ),
        "{err}"
    );

    let mut empty = guest_event(
        "adopt-empty",
        "Empty",
        "2026-10-30T11:00:00-04:00",
        "2026-10-30T12:00:00-04:00",
    );
    empty["description"] = json!("Please bring the form");
    install_lookup(&fake, &empty);
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args(["gcal", "note", "-A", "-i", "adopt-empty"])
        .output()
        .expect("adopt with no directives");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let err = stderr(&output);
    assert!(
        err.contains("no directive or reason lines to adopt"),
        "{err}"
    );
}

#[test]
fn adopt_edit_prefills_the_editor_with_google_lines() {
    let env = GcalEnv::new("bob-cli-gcal-note-adopt-edit");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let mut event = guest_event(
        "adopt-edit",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    event["description"] = json!("@family\n-30\n+15");
    install_lookup(&fake, &event);
    append_journal(
        &env,
        "test-host",
        event_note(
            "test-host",
            "2026-10-30T08:00:00-04:00",
            "adopt-edit",
            "@body",
        ),
    );

    let dump = env.root().join("editor-input");
    let visual = env.root().join("visual.sh");
    write_script(
        &visual,
        &format!(
            "#!/bin/sh\ncp \"$1\" {dump}\nprintf '%s\\n' '@family' '-30 Drive there' > \"$1\"\n",
            dump = dump.display()
        ),
    );
    let output = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-A", "-e", "-i", "adopt-edit"],
        "",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let input = fs::read_to_string(&dump).expect("editor dump");
    let seed = input
        .split("------------------------ >8 ------------------------")
        .next()
        .unwrap_or_default();
    assert!(seed.contains("@family"), "{input}");
    assert!(seed.contains("-30"), "{input}");
    assert!(seed.contains("+15"), "{input}");
    assert!(
        !seed.contains("@body"),
        "adopt prefills Google lines, not the existing note:\n{input}"
    );
    assert!(
        stdout(&output).contains("now live in Bob too"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn explicit_separator_after_text_keeps_dash_d_as_text() {
    let env = GcalEnv::new("bob-cli-gcal-note-separator-text");
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

    // Implicit TEXT then an explicit `--`: the `-d` after the delimiter is
    // note text, not the dry-run flag. The save must append one record.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "e1", "-f", "json", "@family", "--", "-d",
        ])
        .output()
        .expect("save with explicit separator");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let saved = json_stdout(&output);
    assert_eq!(saved["action"], "saved", "{saved}");
    let text = saved["note"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("@family"), "{saved}");
    assert!(text.contains("-d"), "-d must survive as note text: {saved}");
    let journal_path = env.vault().join(".bob/gcal/test-host.jsonl");
    assert_eq!(
        fs::read_to_string(&journal_path).unwrap().lines().count(),
        1,
        "the separator fix must save exactly one record"
    );

    // Options on either side: `-d` before the delimiter still previews.
    let env2 = GcalEnv::new("bob-cli-gcal-note-separator-preview");
    prepare(&env2);
    let fake2 = FakeGog::new(&env2, "fake-gog");
    fake2.event_lookup(
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
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "e1", "-d", "-f", "json", "@family", "--",
            "-d",
        ])
        .output()
        .expect("preview with separator");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let preview = json_stdout(&output);
    assert_eq!(preview["action"], "preview", "{preview}");
    assert!(
        !env2.vault().join(".bob/gcal/test-host.jsonl").exists(),
        "a preview must not append"
    );
}

#[test]
fn edit_series_scope_is_resolved_once_and_carried() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-series-once");
    prepare(&env);
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
    let second = occurrence(
        "gym-2",
        "2026-11-20T09:00:00-05:00",
        "2026-11-20T10:00:00-05:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![first.clone(), second.clone()]));
    fake.event_lookup(
        "gym-1",
        &serde_json::to_string(&json!({"event": first})).expect("lookup"),
    );
    append_journal(
        &env,
        "test-host",
        event_note("test-host", "2026-10-29T12:00:00-04:00", "gym-1", "@body"),
    );
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:01:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "series": "gym-series"},
            "scope": "series",
            "text": "@family",
        }),
    );

    let dump = env.root().join("editor-input");
    let visual = env.root().join("visual.sh");
    write_script(
        &visual,
        &format!(
            "#!/bin/sh\ncp \"$1\" {dump}\nprintf '%s\\n' '@family' 'Bring socks' > \"$1\"\n",
            dump = dump.display()
        ),
    );
    // One prompt only: choosing series seeds from the series note and must
    // save back into the series scope, leaving the event note alone.
    let output = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-i", "gym-1"],
        "2\n",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let seed = fs::read_to_string(&dump).expect("editor seed");
    let seed_top = seed
        .split("------------------------ >8 ------------------------")
        .next()
        .unwrap_or_default();
    assert!(seed_top.contains("@family"), "series seed:\n{seed}");
    assert!(
        !seed_top.contains("@body"),
        "must not seed event text:\n{seed}"
    );

    let mut command = env.command();
    let shown = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "gym-1", "-s", "event", "-f", "json"])
        .output()
        .expect("show event scope");
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    assert_eq!(
        json_stdout(&shown)["event"]["note"]["text"],
        "@body",
        "event text must stay unchanged"
    );
    let mut command = env.command();
    let shown = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "gym-1", "-s", "series", "-f", "json"])
        .output()
        .expect("show series scope");
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    let series_text = json_stdout(&shown)["event"]["note"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(series_text.contains("Bring socks"), "{series_text}");
    assert_eq!(journal_note_count(&env), 3);
}

#[test]
fn edit_empty_series_reports_unchanged_and_keeps_event_note() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-empty-series");
    prepare(&env);
    let occurrence = |id: &str| {
        json!({
            "id": id,
            "summary": "Little Gym",
            "status": "confirmed",
            "recurringEventId": "gym-series",
            "start": {"dateTime": "2026-11-13T09:00:00-05:00"},
            "end": {"dateTime": "2026-11-13T10:00:00-05:00"},
            "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "accepted"}],
        })
    };
    let first = occurrence("gym-1");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![first.clone()]));
    fake.event_lookup(
        "gym-1",
        &serde_json::to_string(&json!({"event": first})).expect("lookup"),
    );
    append_journal(
        &env,
        "test-host",
        event_note("test-host", "2026-10-29T12:00:00-04:00", "gym-1", "@body"),
    );
    let visual = env.root().join("visual.sh");
    write_script(&visual, "#!/bin/sh\n: > \"$1\"\n");

    // Empty series edit with only an event note: the event note survives.
    let output = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-i", "gym-1"],
        "2\n",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("✓ Unchanged"),
        "{}",
        stdout(&output)
    );
    assert_eq!(journal_note_count(&env), 1);
    let mut command = env.command();
    let shown = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "gym-1", "-s", "event", "-f", "json"])
        .output()
        .expect("event note survives");
    assert_eq!(json_stdout(&shown)["event"]["note"]["text"], "@body");

    // Both scopes empty: either scope reports unchanged without an append.
    let env2 = GcalEnv::new("bob-cli-gcal-note-edit-both-empty");
    prepare(&env2);
    let fake2 = FakeGog::new(&env2, "fake-gog");
    fake2.events_json(&events_envelope(vec![first.clone()]));
    fake2.event_lookup(
        "gym-1",
        &serde_json::to_string(&json!({"event": first})).expect("lookup"),
    );
    let visual2 = env2.root().join("visual.sh");
    write_script(&visual2, "#!/bin/sh\n: > \"$1\"\n");
    for (label, scope_input) in [("series", "2\n"), ("event", "1\n")] {
        let output = run_note_edit(
            &env2,
            &fake2,
            &visual2,
            &["note", "-e", "-i", "gym-1"],
            scope_input,
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "{label}: {}",
            stderr(&output)
        );
        assert!(
            stdout(&output).contains("✓ Unchanged"),
            "{label}: {}",
            stdout(&output)
        );
    }
    assert_eq!(journal_note_count(&env2), 0);

    // Explicit `-c` with no note keeps its existing error.
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args(["gcal", "note", "-c", "-s", "series", "-i", "gym-1"])
        .output()
        .expect("explicit clear with no note");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

#[test]
fn edit_dry_run_leaves_journal_and_daily_note_unchanged() {
    let env = GcalEnv::new("bob-cli-gcal-note-edit-dry-run");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let event = guest_event(
        "edit-dry",
        "Deerwood",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    install_lookup(&fake, &event);
    let mut command = env.command();
    let seeded = fake
        .install(&mut command)
        .args(["gcal", "note", "-i", "edit-dry", "--", "@family"])
        .output()
        .expect("seed a note");
    assert_eq!(seeded.status.code(), Some(0), "{}", stderr(&seeded));
    let daily_path = env.vault().join("2026").join("20261030.md");
    fs::create_dir_all(daily_path.parent().expect("daily dir"))
        .expect("daily dir");
    fs::write(&daily_path, "# 20261030\n\n## Pomodoros\n\nrendered\n")
        .expect("daily note");
    let journal_path = env.vault().join(".bob/gcal/test-host.jsonl");
    let journal_before =
        fs::read_to_string(&journal_path).expect("journal bytes");
    let daily_before = fs::read_to_string(&daily_path).expect("daily bytes");

    let visual = env.root().join("visual.sh");
    write_script(&visual, "#!/bin/sh\nprintf '%s\\n' '@body' > \"$1\"\n");
    let output = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-d", "-i", "edit-dry"],
        "",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("Preview"), "{}", stdout(&output));
    assert_eq!(fs::read_to_string(&journal_path).unwrap(), journal_before);
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), daily_before);

    write_script(&visual, "#!/bin/sh\n: > \"$1\"\n");
    let output = run_note_edit(
        &env,
        &fake,
        &visual,
        &["note", "-e", "-d", "-i", "edit-dry"],
        "",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(fs::read_to_string(&journal_path).unwrap(), journal_before);
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), daily_before);
}

#[test]
fn adopt_guard_uses_the_resolved_scope() {
    let env = GcalEnv::new("bob-cli-gcal-note-adopt-scope");
    prepare(&env);
    let occurrence = |id: &str| {
        let mut event = guest_event(
            id,
            "Little Gym",
            "2026-11-13T09:00:00-05:00",
            "2026-11-13T10:00:00-05:00",
        );
        event["recurringEventId"] = json!("gym-series");
        event["description"] = json!("@family -30");
        event
    };
    let first = occurrence("gym-1");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![first.clone()]));
    fake.event_lookup(
        "gym-1",
        &serde_json::to_string(&json!({"event": first})).expect("lookup"),
    );
    append_journal(
        &env,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:00:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "series": "gym-series"},
            "scope": "series",
            "text": "@body",
        }),
    );

    // Choosing This event adopts into the empty event scope.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .env("BOB_GCAL_ASSUME_TTY", "1")
        .args(["gcal", "note", "-A", "-i", "gym-1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = output.spawn().expect("spawn adopt event");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"1\n")
        .expect("choose event");
    let output = child.wait_with_output().expect("adopt event");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    // Choosing All events hits the existing series note guard.
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .env("BOB_GCAL_ASSUME_TTY", "1")
        .args(["gcal", "note", "-A", "-i", "gym-1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = output.spawn().expect("spawn adopt series");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"2\n")
        .expect("choose series");
    let output = child.wait_with_output().expect("adopt series");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("already has a private note"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn clear_live_series_by_master_id_syncs_rendered_days() {
    let env = GcalEnv::new("bob-cli-gcal-note-live-master-clear");
    prepare(&env);
    for day in ["20261030", "20261106"] {
        fs::create_dir_all(env.vault().join("2026")).expect("daily dir");
        fs::write(
            env.vault().join("2026").join(format!("{day}.md")),
            format!("# {day}\n\n## Pomodoros\n"),
        )
        .expect("daily note");
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
    let gym1 = occurrence(
        "gym-1",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    let gym2 = occurrence(
        "gym-2",
        "2026-11-06T09:00:00-05:00",
        "2026-11-06T10:00:00-05:00",
    );
    let master = guest_event(
        "gym-series",
        "Little Gym",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![gym1.clone(), gym2.clone()]));
    for (id, event) in
        [("gym-1", &gym1), ("gym-2", &gym2), ("gym-series", &master)]
    {
        fake.event_lookup(
            id,
            &serde_json::to_string(&json!({"event": event})).expect("lookup"),
        );
    }
    let mut command = env.command();
    let saved = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "gym-1", "-s", "series", "-f", "json", "--",
            "@family",
        ])
        .output()
        .expect("seed series note");
    assert_eq!(saved.status.code(), Some(0), "{}", stderr(&saved));
    for day in ["20261030", "20261106"] {
        let daily = fs::read_to_string(
            env.vault().join("2026").join(format!("{day}.md")),
        )
        .expect("synced day");
        assert!(daily.contains("[root:: family]"), "{day}: {daily}");
    }

    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-c",
            "-s",
            "series",
            "-i",
            "gym-series",
            "-f",
            "json",
        ])
        .output()
        .expect("clear live series by master");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let cleared = json_stdout(&output);
    assert_eq!(cleared["action"], "cleared");
    assert_ne!(cleared["status"]["state"], "cleared");
    let today =
        fs::read_to_string(env.vault().join("2026").join("20261030.md"))
            .expect("today reverted");
    assert!(!today.contains("[root:: family]"), "{today}");
}

#[test]
fn clear_live_series_by_master_off_writer_and_lookup_failure() {
    // Off-writer: the clear is saved but the rendered day is untouched.
    let env = GcalEnv::new("bob-cli-gcal-note-live-master-offwriter");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: other-host\n",
    );
    fs::create_dir_all(env.vault().join("2026")).expect("daily dir");
    fs::write(
        env.vault().join("2026").join("20261030.md"),
        "# 20261030\n\n## Pomodoros\n",
    )
    .expect("daily note");
    let occurrence = json!({
        "id": "gym-1",
        "summary": "Little Gym",
        "status": "confirmed",
        "recurringEventId": "gym-series",
        "start": {"dateTime": "2026-10-30T09:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T10:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=gym-1",
        "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    });
    let master = guest_event(
        "gym-series",
        "Little Gym",
        "2026-10-30T09:00:00-04:00",
        "2026-10-30T10:00:00-04:00",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![occurrence.clone()]));
    for (id, event) in [("gym-1", &occurrence), ("gym-series", &master)] {
        fake.event_lookup(
            id,
            &serde_json::to_string(&json!({"event": event})).expect("lookup"),
        );
    }
    let mut command = env.command();
    let saved = fake
        .install(&mut command)
        .args([
            "gcal", "note", "-i", "gym-1", "-s", "series", "-f", "json", "--",
            "@family",
        ])
        .output()
        .expect("seed series note off-writer");
    assert_eq!(saved.status.code(), Some(0), "{}", stderr(&saved));
    let daily_before =
        fs::read_to_string(env.vault().join("2026").join("20261030.md"))
            .expect("daily before");
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-c",
            "-s",
            "series",
            "-i",
            "gym-series",
            "-f",
            "json",
        ])
        .output()
        .expect("clear live series off-writer");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let cleared = json_stdout(&output);
    assert_eq!(cleared["action"], "cleared");
    assert_eq!(cleared["status"]["state"], "saved_elsewhere");
    assert_eq!(
        fs::read_to_string(env.vault().join("2026").join("20261030.md"))
            .expect("daily untouched"),
        daily_before
    );

    // Window lookup failure: the clear is still saved with an honest warning.
    let env2 = GcalEnv::new("bob-cli-gcal-note-live-master-lookup-fail");
    prepare(&env2);
    fs::create_dir_all(env2.vault().join("2026")).expect("daily dir");
    fs::write(
        env2.vault().join("2026").join("20261030.md"),
        "# 20261030\n\n## Pomodoros\n",
    )
    .expect("daily note");
    let fake2 = FakeGog::new(&env2, "fake-gog").scenario("events_failure");
    fake2.event_lookup(
        "gym-series",
        &serde_json::to_string(&json!({"event": master})).expect("lookup"),
    );
    append_journal(
        &env2,
        "test-host",
        json!({
            "v": 1,
            "ts": "2026-10-29T12:00:00-04:00",
            "host": "test-host",
            "kind": "gcal.note",
            "account": ACCOUNT,
            "key": {"calendar_id": "primary", "series": "gym-series"},
            "scope": "series",
            "text": "@family",
        }),
    );
    let mut command = env2.command();
    let output = fake2
        .install(&mut command)
        .args([
            "gcal",
            "note",
            "-c",
            "-s",
            "series",
            "-i",
            "gym-series",
            "-f",
            "json",
        ])
        .output()
        .expect("clear live series with failed window");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let cleared = json_stdout(&output);
    assert_eq!(cleared["action"], "cleared");
    let warnings = cleared["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("could not inspect other occurrences")),
        "{cleared}"
    );
}

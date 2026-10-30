//! `bob gcal sync` lifecycle: span updates, cancels, RSVP transitions,
//! series learning, plus the `list` footer and `doctor` lifecycle checks.

mod gcal_support;

use gcal_support::{json_stdout, stderr, stdout, FakeGog, GcalEnv};
use serde_json::Value;
use std::fs;

fn prepare(env: &GcalEnv) {
    env.prepare_healthy_vault();
    fs::write(
        env.vault().join("meet.md"),
        "---\ntype: area\ncolor: violet\ntask_type: MEET\n---\n# Meet\n",
    )
    .expect("write meet area");
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

fn write_today(env: &GcalEnv, note: &str) {
    fs::write(env.vault().join("2026/20261030.md"), note).expect("write note");
}

fn journal_lines(env: &GcalEnv) -> Vec<Value> {
    let path = env.vault().join(".bob/gcal/test-host.jsonl");
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("journal JSON"))
        .collect()
}

fn last_row_for(journal: &[Value], event_id: &str) -> Value {
    journal
        .iter()
        .filter(|line| {
            line["kind"] == "gcal.row" && line["key"]["event_id"] == event_id
        })
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("no journal row for {event_id}"))
}

fn dentist_event(patch: &serde_json::Map<String, Value>) -> Value {
    let base: Value =
        serde_json::from_str(include_str!("fixtures/gcal/events.json"))
            .expect("base fixture");
    let mut dentist = base["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["id"] == "dentist")
        .cloned()
        .expect("dentist event");
    let object = dentist.as_object_mut().expect("event object");
    for (key, value) in patch {
        object.insert(key.clone(), value.clone());
    }
    dentist
}

fn events_envelope(events: Vec<Value>) -> String {
    serde_json::to_string(&serde_json::json!({
        "events": events,
        "nextPageToken": serde_json::Value::Null,
    }))
    .expect("envelope")
}

fn base_events() -> Vec<Value> {
    let base: Value =
        serde_json::from_str(include_str!("fixtures/gcal/events.json"))
            .expect("base fixture");
    base["events"].as_array().expect("events").clone()
}

fn dentist_row(note: &str) -> String {
    note.lines()
        .find(|line| line.contains("^dentist"))
        .unwrap_or_else(|| panic!("no dentist row:\n{note}"))
        .to_string()
}

#[test]
fn span_updates_rewrite_clean_spans_and_reslot() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-update");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    // Google renames the visit, moves it earlier, and moves the venue.
    let mut patch = serde_json::Map::new();
    patch.insert("summary".to_string(), "Dentist deep clean".into());
    patch.insert(
        "start".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T07:30:00-04:00"}),
    );
    patch.insert(
        "end".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T08:00:00-04:00"}),
    );
    patch.insert("location".to_string(), "New Venue".into());
    let mut events = base_events();
    events.retain(|event| event["id"] != "dentist");
    events.push(dentist_event(&patch));
    fake.events_json(&events_envelope(events));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = dentist_row(&note);
    assert!(row.contains("**0730-0800** Dentist deep clean"), "{row}");
    assert!(row.starts_with("- [*] #task"), "{row}");
    assert!(row.contains("[root:: body]"), "{row}");
    assert!(row.ends_with("^dentist"), "{row}");
    assert!(row.contains("query=New%20Venue"), "{row}");
    assert!(!row.contains("Main%20St%20Dental"), "{row}");
    assert!(note.contains("[[#^dentist]]"), "{note}");
    // The earlier time re-slots the row ahead of the 0800 interview.
    let events_at = note.find("## Events").expect("Events");
    let section =
        &note[events_at..note.find("## Pomodoros").expect("Pomodoros")];
    let dentist_at = section.find("^dentist").expect("dentist");
    let interview_at = section.find("Product interview").expect("interview");
    assert!(dentist_at < interview_at, "{section}");
    assert!(
        stdout(&output).contains("~ task"),
        "updated row reported: {}",
        stdout(&output)
    );

    // The journal records the fresh spans; a third sync is a no-op.
    let journal = journal_lines(&env);
    let before = journal.len();
    let row_record = last_row_for(&journal, "dentist");
    assert_ne!(
        row_record["spans"]["time"],
        serde_json::Value::Null,
        "{row_record}"
    );
    let second = sync_output(&env, &fake, &[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert!(
        stdout(&second).contains("unchanged"),
        "third sync is a no-op: {}",
        stdout(&second)
    );
    assert_eq!(journal_lines(&env).len(), before);
}

#[test]
fn kept_edit_reported_once() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-kept");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    // Bryan renames the visit; the time span still matches.
    let note = today_note(&env).replace("Dentist ", "Dentist!! ");
    write_today(&env, &note);

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = today_note(&env);
    assert!(after.contains("Dentist!!"), "{after}");
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("kept your edit to title"),
        "kept notice reported: {combined}"
    );

    // Reported once: the flag quiets the next run.
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(!combined.contains("kept your edit"), "{combined}");
    assert!(today_note(&env).contains("Dentist!!"));
}

fn decline_dentist_fixture() -> String {
    let mut patch = serde_json::Map::new();
    patch.insert(
        "attendees".to_string(),
        serde_json::json!([{"self": true, "responseStatus": "declined"}]),
    );
    let mut events = base_events();
    events.retain(|event| event["id"] != "dentist");
    events.push(dentist_event(&patch));
    events_envelope(events)
}

#[test]
fn declined_event_cancels_untouched_task() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-declined");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(today_note(&env).contains("[[#^dentist]]"));

    fake.events_json(&decline_dentist_fixture());
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = dentist_row(&note);
    assert!(row.starts_with("- [-] #task"), "{row}");
    assert!(row.contains("[cancelled:: 2026-10-30]"), "{row}");
    assert!(!note.contains("[[#^dentist]]"), "{note}");
    let cancel_at = note.find("❌ **CANCEL LOG**").expect("cancel log");
    let entry_at = note
        .find("*2026-10-30* — Declined in Google Calendar")
        .expect("cancel entry");
    assert!(entry_at > cancel_at, "{note}");
    // Sibling meetings keep their links and the MEET entry.
    assert!(note.contains("[[#^late-meeting]]"), "{note}");
    assert!(note.contains("— MEET"), "{note}");
    assert!(
        stdout(&output).contains("cancelled"),
        "cancel reported: {}",
        stdout(&output)
    );

    // A second sync leaves the cancelled row alone and reports nothing.
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(
        stdout(&again).contains("unchanged"),
        "cancel is stable: {}",
        stdout(&again)
    );
}

#[test]
fn deleted_event_lookup_cancels_and_reports_once() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-deleted");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    // The event leaves the snapshot; the lookup 404s.
    let mut events = base_events();
    events.retain(|event| event["id"] != "dentist");
    fake.events_json(&events_envelope(events));
    fake.event_lookup(
        "dentist",
        r#"{"_fail": {"stderr": "Not Found: no such event", "code": 1}}"#,
    );

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = dentist_row(&note);
    assert!(row.starts_with("- [-] #task"), "{row}");
    assert!(
        note.contains("*2026-10-30* — Deleted in Google Calendar"),
        "{note}"
    );
    assert!(
        fake.calls().iter().any(|call| {
            call.windows(2).any(|pair| pair == ["calendar", "event"])
                && call.iter().any(|arg| arg == "dentist")
                && call
                    .windows(2)
                    .any(|pair| pair == ["--account", "you@example.com"])
        }),
        "absent-event lookup must pass the snapshot account: {:?}",
        fake.calls()
    );

    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(
        stdout(&again).contains("unchanged"),
        "cancel is stable: {}",
        stdout(&again)
    );
}

#[test]
fn moved_event_names_the_new_day() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-moved");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let mut events = base_events();
    events.retain(|event| event["id"] != "dentist");
    fake.events_json(&events_envelope(events));
    let mut moved = dentist_event(&serde_json::Map::new());
    let object = moved.as_object_mut().expect("event object");
    object.insert(
        "start".to_string(),
        serde_json::json!({"dateTime": "2026-10-31T09:00:00-04:00"}),
    );
    object.insert(
        "end".to_string(),
        serde_json::json!({"dateTime": "2026-10-31T09:30:00-04:00"}),
    );
    fake.event_lookup(
        "dentist",
        &serde_json::to_string(&serde_json::json!({"event": moved}))
            .expect("lookup body"),
    );

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = dentist_row(&note);
    assert!(row.starts_with("- [-] #task"), "{row}");
    assert!(
        note.contains("Moved to Sat 2026-10-31 in Google Calendar"),
        "{note}"
    );
}

#[test]
fn deleted_event_lookup_uses_the_sole_bob_account() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-bob-account");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  writer: test-host\n",
    );
    let fake = FakeGog::new(&env, "fake-gog").scenario("two_default_accounts");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let mut events = base_events();
    events.retain(|event| event["id"] != "dentist");
    fake.events_json(&events_envelope(events));
    fake.event_lookup(
        "dentist",
        r#"{"_fail": {"stderr": "Not Found: no such event", "code": 1}}"#,
    );
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        fake.calls().iter().any(|call| {
            call.windows(2).any(|pair| pair == ["calendar", "event"])
                && call
                    .windows(2)
                    .any(|pair| pair == ["--account", "you@example.com"])
        }),
        "lookup must keep the sole bob-gcal account: {:?}",
        fake.calls()
    );
}

#[test]
fn failed_lookup_writes_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-incomplete");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let journal_before = journal_lines(&env).len();

    // The event leaves the snapshot and the lookup itself fails.
    let mut events = base_events();
    events.retain(|event| event["id"] != "dentist");
    fake.events_json(&events_envelope(events));
    fake.event_lookup(
        "dentist",
        r#"{"_fail": {"stderr": "dial tcp: connection refused", "code": 1}}"#,
    );

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(today_note(&env).contains("- [*] #task **1830-1900** Dentist"));
    assert_eq!(journal_lines(&env).len(), journal_before);
}

#[test]
fn worked_task_is_never_cancelled() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-worked");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    // Bryan works the meeting, then declines it in Google.
    let note = today_note(&env).replace(
        "- [*] #task **1830-1900** Dentist",
        "- [x] #task **1830-1900** Dentist",
    );
    write_today(&env, &note);
    fake.events_json(&decline_dentist_fixture());

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = today_note(&env);
    assert!(
        after.contains("- [x] #task **1830-1900** Dentist"),
        "{after}"
    );
    assert!(!after.contains("CANCEL LOG"), "{after}");
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("left your worked task alone"),
        "worked notice reported: {combined}"
    );

    let again = sync_output(&env, &fake, &[]);
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(!combined.contains("worked task"), "{combined}");
}

#[test]
fn emptied_meet_entry_is_removed_with_the_cancel() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-empty-meet");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    // One future meeting only: its cancel empties the MEET entry.
    let solo = vec![dentist_event(&serde_json::Map::new())];
    fake.events_json(&events_envelope(solo));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(today_note(&env).contains("— MEET"));

    let declined = vec![dentist_event(
        &serde_json::from_value(serde_json::json!({
            "attendees": [{"self": true, "responseStatus": "declined"}],
        }))
        .expect("patch"),
    )];
    fake.events_json(&events_envelope(declined));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(!note.contains("— MEET"), "{note}");
    assert!(!note.contains("[[#^dentist]]"), "{note}");
    assert!(note.contains("❌ **CANCEL LOG**"), "{note}");
}

fn evening_class(answer: &str) -> Value {
    serde_json::json!({
        "id": "evening-class",
        "summary": "Evening class",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T20:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T21:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=evening-class",
        "attendees": [{"self": true, "responseStatus": answer}],
    })
}

#[test]
fn maybe_yes_upgrades_the_context_row_and_links() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-upgrade");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![evening_class("tentative")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let first = today_note(&env);
    assert!(
        first.contains("- **2000-2100** Evening class · *maybe*"),
        "{first}"
    );

    // Maybe goes Yes: the row upgrades in place and earns its link.
    fake.events_json(&events_envelope(vec![evening_class("accepted")]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = note
        .lines()
        .find(|line| line.contains("^evening-class"))
        .unwrap_or_else(|| panic!("no upgraded row:\n{note}"))
        .to_string();
    assert!(
        row.starts_with("- [*] #task **2000-2100** Evening class"),
        "{row}"
    );
    assert!(row.contains("[root:: meet]"), "{row}");
    assert!(!row.contains("maybe"), "{row}");
    assert!(note.contains("[[#^evening-class]]"), "{note}");

    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(
        stdout(&again).contains("unchanged"),
        "upgrade is stable: {}",
        stdout(&again)
    );
}

#[test]
fn yes_maybe_leaves_the_row_and_reports_once() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-downgrade");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![evening_class("accepted")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(today_note(&env).contains("[[#^evening-class]]"));

    // Yes goes Maybe: the task row stays, reported once.
    fake.events_json(&events_envelope(vec![evening_class("tentative")]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(
        note.lines().any(|line| line.contains("^evening-class")
            && line.starts_with("- [*] #task")),
        "{note}"
    );
    assert!(note.contains("[[#^evening-class]]"), "{note}");
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("now Maybe"),
        "downgrade reported: {combined}"
    );

    let again = sync_output(&env, &fake, &[]);
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(!combined.contains("now Maybe"), "{combined}");
}

fn standup_event(id: &str) -> Value {
    serde_json::json!({
        "id": id,
        "summary": "Weekly 1:1",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T09:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T09:30:00-04:00"},
        "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
        "recurringEventId": "series-standup",
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    })
}

#[test]
fn series_edit_teaches_and_reteaches_the_root() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-series");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![standup_event("standup-1")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let first = today_note(&env);
    let row = first
        .lines()
        .find(|line| line.contains("^weekly-1-1"))
        .unwrap_or_else(|| panic!("no standup row:\n{first}"))
        .to_string();
    assert!(row.contains("[root:: meet]"), "{row}");

    // Bryan reroutes one occurrence; the series learns it.
    let edited = today_note(&env).replace("[root:: meet]", "[root:: job]");
    write_today(&env, &edited);
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let journal = journal_lines(&env);
    let learned = journal
        .iter()
        .filter(|line| line["kind"] == "gcal.series_root")
        .last()
        .cloned()
        .expect("series root learned");
    assert_eq!(learned["key"]["series"], "series-standup");
    assert_eq!(learned["root"], "job");
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("learned"),
        "learning reported: {combined}"
    );

    // A new occurrence of the same series inherits the learned root.
    fake.events_json(&events_envelope(vec![
        standup_event("standup-1"),
        standup_event("standup-2"),
    ]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let new_row = note
        .lines()
        .filter(|line| line.contains("Weekly 1:1"))
        .nth(1)
        .unwrap_or_else(|| panic!("no second standup row:\n{note}"))
        .to_string();
    assert!(new_row.contains("[root:: job]"), "{new_row}");
    assert!(new_row.contains("[type:: MEET]"), "{new_row}");

    // Changing it back re-teaches the series.
    let edited = note.replace("[root:: job]", "[root:: meet]");
    write_today(&env, &edited);
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let journal = journal_lines(&env);
    let relearned = journal
        .iter()
        .filter(|line| line["kind"] == "gcal.series_root")
        .last()
        .cloned()
        .expect("series root re-taught");
    assert_eq!(relearned["root"], "meet");
}

#[test]
fn list_footer_surfaces_open_past_meetings() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-footer");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    // Yesterday's note still holds one open and one closed meeting.
    fs::write(
        env.vault().join("2026/20261029.md"),
        "# Thursday 2026-10-29\n\n## Events\n\n- [ ] #task **0900-0930** Dentist [root:: body] ^thu-dentist\n- [x] #task **1000-1030** Done review [root:: job] ^done-review\n\n## Pomodoros\n",
    )
    .expect("write yesterday");

    let mut command = env.command();
    fake.install(&mut command);
    let output = command.arg("gcal").output().expect("run gcal list");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("1 past meeting still open: Thu Dentist"),
        "footer shown: {}",
        stdout(&output)
    );

    let mut command = env.command();
    fake.install(&mut command);
    let output = command
        .args(["gcal", "-f", "json"])
        .output()
        .expect("run gcal list json");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["past_open"][0]["day"], "2026-10-29");
    assert_eq!(doc["past_open"][0]["title"], "Dentist");
}

#[test]
fn doctor_reports_malformed_journal_and_last_error() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-doctor");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    // A malformed journal line warns instead of failing.
    let journal_path = env.vault().join(".bob/gcal/test-host.jsonl");
    let mut journal = fs::read_to_string(&journal_path).expect("read journal");
    journal.push_str("this is not json\n");
    journal.push_str("{}\n");
    fs::write(&journal_path, journal).expect("dirty the journal");

    let mut command = env.command();
    fake.install(&mut command);
    let output = command
        .args(["gcal", "doctor", "-f", "json"])
        .output()
        .expect("run doctor");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json_stdout(&output);
    let malformed = doc["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["id"] == "journal_malformed")
        .expect("malformed check");
    assert_eq!(malformed["status"], "warn");
    assert!(
        malformed["summary"]
            .as_str()
            .unwrap_or_default()
            .contains('2'),
        "both bad lines counted: {malformed}"
    );

    // A failed sync records its error for doctor.
    let failing =
        FakeGog::new(&env, "fake-gog-failing").scenario("events_failure");
    let output = sync_output(&env, &failing, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    let mut command = env.command();
    failing.install(&mut command);
    let output = command
        .args(["gcal", "doctor", "-f", "json"])
        .output()
        .expect("run doctor again");
    let doc = json_stdout(&output);
    let last = doc["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["id"] == "last_sync")
        .expect("last sync check");
    assert_eq!(last["status"], "fail", "{last}");
}

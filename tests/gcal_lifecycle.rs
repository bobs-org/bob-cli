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
    patch.insert("summary".to_string(), "Dentist deep clean @body".into());
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
    // The earlier time re-slots the row ahead of the 0800 interview,
    // together with its description bullets.
    let events_at = note.find("## Events").expect("Events");
    let section =
        &note[events_at..note.find("## Pomodoros").expect("Pomodoros")];
    assert!(section.starts_with("## Events\n\n"), "{section}");
    let dentist_at = section.find("^dentist").expect("dentist");
    let interview_at = section.find("Product interview").expect("interview");
    assert!(dentist_at < interview_at, "{section}");
    let bullet_at = section.find("Bring insurance card").expect("bullet");
    assert!(
        dentist_at < bullet_at && bullet_at < interview_at,
        "bullets move with the reslotted row:\n{section}"
    );
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
    assert!(
        note.contains(
            "\t- ❌ **CANCEL LOG**\n\t\t- *2026-10-30* — Declined in Google Calendar\n\t- Bring insurance card\n"
        ),
        "cancel log lands first; description bullets stay as history:\n{note}"
    );
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
fn declined_event_nests_cancel_log_above_tab_indented_child() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-cancel-tab-child");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let note = today_note(&env);
    let row = dentist_row(&note);
    let with_child =
        note.replacen(&row, &format!("{row}\n\t- And retainers!"), 1);
    write_today(&env, &with_child);

    fake.events_json(&decline_dentist_fixture());
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = today_note(&env);
    assert!(
        after.contains(concat!(
            "\t- ❌ **CANCEL LOG**\n",
            "\t\t- *2026-10-30* — Declined in Google Calendar\n",
            "\t- And retainers!\n",
        )),
        "cancel log lands first at the tab child indent:\n{after}"
    );
    let cancel_at = after.find("❌ **CANCEL LOG**").expect("cancel log");
    let child_at = after.find("And retainers!").expect("child");
    assert!(cancel_at < child_at, "{after}");
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
fn deleted_past_row_is_left_alone_and_reported_once() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-past-kept");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let interview = today_note(&env)
        .lines()
        .find(|line| line.contains("^product-interview"))
        .expect("interview")
        .to_string();
    assert!(interview.starts_with("- [/]"), "{interview}");

    let mut events = base_events();
    events.retain(|event| event["id"] != "interview");
    fake.events_json(&events_envelope(events));
    fake.event_lookup(
        "interview",
        r#"{"_fail": {"stderr": "Not Found: no such event", "code": 1}}"#,
    );

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = note
        .lines()
        .find(|line| line.contains("^product-interview"))
        .expect("interview");
    assert_eq!(row, interview, "worked past row must stay:\n{note}");
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("left your worked task alone"),
        "{combined}"
    );

    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(
        stdout(&again).contains("unchanged"),
        "kept notice is once: {}",
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
fn unedited_root_reroutes_when_google_adds_a_token() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-reroute-token");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![standup_event("standup-1")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let first = today_note(&env);
    let row = first
        .lines()
        .find(|line| line.contains("Weekly 1:1"))
        .unwrap_or_else(|| panic!("no standup row:\n{first}"))
        .to_string();
    assert!(row.contains("[root:: meet]"), "{row}");

    let mut event = standup_event("standup-1");
    event["summary"] = "Weekly 1:1 @body".into();
    fake.events_json(&events_envelope(vec![event]));
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = note
        .lines()
        .find(|line| line.contains("Weekly 1:1"))
        .unwrap_or_else(|| panic!("no standup row:\n{note}"))
        .to_string();
    assert!(row.contains("**0900-0930** Weekly 1:1"), "{row}");
    assert!(!row.contains("@body"), "{row}");
    assert!(row.contains("[root:: body]"), "{row}");
    assert!(row.contains("[type:: MEET]"), "{row}");
    let journal = journal_lines(&env);
    let row_record = last_row_for(&journal, "standup-1");
    assert_eq!(row_record["root"]["value"], "body");
    assert_eq!(row_record["root"]["source"], "@body");
    let doc = json_stdout(&output);
    let updated = doc["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["action"] == "updated")
        .cloned()
        .expect("updated row");
    assert_eq!(updated["root"], "body");
    assert_eq!(updated["source"], "@body");
}

#[test]
fn unedited_root_reroutes_when_google_colour_changes() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-reroute-colour");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![standup_event("standup-1")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(today_note(&env).contains("[root:: meet]"));

    let mut event = standup_event("standup-1");
    event["colorId"] = "11".into();
    fake.events_json(&events_envelope(vec![event]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = note
        .lines()
        .find(|line| line.contains("Weekly 1:1"))
        .unwrap_or_else(|| panic!("no standup row:\n{note}"))
        .to_string();
    assert!(row.contains("[root:: job]"), "{row}");
    assert!(
        stdout(&output).contains("~ task"),
        "updated row reported: {}",
        stdout(&output)
    );
}

#[test]
fn bob_reroute_does_not_teach_the_series() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-reroute-no-learn");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![standup_event("standup-1")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let mut event = standup_event("standup-1");
    event["summary"] = "Weekly 1:1 @body".into();
    fake.events_json(&events_envelope(vec![event.clone()]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let before = journal_lines(&env)
        .iter()
        .filter(|line| line["kind"] == "gcal.series_root")
        .count();
    assert_eq!(before, 0, "Bob's rewrite must not teach the series");

    event["id"] = "standup-2".into();
    event["htmlLink"] =
        "https://calendar.google.com/event?eid=standup-2".into();
    fake.events_json(&events_envelope(vec![
        {
            let mut first = standup_event("standup-1");
            first["summary"] = "Weekly 1:1 @body".into();
            first
        },
        event,
    ]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = journal_lines(&env)
        .iter()
        .filter(|line| line["kind"] == "gcal.series_root")
        .count();
    assert_eq!(after, before, "Bob's rewrite must not teach the series");
    let note = today_note(&env);
    let rows: Vec<&str> = note
        .lines()
        .filter(|line| line.contains("Weekly 1:1"))
        .collect();
    assert_eq!(rows.len(), 2, "{note}");
    assert!(
        rows.iter().all(|row| row.contains("[root:: body]")),
        "{note}"
    );
}

#[test]
fn dry_run_diff_shows_the_root_change() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-reroute-diff");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![standup_event("standup-1")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let before = today_note(&env);

    let mut event = standup_event("standup-1");
    event["summary"] = "Weekly 1:1 @body".into();
    fake.events_json(&events_envelope(vec![event]));
    let output = sync_output(&env, &fake, &["-d"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let diff = stdout(&output);
    assert!(diff.contains("[root:: meet]"), "{diff}");
    assert!(diff.contains("[root:: body]"), "{diff}");
    assert_eq!(today_note(&env), before, "dry-run must not write");
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

#[test]
fn off_clock_tag_flip_updates_untouched_row_in_place() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-untracked-touch");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    // Future dentist stays untouched (open placeholder link, no closed work).
    let mut future = dentist_event(&serde_json::Map::new());
    future["start"] =
        serde_json::json!({"dateTime": "2026-10-30T18:30:00-04:00"});
    future["end"] =
        serde_json::json!({"dateTime": "2026-10-30T19:00:00-04:00"});
    fake.events_json(&events_envelope(vec![future]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let before = dentist_row(&today_note(&env));
    assert!(!before.contains("#untracked"), "{before}");

    // Google adds the token: title stays clean, badges gain the tag.
    let mut patch = serde_json::Map::new();
    patch.insert("summary".to_string(), "Dentist @body #untracked".into());
    let mut events = base_events();
    events.retain(|event| event["id"] != "dentist");
    events.push(dentist_event(&patch));
    // Keep only the dentist to stay untouched: drop other tasks' links.
    events.retain(|event| event["id"] == "dentist");
    events[0]["start"] =
        serde_json::json!({"dateTime": "2026-10-30T18:30:00-04:00"});
    events[0]["end"] =
        serde_json::json!({"dateTime": "2026-10-30T19:00:00-04:00"});
    fake.events_json(&events_envelope(events));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let row = dentist_row(&today_note(&env));
    assert!(row.contains("**1830-1900** Dentist "), "{row}");
    assert!(!row.contains("Dentist #untracked"), "{row}");
    assert!(row.contains(" #off [root:: body]"), "{row}");
    assert!(
        stdout(&output).contains("~ task"),
        "tag flip updates in place: {}",
        stdout(&output)
    );

    // A third sync is a no-op.
    let journal_count = journal_lines(&env).len();
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(
        stdout(&again).contains("unchanged"),
        "tag flip settles: {}",
        stdout(&again)
    );
    assert_eq!(journal_lines(&env).len(), journal_count);
}

#[test]
fn explicit_on_tag_adds_and_removes_in_place_on_untouched_rows() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-on-tag-flips");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let patch_event = |summary: &str| {
        let mut patch = serde_json::Map::new();
        patch.insert("summary".to_string(), summary.into());
        patch.insert(
            "start".to_string(),
            serde_json::json!({"dateTime": "2026-10-30T18:30:00-04:00"}),
        );
        patch.insert(
            "end".to_string(),
            serde_json::json!({"dateTime": "2026-10-30T19:00:00-04:00"}),
        );
        dentist_event(&patch)
    };

    fake.events_json(&events_envelope(vec![patch_event("Dentist @body")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let initial = dentist_row(&today_note(&env));
    assert!(!initial.contains(" #off"), "{initial}");
    assert!(!initial.contains(" #on"), "{initial}");

    fake.events_json(&events_envelope(vec![patch_event("Dentist @body #on")]));
    let added = sync_output(&env, &fake, &[]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    let on = dentist_row(&today_note(&env));
    assert!(on.contains(" #on [root:: body]"), "{on}");

    fake.events_json(&events_envelope(vec![patch_event("Dentist @body")]));
    let removed = sync_output(&env, &fake, &[]);
    assert_eq!(removed.status.code(), Some(0), "{}", stderr(&removed));
    let none = dentist_row(&today_note(&env));
    assert!(!none.contains(" #off"), "{none}");
    assert!(!none.contains(" #on"), "{none}");
}

#[test]
fn off_clock_tag_flip_on_worked_row_is_kept_once() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-untracked-worked");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    // Past dentist is worked from birth ([/] + closed MEET link).
    let mut past = dentist_event(&serde_json::Map::new());
    past["start"] =
        serde_json::json!({"dateTime": "2026-10-30T08:00:00-04:00"});
    past["end"] = serde_json::json!({"dateTime": "2026-10-30T08:30:00-04:00"});
    fake.events_json(&events_envelope(vec![past]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let before = dentist_row(&today_note(&env));
    assert!(before.starts_with("- [/]"), "{before}");
    assert!(!before.contains("#untracked"), "{before}");

    // Google adds the token after the session closed: report once, leave
    // the row for a hand edit.
    let mut patch = serde_json::Map::new();
    patch.insert("summary".to_string(), "Dentist @body #untracked".into());
    patch.insert(
        "start".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T08:00:00-04:00"}),
    );
    patch.insert(
        "end".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T08:30:00-04:00"}),
    );
    fake.events_json(&events_envelope(vec![dentist_event(&patch)]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let row = dentist_row(&today_note(&env));
    assert!(!row.contains("#untracked"), "worked row kept:\n{row}");
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("add #off by hand to follow Google"),
        "flip reported once: {combined}"
    );

    // A rerun does not report again and still leaves the row alone.
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let row_again = dentist_row(&today_note(&env));
    assert!(!row_again.contains("#untracked"), "{row_again}");
    let combined_again = format!("{}{}", stdout(&again), stderr(&again));
    assert!(
        !combined_again.contains("add #off by hand to follow Google"),
        "reported once only: {combined_again}"
    );
}

#[test]
fn off_to_on_tag_flip_on_worked_row_is_kept_once() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-off-to-on-worked");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let patch_event = |summary: &str| {
        let mut patch = serde_json::Map::new();
        patch.insert("summary".to_string(), summary.into());
        patch.insert(
            "start".to_string(),
            serde_json::json!({"dateTime": "2026-10-30T08:00:00-04:00"}),
        );
        patch.insert(
            "end".to_string(),
            serde_json::json!({"dateTime": "2026-10-30T08:30:00-04:00"}),
        );
        dentist_event(&patch)
    };

    fake.events_json(&events_envelope(vec![patch_event("Dentist @body #off")]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(dentist_row(&today_note(&env)).contains(" #off [root:: body]"));

    fake.events_json(&events_envelope(vec![patch_event("Dentist @body #on")]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let row = dentist_row(&today_note(&env));
    assert!(row.starts_with("- [/]"), "worked row stays: {row}");
    assert!(
        row.contains(" #off [root:: body]"),
        "Google flip is kept: {row}"
    );
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("replace #off with #on by hand to follow Google"),
        "worked flip detail: {combined}"
    );

    let again = sync_output(&env, &fake, &[]);
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(
        !combined.contains("replace #off with #on by hand to follow Google"),
        "worked flip is reported once: {combined}"
    );
}

#[test]
fn preexisting_off_clock_title_migrates_both_spans_in_place() {
    use sha2::{Digest, Sha256};
    fn span_hash(text: &str) -> String {
        hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
    }

    let env = GcalEnv::new("bob-cli-gcal-lifecycle-untracked-migrate");
    prepare(&env);
    // Old-style row: the token lives in the title, badges carry no tag.
    let html = "https://calendar.google.com/event?eid=lunch";
    let old_badges = format!(
        " [📅]({html} \"Open in Google Calendar\") #OFFCLOCK [buffer:: -30m]"
    );
    let old_row = format!(
        "- [/] #task **1200-1300** Lunch #untracked{old_badges} \
         [root:: meet] ^lunch"
    );
    let note = format!(
        "# Friday 2026-10-30\n\n## Events\n\n\
         *↻ synced 11:00 · [you@example.com](https://calendar.google.com/calendar/r/day/2026/10/30?authuser=you%40example.com)*\n\
         {old_row}\n\n## Pomodoros\n"
    );
    fs::write(env.vault().join("2026/20261030.md"), &note).expect("write note");
    // Old journal spans match the old write.
    let journal_line = serde_json::json!({
        "v": 1,
        "ts": "2026-10-30T11:00:00-04:00",
        "host": "test-host",
        "kind": "gcal.row",
        "key": {"calendar_id": "primary", "event_id": "lunch"},
        "day": "2026-10-30",
        "path": "2026/20261030.md",
        "block_id": "lunch",
        "url": html,
        "row": "task",
        "spans": {
            "time": span_hash("1200-1300"),
            "title": span_hash("Lunch #untracked"),
            "badges": span_hash(&old_badges),
        },
        "root": {"value": "meet", "source": "default"},
    });
    let journal_dir = env.vault().join(".bob/gcal");
    fs::create_dir_all(&journal_dir).expect("journal dir");
    fs::write(
        journal_dir.join("test-host.jsonl"),
        serde_json::to_string(&journal_line).expect("journal") + "\n",
    )
    .expect("write journal");

    let fake = FakeGog::new(&env, "fake-gog");
    let events = serde_json::json!({
        "events": [{
            "id": "lunch",
            "summary": "Lunch #untracked",
            "status": "confirmed",
            "start": {"dateTime": "2026-10-30T12:00:00-04:00"},
            "end": {"dateTime": "2026-10-30T13:00:00-04:00"},
            "description": "-30",
            "htmlLink": html,
            "attendees": [{"self": true, "responseStatus": "accepted"}]
        }],
        "nextPageToken": serde_json::Value::Null,
    });
    fake.events_json(&serde_json::to_string(&events).expect("events"));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = note
        .lines()
        .find(|line| line.contains("^lunch"))
        .expect("lunch row");
    assert!(row.contains("**1200-1300** Lunch "), "{row}");
    assert!(row.starts_with("- [/]"), "worked checkbox stays: {row}");
    assert!(!row.contains("Lunch #untracked"), "{row}");
    assert!(
        row.contains(&format!(
            " [📅]({html}) #off [buffer:: -30m] [root:: meet]"
        )),
        "legacy badge alias and buffer migrate in place: {row}"
    );
    assert!(
        row.contains(&format!(" [📅]({html})")),
        "badges migrate title-free: {row}"
    );
    assert!(!row.contains("\"Open in Google Calendar\""), "{row}");
}

#[test]
fn preexisting_off_clock_badge_without_calendar_link_migrates_with_buffer() {
    use sha2::{Digest, Sha256};
    fn span_hash(text: &str) -> String {
        hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
    }

    let env = GcalEnv::new("bob-cli-gcal-lifecycle-off-badge-no-calendar");
    prepare(&env);
    let row = "- [/] #task **1200-1300** Lunch #offclock \
               [buffer:: -30m] [root:: body] ^lunch";
    let badges = " #offclock [buffer:: -30m]";
    let note = format!(
        "# Friday 2026-10-30\n\n## Events\n\n\
         *↻ synced 11:00 · [you@example.com](https://calendar.google.com/calendar/r/day/2026/10/30?authuser=you%40example.com)*\n\
         {row}\n\n## Pomodoros\n"
    );
    fs::write(env.vault().join("2026/20261030.md"), &note).expect("write note");
    let journal_line = serde_json::json!({
        "v": 1,
        "ts": "2026-10-30T11:00:00-04:00",
        "host": "test-host",
        "kind": "gcal.row",
        "key": {"calendar_id": "primary", "event_id": "lunch"},
        "day": "2026-10-30",
        "path": "2026/20261030.md",
        "block_id": "lunch",
        "url": null,
        "row": "task",
        "spans": {
            "time": span_hash("1200-1300"),
            "title": span_hash("Lunch"),
            "badges": span_hash(badges),
        },
        "root": {"value": "body", "source": "token"},
    });
    let journal_dir = env.vault().join(".bob/gcal");
    fs::create_dir_all(&journal_dir).expect("journal dir");
    fs::write(
        journal_dir.join("test-host.jsonl"),
        serde_json::to_string(&journal_line).expect("journal") + "\n",
    )
    .expect("write journal");

    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![serde_json::json!({
        "id": "lunch",
        "summary": "Lunch @body",
        "description": "#off -30",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T12:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T13:00:00-04:00"},
        "attendees": [{"self": true, "responseStatus": "accepted"}]
    })]));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let notices = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        !notices.contains("kept_off_clock"),
        "normalizing a worked alias to canonical off is not a flip: {notices}"
    );
    let updated_note = today_note(&env);
    let migrated = updated_note
        .lines()
        .find(|line| line.contains("^lunch"))
        .expect("lunch row");
    assert!(
        migrated.starts_with("- [/]"),
        "worked checkbox stays: {migrated}"
    );
    assert!(
        migrated.contains("Lunch #off [buffer:: -30m] [root:: body]"),
        "badge alias migrates without a Calendar badge: {migrated}"
    );
    assert!(!migrated.contains("#offclock"), "{migrated}");
}

#[test]
fn preexisting_titled_badges_migrate_in_place() {
    use sha2::{Digest, Sha256};
    fn span_hash(text: &str) -> String {
        hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
    }

    let env = GcalEnv::new("bob-cli-gcal-lifecycle-titled-migrate");
    prepare(&env);

    let migrate_html = "https://calendar.google.com/event?eid=migrate";
    let migrate_join = "https://meet.google.com/abc";
    let migrate_maps =
        "https://www.google.com/maps/search/?api=1&query=Main%20St%20Dental";
    let migrate_legacy = format!(
        " [📅]({migrate_html} \"Open in Google Calendar\") \
         [📹]({migrate_join} \"Join call\") [📍]({migrate_maps} \"Main St Dental\")"
    );
    let migrate_fresh = format!(
        " [📅]({migrate_html}) [📹]({migrate_join}) [📍]({migrate_maps})"
    );

    let hand_html = "https://calendar.google.com/event?eid=hand";
    let hand_join = "https://meet.google.com/hand";
    let hand_maps =
        "https://www.google.com/maps/search/?api=1&query=Hand%20Place";
    let hand_legacy = format!(
        " [📅]({hand_html} \"Open in Google Calendar\") \
         [📹]({hand_join} \"Join call\") [📍]({hand_maps} \"Hand Place\")"
    );
    let hand_fresh =
        format!(" [📅]({hand_html}) [📹]({hand_join}) [📍]({hand_maps})");
    // Bryan removed the 📍 badge by hand: badges no longer match the journal.
    let hand_edited = format!(
        " [📅]({hand_html} \"Open in Google Calendar\") \
         [📹]({hand_join} \"Join call\")"
    );

    let done_html = "https://calendar.google.com/event?eid=done";
    let done_join = "https://meet.google.com/done";
    let done_maps =
        "https://www.google.com/maps/search/?api=1&query=Done%20Place";
    let done_legacy = format!(
        " [📅]({done_html} \"Open in Google Calendar\") \
         [📹]({done_join} \"Join call\") [📍]({done_maps} \"Done Place\")"
    );
    let done_fresh =
        format!(" [📅]({done_html}) [📹]({done_join}) [📍]({done_maps})");

    let migrate_row = format!(
        "- [*] #task **1830-1900** Migrate{migrate_legacy} [root:: meet] ^migrate"
    );
    let hand_row = format!(
        "- [*] #task **1900-1930** Hand{hand_edited} [root:: meet] ^hand"
    );
    let done_row = format!(
        "- [x] #task **2000-2030** Done{done_legacy} [root:: meet] ^done"
    );
    let note = format!(
        "# Friday 2026-10-30\n\n## Events\n\n\
         *↻ synced 11:00 · [you@example.com](https://calendar.google.com/calendar/r/day/2026/10/30?authuser=you%40example.com)*\n\
         {migrate_row}\n{hand_row}\n{done_row}\n\n## Pomodoros\n"
    );
    fs::write(env.vault().join("2026/20261030.md"), &note).expect("write note");

    let journal_entry = |event_id: &str,
                         block_id: &str,
                         url: &str,
                         time: &str,
                         title: &str,
                         badges: &str| {
        serde_json::json!({
            "v": 1,
            "ts": "2026-10-30T11:00:00-04:00",
            "host": "test-host",
            "kind": "gcal.row",
            "key": {"calendar_id": "primary", "event_id": event_id},
            "day": "2026-10-30",
            "path": "2026/20261030.md",
            "block_id": block_id,
            "url": url,
            "row": "task",
            "spans": {
                "time": span_hash(time),
                "title": span_hash(title),
                "badges": span_hash(badges),
            },
            "root": {"value": "meet", "source": "default"},
        })
    };
    let journal = vec![
        journal_entry(
            "migrate",
            "migrate",
            migrate_html,
            "1830-1900",
            "Migrate",
            &migrate_legacy,
        ),
        journal_entry(
            "hand",
            "hand",
            hand_html,
            "1900-1930",
            "Hand",
            &hand_legacy,
        ),
        journal_entry(
            "done",
            "done",
            done_html,
            "2000-2030",
            "Done",
            &done_legacy,
        ),
    ];
    let journal_dir = env.vault().join(".bob/gcal");
    fs::create_dir_all(&journal_dir).expect("journal dir");
    let journal_text = journal
        .iter()
        .map(|line| serde_json::to_string(line).expect("journal"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(journal_dir.join("test-host.jsonl"), journal_text)
        .expect("write journal");

    let event = |id: &str,
                 summary: &str,
                 html: &str,
                 join: &str,
                 location: &str,
                 start: &str,
                 end: &str| {
        serde_json::json!({
            "id": id,
            "summary": summary,
            "status": "confirmed",
            "location": location,
            "htmlLink": html,
            "hangoutLink": join,
            "start": {"dateTime": start},
            "end": {"dateTime": end},
            "attendees": [{"self": true, "responseStatus": "accepted"}]
        })
    };
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &serde_json::to_string(&serde_json::json!({
            "events": [
                event(
                    "migrate",
                    "Migrate",
                    migrate_html,
                    migrate_join,
                    "Main St Dental",
                    "2026-10-30T18:30:00-04:00",
                    "2026-10-30T19:00:00-04:00"
                ),
                event(
                    "hand",
                    "Hand",
                    hand_html,
                    hand_join,
                    "Hand Place",
                    "2026-10-30T19:00:00-04:00",
                    "2026-10-30T19:30:00-04:00"
                ),
                event(
                    "done",
                    "Done",
                    done_html,
                    done_join,
                    "Done Place",
                    "2026-10-30T20:00:00-04:00",
                    "2026-10-30T20:30:00-04:00"
                ),
            ],
            "nextPageToken": serde_json::Value::Null,
        }))
        .expect("events"),
    );

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let migrated = note
        .lines()
        .find(|line| line.contains("^migrate"))
        .expect("migrate row")
        .to_string();
    assert!(migrated.contains(&migrate_fresh), "{migrated}");
    assert!(!migrated.contains('"'), "{migrated}");
    assert!(
        migrated.starts_with("- [*] #task **1830-1900** Migrate "),
        "{migrated}"
    );
    assert!(migrated.contains("[root:: meet]"), "{migrated}");

    let done = note
        .lines()
        .find(|line| line.contains("^done"))
        .expect("done row")
        .to_string();
    assert!(done.contains(&done_fresh), "{done}");
    assert!(!done.contains('"'), "{done}");
    assert!(
        done.starts_with("- [x] #task **2000-2030** Done "),
        "{done}"
    );

    let hand = note
        .lines()
        .find(|line| line.contains("^hand"))
        .expect("hand row")
        .to_string();
    assert!(!hand.contains(&hand_fresh), "hand edit kept: {hand}");
    assert!(hand.contains(&hand_edited), "hand edit kept: {hand}");
    assert!(!hand.contains("[📍]("), "hand edit kept: {hand}");

    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("kept your edit"),
        "hand kept once: {combined}"
    );

    let journal = journal_lines(&env);
    assert_eq!(
        last_row_for(&journal, "migrate")["spans"]["badges"],
        serde_json::Value::String(span_hash(&migrate_fresh)),
        "journal records title-free badges"
    );
    assert_eq!(
        last_row_for(&journal, "done")["spans"]["badges"],
        serde_json::Value::String(span_hash(&done_fresh)),
        "done row also migrates"
    );

    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert_eq!(today_note(&env), note, "rerun is a no-op");
    let combined_again = format!("{}{}", stdout(&again), stderr(&again));
    assert!(
        !combined_again.contains("kept your edit"),
        "reported once: {combined_again}"
    );
}

fn dentist_with_description(description: &str) -> Value {
    let mut patch = serde_json::Map::new();
    patch.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    patch.insert(
        "start".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T18:30:00-04:00"}),
    );
    patch.insert(
        "end".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T19:00:00-04:00"}),
    );
    dentist_event(&patch)
}

#[test]
fn buffer_change_updates_untouched_row_in_place_and_keeps_edits() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-buffer-update");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    // Future dentist stays untouched (open placeholder link, no closed work).
    fake.events_json(&events_envelope(vec![dentist_event(
        &serde_json::Map::new(),
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(!dentist_row(&today_note(&env)).contains("[buffer::"));

    // Google adds a buffer: the untouched row gains the field in place.
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card\n@job -30 +15",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let row = dentist_row(&today_note(&env));
    assert!(
        row.contains(" [buffer:: -30m +15m] [root:: body]"),
        "buffer updates in place: {row}"
    );
    assert!(stdout(&output).contains("~ task"), "{}", stdout(&output));
    let journal_count = journal_lines(&env).len();
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(stdout(&again).contains("unchanged"), "{}", stdout(&again));
    assert_eq!(journal_lines(&env).len(), journal_count);

    // A hand-removed field wins over a later Google change, reported once.
    let edited = today_note(&env).replacen(" [buffer:: -30m +15m]", "", 1);
    assert_ne!(edited, today_note(&env), "field present to remove");
    write_today(&env, &edited);
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card\n@job -45 +15",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(combined.contains("kept your edit"), "kept once: {combined}");
    assert!(
        !dentist_row(&today_note(&env)).contains("[buffer::"),
        "hand edit kept"
    );
    let again = sync_output(&env, &fake, &[]);
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(
        !combined.contains("kept your edit"),
        "reported once: {combined}"
    );
}

#[test]
fn root_reroute_keeps_buffered_google_run() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-buffer-reroute");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let mut patch = serde_json::Map::new();
    patch.insert("summary".to_string(), "Dentist @body #untracked".into());
    patch.insert(
        "description".to_string(),
        "Bring insurance card\n-30 +15".into(),
    );
    patch.insert(
        "start".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T18:30:00-04:00"}),
    );
    patch.insert(
        "end".to_string(),
        serde_json::json!({"dateTime": "2026-10-30T19:00:00-04:00"}),
    );
    fake.events_json(&events_envelope(vec![dentist_event(&patch)]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let before = dentist_row(&today_note(&env));
    assert!(
        before.contains(" #off [buffer:: -30m +15m] [root:: body]"),
        "{before}"
    );

    // Google reroutes to @job: the root swaps in place and the badges
    // span (off-clock marker plus buffer) is untouched.
    patch.insert("summary".to_string(), "Dentist @job #untracked".into());
    fake.events_json(&events_envelope(vec![dentist_event(&patch)]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("~ task"), "{}", stdout(&output));
    let row = dentist_row(&today_note(&env));
    assert!(
        row.contains(" #off [buffer:: -30m +15m] [root:: job]"),
        "reroute keeps the Google run: {row}"
    );

    // The badges span still matches the journal, so the next sync settles
    // instead of reporting a phantom badges edit.
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(stdout(&again).contains("unchanged"), "{}", stdout(&again));
}

#[test]
fn maybe_upgrade_gains_buffer_field() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-buffer-upgrade");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let mut tentative = evening_class("tentative");
    tentative["description"] = "Stretch\n-15".into();
    fake.events_json(&events_envelope(vec![tentative]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let first = today_note(&env);
    assert!(
        first.contains("- **2000-2100** Evening class · *maybe*"),
        "{first}"
    );
    assert!(!first.contains("[buffer::"), "context rows carry no buffer");

    // Maybe goes Yes: the upgrade renders the buffer field.
    let mut accepted = evening_class("accepted");
    accepted["description"] = "Stretch\n-15".into();
    fake.events_json(&events_envelope(vec![accepted]));
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
    assert!(row.contains(" [buffer:: -15m] [root:: meet]"), "{row}");
    // The upgrade also writes the description bullet for "Stretch"
    // (`-15` is a directive); the rerun below proves it is stable.
    assert!(note.contains("\t- Stretch"), "{note}");
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
fn google_description_edit_rewrites_untouched_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-edit");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(today_note(&env).contains("\t- Bring insurance card"));

    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring floss too",
    )]));
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(note.contains("\t- Bring floss too"), "{note}");
    assert!(!note.contains("Bring insurance card"), "{note}");
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    let updated = report["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["action"] == "updated")
        .expect("updated row");
    assert_eq!(updated["description_lines"], 1);
    assert!(
        !stdout(&output).contains("floss"),
        "no description text in JSON: {}",
        stdout(&output)
    );

    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(stdout(&again).contains("unchanged"), "{}", stdout(&again));
}

#[test]
fn google_clearing_description_removes_the_block() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-clear");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    fake.events_json(&events_envelope(vec![dentist_with_description("")]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(!note.contains("Bring insurance card"), "{note}");
    assert!(note.contains("^dentist"), "{note}");
}

#[test]
fn edited_description_is_kept_silently_then_reported_once() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-kept");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let edited = today_note(&env).replace(
        "\t- Bring insurance card",
        "\t- Bring insurance card (and a book)",
    );
    write_today(&env, &edited);

    let silent = sync_output(&env, &fake, &[]);
    assert_eq!(silent.status.code(), Some(0), "{}", stderr(&silent));
    let combined = format!("{}{}", stdout(&silent), stderr(&silent));
    assert!(!combined.contains("kept_description"), "{combined}");
    assert!(!combined.contains("edited description"), "{combined}");
    assert!(
        today_note(&env).contains("and a book"),
        "{}",
        today_note(&env)
    );

    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring floss too",
    )]));
    let reported = sync_output(&env, &fake, &[]);
    assert_eq!(reported.status.code(), Some(0), "{}", stderr(&reported));
    let combined = format!("{}{}", stdout(&reported), stderr(&reported));
    assert!(
        combined.contains("kept your edited description bullets"),
        "kept once: {combined}"
    );
    assert!(
        today_note(&env).contains("and a book"),
        "{}",
        today_note(&env)
    );
    assert!(!today_note(&env).contains("floss"), "{}", today_note(&env));

    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(!combined.contains("edited description"), "{combined}");
}

#[test]
fn deleted_description_block_is_never_re_added() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-deleted");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let stripped = today_note(&env).replace("\t- Bring insurance card\n", "");
    write_today(&env, &stripped);
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(
        !today_note(&env).contains("Bring insurance card"),
        "{}",
        today_note(&env)
    );

    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring floss too",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!today_note(&env).contains("floss"), "{}", today_note(&env));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("kept your edited description bullets"),
        "{combined}"
    );
}

#[test]
fn pre_feature_row_backfills_on_the_next_sync() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-backfill");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  description: off\n  colors:\n    tomato: job\n",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    assert!(
        !today_note(&env).contains("Bring insurance card"),
        "{}",
        today_note(&env)
    );

    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  colors:\n    tomato: job\n",
    );
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        today_note(&env).contains("\t- Bring insurance card"),
        "{}",
        today_note(&env)
    );
    let journal_count = journal_lines(&env).len();
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(stdout(&again).contains("unchanged"), "{}", stdout(&again));
    assert_eq!(journal_lines(&env).len(), journal_count);
}

#[test]
fn crash_adoption_matching_bullets_does_not_duplicate() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-adopt");
    prepare(&env);
    let hand = concat!(
        "# Friday 2026-10-30\n\n## Events\n\n",
        "- [*] #task **1830-1900** Dentist [📅](https://calendar.google.com/event?eid=dentist \"Open in Google Calendar\") [📍](https://www.google.com/maps/search/?api=1&query=Main%20St%20Dental \"Main St Dental\") [root:: body] [type:: MEET] ^dentist\n",
        "\t- Bring insurance card\n",
        "\n## Pomodoros\n\n- [ ] () — MEET\n\t- [[#^dentist]]\n",
    );
    fs::write(env.vault().join("2026/20261030.md"), hand).expect("hand row");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert_eq!(
        note.matches("Bring insurance card").count(),
        1,
        "never duplicated:\n{note}"
    );
    let latest = last_row_for(&journal_lines(&env), "dentist");
    assert_eq!(latest["description"]["lines"], 1, "{latest}");
}

#[test]
fn malformed_description_record_is_hands_off() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-malformed");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let path = env.vault().join(".bob/gcal/test-host.jsonl");
    let text = fs::read_to_string(&path).expect("journal");
    let mut rewritten = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut value: Value =
            serde_json::from_str(line).expect("journal JSON");
        if value["kind"] == "gcal.row" && value["key"]["event_id"] == "dentist"
        {
            value["description"] = Value::String("bogus".to_string());
        }
        rewritten.push_str(&value.to_string());
        rewritten.push('\n');
    }
    fs::write(&path, rewritten).expect("write malformed journal");

    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring floss too",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        today_note(&env).contains("\t- Bring insurance card"),
        "{}",
        today_note(&env)
    );
    assert!(!today_note(&env).contains("floss"), "{}", today_note(&env));
}

#[test]
fn tomato_in_description_does_not_count_as_worked() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-tomato");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring 🍅 salad",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let note = today_note(&env);
    assert!(note.contains("&#127813;"), "{note}");
    assert!(!note.contains('🍅'), "{note}");

    fake.events_json(&decline_dentist_fixture());
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = today_note(&env);
    assert!(
        dentist_row(&after).starts_with("- [-] #task"),
        "tomato bullet is not worked:\n{after}"
    );
    assert!(after.contains("CANCEL LOG"), "{after}");
}

#[test]
fn description_off_leaves_existing_bullets_alone() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-off-carry");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring insurance card",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  description: off\n  colors:\n    tomato: job\n",
    );
    fake.events_json(&events_envelope(vec![dentist_with_description(
        "Bring floss too",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        today_note(&env).contains("\t- Bring insurance card"),
        "{}",
        today_note(&env)
    );
    assert!(!today_note(&env).contains("floss"), "{}", today_note(&env));
    let latest = last_row_for(&journal_lines(&env), "dentist");
    assert_eq!(latest["description"]["lines"], 1, "{latest}");
}

#[test]
fn context_rows_still_never_get_description_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-lifecycle-desc-context");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let mut class = serde_json::json!({
        "id": "class",
        "status": "confirmed",
        "summary": "Toddler class",
        "description": "Bring shoes",
        "eventType": "default",
        "start": {"dateTime": "2026-10-30T09:15:00-04:00"},
        "end": {"dateTime": "2026-10-30T10:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=class",
        "attendees": [{"self": true, "responseStatus": "tentative"}]
    });
    fake.events_json(&events_envelope(vec![class.clone()]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let note = today_note(&env);
    assert!(note.contains("Toddler class"), "{note}");
    assert!(!note.contains("Bring shoes"), "{note}");

    class["attendees"] =
        serde_json::json!([{"self": true, "responseStatus": "declined"}]);
    fake.events_json(&events_envelope(vec![class]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        !today_note(&env).contains("Toddler class"),
        "declined context row is removed:\n{}",
        today_note(&env)
    );
}

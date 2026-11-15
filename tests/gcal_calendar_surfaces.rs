//! Calendar marks across the public gcal command surfaces.

mod gcal_support;

use gcal_support::{json_stdout, stderr, stdout, FakeGog, GcalEnv, ACCOUNT};
use serde_json::json;

fn marked_env() -> (GcalEnv, FakeGog) {
    let env = GcalEnv::new("bob-cli-gcal-calendar-surfaces");
    env.prepare_healthy_vault();
    env.write_meet_inbox_area();
    env.write_config(&format!(
        "timezone: America/New_York\ngcal:\n  account: {ACCOUNT}\n  calendars:\n    - id: primary\n    - id: bdays@group.calendar.google.com\n      mark: 🎂\n"
    ));
    let fake = FakeGog::new(&env, "fake-gog");
    fake.add_birthdays_calendar();
    let empty = json!({"events": [], "nextPageToken": null}).to_string();
    fake.events_json_for(ACCOUNT, &empty);
    fake.events_json_for(
        "bdays@group.calendar.google.com",
        &json!({
            "events": [{
                "id": "ava-birthday",
                "iCalUID": "ava-birthday@example.test",
                "status": "confirmed",
                "summary": "Ava's Birthday",
                "eventType": "default",
                "transparency": "transparent",
                "start": {"date": "2026-10-30"},
                "end": {"date": "2026-10-31"},
                "attendees": [],
            }, {
                "id": "planning-call",
                "iCalUID": "planning-call@example.test",
                "status": "confirmed",
                "summary": "Planning call",
                "eventType": "default",
                "organizer": {
                    "email": "host@example.test",
                    "displayName": "Host",
                    "self": false,
                },
                "start": {"dateTime": "2026-10-30T12:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T12:30:00-04:00"},
                "attendees": [{"self": true, "responseStatus": "accepted"}],
            }],
            "nextPageToken": null,
        })
        .to_string(),
    );
    (env, fake)
}

fn run(env: &GcalEnv, fake: &FakeGog, args: &[&str]) -> std::process::Output {
    let mut command = env.command();
    fake.install(&mut command)
        .args(args)
        .output()
        .expect("run gcal command")
}

#[test]
fn calendars_list_sync_and_doctor_expose_calendar_marks() {
    let (env, fake) = marked_env();

    let output = run(&env, &fake, &["gcal", "calendars"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let table = stdout(&output);
    assert!(
        table.contains("Google Calendar · calendars · you@example.com"),
        "{table}"
    );
    assert!(
        table.lines().any(|line| {
            line.split_whitespace().take(4).collect::<Vec<_>>()
                == ["NAME", "SYNC", "MARK", "ROOT"]
        }),
        "{table}"
    );
    assert!(table.contains("Birthdays  yes   🎂"), "{table}");
    assert!(table.contains("Work       no    —"), "{table}");
    assert!(
        table.find("Birthdays").unwrap() < table.find("Work").unwrap(),
        "{table}"
    );

    let output = run(&env, &fake, &["gcal", "calendars", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let calendars = json_stdout(&output);
    let birthdays = calendars["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|calendar| calendar["id"] == "bdays@group.calendar.google.com")
        .unwrap();
    assert_eq!(birthdays["mark"], "🎂");
    let primary = calendars["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|calendar| calendar["primary"] == true)
        .unwrap();
    assert!(primary["mark"].is_null());

    let output = run(
        &env,
        &fake,
        &["gcal", "list", "-D", "2026-10-30", "-f", "json"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let list = json_stdout(&output);
    assert_eq!(list["calendars"][1]["mark"], "🎂");
    assert_eq!(list["events"][0]["title"], "Ava's Birthday");
    assert_eq!(
        list["events"][0]["calendar_id"],
        "bdays@group.calendar.google.com"
    );
    assert_eq!(
        list["events"][0]["calendar"],
        json!({
            "id": "bdays@group.calendar.google.com",
            "name": "Birthdays",
            "mark": "🎂",
        })
    );

    let output = run(&env, &fake, &["gcal", "list", "-D", "2026-10-30"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(
        listing.contains("you@example.com + 🎂 Birthdays"),
        "{listing}"
    );
    assert!(listing.contains("🎂 Ava's Birthday"), "{listing}");

    let output = run(&env, &fake, &["gcal", "ack", "-l", "-D", "2026-10-30"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("🎂 Planning call"),
        "{}",
        stdout(&output)
    );

    let output = run(
        &env,
        &fake,
        &["gcal", "note", "-a", "-l", "-D", "2026-10-30"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("🎂 Planning call"),
        "{}",
        stdout(&output)
    );

    let output = run(
        &env,
        &fake,
        &["gcal", "sync", "-D", "2026-10-30", "-d", "-f", "json"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let sync = json_stdout(&output);
    let row = sync["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == "Ava's Birthday")
        .unwrap();
    assert_eq!(row["calendar_id"], "bdays@group.calendar.google.com");
    assert_eq!(
        row["calendar"],
        json!({
            "id": "bdays@group.calendar.google.com",
            "name": "Birthdays",
            "mark": "🎂",
        })
    );

    let output = run(&env, &fake, &["gcal", "sync", "-D", "2026-10-30", "-d"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("🎂 Ava's Birthday"),
        "{}",
        stdout(&output)
    );

    let output = run(&env, &fake, &["gcal", "doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doctor = json_stdout(&output);
    let calendars = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "calendars")
        .unwrap();
    assert_eq!(calendars["status"], "ok");
    assert_eq!(
        calendars["summary"],
        "2 synced · you@example.com, 🎂 Birthdays"
    );

    let output = run(&env, &fake, &["gcal", "doctor"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("2 synced · you@example.com, 🎂 Birthdays"),
        "{}",
        stdout(&output)
    );
}

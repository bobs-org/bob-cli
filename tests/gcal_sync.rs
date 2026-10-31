//! `bob gcal sync`: Events rows, MEET links, journal, and outputs.

mod gcal_support;

use gcal_support::{stderr, stdout, FakeGog, GcalEnv};
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

fn journal_lines(env: &GcalEnv) -> Vec<Value> {
    let path = env.vault().join(".bob/gcal/test-host.jsonl");
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("journal JSON"))
        .collect()
}

fn status(env: &GcalEnv) -> Value {
    let path = env.root().join("state/bob/gcal/status.json");
    serde_json::from_str(&fs::read_to_string(path).expect("status file"))
        .expect("status JSON")
}

#[test]
fn first_sync_creates_events_meet_and_links() {
    let env = GcalEnv::new("bob-cli-gcal-sync-first");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);

    assert!(
        note.contains(
            "# Friday 2026-10-30\n## Events\n\n\
             *↻ synced 12:00 · [you@example.com](\
             https://calendar.google.com/calendar/r/day/2026/10/30\
             ?authuser=you%40example.com)*\n\
             - **all day** Conference"
        ),
        "{note}"
    );

    // Events section above Pomodoros with time-ordered rows.
    let events_at = note.find("## Events").expect("Events section");
    let pomodoros_at = note.find("## Pomodoros").expect("Pomodoros");
    assert!(events_at < pomodoros_at, "{note}");
    let events_section = &note[events_at..pomodoros_at];
    let rows: Vec<&str> = events_section
        .lines()
        .filter(|line| line.starts_with("- "))
        .collect();
    assert_eq!(rows.len(), 10, "{note}");
    // All-day first (by start), then by start time.
    assert!(
        rows[0].starts_with("- **all day** Conference · *day 2 of 2*"),
        "{note}"
    );
    assert!(
        rows[1].starts_with("- **all day** School holiday"),
        "{note}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("**0800-0830** Product interview")),
        "{note}"
    );

    // Interview ended before sync: In Progress under a closed MEET.
    let interview = rows
        .iter()
        .find(|row| row.contains("Product interview"))
        .expect("interview row");
    assert!(interview.starts_with("- [/] #task"), "{interview}");
    assert!(interview.contains("[root:: job]"), "{interview}");
    assert!(interview.contains("[type:: MEET]"), "{interview}");
    assert!(interview.contains("[📹]("), "{interview}");
    assert!(interview.ends_with("^product-interview"), "{interview}");

    // Dentist is tonight: linked, rooted by its @body token, typed.
    let dentist = rows
        .iter()
        .find(|row| row.contains("**1830-1900** Dentist "))
        .expect("dentist row");
    assert!(dentist.starts_with("- [*] #task"), "{dentist}");
    assert!(!dentist.contains("@body"), "{dentist}");
    assert!(dentist.contains("[root:: body]"), "{dentist}");
    assert!(dentist.contains("[type:: MEET]"), "{dentist}");
    assert!(dentist.contains("[📍]("), "{dentist}");
    assert!(dentist.ends_with("^dentist"), "{dentist}");

    // Default-root rows carry no type field once meet declares it.
    let gmail = rows
        .iter()
        .find(|row| row.contains("Flight from Gmail"))
        .expect("gmail row");
    assert!(gmail.contains("[root:: meet]"), "{gmail}");
    assert!(!gmail.contains("[type::"), "{gmail}");

    // Closed MEET for the ended interview, then one placeholder for the rest.
    let pomodoros = &note[pomodoros_at..];
    assert!(
        pomodoros.contains("- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{pomodoros}"
    );
    assert!(
        pomodoros.contains("\t- 🍅 [[#^product-interview]]"),
        "{pomodoros}"
    );
    assert!(pomodoros.contains("- [ ] () — MEET"), "{pomodoros}");
    for id in ["dentist", "flight-gmail", "late-call", "late-meeting"] {
        assert!(
            pomodoros.contains(&format!("\t- [[#^{id}]]")),
            "missing link {id}:\n{pomodoros}"
        );
    }

    // Journal: one row per included occurrence plus five links.
    let journal = journal_lines(&env);
    let rows_journal = journal
        .iter()
        .filter(|line| line["kind"] == "gcal.row")
        .count();
    let links = journal
        .iter()
        .filter(|line| line["kind"] == "gcal.link")
        .count();
    assert_eq!(rows_journal, 10, "{journal:?}");
    assert_eq!(links, 5, "{journal:?}");
    assert!(
        journal
            .iter()
            .all(|line| line["v"] == 1 && line["host"] == "test-host"),
        "{journal:?}"
    );

    // Host-local status.
    let status = status(&env);
    assert_eq!(status["outcome"], "written");
    assert_eq!(status["day"], "2026-10-30");
    assert_eq!(status["tasks_added"], 5);
    assert_eq!(status["links_added"], 5);

    let out = stdout(&output);
    assert!(out.contains("closed 08:00–08:30 — MEET"), "{out}");
    assert!(out.contains("created () — MEET"), "{out}");
    assert!(out.contains("Pomodoros +2 entries, +5 links"), "{out}");
}

#[test]
fn sync_restores_missing_blank_under_events_heading() {
    let env = GcalEnv::new("bob-cli-gcal-sync-heading-gap");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let first = sync_output(&env, &fake, &[]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let original = today_note(&env);
    let journal_count = journal_lines(&env).len();
    let without_gap = original.replacen("## Events\n\n", "## Events\n", 1);
    assert_ne!(without_gap, original, "fixture should have an Events gap");
    fs::write(env.vault().join("2026/20261030.md"), without_gap)
        .expect("remove heading gap");

    let repaired = sync_output(&env, &fake, &[]);
    assert_eq!(repaired.status.code(), Some(0), "{}", stderr(&repaired));
    let repaired_note = today_note(&env);
    assert!(repaired_note.contains("## Events\n\n"), "{repaired_note}");
    assert_eq!(journal_lines(&env).len(), journal_count);
    assert_eq!(status(&env)["outcome"], "written");

    let third = sync_output(&env, &fake, &[]);
    assert_eq!(third.status.code(), Some(0), "{}", stderr(&third));
    assert!(stdout(&third).contains("unchanged"), "{}", stdout(&third));
    assert_eq!(today_note(&env), repaired_note);
}

#[test]
fn second_run_is_byte_identical_and_appends_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-sync-idempotent");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let first = sync_output(&env, &fake, &[]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let note = today_note(&env);
    let journal_before = journal_lines(&env).len();

    let second = sync_output(&env, &fake, &[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert_eq!(today_note(&env), note, "second run must be byte-identical");
    assert_eq!(
        journal_lines(&env).len(),
        journal_before,
        "no-op appends nothing"
    );
    assert!(stdout(&second).contains("unchanged"), "{}", stdout(&second));
}

#[test]
fn dry_run_prints_diff_and_writes_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-sync-dry");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-d"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let diff = stdout(&output);
    assert!(diff.contains("## Events"), "{diff}");
    assert!(
        diff.contains("+- [*] #task **1830-1900** Dentist"),
        "{diff}"
    );
    assert!(
        diff.contains("+- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{diff}"
    );
    // Nothing written: no rows, no journal, no status file.
    assert!(
        !today_note(&env).contains("## Events"),
        "{}",
        today_note(&env)
    );
    assert!(
        !env.vault().join(".bob/gcal/test-host.jsonl").exists(),
        "dry run appends nothing"
    );
    assert!(
        !env.root().join("state/bob/gcal/status.json").exists(),
        "dry run records no status"
    );
}

#[test]
fn deleted_row_and_removed_link_are_never_readded() {
    let env = GcalEnv::new("bob-cli-gcal-sync-tombstone");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    // Delete the dentist row but keep its link; remove the gmail link.
    let note = today_note(&env)
        .lines()
        .filter(|line| !line.contains("**1830-1900** Dentist "))
        .filter(|line| !line.contains("[[#^flight-gmail]]"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(env.vault().join("2026/20261030.md"), &note).expect("edit");

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = today_note(&env);
    assert!(!after.contains("Dentist @body"), "{after}");
    assert!(!after.contains("Dentist"), "{after}");
    assert!(!after.contains("[[#^flight-gmail]]"), "{after}");
    let journal = journal_lines(&env);
    assert!(
        journal.iter().any(|line| line["kind"] == "gcal.gone"),
        "tombstone the removed row: {journal:?}"
    );
}

#[test]
fn crash_recovery_adopts_unjournaled_rows() {
    let env = GcalEnv::new("bob-cli-gcal-sync-adopt");
    prepare(&env);
    // Hand-write the dentist row (as a crash would leave it) with the
    // same block id sync would mint, plus its MEET link.
    let hand = "# Friday 2026-10-30\n\n## Events\n\n- [*] #task **1830-1900** Dentist [📅](https://calendar.google.com/event?eid=dentist \"Open in Google Calendar\") [📍](https://www.google.com/maps/search/?api=1&query=Main%20St%20Dental \"Main St Dental\") [root:: body] [type:: MEET] ^dentist\n\n## Pomodoros\n\n- [ ] () — MEET\n\t- [[#^dentist]]\n";
    fs::write(env.vault().join("2026/20261030.md"), hand).expect("hand row");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert_eq!(
        note.matches("^dentist").count(),
        2,
        "one adopted row plus one link, no duplicate:\n{note}"
    );
    let journal = journal_lines(&env);
    assert!(
        journal
            .iter()
            .any(|line| line["kind"] == "gcal.row"
                && line["block_id"] == "dentist"),
        "adopted row is journaled: {journal:?}"
    );
    assert!(
        journal
            .iter()
            .any(|line| line["kind"] == "gcal.link"
                && line["block_id"] == "dentist"),
        "adopted link is journaled: {journal:?}"
    );
    assert!(
        stdout(&output).contains("adopted") || note.contains("^dentist"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn meet_placeholder_follows_the_last_open_entry() {
    let env = GcalEnv::new("bob-cli-gcal-sync-placement");
    prepare(&env);
    // A PLAN entry with an in-flight session: MEET goes after it, so a
    // bare `bob capture '='` still starts PLAN.
    fs::write(
        env.vault().join("2026/20261030.md"),
        "# Friday 2026-10-30\n\n## Pomodoros\n\n- [ ] (**0900-0925** [t:: 25m]) — PLAN\n\t- [[work#^outline]]\n",
    )
    .expect("plan entry");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let closed_at = note
        .find("- [x] (**0800-0830** [t:: 30m]) — MEET")
        .expect("closed MEET");
    let plan_at = note.find("— PLAN").expect("PLAN entry");
    let placeholder_at = note.find("- [ ] () — MEET").expect("placeholder");
    assert!(closed_at < plan_at && plan_at < placeholder_at, "{note}");
}

#[test]
fn existing_open_meet_is_reused() {
    let env = GcalEnv::new("bob-cli-gcal-sync-reuse");
    prepare(&env);
    fs::write(
        env.vault().join("2026/20261030.md"),
        "# Friday 2026-10-30\n\n## Pomodoros\n\n- [ ] (**0900-0925** [t:: 25m]) — MEET\n",
    )
    .expect("running meet");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(
        note.contains("- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{note}"
    );
    assert_eq!(
        note.lines()
            .filter(|line| line.contains("— MEET") && line.contains("[ ]"))
            .count(),
        1,
        "no second open MEET:\n{note}"
    );
    assert!(
        note.contains("- [ ] (**0900-0925** [t:: 25m]) — MEET"),
        "{note}"
    );
    assert!(note.contains("\t- [[#^dentist]]"), "{note}");
}

#[test]
fn past_day_gets_closed_meet_entries() {
    let env = GcalEnv::new("bob-cli-gcal-sync-past");
    prepare(&env);
    fs::write(
        env.vault().join("2026/20261029.md"),
        "# Thursday 2026-10-29\n\n## Pomodoros\n",
    )
    .expect("yesterday note");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &serde_json::json!({
            "events": [{
                "id": "yesterday-standup",
                "summary": "Yesterday standup",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-29T09:00:00-04:00"},
                "end": {"dateTime": "2026-10-29T09:30:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=yesterday-standup",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            }],
            "nextPageToken": serde_json::Value::Null,
        })
        .to_string(),
    );

    let output = sync_output(&env, &fake, &["-D", "yesterday"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = fs::read_to_string(env.vault().join("2026/20261029.md"))
        .expect("yesterday");
    assert!(note.contains("## Events"), "{note}");
    assert!(
        note.contains("- [x] (**0900-0930** [t:: 30m]) — MEET"),
        "past days gain closed MEET entries:\n{note}"
    );
    assert!(
        note.lines().any(|line| line.starts_with("- [/] #task")),
        "{note}"
    );
    assert!(note.contains("\t- 🍅 [[#^yesterday-standup]]"), "{note}");
    // Today's note is untouched.
    assert!(
        !today_note(&env).contains("## Events"),
        "{}",
        today_note(&env)
    );
}

#[test]
fn future_day_gets_placeholder_and_starred_rows() {
    let env = GcalEnv::new("bob-cli-gcal-sync-future");
    prepare(&env);
    fs::write(
        env.vault().join("2026/20261031.md"),
        "# Saturday 2026-10-31\n\n## Pomodoros\n",
    )
    .expect("tomorrow note");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(
        &serde_json::json!({
            "events": [{
                "id": "tomorrow-standup",
                "summary": "Tomorrow standup",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-31T09:00:00-04:00"},
                "end": {"dateTime": "2026-10-31T09:30:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=tomorrow-standup",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            }],
            "nextPageToken": serde_json::Value::Null,
        })
        .to_string(),
    );

    let output = sync_output(&env, &fake, &["-D", "tomorrow"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = fs::read_to_string(env.vault().join("2026/20261031.md"))
        .expect("tomorrow");
    assert!(note.contains("## Events"), "{note}");
    assert!(note.contains("- [ ] () — MEET"), "{note}");
    assert!(
        note.lines().any(|line| line.starts_with("- [*] #task")),
        "{note}"
    );
    assert!(note.contains("\t- [[#^tomorrow-standup]]"), "{note}");
}

#[test]
fn missing_note_waits_with_exit_zero() {
    let env = GcalEnv::new("bob-cli-gcal-sync-waiting");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-D", "2026-11-01"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("waiting for 2026/20261101.md"),
        "{}",
        stdout(&output)
    );
    assert!(
        !env.vault().join(".bob/gcal/test-host.jsonl").exists(),
        "waiting writes nothing"
    );
}

#[test]
fn incomplete_snapshot_writes_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-sync-incomplete");
    prepare(&env);
    let before = today_note(&env);
    let fake = FakeGog::new(&env, "fake-gog").scenario("events_failure");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(today_note(&env), before, "failed fetch writes nothing");
    assert!(
        !env.vault().join(".bob/gcal/test-host.jsonl").exists(),
        "failed fetch appends nothing"
    );
}

#[test]
fn foreign_writer_refuses_but_dry_run_previews() {
    let env = GcalEnv::new("bob-cli-gcal-sync-writer");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: other-host\n  colors:\n    tomato: job\n",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("other-host"),
        "{}",
        stderr(&output)
    );
    assert!(
        !today_note(&env).contains("## Events"),
        "refused sync writes nothing"
    );

    let dry = sync_output(&env, &fake, &["-d"]);
    assert_eq!(dry.status.code(), Some(0), "{}", stderr(&dry));
    assert!(stdout(&dry).contains("## Events"), "{}", stdout(&dry));
}

#[test]
fn sync_json_reports_rows_and_pomodoro() {
    let env = GcalEnv::new("bob-cli-gcal-sync-json");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["ok"], true);
    assert_eq!(report["day"], "2026-10-30");
    assert_eq!(report["path"], "2026/20261030.md");
    assert_eq!(report["status"], "written");
    assert_eq!(report["pomodoro"]["links_added"], 5);
    assert_eq!(report["pomodoro"]["created"], true);
    let rows = report["rows"].as_array().expect("rows");
    let dentist = rows
        .iter()
        .find(|row| row["title"] == "Dentist")
        .expect("dentist row");
    assert_eq!(dentist["action"], "added");
    assert_eq!(dentist["kind"], "task");
    assert_eq!(dentist["time"], "1830-1900");
    assert_eq!(dentist["root"], "body");
    assert_eq!(dentist["source"], "@body");
    assert_eq!(report["stamp"]["account"], "you@example.com");
    assert_eq!(report["stamp"]["synced_at"], "2026-10-30T12:00");
    assert_eq!(report["stamp"]["refreshed"], true);
    assert!(
        !stdout(&output).contains("Main St Dental\"],")
            && !stdout(&output).contains("attendees"),
        "no descriptions or attendees leak"
    );
}

#[test]
fn hostile_titles_stay_data() {
    let env = GcalEnv::new("bob-cli-gcal-sync-hostile");
    prepare(&env);
    let hostile = serde_json::json!({
        "events": [
            {
                "id": "h1",
                "summary": "Fix #task now",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T18:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T19:00:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=h1",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "h2",
                "summary": "Party 📅 2026-10-30",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T20:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T21:00:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=h2",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "h3",
                "summary": "#hashtag retro",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T21:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T22:00:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=h3",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            }
        ],
        "nextPageToken": serde_json::Value::Null,
    });
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&serde_json::to_string(&hostile).unwrap());

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(note.contains("\\#task"), "{note}");
    assert!(note.contains("📅\u{200b} 2026-10-30"), "{note}");
    assert!(note.contains("\\#hashtag retro"), "{note}");
}

fn run_bob(env: &GcalEnv, args: &[&str], now: &str) -> std::process::Output {
    let mut command = env.command();
    command.env("BOB_NOW", now);
    command.args(args).output().expect("run bob")
}

#[test]
fn reconcile_keeps_linked_rows() {
    let env = GcalEnv::new("bob-cli-gcal-sync-reconcile");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let before = today_note(&env);
    let tomato = before
        .lines()
        .find(|line| line.contains("🍅 [[#^product-interview]]"))
        .expect("tomato link")
        .to_string();
    let output = run_bob(&env, &["task", "reconcile"], "2026-10-30 12:00:00");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let interview = note
        .lines()
        .find(|line| line.contains("^product-interview"))
        .expect("interview row");
    assert!(
        interview.starts_with("- [/]"),
        "reconcile cleared the past row: {interview}"
    );
    assert!(
        note.lines().any(|line| line == tomato),
        "reconcile repaired the tomato link:\nbefore={tomato}\n{note}"
    );
    for id in ["dentist", "late-call", "flight-gmail", "late-meeting"] {
        let row = note
            .lines()
            .find(|line| line.contains(&format!("^{id}")))
            .unwrap_or_else(|| panic!("missing row {id}:\n{note}"));
        assert!(row.starts_with("- [*]"), "reconcile cleared {id}: {row}");
        assert!(
            note.contains(&format!("\t- [[#^{id}]]")),
            "reconcile dropped the {id} link:\n{note}"
        );
    }
}

#[test]
fn closed_meet_session_attributes_body_in_stats() {
    let env = GcalEnv::new("bob-cli-gcal-sync-close");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    // The Tasks plugin format must be Dataview for stats to run.
    let settings = env
        .vault()
        .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    fs::create_dir_all(settings.parent().expect("settings dir"))
        .expect("settings dir");
    fs::write(&settings, "{\"taskFormat\":\"dataview\"}").expect("settings");

    let start = run_bob(&env, &["capture", "=meet"], "2026-10-30 18:30:00");
    assert_eq!(start.status.code(), Some(0), "{}", stderr(&start));
    let close = run_bob(&env, &["capture", "x"], "2026-10-30 19:05:00");
    assert_eq!(close.status.code(), Some(0), "{}", stderr(&close));
    assert!(stdout(&close).contains("ended 19:00"), "{}", stdout(&close));

    let note = today_note(&env);
    let dentist = note
        .lines()
        .find(|line| line.contains("^dentist"))
        .expect("dentist row");
    assert!(dentist.starts_with("- [x]"), "{dentist}");
    let gmail = note
        .lines()
        .find(|line| line.contains("^flight-gmail"))
        .expect("gmail row");
    assert!(
        gmail.starts_with("- [*]"),
        "running meetings stay open: {gmail}"
    );
    assert!(
        note.contains("\t- [[#^flight-gmail]]"),
        "deferred meetings are carried:\n{note}"
    );

    let stats = run_bob(
        &env,
        &["stats", "show", "worked", "-r", "1d", "-f", "json"],
        "2026-10-30 19:05:00",
    );
    assert_eq!(stats.status.code(), Some(0), "{}", stderr(&stats));
    let report: Value =
        serde_json::from_str(&stdout(&stats)).expect("stats JSON");
    let body_minutes = report["metrics"][0]["points"]
        .as_array()
        .expect("points")
        .iter()
        .filter_map(|point| point["parts"]["area"]["body.md"].as_u64())
        .sum::<u64>();
    assert!(
        body_minutes > 0,
        "dentist time attributes to body: {report}"
    );
}

#[test]
fn agenda_reads_synced_rows_cleanly() {
    let env = GcalEnv::new("bob-cli-gcal-sync-agenda");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let output = run_bob(
        &env,
        &["capture-pomodoros", "--tasks", "-f", "json"],
        "2026-10-30 12:00:00",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let agenda: Value =
        serde_json::from_str(&stdout(&output)).expect("agenda JSON");
    let text = agenda.to_string();
    // Linked rows resolve with clean titles; the ended interview sits
    // under a closed MEET, so it stays out of the open agenda.
    assert!(text.contains("Dentist"), "{text}");
    assert!(text.contains("Late call"), "{text}");
    assert!(!text.contains("Product interview"), "{text}");
}

#[test]
fn quiet_prints_nothing_on_success() {
    let env = GcalEnv::new("bob-cli-gcal-sync-quiet");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-q"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).is_empty(), "{:?}", stdout(&output));
    assert!(today_note(&env).contains("## Events"));
}

fn sync_at(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    now: &str,
) -> std::process::Output {
    let mut command = env.command();
    let mut full = vec!["gcal", "sync"];
    full.extend(args);
    fake.install(&mut command).env("BOB_NOW", now);
    command.args(full).output().expect("run sync")
}

fn today_stamp(time: &str) -> String {
    format!(
        "*↻ synced {time} · [you@example.com](\
         https://calendar.google.com/calendar/r/day/2026/10/30\
         ?authuser=you%40example.com)*"
    )
}

#[test]
fn stamp_refreshes_every_run_without_row_churn() {
    let env = GcalEnv::new("bob-cli-gcal-sync-stamp-idle");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let first = sync_at(&env, &fake, &[], "2026-10-30 12:00:00");
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let note = today_note(&env);
    assert!(note.contains(&today_stamp("12:00")), "{note}");
    assert!(
        stdout(&first).contains("↻ synced 12:00 · you@example.com"),
        "{}",
        stdout(&first)
    );
    let journal_count = journal_lines(&env).len();

    let same_minute =
        sync_at(&env, &fake, &["-f", "json"], "2026-10-30 12:00:40");
    assert_eq!(
        same_minute.status.code(),
        Some(0),
        "{}",
        stderr(&same_minute)
    );
    assert_eq!(
        today_note(&env),
        note,
        "same-minute rerun is byte-identical"
    );
    assert_eq!(journal_lines(&env).len(), journal_count);
    let report: Value =
        serde_json::from_str(&stdout(&same_minute)).expect("JSON report");
    assert_eq!(report["status"], "unchanged");
    assert_eq!(report["stamp"]["account"], "you@example.com");
    assert_eq!(report["stamp"]["synced_at"], "2026-10-30T12:00");
    assert_eq!(report["stamp"]["refreshed"], false);

    let dry = sync_at(&env, &fake, &["-d"], "2026-10-30 12:20:00");
    assert_eq!(dry.status.code(), Some(0), "{}", stderr(&dry));
    let dry_json =
        sync_at(&env, &fake, &["-d", "-f", "json"], "2026-10-30 12:20:00");
    assert_eq!(dry_json.status.code(), Some(0), "{}", stderr(&dry_json));
    let dry_report: Value =
        serde_json::from_str(&stdout(&dry_json)).expect("JSON dry-run");
    assert_eq!(dry_report["stamp"]["synced_at"], "2026-10-30T12:20");
    assert_eq!(dry_report["stamp"]["refreshed"], true);
    let diff = stdout(&dry);
    assert!(
        diff.contains(&format!("-{}", today_stamp("12:00"))),
        "diff={diff}\njson={dry_report}"
    );
    assert!(
        diff.contains(&format!("+{}", today_stamp("12:20"))),
        "diff={diff}\njson={dry_report}"
    );
    assert_eq!(today_note(&env), note, "dry run writes nothing");

    let later = sync_at(&env, &fake, &["-f", "json"], "2026-10-30 12:20:00");
    assert_eq!(later.status.code(), Some(0), "{}", stderr(&later));
    let refreshed = today_note(&env);
    let before: Vec<&str> = note.lines().collect();
    let after: Vec<&str> = refreshed.lines().collect();
    assert_eq!(before.len(), after.len());
    let diffs: Vec<(&str, &str)> = before
        .iter()
        .zip(after.iter())
        .filter(|(old, new)| old != new)
        .map(|(old, new)| (*old, *new))
        .collect();
    assert_eq!(diffs.len(), 1, "{diffs:?}");
    assert_eq!(diffs[0].0, today_stamp("12:00"));
    assert_eq!(diffs[0].1, today_stamp("12:20"));
    assert_eq!(journal_lines(&env).len(), journal_count);
    let later_report: Value =
        serde_json::from_str(&stdout(&later)).expect("JSON report");
    assert_eq!(later_report["status"], "written");
    assert_eq!(later_report["rows"].as_array().map(Vec::len), Some(0));
    assert_eq!(later_report["stamp"]["synced_at"], "2026-10-30T12:20");
    assert_eq!(later_report["stamp"]["refreshed"], true);

    let human = sync_at(&env, &fake, &[], "2026-10-30 13:00:00");
    assert_eq!(human.status.code(), Some(0), "{}", stderr(&human));
    assert!(
        stdout(&human).contains("✓ Events unchanged · stamp refreshed"),
        "{}",
        stdout(&human)
    );
    assert!(
        stdout(&human).contains("↻ synced 13:00 · you@example.com"),
        "{}",
        stdout(&human)
    );
}

#[test]
fn dry_run_diff_shows_the_stamp_line() {
    let env = GcalEnv::new("bob-cli-gcal-sync-stamp-dry");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-d"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let diff = stdout(&output);
    assert!(
        diff.contains(&format!("+{}", today_stamp("12:00"))),
        "{diff}"
    );
}

#[test]
fn tomorrow_stamp_uses_the_dated_form() {
    let env = GcalEnv::new("bob-cli-gcal-sync-stamp-tomorrow");
    prepare(&env);
    fs::write(
        env.vault().join("2026/20261031.md"),
        "# Saturday 2026-10-31\n\n## Events\n\n## Pomodoros\n",
    )
    .expect("tomorrow note");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_at(
        &env,
        &fake,
        &["-D", "tomorrow", "-f", "json"],
        "2026-10-30 21:40:00",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = fs::read_to_string(env.vault().join("2026/20261031.md"))
        .expect("tomorrow");
    assert!(
        note.contains(
            "*↻ synced Fri 2026-10-30 21:40 · [you@example.com](\
             https://calendar.google.com/calendar/r/day/2026/10/31\
             ?authuser=you%40example.com)*"
        ),
        "{note}"
    );
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    assert_eq!(report["stamp"]["synced_at"], "2026-10-30T21:40");
    assert_eq!(report["stamp"]["refreshed"], true);
    assert_eq!(report["day"], "2026-10-31");
}

#[test]
fn account_guard_refuses_a_foreign_stamp() {
    let env = GcalEnv::new("bob-cli-gcal-sync-stamp-guard");
    prepare(&env);
    let foreign = "*↻ synced 12:00 · [other@example.com](\
         https://calendar.google.com/calendar/r/day/2026/10/30\
         ?authuser=other%40example.com)*";
    fs::write(
        env.vault().join("2026/20261030.md"),
        format!(
            "# Friday 2026-10-30\n\n## Events\n\n{foreign}\n\n## Pomodoros\n"
        ),
    )
    .expect("seed foreign stamp");
    let before = today_note(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON error");
    assert_eq!(report["ok"], false);
    assert_eq!(report["error"]["kind"], "account_changed");
    assert!(
        report["error"]["message"].as_str().is_some_and(|message| {
            message
                .contains("2026/20261030.md was synced with other@example.com")
                && message.contains("this run uses you@example.com")
        }),
        "{report}"
    );
    assert_eq!(today_note(&env), before, "guard writes nothing");
    assert!(
        !env.vault().join(".bob/gcal/test-host.jsonl").exists(),
        "guard leaves the journal untouched"
    );

    let dry = sync_output(&env, &fake, &["-d", "-f", "json"]);
    assert_eq!(dry.status.code(), Some(2), "{}", stderr(&dry));
}

#[test]
fn deleted_stamp_is_restored_without_row_churn() {
    let env = GcalEnv::new("bob-cli-gcal-sync-stamp-restore");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let original = today_note(&env);
    let journal_count = journal_lines(&env).len();
    let stripped = original.replace(&format!("{}\n", today_stamp("12:00")), "");
    assert_ne!(stripped, original);
    fs::write(env.vault().join("2026/20261030.md"), stripped)
        .expect("delete stamp");

    let restored = sync_output(&env, &fake, &[]);
    assert_eq!(restored.status.code(), Some(0), "{}", stderr(&restored));
    assert_eq!(today_note(&env), original);
    assert_eq!(journal_lines(&env).len(), journal_count);
    assert!(
        stdout(&restored).contains("✓ Events unchanged · stamp refreshed"),
        "{}",
        stdout(&restored)
    );
}

#[test]
fn missing_paragraph_gap_is_repaired_without_journal_churn() {
    let env = GcalEnv::new("bob-cli-gcal-sync-stamp-paragraph-gap");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let baseline = today_note(&env);
    let journal_count = journal_lines(&env).len();
    let old_stamp = today_stamp("12:00");
    assert!(baseline.contains(&format!("{old_stamp}\n")), "{baseline}");
    let without_gap = baseline.replacen(
        &format!("{old_stamp}\n"),
        &format!("{old_stamp}\nPersonal notes for the day.\n"),
        1,
    );
    assert_ne!(without_gap, baseline);
    fs::write(env.vault().join("2026/20261030.md"), &without_gap)
        .expect("remove paragraph gap");

    let dry = sync_at(&env, &fake, &["-d"], "2026-10-30 12:20:00");
    assert_eq!(dry.status.code(), Some(0), "{}", stderr(&dry));
    let diff = stdout(&dry);
    assert!(diff.contains(&today_stamp("12:20")), "{diff}");
    assert_eq!(today_note(&env), without_gap, "dry run writes nothing");
    assert_eq!(journal_lines(&env).len(), journal_count);

    let repaired = sync_at(&env, &fake, &[], "2026-10-30 12:20:00");
    assert_eq!(repaired.status.code(), Some(0), "{}", stderr(&repaired));
    let fresh_stamp = today_stamp("12:20");
    let note = today_note(&env);
    assert!(
        note.contains(&format!(
            "{fresh_stamp}\n\nPersonal notes for the day.\n"
        )),
        "{note}"
    );
    assert!(
        stdout(&repaired).contains("✓ Events unchanged · stamp refreshed"),
        "{}",
        stdout(&repaired)
    );
    assert_eq!(journal_lines(&env).len(), journal_count);

    let idle = sync_at(&env, &fake, &["-f", "json"], "2026-10-30 13:00:00");
    assert_eq!(idle.status.code(), Some(0), "{}", stderr(&idle));
    let idle_report: Value =
        serde_json::from_str(&stdout(&idle)).expect("JSON report");
    let after = today_note(&env);
    assert_eq!(idle_report["stamp"]["synced_at"], "2026-10-30T13:00");
    assert_eq!(idle_report["stamp"]["refreshed"], true);
    assert!(
        after.contains(&format!(
            "{}\n\nPersonal notes for the day.\n",
            today_stamp("13:00")
        )),
        "json={idle_report}\n{after}"
    );
    assert_eq!(journal_lines(&env).len(), journal_count);
    assert_eq!(idle_report["status"], "written");

    let same = sync_at(&env, &fake, &["-f", "json"], "2026-10-30 13:00:20");
    assert_eq!(same.status.code(), Some(0), "{}", stderr(&same));
    assert_eq!(
        today_note(&env),
        after,
        "same-minute rerun is byte-identical"
    );
    assert_eq!(journal_lines(&env).len(), journal_count);
    let same_report: Value =
        serde_json::from_str(&stdout(&same)).expect("JSON report");
    assert_eq!(same_report["status"], "unchanged");
    assert_eq!(same_report["stamp"]["synced_at"], "2026-10-30T13:00");
    assert_eq!(same_report["stamp"]["refreshed"], false);
}

#[test]
fn waiting_json_reports_a_null_stamp() {
    let env = GcalEnv::new("bob-cli-gcal-sync-stamp-waiting");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-D", "2026-11-01", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    assert_eq!(report["status"], "waiting");
    assert!(report["stamp"].is_null(), "{report}");
}

#[test]
fn json_lists_meet_entries_and_keeps_open_entry_keys() {
    let env = GcalEnv::new("bob-cli-gcal-sync-json-entries");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let diff = sync_output(&env, &fake, &["-d"]);
    assert_eq!(diff.status.code(), Some(0), "{}", stderr(&diff));
    let diff_text = stdout(&diff);
    assert!(
        diff_text.contains("+- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{diff_text}"
    );
    assert!(diff_text.contains("+- [ ] () — MEET"), "{diff_text}");

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    let entries = report["pomodoro"]["entries"].as_array().expect("entries");
    assert!(
        entries.iter().any(|entry| {
            entry["state"] == "closed"
                && entry["time"] == "0800-0830"
                && entry["created"] == true
                && entry["links"] == 1
        }),
        "{report}"
    );
    assert!(
        entries.iter().any(|entry| {
            entry["state"] == "planned"
                && entry["created"] == true
                && entry["links"] == 4
        }),
        "{report}"
    );
    assert_eq!(report["pomodoro"]["created"], true);
    assert_eq!(report["pomodoro"]["links_added"], 5);
    assert!(
        report["pomodoro"]["entry_line"].as_u64().is_some(),
        "{report}"
    );
    let note = today_note(&env);
    assert!(
        note.contains("- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{note}"
    );
}

#[test]
fn list_reports_task_meet_for_a_past_row() {
    let env = GcalEnv::new("bob-cli-gcal-sync-list-past");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));

    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let output = command.output().expect("list");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    let interview = report["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|row| row["title"] == "Product interview")
        .expect("interview");
    assert_eq!(interview["vault"], "task · MEET", "{interview}");
}

#[test]
fn current_meeting_reconcile_then_clock_close() {
    let env = GcalEnv::new("bob-cli-gcal-sync-current");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));
    let synced = sync_at(&env, &fake, &[], "2026-10-30 18:45:00");
    assert_eq!(synced.status.code(), Some(0), "{}", stderr(&synced));
    let note = today_note(&env);
    assert_eq!(
        note.lines()
            .filter(
                |line| line.starts_with("- [ ] (**") && line.contains("— MEET")
            )
            .count(),
        1,
        "exactly one running MEET:\n{note}"
    );
    assert!(
        !note.contains("- [ ] () — MEET"),
        "one_open must not add a second MEET:\n{note}"
    );

    let reconcile =
        run_bob(&env, &["task", "reconcile"], "2026-10-30 18:45:00");
    assert_eq!(
        reconcile.status.code(),
        Some(0),
        "double running entry would fail reconcile: {}",
        stderr(&reconcile)
    );

    let close = run_bob(&env, &["capture", "x"], "2026-10-30 19:05:00");
    assert_eq!(close.status.code(), Some(0), "{}", stderr(&close));
    let after = today_note(&env);
    let dentist = after
        .lines()
        .find(|line| line.contains("^dentist"))
        .expect("dentist");
    assert!(dentist.starts_with("- [x]"), "{dentist}");
}

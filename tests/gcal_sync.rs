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
fn title_free_row_is_adopted_without_duplicate() {
    let env = GcalEnv::new("bob-cli-gcal-sync-adopt-title-free");
    prepare(&env);
    // Title-free hand row (new Bob format) with the same block id sync would
    // mint, plus its MEET link: adoption keys on the bare URL.
    let hand = "# Friday 2026-10-30\n\n## Events\n\n- [*] #task **1830-1900** Dentist [📅](https://calendar.google.com/event?eid=dentist) [📍](https://www.google.com/maps/search/?api=1&query=Main%20St%20Dental) [root:: body] [type:: MEET] ^dentist\n\n## Pomodoros\n\n- [ ] () — MEET\n\t- [[#^dentist]]\n";
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
    assert!(!note.contains("\"Open in Google Calendar\""), "{note}");

    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let list = command.output().expect("list");
    assert_eq!(list.status.code(), Some(0), "{}", stderr(&list));
    let report: Value =
        serde_json::from_str(&stdout(&list)).expect("JSON report");
    let dentist = report["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|row| row["title"] == "Dentist")
        .expect("dentist");
    assert_eq!(dentist["vault"], "task · MEET", "{dentist}");
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
    // The fixture dentist description now yields one bullet, counted.
    assert_eq!(dentist["description_lines"], 1);
    assert!(
        today_note(&env).contains("\t- Bring insurance card"),
        "{}",
        today_note(&env)
    );
    assert_eq!(report["stamp"]["account"], "you@example.com");
    assert_eq!(report["stamp"]["synced_at"], "2026-10-30T12:00");
    assert_eq!(report["stamp"]["refreshed"], true);
    assert!(
        !stdout(&output).contains("Main St Dental\"],")
            && !stdout(&output).contains("attendees")
            && !stdout(&output).contains("Bring insurance card"),
        "no descriptions or attendees leak"
    );
    let journal_text =
        fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
            .expect("journal file");
    assert!(
        !journal_text.contains("Bring insurance card"),
        "no description text in the journal"
    );
}

#[test]
fn sync_sorts_event_task_links_and_reports_sort_only_runs() {
    let env = GcalEnv::new("bob-cli-gcal-sync-link-order");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let event = |id: &str, summary: &str, start: &str, end: &str| {
        serde_json::json!({
            "id": id,
            "summary": summary,
            "status": "confirmed",
            "start": {"dateTime": start},
            "end": {"dateTime": end},
            "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
            "attendees": [{"self": true, "responseStatus": "accepted"}]
        })
    };
    let later_only = serde_json::json!({
        "events": [event(
            "lincoln-park-w",
            "Lincoln Park W",
            "2026-10-30T19:00:00-04:00",
            "2026-10-30T19:30:00-04:00"
        )],
        "nextPageToken": null,
    });
    fake.events_json(&later_only.to_string());

    let first = sync_at(&env, &fake, &["-f", "json"], "2026-10-30 06:44:00");
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    assert!(today_note(&env).contains("[[#^lincoln-park-w]]"));

    let both = serde_json::json!({
        "events": [
            event(
                "lincoln-park-w",
                "Lincoln Park W",
                "2026-10-30T19:00:00-04:00",
                "2026-10-30T19:30:00-04:00"
            ),
            event(
                "swings",
                "Swings",
                "2026-10-30T08:00:00-04:00",
                "2026-10-30T08:30:00-04:00"
            ),
        ],
        "nextPageToken": null,
    });
    fake.events_json(&both.to_string());

    let added = sync_at(&env, &fake, &["-f", "json"], "2026-10-30 06:44:00");
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    let report: Value =
        serde_json::from_str(&stdout(&added)).expect("JSON report");
    assert_eq!(report["status"], "written");
    assert_eq!(report["pomodoro"]["links_added"], 1);
    assert_eq!(report["pomodoro"]["links_sorted"], 2);
    let note = today_note(&env);
    assert!(
        note.find("[[#^swings]]") < note.find("[[#^lincoln-park-w]]"),
        "{note}"
    );

    let sorted_pair = "\t- [[#^swings]]\n\t- [[#^lincoln-park-w]]";
    let reversed_pair = "\t- [[#^lincoln-park-w]]\n\t- [[#^swings]]";
    assert!(note.contains(sorted_pair), "{note}");
    fs::write(
        env.vault().join("2026/20261030.md"),
        note.replacen(sorted_pair, reversed_pair, 1),
    )
    .expect("hand-reorder Event Task links");

    let repaired = sync_at(&env, &fake, &[], "2026-10-30 06:44:00");
    assert_eq!(repaired.status.code(), Some(0), "{}", stderr(&repaired));
    assert_eq!(
        stdout(&repaired)
            .matches("✓ Events unchanged · Pomodoros ↕2 links sorted")
            .count(),
        1,
        "{}",
        stdout(&repaired)
    );
    assert!(today_note(&env).contains(sorted_pair));

    let unchanged =
        sync_at(&env, &fake, &["-f", "json"], "2026-10-30 06:44:00");
    assert_eq!(unchanged.status.code(), Some(0), "{}", stderr(&unchanged));
    let report: Value =
        serde_json::from_str(&stdout(&unchanged)).expect("JSON report");
    assert_eq!(report["status"], "unchanged");
    assert_eq!(report["pomodoro"]["links_sorted"], 0);
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
    let items: String = agenda["pomodoros"]
        .as_array()
        .expect("pomodoros")
        .iter()
        .flat_map(|entry| entry["items"].as_array().into_iter().flatten())
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    // Linked rows resolve with clean titles; the ended interview sits
    // under a closed MEET, so it stays out of the open agenda. Event
    // rows still list it; the open-agenda items must not.
    assert!(items.contains("Dentist"), "{items}");
    assert!(items.contains("Late call"), "{items}");
    assert!(!items.contains("Product interview"), "{items}");
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

#[test]
fn off_clock_aliases_split_clusters_and_report() {
    let env = GcalEnv::new("bob-cli-gcal-sync-untracked");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let events = serde_json::json!({
        "events": [
            {
                "id": "standup",
                "summary": "Standup",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T08:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T08:30:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=standup",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "lunch",
                "summary": "Lunch #untracked",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T08:15:00-04:00"},
                "end": {"dateTime": "2026-10-30T09:00:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=lunch",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "gym",
                "summary": "Gym",
                "description": "Bring shoes\n#untracked",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T09:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T09:30:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=gym",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "team-lunch",
                "summary": "Team Lunch @job #untracked",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T11:00:00-04:00"},
                "end": {"dateTime": "2026-10-30T11:30:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=team-lunch",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            },
            {
                "id": "focusnow",
                "summary": "Focus #UNTRACKED",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-30T11:30:00-04:00"},
                "end": {"dateTime": "2026-10-30T12:30:00-04:00"},
                "htmlLink": "https://calendar.google.com/event?eid=focusnow",
                "attendees": [{"self": true, "responseStatus": "accepted"}]
            }
        ],
        "nextPageToken": serde_json::Value::Null,
    });
    fake.events_json(&serde_json::to_string(&events).expect("events"));

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);

    // Title token is stripped; the row carries the tag at the end of the
    // badge run, before [root::].
    let lunch = note
        .lines()
        .find(|line| line.contains("^lunch"))
        .expect("lunch row");
    assert!(lunch.contains("**0815-0900** Lunch "), "{lunch}");
    assert!(!lunch.contains("Lunch #untracked"), "{lunch}");
    assert!(lunch.contains(" #off [root::"), "{lunch}");
    // Description-only token also marks the row.
    let gym = note
        .lines()
        .find(|line| line.contains("^gym"))
        .expect("gym row");
    assert!(gym.contains("**0900-0930** Gym "), "{gym}");
    assert!(gym.contains(" #off"), "{gym}");
    // The human line becomes a bullet; the directive line writes none.
    assert!(note.contains("\t- Bring shoes"), "{note}");
    // @route is read after the tag is stripped.
    let team = note
        .lines()
        .find(|line| line.contains("^team-lunch"))
        .expect("team lunch row");
    assert!(team.contains("**1100-1130** Team Lunch "), "{team}");
    assert!(!team.contains("@job"), "{team}");
    assert!(team.contains("[root:: job]"), "{team}");
    assert!(team.contains(" #off [root:: job]"), "{team}");
    // Tracked rows carry no tag.
    let standup = note
        .lines()
        .find(|line| line.contains("^standup"))
        .expect("standup row");
    assert!(!standup.contains("#untracked"), "{standup}");

    // Mixed past cluster splits by clock verdict: one worked MEET and one
    // off-clock MEET that overlap.
    let pomodoros = &note[note.find("## Pomodoros").expect("Pomodoros")..];
    assert!(
        pomodoros.contains("- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{pomodoros}"
    );
    assert!(
        pomodoros.contains("- [x] (**0815-0930** [off:: 75m]) — MEET"),
        "lunch+gym share one off-clock cluster:\n{pomodoros}"
    );
    assert!(
        !pomodoros.contains("(**0815-0930** [t::"),
        "off-clock cluster uses [off::]:\n{pomodoros}"
    );
    // An all-off-clock current run creates a running MEET with [off::].
    assert!(
        pomodoros.contains("- [ ] (**1130-1230** [off:: 60m]) — MEET"),
        "running off-clock MEET:\n{pomodoros}"
    );

    // Sync JSON rows carry the new and deprecated fields.
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    let rows = report["rows"].as_array().expect("rows");
    let lunch_json = rows
        .iter()
        .find(|row| row["title"] == "Lunch")
        .expect("lunch json");
    assert_eq!(lunch_json["off_clock"], true);
    assert_eq!(lunch_json["untracked"], true);
    let standup_json = rows
        .iter()
        .find(|row| row["title"] == "Standup")
        .expect("standup json");
    assert_eq!(standup_json["off_clock"], false);
    assert_eq!(standup_json["untracked"], false);

    // List JSON and VAULT report the flag.
    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let list_out = command.output().expect("run list");
    assert_eq!(list_out.status.code(), Some(0), "{}", stderr(&list_out));
    let list: Value =
        serde_json::from_str(&stdout(&list_out)).expect("list JSON");
    let events = list["events"].as_array().expect("events");
    let lunch_list = events
        .iter()
        .find(|row| row["title"] == "Lunch")
        .expect("lunch list");
    assert_eq!(lunch_list["off_clock"], true);
    assert_eq!(lunch_list["untracked"], true);
    assert!(
        lunch_list["vault"]
            .as_str()
            .unwrap_or_default()
            .contains("off-clock"),
        "VAULT shows the flag: {lunch_list}"
    );
    let standup_list = events
        .iter()
        .find(|row| row["title"] == "Standup")
        .expect("standup list");
    assert_eq!(standup_list["off_clock"], false);
    assert_eq!(standup_list["untracked"], false);
}

fn buffer_event(
    id: &str,
    summary: &str,
    description: Option<&str>,
    start: &str,
    end: &str,
) -> Value {
    let mut event = serde_json::json!({
        "id": id,
        "summary": summary,
        "status": "confirmed",
        "start": {"dateTime": start},
        "end": {"dateTime": end},
        "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    });
    if let Some(description) = description {
        event["description"] = Value::String(description.to_string());
    }
    event
}

fn buffer_envelope(events: Vec<Value>) -> String {
    serde_json::to_string(&serde_json::json!({
        "events": events,
        "nextPageToken": serde_json::Value::Null,
    }))
    .expect("envelope")
}

fn event_row(note: &str, span: &str) -> String {
    note.lines()
        .find(|line| line.contains(span))
        .unwrap_or_else(|| panic!("no {span} row:\n{note}"))
        .to_string()
}

fn journal_spans(journal: &[Value], event_id: &str) -> Value {
    journal
        .iter()
        .filter(|line| {
            line["kind"] == "gcal.row" && line["key"]["event_id"] == event_id
        })
        .last()
        .unwrap_or_else(|| panic!("no journal row for {event_id}"))
        .get("row")
        .and_then(|row| row.get("spans"))
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn buffered_rows_render_update_and_report() {
    let env = GcalEnv::new("bob-cli-gcal-sync-buffers");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    // HTML description form: `<br>` splits lines, and the directive line
    // routes, untracks, and buffers at once.
    fake.events_json(&buffer_envelope(vec![
        buffer_event(
            "lunch",
            "Lunch with Kelly",
            Some("Fuel<br>@job #untracked -30 +15"),
            "2026-10-30T12:00:00-04:00",
            "2026-10-30T13:00:00-04:00",
        ),
        buffer_event(
            "standup",
            "Standup",
            Some("+15"),
            "2026-10-30T14:00:00-04:00",
            "2026-10-30T14:30:00-04:00",
        ),
        buffer_event(
            "dentist",
            "Dentist @body",
            None,
            "2026-10-30T18:30:00-04:00",
            "2026-10-30T19:00:00-04:00",
        ),
    ]));

    // The first sync writes; parse its JSON report for the row payloads.
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    let note = today_note(&env);

    // New rows render the field at the end of the badge run.
    let lunch = event_row(&note, "**1200-1300**");
    assert!(lunch.starts_with("- [*] #task"), "{lunch}");
    assert!(lunch.contains("Lunch with Kelly "), "{lunch}");
    assert!(
        lunch.contains(" #off [buffer:: -30m +15m] [root:: job]"),
        "{lunch}"
    );
    assert!(lunch.ends_with("^lunch-kelly"), "{lunch}");
    let standup = event_row(&note, "**1400-1430**");
    assert!(standup.contains(" [buffer:: +15m]"), "{standup}");
    assert!(!standup.contains("#untracked"), "{standup}");
    let dentist = event_row(&note, "**1830-1900**");
    assert!(!dentist.contains("[buffer::"), "{dentist}");
    assert!(!dentist.contains("#untracked"), "{dentist}");

    // Lunch is current on its buffered span (1130 ≤ now < 1315), so the
    // running MEET it opens carries the buffered span as off-clock.
    let pomodoros = &note[note.find("## Pomodoros").expect("Pomodoros")..];
    assert!(
        pomodoros.contains("- [ ] (**1130-1315** [off:: 105m]) — MEET"),
        "running lead-in MEET:\n{pomodoros}"
    );
    assert!(
        !pomodoros.contains("(**1130-1315** [t::"),
        "off-clock run omits [t::]:\n{pomodoros}"
    );

    // Sync JSON rows carry the buffer object; unbuffered rows carry null.
    let rows = report["rows"].as_array().expect("rows");
    let lunch_json = rows
        .iter()
        .find(|row| row["title"] == "Lunch with Kelly")
        .expect("lunch json");
    assert_eq!(
        lunch_json["buffer"],
        serde_json::json!({"before_minutes": 30, "after_minutes": 15}),
    );
    let dentist_json = rows
        .iter()
        .find(|row| row["title"] == "Dentist")
        .expect("dentist json");
    assert_eq!(dentist_json["buffer"], Value::Null);

    // List JSON and the table show the buffer.
    let mut command = env.command();
    fake.install(&mut command)
        .args(["gcal", "list", "-f", "json"]);
    let list_out = command.output().expect("run list");
    assert_eq!(list_out.status.code(), Some(0), "{}", stderr(&list_out));
    let list: Value =
        serde_json::from_str(&stdout(&list_out)).expect("list JSON");
    let lunch_list = list["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|row| row["title"] == "Lunch with Kelly")
        .expect("lunch list");
    assert_eq!(
        lunch_list["buffer"],
        serde_json::json!({"before_minutes": 30, "after_minutes": 15}),
    );
    let mut command = env.command();
    fake.install(&mut command).args(["gcal", "list"]);
    let table_out = command.output().expect("run list table");
    assert!(
        stdout(&table_out).contains("12:00-13:00 −30m +15m"),
        "{}",
        stdout(&table_out)
    );

    let dentist_spans_before = journal_spans(&journal_lines(&env), "dentist");
    assert!(
        stdout(&sync_output(&env, &fake, &[])).contains("unchanged"),
        "buffered sync settles"
    );

    // A buffer added in Google on an untouched row updates in place.
    fake.events_json(&buffer_envelope(vec![
        buffer_event(
            "lunch",
            "Lunch with Kelly",
            Some("Fuel<br>@job #untracked -45 +15"),
            "2026-10-30T12:00:00-04:00",
            "2026-10-30T13:00:00-04:00",
        ),
        buffer_event(
            "standup",
            "Standup",
            Some("+15"),
            "2026-10-30T14:00:00-04:00",
            "2026-10-30T14:30:00-04:00",
        ),
        buffer_event(
            "dentist",
            "Dentist @body",
            None,
            "2026-10-30T18:30:00-04:00",
            "2026-10-30T19:00:00-04:00",
        ),
    ]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("~ task"), "{}", stdout(&output));
    let lunch = event_row(&today_note(&env), "**1200-1300**");
    assert!(
        lunch.contains(" [buffer:: -45m +15m] [root:: job]"),
        "{lunch}"
    );
    assert!(
        stdout(&sync_output(&env, &fake, &[])).contains("unchanged"),
        "buffer change settles"
    );
    // The unbuffered row's journal spans never changed.
    assert_eq!(
        journal_spans(&journal_lines(&env), "dentist"),
        dentist_spans_before
    );

    // A hand-removed field wins over a later Google change, reported once.
    let edited = today_note(&env).replacen(" [buffer:: -45m +15m]", "", 1);
    assert_ne!(edited, today_note(&env), "field present to remove");
    fs::write(env.vault().join("2026/20261030.md"), edited)
        .expect("remove buffer field");
    fake.events_json(&buffer_envelope(vec![
        buffer_event(
            "lunch",
            "Lunch with Kelly",
            Some("Fuel<br>@job #untracked -45 +30"),
            "2026-10-30T12:00:00-04:00",
            "2026-10-30T13:00:00-04:00",
        ),
        buffer_event(
            "standup",
            "Standup",
            Some("+15"),
            "2026-10-30T14:00:00-04:00",
            "2026-10-30T14:30:00-04:00",
        ),
        buffer_event(
            "dentist",
            "Dentist @body",
            None,
            "2026-10-30T18:30:00-04:00",
            "2026-10-30T19:00:00-04:00",
        ),
    ]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(combined.contains("kept your edit"), "kept once: {combined}");
    let lunch = event_row(&today_note(&env), "**1200-1300**");
    assert!(!lunch.contains("[buffer::"), "hand edit kept:\n{lunch}");
    let again = sync_output(&env, &fake, &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let combined = format!("{}{}", stdout(&again), stderr(&again));
    assert!(
        !combined.contains("kept your edit"),
        "reported once: {combined}"
    );
    assert!(
        !event_row(&today_note(&env), "**1200-1300**").contains("[buffer::")
    );
}

#[test]
fn buffered_spans_drive_meet_phases_and_clusters() {
    let env = GcalEnv::new("bob-cli-gcal-sync-buffer-phases");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    // NOW is 12:00. Standup's +15 bridges it to Planning in one past
    // cluster; the mixed-trackedness morning pair splits; the current
    // pair shares one buffered running MEET.
    fake.events_json(&buffer_envelope(vec![
        buffer_event(
            "gym",
            "Gym",
            Some("+15"),
            "2026-10-30T08:00:00-04:00",
            "2026-10-30T08:30:00-04:00",
        ),
        buffer_event(
            "yoga",
            "Yoga",
            Some("#untracked"),
            "2026-10-30T08:30:00-04:00",
            "2026-10-30T09:00:00-04:00",
        ),
        buffer_event(
            "standup",
            "Standup",
            Some("+15"),
            "2026-10-30T10:00:00-04:00",
            "2026-10-30T10:30:00-04:00",
        ),
        buffer_event(
            "planning",
            "Planning",
            None,
            "2026-10-30T10:45:00-04:00",
            "2026-10-30T11:15:00-04:00",
        ),
        buffer_event(
            "review",
            "Review",
            None,
            "2026-10-30T11:00:00-04:00",
            "2026-10-30T12:30:00-04:00",
        ),
        buffer_event(
            "lunch",
            "Lunch",
            Some("#untracked -30 +15"),
            "2026-10-30T12:00:00-04:00",
            "2026-10-30T13:00:00-04:00",
        ),
    ]));

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);

    // Past rows work from birth; the rows keep their meeting spans.
    let gym = event_row(&note, "**0800-0830**");
    assert!(gym.starts_with("- [/] #task"), "{gym}");
    assert!(gym.contains(" [buffer:: +15m]"), "{gym}");
    let lunch = event_row(&note, "**1200-1300**");
    assert!(lunch.starts_with("- [*] #task"), "{lunch}");
    assert!(
        lunch.contains(" #off [buffer:: -30m +15m] [root:: meet]"),
        "{lunch}"
    );

    let pomodoros = &note[note.find("## Pomodoros").expect("Pomodoros")..];
    // The +15 bridges standup to planning in one closed 75-minute MEET.
    assert!(
        pomodoros.contains("- [x] (**1000-1115** [t:: 75m]) — MEET"),
        "bridged past cluster:\n{pomodoros}"
    );
    assert!(pomodoros.contains("\t- 🍅 [[#^standup]]"), "{pomodoros}");
    assert!(pomodoros.contains("\t- 🍅 [[#^planning]]"), "{pomodoros}");
    // Mixed trackedness splits the morning pair despite the overlap.
    assert!(
        pomodoros.contains("- [x] (**0800-0845** [t:: 45m]) — MEET"),
        "tracked lead-out cluster:\n{pomodoros}"
    );
    assert!(
        pomodoros.contains("- [x] (**0830-0900** [off:: 30m]) — MEET"),
        "off-clock cluster records its elapsed span:\n{pomodoros}"
    );
    // The current pair shares one buffered running MEET, tracked.
    assert!(
        pomodoros.contains("- [ ] (**1100-1315** [t:: 135m]) — MEET"),
        "buffered running MEET:\n{pomodoros}"
    );
    assert!(pomodoros.contains("\t- [[#^review]]"), "{pomodoros}");
    assert!(pomodoros.contains("\t- [[#^lunch]]"), "{pomodoros}");

    let out = stdout(&output);
    assert!(out.contains("closed 10:00–11:15 — MEET"), "{out}");
    assert!(out.contains("running 11:00–13:15 — MEET"), "{out}");

    // The buffered run settles: a rerun writes nothing.
    let again = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let report: Value =
        serde_json::from_str(&stdout(&again)).expect("JSON report");
    assert_eq!(report["status"], "unchanged");
}

fn desc_event(
    id: &str,
    summary: &str,
    description: &str,
    start: &str,
    end: &str,
) -> Value {
    serde_json::json!({
        "id": id,
        "summary": summary,
        "description": description,
        "status": "confirmed",
        "start": {"dateTime": start},
        "end": {"dateTime": end},
        "htmlLink": format!("https://calendar.google.com/event?eid={id}"),
        "attendees": [{"self": true, "responseStatus": "accepted"}]
    })
}

fn desc_envelope(events: Vec<Value>) -> String {
    serde_json::to_string(&serde_json::json!({
        "events": events,
        "nextPageToken": serde_json::Value::Null,
    }))
    .expect("envelope")
}

fn write_love_area(env: &GcalEnv) {
    fs::write(
        env.vault().join("love.md"),
        "---\ntype: area\n---\n# Love\n",
    )
    .expect("write love area");
}

#[test]
fn descriptions_become_bullets_in_every_phase() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-phases");
    prepare(&env);
    write_love_area(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![
        desc_event(
            "swings",
            "Swings!",
            "And retainers!\n\n@love #offclock",
            "2026-10-30T08:00:00-04:00",
            "2026-10-30T08:15:00-04:00",
        ),
        desc_event(
            "lunch",
            "Lunch",
            "Bring laptop & ID",
            "2026-10-30T11:30:00-04:00",
            "2026-10-30T12:30:00-04:00",
        ),
        desc_event(
            "dentist",
            "Dentist @body",
            "Bring insurance card",
            "2026-10-30T18:30:00-04:00",
            "2026-10-30T19:00:00-04:00",
        ),
    ]));

    let output = sync_at(&env, &fake, &[], "2026-10-30 12:00:00");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);

    // The motivating case: the directive line still drives canonical `#off`
    // and `[root:: love]`, and the human line becomes the bullet.
    let swings = note
        .lines()
        .find(|line| line.contains("^swings"))
        .expect("swings row");
    assert!(swings.starts_with("- [/] #task"), "{swings}");
    assert!(swings.contains(" #off [root:: love]"), "{swings}");
    // The past row works from birth under a closed MEET.
    assert!(note.contains("\t- And retainers!"), "{note}");
    let lunch = note
        .lines()
        .find(|line| line.contains("^lunch"))
        .expect("lunch row");
    assert!(lunch.starts_with("- [*] #task"), "{lunch}");
    assert!(note.contains("\t- Bring laptop & ID"), "{note}");
    let dentist = note
        .lines()
        .find(|line| line.contains("^dentist"))
        .expect("dentist row");
    assert!(dentist.starts_with("- [*] #task"), "{dentist}");
    assert!(dentist.contains("[root:: body]"), "{dentist}");
    assert!(note.contains("\t- Bring insurance card"), "{note}");

    // Human rows end with a dim `≡ N` when the run wrote N bullet lines.
    let out = stdout(&output);
    assert_eq!(out.matches("≡ 1").count(), 3, "{out}");
    assert!(out.contains("Swings!  ● love  via @love ≡ 1"), "{out}");

    // The journal carries only the hash and line count, never the text.
    let journal_text =
        fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
            .expect("journal file");
    assert!(!journal_text.contains("retainers"), "no text leaks");
    assert!(!journal_text.contains("insurance"), "no text leaks");
    for line in journal_lines(&env) {
        if line["kind"] == "gcal.row" {
            assert_eq!(line["description"]["lines"], 1, "{line}");
        }
    }
}

#[test]
fn sync_json_rows_carry_description_lines() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-json");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![
        desc_event(
            "swings",
            "Swings!",
            "And retainers!",
            "2026-10-30T08:00:00-04:00",
            "2026-10-30T08:15:00-04:00",
        ),
        desc_event(
            "lunch",
            "Lunch",
            "",
            "2026-10-30T11:30:00-04:00",
            "2026-10-30T12:30:00-04:00",
        ),
    ]));
    let output = sync_at(&env, &fake, &["-f", "json"], "2026-10-30 12:00:00");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let json_text = stdout(&output);
    let report: Value = serde_json::from_str(&json_text).expect("JSON report");
    assert_eq!(report["schema_version"], 1);
    let rows = report["rows"].as_array().expect("rows");
    let swings = rows
        .iter()
        .find(|row| row["title"] == "Swings!")
        .expect("swings row");
    assert_eq!(swings["description_lines"], 1);
    let lunch = rows
        .iter()
        .find(|row| row["title"] == "Lunch")
        .expect("lunch row");
    assert_eq!(lunch["description_lines"], 0);
    assert!(
        !json_text.contains("retainers"),
        "no description text leaks"
    );
}

#[test]
fn second_run_with_bullets_is_a_no_op() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-noop");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![desc_event(
        "swings",
        "Swings!",
        "And retainers!",
        "2026-10-30T08:00:00-04:00",
        "2026-10-30T08:15:00-04:00",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    let before = today_note(&env);
    assert!(before.contains("\t- And retainers!"), "{before}");
    let journal_count = journal_lines(&env).len();

    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("unchanged"), "{}", stdout(&output));
    assert_eq!(today_note(&env), before);
    assert_eq!(journal_lines(&env).len(), journal_count);
}

#[test]
fn dry_run_diff_shows_the_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-dry");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![desc_event(
        "swings",
        "Swings!",
        "And retainers!",
        "2026-10-30T08:00:00-04:00",
        "2026-10-30T08:15:00-04:00",
    )]));
    let output = sync_output(&env, &fake, &["-d"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("\t- And retainers!"),
        "{}",
        stdout(&output)
    );
    // Nothing was written.
    assert!(!today_note(&env).contains("And retainers!"));
}

#[test]
fn description_off_writes_no_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-off");
    prepare(&env);
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n  description: off\n",
    );
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![desc_event(
        "swings",
        "Swings!",
        "And retainers!",
        "2026-10-30T08:00:00-04:00",
        "2026-10-30T08:15:00-04:00",
    )]));
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(!note.contains("And retainers!"), "{note}");
    assert!(!stdout(&output).contains("≡"), "{}", stdout(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    assert_eq!(report["rows"][0]["description_lines"], 0);
    // The kill switch records `Absent`: the key is omitted entirely.
    for line in journal_lines(&env) {
        if line["kind"] == "gcal.row" {
            assert!(line.get("description").is_none(), "{line}");
        }
    }
}

#[test]
fn space_indented_note_gets_space_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-spaces");
    prepare(&env);
    fs::write(
        env.vault().join("2026/20261030.md"),
        "# Friday 2026-10-30\n\n## Pomodoros\n\n- [ ] 1200-1300 — Focus\n  - my setup note\n",
    )
    .expect("seed space-indented note");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![desc_event(
        "swings",
        "Swings!",
        "And retainers!",
        "2026-10-30T08:00:00-04:00",
        "2026-10-30T08:15:00-04:00",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(note.contains("  - And retainers!"), "{note}");
    assert!(!note.contains("\t- And retainers!"), "{note}");
}

#[test]
fn new_row_lands_after_the_previous_rows_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-order");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![
        desc_event(
            "early",
            "Early standup",
            "First point\nSecond point",
            "2026-10-30T08:00:00-04:00",
            "2026-10-30T08:30:00-04:00",
        ),
        desc_event(
            "later",
            "Later review",
            "",
            "2026-10-30T09:00:00-04:00",
            "2026-10-30T09:30:00-04:00",
        ),
    ]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let early = note.find("^early-standup").expect("early row");
    let first_bullet = note.find("\t- First point").expect("first bullet");
    let second_bullet = note.find("\t- Second point").expect("second bullet");
    let later = note.find("^later-review").expect("later row");
    assert!(
        early < first_bullet
            && first_bullet < second_bullet
            && second_bullet < later,
        "{note}"
    );
}

#[test]
fn context_rows_never_get_bullets() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-context");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![serde_json::json!({
        "id": "klass",
        "summary": "Toddler class",
        "description": "Bring shoes",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T09:15:00-04:00"},
        "end": {"dateTime": "2026-10-30T10:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=klass",
        "attendees": [{"self": true, "responseStatus": "tentative"}]
    })]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    assert!(
        note.contains("- **0915-1000** Toddler class · *maybe*"),
        "{note}"
    );
    assert!(!note.contains("Bring shoes"), "{note}");
    assert!(!stdout(&output).contains("≡"), "{}", stdout(&output));
}

#[test]
fn upgrade_lands_bullets_above_existing_notes() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-upgrade");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let tentative = serde_json::json!({
        "id": "klass",
        "summary": "Evening class",
        "description": "Stretch\n-15",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T20:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T21:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=klass",
        "attendees": [{"self": true, "responseStatus": "tentative"}]
    });
    fake.events_json(&desc_envelope(vec![tentative]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    // Bryan adds a note under the Maybe row before it goes Yes.
    let first = today_note(&env);
    let mut noted: Vec<String> = first.lines().map(str::to_string).collect();
    let at = noted
        .iter()
        .position(|line| line.contains("Evening class"))
        .expect("context row");
    noted.insert(at + 1, "  - my note".to_string());
    fs::write(
        env.vault().join("2026/20261030.md"),
        noted.join("\n") + "\n",
    )
    .expect("hand note");
    let accepted = serde_json::json!({
        "id": "klass",
        "summary": "Evening class",
        "description": "Stretch\n-15",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T20:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T21:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=klass",
        "attendees": [{"self": true, "responseStatus": "accepted"}]
    });
    fake.events_json(&desc_envelope(vec![accepted]));
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = note
        .lines()
        .find(|line| line.contains("^evening-class"))
        .expect("upgraded row");
    assert!(row.contains(" [buffer:: -15m]"), "{row}");
    let row_at = note.find(row).expect("row");
    let bullet_at = note.find("  - Stretch").expect("bullet");
    let note_at = note.find("  - my note").expect("hand note");
    assert!(row_at < bullet_at && bullet_at < note_at, "{note}");
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    let upgraded = report["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["action"] == "upgraded")
        .expect("upgraded row");
    assert_eq!(upgraded["description_lines"], 1);
}

#[test]
fn long_description_caps_at_eight_with_an_overflow_line() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-cap");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let description = (1..=10)
        .map(|n| format!("Line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    fake.events_json(&desc_envelope(vec![desc_event(
        "talk",
        "Long talk",
        &description,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    for n in 1..=7 {
        assert!(note.contains(format!("\t- Line {n}").as_str()), "{note}");
    }
    assert!(!note.contains("\t- Line 8"), "{note}");
    assert!(
        note.contains(
            "\t- *…3 more lines in [Google Calendar](https://calendar.google.com/event?eid=talk)*"
        ),
        "{note}"
    );
    assert!(stdout(&output).contains("≡ 8"), "{}", stdout(&output));
    let journal = journal_lines(&env);
    let latest = journal
        .iter()
        .filter(|line| {
            line["kind"] == "gcal.row" && line["key"]["event_id"] == "talk"
        })
        .last()
        .expect("talk row record");
    assert_eq!(latest["description"]["lines"], 8, "{latest}");
}

#[test]
fn teams_only_description_writes_nothing() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-teams");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![serde_json::json!({
        "id": "review",
        "summary": "Sprint review",
        "description": "Microsoft Teams meeting\nJoin on your computer\nhttps://teams.microsoft.com/l/meetup-join/19%3Ameeting_FAKE123\nMeeting ID: 234 567 890\nPasscode: AbC123",
        "status": "confirmed",
        "start": {"dateTime": "2026-10-30T18:00:00-04:00"},
        "end": {"dateTime": "2026-10-30T19:00:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=review",
        "conferenceData": {"entryPoints": [{"entryPointType": "video", "uri": "https://teams.microsoft.com/l/meetup-join/19%3Ameeting_FAKE123"}]},
        "attendees": [{"self": true, "responseStatus": "accepted"}]
    })]));
    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
    let row = note
        .lines()
        .find(|line| line.contains("^sprint-review"))
        .expect("review row");
    assert!(row.contains("[📹]("), "{row}");
    // The row looks exactly as it does today: no bullets.
    let events = &note[note.find("## Events").expect("Events")
        ..note.find("## Pomodoros").expect("Pomodoros")];
    assert!(
        events.lines().all(|line| !line.starts_with([' ', '\t'])),
        "{note}"
    );
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("JSON report");
    assert_eq!(report["rows"][0]["description_lines"], 0);
    assert!(!stdout(&output).contains("≡"), "{}", stdout(&output));
}

#[test]
fn deleted_row_with_overflow_bullet_is_tombstoned_not_adopted() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-tombstone");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    let description = (1..=10)
        .map(|n| format!("Line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    fake.events_json(&desc_envelope(vec![desc_event(
        "talk",
        "Long talk",
        &description,
        "2026-10-30T18:00:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    // The overflow bullet carries the event URL. Deleting only the row
    // must tombstone it: the bullet is never mistaken for the row.
    let epilogue = today_note(&env)
        .lines()
        .filter(|line| {
            !line.contains("^long-talk") || line.contains("[[#^long-talk]]")
        })
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert!(epilogue.contains("eid=talk"), "overflow bullet kept");
    fs::write(env.vault().join("2026/20261030.md"), &epilogue)
        .expect("delete row");
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("removed"), "{}", stdout(&output));
    assert!(
        !today_note(&env).contains("^long-talk "),
        "never re-added:\n{}",
        today_note(&env)
    );
}

#[test]
fn title_update_keeps_the_description_record() {
    let env = GcalEnv::new("bob-cli-gcal-sync-birth-carry");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&desc_envelope(vec![desc_event(
        "dentist",
        "Dentist",
        "Bring insurance card",
        "2026-10-30T18:30:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    assert_eq!(sync_output(&env, &fake, &[]).status.code(), Some(0));
    fake.events_json(&desc_envelope(vec![desc_event(
        "dentist",
        "Dentist deep clean",
        "Bring insurance card",
        "2026-10-30T18:30:00-04:00",
        "2026-10-30T19:00:00-04:00",
    )]));
    let output = sync_output(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("~ task"), "{}", stdout(&output));
    let journal = journal_lines(&env);
    let latest = journal
        .iter()
        .filter(|line| {
            line["kind"] == "gcal.row" && line["key"]["event_id"] == "dentist"
        })
        .last()
        .expect("dentist row record");
    assert_eq!(latest["description"]["lines"], 1, "{latest}");
}

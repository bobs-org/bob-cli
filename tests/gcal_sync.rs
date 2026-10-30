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

    // Interview ended before sync: a task with no link.
    let interview = rows
        .iter()
        .find(|row| row.contains("Product interview"))
        .expect("interview row");
    assert!(interview.starts_with("- [ ] #task"), "{interview}");
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

    // MEET placeholder after the last open entry with four links.
    let pomodoros = &note[pomodoros_at..];
    assert!(pomodoros.contains("- [ ] () — MEET"), "{pomodoros}");
    for id in ["dentist", "flight-gmail", "late-call", "late-meeting"] {
        assert!(
            pomodoros.contains(&format!("\t- [[#^{id}]]")),
            "missing link {id}:\n{pomodoros}"
        );
    }
    assert!(
        !pomodoros.contains("[[#^product-interview]]"),
        "ended meetings earn no link:\n{pomodoros}"
    );

    // Journal: one row per included occurrence plus four links.
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
    assert_eq!(links, 4, "{journal:?}");
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
    assert_eq!(status["links_added"], 4);
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
    let plan_at = note.find("— PLAN").expect("PLAN entry");
    let meet_at = note.find("— MEET").expect("MEET entry");
    assert!(plan_at < meet_at, "{note}");
    let meet_line = note
        .lines()
        .find(|line| line.contains("— MEET"))
        .unwrap_or_default();
    assert_eq!(meet_line, "- [ ] () — MEET", "{note}");
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
    assert_eq!(
        note.matches("— MEET").count(),
        1,
        "no second open MEET:\n{note}"
    );
    assert!(note.contains("\t- [[#^dentist]]"), "{note}");
}

#[test]
fn past_day_gets_rows_only_and_no_meet() {
    let env = GcalEnv::new("bob-cli-gcal-sync-past");
    prepare(&env);
    fs::write(
        env.vault().join("2026/20261029.md"),
        "# Thursday 2026-10-29\n\n## Pomodoros\n",
    )
    .expect("yesterday note");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(include_str!("fixtures/gcal/events.json"));

    let output = sync_output(&env, &fake, &["-D", "yesterday"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = fs::read_to_string(env.vault().join("2026/20261029.md"))
        .expect("yesterday");
    assert!(note.contains("## Events"), "{note}");
    assert!(!note.contains("— MEET"), "past days earn no MEET:\n{note}");
    assert!(!note.contains("[[#^"), "past days earn no links:\n{note}");
    // Today's note is untouched.
    assert!(
        !today_note(&env).contains("## Events"),
        "{}",
        today_note(&env)
    );
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
    assert_eq!(report["pomodoro"]["links_added"], 4);
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
    assert!(
        !stdout(&output).contains("Main St Dental\"],")
            && stdout(&output).contains("attendees") == false,
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

    let output = run_bob(&env, &["task", "reconcile"], "2026-10-30 12:00:00");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let note = today_note(&env);
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
    // Linked rows resolve with clean titles; the ended interview has
    // no link by design, so it stays out of the agenda.
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

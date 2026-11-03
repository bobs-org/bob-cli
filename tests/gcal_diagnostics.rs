//! `bob gcal` clock deprecation diagnostics: warnings and structured
//! deprecations in sync and list.

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

fn list_output(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
) -> std::process::Output {
    let mut command = env.command();
    let mut full = vec!["gcal", "list"];
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

fn journal_lines(env: &GcalEnv) -> Vec<Value> {
    let path = env.vault().join(".bob/gcal/test-host.jsonl");
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("journal JSON"))
        .collect()
}

fn clock_event(
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

fn envelope(events: Vec<Value>) -> String {
    serde_json::to_string(&serde_json::json!({
        "events": events,
        "nextPageToken": serde_json::Value::Null,
    }))
    .expect("envelope")
}

fn deprecation_for<'a>(report: &'a Value, event_id: &str) -> Vec<&'a Value> {
    report["deprecations"]
        .as_array()
        .expect("deprecations array")
        .iter()
        .filter(|record| record["event_id"] == event_id)
        .collect()
}

#[test]
fn deprecated_aliases_warn_once_per_occurrence() {
    let env = GcalEnv::new("bob-cli-gcal-diag-matrix");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&envelope(vec![
        clock_event(
            "swings",
            "Swings! #offclock",
            None,
            "2026-10-30T08:00:00-04:00",
            "2026-10-30T08:15:00-04:00",
        ),
        clock_event(
            "gym",
            "Gym",
            Some("Bring shoes\n#off-clock"),
            "2026-10-30T08:30:00-04:00",
            "2026-10-30T09:00:00-04:00",
        ),
        clock_event(
            "both",
            "Both #untracked",
            Some("@meet #untracked"),
            "2026-10-30T09:00:00-04:00",
            "2026-10-30T09:15:00-04:00",
        ),
        clock_event(
            "repeat",
            "Repeat #offclock #offclock",
            None,
            "2026-10-30T09:30:00-04:00",
            "2026-10-30T10:00:00-04:00",
        ),
        clock_event(
            "multi",
            "Multi #offclock #untracked",
            None,
            "2026-10-30T10:00:00-04:00",
            "2026-10-30T10:15:00-04:00",
        ),
        clock_event(
            "upper",
            "Upper #OFFCLOCK!",
            None,
            "2026-10-30T10:15:00-04:00",
            "2026-10-30T10:30:00-04:00",
        ),
        clock_event(
            "onwins",
            "Planning #offclock #on",
            None,
            "2026-10-30T11:30:00-04:00",
            "2026-10-30T12:00:00-04:00",
        ),
        clock_event(
            "canonical",
            "Canonical #off #on",
            None,
            "2026-10-30T12:00:00-04:00",
            "2026-10-30T12:15:00-04:00",
        ),
        clock_event(
            "prose",
            "Prose",
            Some("Bring shoes #offclock"),
            "2026-10-30T13:00:00-04:00",
            "2026-10-30T13:15:00-04:00",
        ),
        clock_event(
            "plain",
            "Plain meeting",
            None,
            "2026-10-30T14:00:00-04:00",
            "2026-10-30T14:15:00-04:00",
        ),
    ]));

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("sync JSON");

    // One warning per deprecated occurrence plus the canonical conflict;
    // prose and canonical inputs stay silent apart from the conflict.
    let warnings = report["warnings"].as_array().expect("warnings");
    for (event_id, title) in [
        ("swings", "Swings!"),
        ("gym", "Gym"),
        ("both", "Both"),
        ("repeat", "Repeat"),
        ("multi", "Multi"),
        ("upper", "Upper"),
        ("onwins", "Planning"),
        ("canonical", "Canonical"),
    ] {
        let hits: Vec<&Value> = warnings
            .iter()
            .filter(|warning| {
                warning
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(&format!("{title}:"))
            })
            .collect();
        assert_eq!(hits.len(), 1, "one warning for {event_id}: {warnings:?}");
    }
    assert!(
        !warnings.iter().any(|warning| {
            warning.as_str().unwrap_or_default().starts_with("Prose:")
        }),
        "prose aliases never warn: {warnings:?}"
    );
    assert!(
        !warnings.iter().any(|warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .starts_with("Plain meeting:")
        }),
        "canonical input stays silent: {warnings:?}"
    );

    // Structured records carry exactly the additive envelope keys.
    let deprecations = report["deprecations"].as_array().expect("deprecations");
    for record in deprecations {
        let object = record.as_object().expect("record object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "calendar_id",
                "drop_suggested",
                "event_id",
                "on_wins",
                "replacement",
                "title",
                "token",
                "where"
            ],
            "{record}"
        );
        assert_eq!(record["calendar_id"], "primary");
        assert_eq!(record["replacement"], "#off");
    }
    assert_eq!(deprecation_for(&report, "swings").len(), 1);
    assert_eq!(deprecation_for(&report, "swings")[0]["where"], "title");
    assert_eq!(deprecation_for(&report, "swings")[0]["token"], "#offclock");
    assert_eq!(deprecation_for(&report, "gym")[0]["where"], "description");
    assert_eq!(deprecation_for(&report, "both")[0]["where"], "both");
    assert_eq!(deprecation_for(&report, "both").len(), 1);
    assert_eq!(deprecation_for(&report, "repeat").len(), 1);
    assert_eq!(deprecation_for(&report, "multi").len(), 2);
    assert_eq!(deprecation_for(&report, "upper")[0]["token"], "#offclock");
    assert_eq!(deprecation_for(&report, "onwins")[0]["on_wins"], true);
    assert_eq!(
        deprecation_for(&report, "onwins")[0]["drop_suggested"],
        true
    );
    assert!(deprecation_for(&report, "canonical").is_empty());
    assert!(deprecation_for(&report, "prose").is_empty());
    assert!(deprecation_for(&report, "plain").is_empty());

    // Canonical conflict still warns with its code and the winner.
    let canonical_warning = warnings
        .iter()
        .find(|warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .starts_with("Canonical:")
        })
        .expect("canonical warning");
    assert!(
        canonical_warning
            .as_str()
            .unwrap_or_default()
            .contains("clock_tag_conflict"),
        "{canonical_warning}"
    );
    assert!(
        canonical_warning
            .as_str()
            .unwrap_or_default()
            .contains("#on wins"),
        "{canonical_warning}"
    );
    // On-wins copy names the winner and the drop action.
    let onwins_warning = warnings
        .iter()
        .find(|warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .starts_with("Planning:")
        })
        .expect("on-wins warning");
    assert!(
        onwins_warning
            .as_str()
            .unwrap_or_default()
            .contains("#on wins"),
        "{onwins_warning}"
    );

    // No raw description disclosure in the structured envelope.
    let raw = stdout(&output);
    assert!(!raw.contains("Bring shoes"), "{raw}");

    // List carries the same top-level arrays and per-event clock fields.
    let listed = list_output(&env, &fake, &["-f", "json"]);
    assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
    let list: Value =
        serde_json::from_str(&stdout(&listed)).expect("list JSON");
    assert_eq!(
        list["deprecations"]
            .as_array()
            .expect("list deprecations")
            .len(),
        deprecations.len(),
        "list mirrors sync deprecations"
    );
    let events = list["events"].as_array().expect("events");
    let planning = events
        .iter()
        .find(|row| row["title"] == "Planning")
        .expect("planning row");
    assert_eq!(planning["clock"], "on");
    assert_eq!(planning["off_clock"], false);
}

#[test]
fn sync_deprecations_survive_all_reports_without_extra_writes() {
    let env = GcalEnv::new("bob-cli-gcal-diag-lifecycle");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&envelope(vec![clock_event(
        "swings",
        "Swings! #offclock",
        None,
        "2026-10-30T08:00:00-04:00",
        "2026-10-30T08:15:00-04:00",
    )]));

    // Human dry-run prints the diff on stdout and warnings on stderr.
    // Dry runs happen before the first write so the diff shows the row.
    let dry_human = sync_output(&env, &fake, &["-d"]);
    assert_eq!(dry_human.status.code(), Some(0), "{}", stderr(&dry_human));
    assert!(
        stdout(&dry_human).contains("Swings!"),
        "{}",
        stdout(&dry_human)
    );
    assert!(
        stderr(&dry_human).contains(
            "Swings!: #offclock is deprecated in Google Calendar; write #off"
        ),
        "{}",
        stderr(&dry_human)
    );

    // JSON dry-run carries the arrays without stderr duplicates.
    let dry_json = sync_output(&env, &fake, &["-d", "-f", "json"]);
    assert_eq!(dry_json.status.code(), Some(0), "{}", stderr(&dry_json));
    let dry_report: Value =
        serde_json::from_str(&stdout(&dry_json)).expect("dry JSON");
    assert_eq!(dry_report["status"], "dry_run");
    assert_eq!(dry_report["deprecations"].as_array().unwrap().len(), 1);
    assert!(stderr(&dry_json).is_empty(), "{}", stderr(&dry_json));

    // Written report carries the arrays.
    let written = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(written.status.code(), Some(0), "{}", stderr(&written));
    let written_report: Value =
        serde_json::from_str(&stdout(&written)).expect("written JSON");
    assert_eq!(written_report["status"], "written");
    assert_eq!(written_report["deprecations"].as_array().unwrap().len(), 1);
    assert_eq!(written_report["warnings"].as_array().unwrap().len(), 1);
    let note_after_write = today_note(&env);
    let journal_after_write = journal_lines(&env).len();

    // Same-minute rerun is unchanged but repeats the diagnostics.
    let unchanged = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(unchanged.status.code(), Some(0), "{}", stderr(&unchanged));
    let unchanged_report: Value =
        serde_json::from_str(&stdout(&unchanged)).expect("unchanged JSON");
    assert_eq!(unchanged_report["status"], "unchanged");
    assert_eq!(
        unchanged_report["deprecations"],
        written_report["deprecations"]
    );
    assert_eq!(unchanged_report["warnings"], written_report["warnings"]);
    assert_eq!(today_note(&env), note_after_write);
    assert_eq!(journal_lines(&env).len(), journal_after_write);

    // Quiet human mode suppresses the summary but keeps warnings.
    let quiet = sync_output(&env, &fake, &["-q"]);
    assert_eq!(quiet.status.code(), Some(0), "{}", stderr(&quiet));
    assert!(stdout(&quiet).is_empty(), "{:?}", stdout(&quiet));
    assert!(
        stderr(&quiet).contains("#offclock is deprecated"),
        "{}",
        stderr(&quiet)
    );

    // Waiting still reports diagnostics without journal writes.
    let note_path = env.vault().join("2026/20261030.md");
    fs::write(&note_path, "# Friday 2026-10-30\n\nNo pomodoros here\n")
        .expect("remove pomodoros");
    let journal_before_waiting = journal_lines(&env).len();
    let waiting = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(waiting.status.code(), Some(0), "{}", stderr(&waiting));
    let waiting_report: Value =
        serde_json::from_str(&stdout(&waiting)).expect("waiting JSON");
    assert_eq!(waiting_report["status"], "waiting");
    assert_eq!(waiting_report["deprecations"].as_array().unwrap().len(), 1);
    assert_eq!(waiting_report["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(journal_lines(&env).len(), journal_before_waiting);
}

#[test]
fn inherited_drop_advice_needs_a_usable_off_clock_root() {
    let env = GcalEnv::new("bob-cli-gcal-diag-inherit");
    prepare(&env);
    fs::write(
        env.vault().join("love.md"),
        "---\ntype: area\noffclock: true\n---\n# Love\n",
    )
    .expect("write love");
    fs::write(
        env.vault().join("abby.md"),
        "---\ntype: area\nparent: [[love]]\n---\n# Abby\n",
    )
    .expect("write abby");
    fs::write(
        env.vault().join("fun.md"),
        "---\ntype: area\nparent: [[love]]\noffclock: false\n---\n# Fun\n",
    )
    .expect("write fun");
    fs::write(
        env.vault().join("bad.md"),
        "---\ntype: area\nparent: [[love]]\noffclock: maybe\n---\n# Bad\n",
    )
    .expect("write bad");
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&envelope(vec![
        clock_event(
            "swings",
            "Swings! @abby #offclock",
            None,
            "2026-10-30T08:00:00-04:00",
            "2026-10-30T08:15:00-04:00",
        ),
        clock_event(
            "fun-event",
            "Fun @fun #offclock",
            None,
            "2026-10-30T09:00:00-04:00",
            "2026-10-30T09:15:00-04:00",
        ),
        clock_event(
            "bad-event",
            "Bad @bad #offclock",
            None,
            "2026-10-30T10:00:00-04:00",
            "2026-10-30T10:15:00-04:00",
        ),
        clock_event(
            "unknown",
            "Unknown @missing #offclock",
            None,
            "2026-10-30T11:00:00-04:00",
            "2026-10-30T11:15:00-04:00",
        ),
        clock_event(
            "onwins",
            "Planning @abby #offclock #on",
            None,
            "2026-10-30T11:30:00-04:00",
            "2026-10-30T12:00:00-04:00",
        ),
    ]));

    let output = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value =
        serde_json::from_str(&stdout(&output)).expect("sync JSON");
    let warnings = report["warnings"].as_array().expect("warnings");

    let swings_warning = warnings
        .iter()
        .find(|warning| {
            warning.as_str().unwrap_or_default().starts_with("Swings!:")
        })
        .expect("swings warning");
    assert!(
        swings_warning.as_str().unwrap_or_default().contains(
            "(or drop it: abby is already off the clock via love.md)"
        ),
        "{swings_warning}"
    );
    assert_eq!(
        deprecation_for(&report, "swings")[0]["drop_suggested"],
        true
    );

    for event_id in ["fun-event", "bad-event", "unknown"] {
        let records = deprecation_for(&report, event_id);
        assert_eq!(records.len(), 1, "{event_id}");
        assert_eq!(records[0]["drop_suggested"], false, "{event_id}");
    }
    let fun_warning = warnings
        .iter()
        .find(|warning| {
            warning.as_str().unwrap_or_default().starts_with("Fun:")
        })
        .expect("fun warning");
    assert!(
        !fun_warning
            .as_str()
            .unwrap_or_default()
            .contains("already off the clock"),
        "{fun_warning}"
    );

    let onwins_warning = warnings
        .iter()
        .find(|warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .starts_with("Planning:")
        })
        .expect("on-wins warning");
    assert!(
        onwins_warning
            .as_str()
            .unwrap_or_default()
            .contains("#on wins"),
        "{onwins_warning}"
    );
    assert!(
        !onwins_warning
            .as_str()
            .unwrap_or_default()
            .contains("already off the clock"),
        "{onwins_warning}"
    );
}

#[test]
fn canonical_input_reports_empty_arrays() {
    let env = GcalEnv::new("bob-cli-gcal-diag-empty");
    prepare(&env);
    let fake = FakeGog::new(&env, "fake-gog");
    fake.events_json(&envelope(vec![clock_event(
        "plain",
        "Plain meeting",
        None,
        "2026-10-30T08:00:00-04:00",
        "2026-10-30T08:15:00-04:00",
    )]));

    let sync = sync_output(&env, &fake, &["-f", "json"]);
    assert_eq!(sync.status.code(), Some(0), "{}", stderr(&sync));
    let sync_report: Value =
        serde_json::from_str(&stdout(&sync)).expect("sync JSON");
    assert_eq!(sync_report["deprecations"].as_array().unwrap().len(), 0);
    assert_eq!(sync_report["warnings"].as_array().unwrap().len(), 0);

    let listed = list_output(&env, &fake, &["-f", "json"]);
    assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
    let list: Value =
        serde_json::from_str(&stdout(&listed)).expect("list JSON");
    assert_eq!(list["deprecations"].as_array().unwrap().len(), 0);
    assert_eq!(list["warnings"].as_array().unwrap().len(), 0);
}

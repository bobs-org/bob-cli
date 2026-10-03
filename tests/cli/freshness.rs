//! `bob freshness list` and `bob freshness seed`: the headless
//! review queue and the guarded cutover seed.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-10-08";

const A_TASKS: &str = "\
- [ ] #task New capture [created:: 2026-09-30]\n\
- [ ] #task Stale bread [fresh:: 2026-09-20] [created:: 2026-09-01]\n\
- [ ] #task Fresh eggs [fresh:: 2026-10-05] [created:: 2026-10-01]\n\
- [ ] #task Deferred pie [fresh:: 2026-10-05] [scheduled:: 2026-10-07]\n\
- [ ] #task Bad date [fresh:: 2026-13-01]\n\
- [ ] #task Future stamp [fresh:: 2026-10-09]\n\
- [ ] #task Hidden chore #hide\n\
- [ ] #task Water plants [repeat:: every week] [created:: 2026-09-01]\n\
- [ ] #task Root [fresh:: 2026-10-05] [id:: lane-root]\n\
- [ ] #task Waiting [dependsOn:: lane-root]\n\
- [*] #task Next thing ^next-one\n\
- [*] #task Refreshed next [fresh:: 2026-10-08]\n\
- [x] #task Done deed [completion:: 2026-10-07]\n\
- [x] #task Refreshed done [fresh:: 2026-10-08] [completion:: 2026-10-08]\n";

const B_TASKS: &str = "\
---\n\
task_refresh: 3\n\
---\n\
- [ ] #task Quick note task [fresh:: 2026-10-05]\n\
- [ ] #task Slow note task [refresh:: 14] [fresh:: 2026-10-01] [created:: 2026-09-10]\n";

const DAILY_NOTE: &str = "\
# 2026-10-08\n\
\n\
## Pomodoros\n\
\n\
- [ ] (0900-0930) — REVIEW\n\
\x20   - [[a#^today-task]]\n";

fn freshness_vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("a.md"), A_TASKS);
    write_file(&vault.join("b.md"), B_TASKS);
    write_file(
        &vault.join("_templates/tpl.md"),
        "- [ ] #task Templated ^tpl-task\n",
    );
    temp
}

fn vault_dir(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("vault")
}

fn list_json(temp: &TempDir, extra: &[&str]) -> (std::process::Output, Value) {
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(temp))
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json");
    for arg in extra {
        command.arg(arg);
    }
    let output = command.output().expect("run bob freshness list -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    (output, value)
}

#[test]
fn list_json_reports_queue_counts_and_contract() {
    let temp = freshness_vault("bob-cli-freshness-list");
    let (_, value) = list_json(&temp, &[]);

    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 4);
    assert_eq!(value["date"], NOW);
    assert_eq!(value["config"]["interval"], 7);
    assert_eq!(value["config"]["pending_interval"], 1);
    assert_eq!(value["config"]["next_interval"], 1);
    assert!(value["config"]["rotten_daily_budget"].is_null());

    let counts = &value["counts"];
    // NEW: New capture, Bad date, Future stamp. NEXT: the
    // never-stamped [*] Next thing. RETURNED: Deferred pie.
    // ROTTEN: Stale bread, Quick note task (note interval 3); Slow
    // note task is FRESH (task interval 14).
    assert_eq!(counts["new"], 3);
    assert_eq!(counts["pending_due"], 0);
    assert_eq!(counts["next_due"], 1);
    assert_eq!(counts["resurfaced"], 1);
    assert_eq!(counts["rotten"], 2);
    assert_eq!(counts["due"], 6);
    assert_eq!(counts["walk"], 7);
    // FRESH: Fresh eggs, Slow note task, Root.
    assert_eq!(counts["fresh"], 3);
    // Refreshed today: the [*] and [x] stamped 2026-10-08.
    // Upkeep excludes the [*] lane stamp.
    assert_eq!(counts["refreshed_today"], 2);
    assert_eq!(counts["upkeep_today"], 1);
    assert_eq!(counts["budget_met"], false);

    let queue = value["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 7);
    // Tier order NEW → PENDING → NEXT → RETURNED → ROTTEN: the
    // never-stamped [*] walks in NEXT between NEW and RETURNED.
    let tiers: Vec<&str> = queue
        .iter()
        .map(|entry| entry["tier"].as_str().unwrap())
        .collect();
    assert_eq!(
        tiers,
        vec!["new", "new", "new", "next", "returned", "rotten", "rotten"]
    );
    // Lane rows carry a null state and bucket; Ready rows keep them.
    let lane_row = &queue[3];
    assert!(
        lane_row["text"] == "Next thing"
            || lane_row["text"] == "Next thing ^next-one",
        "lane text:\n{value}"
    );
    assert_eq!(lane_row["tier"], "next");
    assert_eq!(lane_row["lane"], "next");
    assert!(lane_row["state"].is_null());
    assert!(lane_row["bucket"].is_null());
    assert_eq!(lane_row["interval"], 1);
    assert_eq!(lane_row["interval_source"], "next");
    assert_eq!(lane_row["due_on"], serde_json::Value::Null);
    let states: Vec<Option<&str>> =
        queue.iter().map(|entry| entry["state"].as_str()).collect();
    assert_eq!(
        states,
        vec![
            Some("new"),
            Some("new"),
            Some("new"),
            None,
            Some("resurfaced"),
            Some("rotten"),
            Some("rotten"),
        ]
    );
    // Schema 3 uses the rotten vocabulary for machine `state` names;
    // the `bucket` still carries the stable gating vocabulary: new →
    // new, rotten and resurfaced → rotten, lane rows → null.
    let buckets: Vec<Option<&str>> =
        queue.iter().map(|entry| entry["bucket"].as_str()).collect();
    assert_eq!(
        buckets,
        vec![
            Some("new"),
            Some("new"),
            Some("new"),
            None,
            Some("rotten"),
            Some("rotten"),
            Some("rotten"),
        ]
    );
    let first = &queue[0];
    assert_eq!(first["rank"], 1);
    assert_eq!(first["tier"], "new");
    assert_eq!(first["lane"], "ready");
    assert_eq!(first["path"], "a.md");
    assert_eq!(first["line"], 1);
    assert_eq!(first["text"], "New capture");
    assert_eq!(first["created"], "2026-09-30");
    assert!(first["fresh"].is_null());
    assert_eq!(first["interval"], 7);
    assert_eq!(first["interval_source"], "default");
    assert!(first["due_on"].is_null());

    // ROTTEN sorts by interval, then due date: the note-interval
    // task (3d) comes before the default-interval task (7d).
    let note_task = &queue[5];
    assert_eq!(note_task["path"], "b.md");
    assert_eq!(note_task["tier"], "rotten");
    assert_eq!(note_task["interval"], 3);
    assert_eq!(note_task["interval_source"], "note");

    let rotten = &queue[6];
    assert_eq!(rotten["path"], "a.md");
    assert_eq!(rotten["line"], 2);
    assert_eq!(rotten["tier"], "rotten");
    assert_eq!(rotten["fresh"], "2026-09-20");
    assert_eq!(rotten["due_on"], "2026-09-27");
    assert_eq!(rotten["days_overdue"], 11);

    let resurfaced = &queue[4];
    assert_eq!(resurfaced["tier"], "returned");
    assert_eq!(resurfaced["state"], "resurfaced");
    assert_eq!(resurfaced["bucket"], "rotten");
    assert_eq!(resurfaced["due_on"], "2026-10-07");
    assert_eq!(resurfaced["days_overdue"], 1);

    // Slow note task (task interval 14) is FRESH, so absent.
    assert!(
        !queue.iter().any(|entry| entry["text"] == "Slow note task"),
        "task interval must beat the note interval:\n{value}"
    );
    // Out-of-scope tasks never queue. ("Next thing" now walks in
    // NEXT, so it is asserted present above.)
    for missing in [
        "Hidden chore",
        "Water plants",
        "Waiting",
        "Fresh eggs",
        "Templated",
    ] {
        assert!(
            !queue.iter().any(|entry| entry["text"] == missing),
            "out-of-scope task queued:\n{value}"
        );
    }

    let codes: Vec<&str> = value["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .map(|warning| warning["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"fresh_malformed"), "warnings:\n{value}");
    assert!(codes.contains(&"fresh_future"), "warnings:\n{value}");
}

#[test]
fn list_human_has_sections_and_no_ansi() {
    let temp = freshness_vault("bob-cli-freshness-human");
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob freshness list");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("bob freshness")
            && human.contains("every 7d")
            && human.contains("pending 1d")
            && human.contains("next 1d")
            && human.contains("REVIEW 7 due")
            && human.contains("3 new")
            && human.contains("1 next")
            && human.contains("1 returned")
            && human.contains("2 rotten")
            && human.contains("✓ 1 today"),
        "expected a REVIEW summary:\n{human}"
    );
    assert!(
        human.contains("NEW 3")
            && human.contains("NEXT 1")
            && human.contains("RETURNED 1")
            && human.contains("ROTTEN 2")
            && human.contains("commitments done above"),
        "expected tiered sections and divider:\n{human}"
    );
    assert!(
        !human.contains("PENDING"),
        "empty PENDING tier is omitted:\n{human}"
    );
    assert!(
        human.contains("LINTS")
            && human.contains("fresh_malformed")
            && human.contains("fresh_future"),
        "expected lints last:\n{human}"
    );
    assert_text_order(
        &human,
        &[
            "REVIEW",
            "NEW",
            "NEXT",
            "RETURNED",
            "commitments done",
            "ROTTEN",
            "LINTS",
        ],
    );
    assert_stdout_has_no_ansi(&output);

    // Bare `bob freshness` is `list`.
    let bare = bob_command()
        .arg("freshness")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bare bob freshness");
    assert_success(&bare);
    assert_eq!(stdout(&bare), human);
}

#[test]
fn list_limit_truncates_rows_not_counts() {
    let temp = freshness_vault("bob-cli-freshness-limit");
    let (_, value) = list_json(&temp, &["--limit", "2"]);
    let queue = value["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0]["rank"], 1);
    assert_eq!(queue[1]["rank"], 2);
    assert_eq!(value["counts"]["due"], 6);
    assert_eq!(value["counts"]["walk"], 7);
}

#[test]
fn list_excludes_today_tasks() {
    let temp = freshness_vault("bob-cli-freshness-today");
    // Link a Ready task under today's open Pomodoro: it leaves Today
    // out of the queue.
    write_file(&vault_dir(&temp).join("2026/20261008.md"), DAILY_NOTE);
    let a_path = vault_dir(&temp).join("a.md");
    let mut contents = fs::read_to_string(&a_path).expect("read a.md");
    contents.push_str("- [ ] #task Today chore ^today-task\n");
    write_file(&a_path, &contents);

    let (_, value) = list_json(&temp, &[]);
    let queue = value["queue"].as_array().expect("queue array");
    assert!(
        !queue.iter().any(|entry| entry["text"] == "Today chore"),
        "Today task must be excluded:\n{value}"
    );
    assert_eq!(value["counts"]["due"], 6);
}

#[test]
fn list_config_interval_and_budget() {
    let temp = freshness_vault("bob-cli-freshness-config");
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        "freshness:\n  interval: 10\n  rotten_daily_budget: 15\n",
    );
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json");
    let output = command.output().expect("run with config");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    // Stale bread (fresh 09-20 + 10d = 09-30) is still rotten, but with
    // a config source now.
    assert_eq!(value["config"]["interval"], 10);
    assert_eq!(value["config"]["rotten_daily_budget"], 15);
    let queue = value["queue"].as_array().expect("queue array");
    let rotten = queue
        .iter()
        .find(|entry| entry["text"] == "Stale bread")
        .expect("stale bread queues");
    assert_eq!(rotten["state"], "rotten");
    assert_eq!(rotten["interval_source"], "config");
    assert_eq!(value["counts"]["budget"], 15);
    assert_eq!(value["counts"]["budget_met"], false);
}

#[test]
fn list_legacy_budget_key_warns_once_and_still_counts() {
    let temp = freshness_vault("bob-cli-freshness-legacy");
    let config = temp.path().join("config.yml");
    write_file(&config, "freshness:\n  stale_daily_budget: 15\n");
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json");
    let output = command.output().expect("run with legacy config");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    // The legacy key still supplies the budget under schema 3.
    assert_eq!(value["config"]["rotten_daily_budget"], 15);
    assert_eq!(value["counts"]["budget"], 15);
    // Exactly one deprecation diagnostic, not one per task.
    let deprecated: Vec<&Value> = value["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .filter(|warning| {
            warning["code"] == "freshness_stale_daily_budget_deprecated"
        })
        .collect();
    assert_eq!(deprecated.len(), 1);
    assert!(deprecated[0]["line"].is_null());

    // The same legacy config warns in human output too.
    let human = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run legacy config human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("freshness_stale_daily_budget_deprecated"),
        "expected the deprecation lint:\n{text}"
    );
}

#[test]
fn list_canonical_budget_key_wins_and_warns() {
    let temp = freshness_vault("bob-cli-freshness-both-keys");
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        "freshness:\n  rotten_daily_budget: 20\n  stale_daily_budget: 15\n",
    );
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json");
    let output = command.output().expect("run with both keys");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    assert_eq!(value["config"]["rotten_daily_budget"], 20);
    assert_eq!(value["counts"]["budget"], 20);
    assert!(
        value["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning["code"]
                == "freshness_stale_daily_budget_deprecated"),
        "expected the deprecation lint:\n{value}"
    );
}

#[test]
fn list_invalid_canonical_budget_exits_2() {
    let temp = freshness_vault("bob-cli-freshness-bad-budget");
    let config = temp.path().join("config.yml");
    write_file(&config, "freshness:\n  rotten_daily_budget: soon\n");
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run with invalid budget");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn list_invalid_config_exits_2() {
    let temp = freshness_vault("bob-cli-freshness-bad-config");
    let config = temp.path().join("config.yml");
    write_file(&config, "freshness:\n  interval: soon\n");
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run with invalid config");
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(value["ok"], false);
}

#[test]
fn list_emoji_vault_is_refused_with_exit_2() {
    // No Tasks settings file means the Emoji format: placement is
    // Dataview-only, so the command refuses.
    let temp = TempDir::new("bob-cli-freshness-emoji");
    let vault = temp.path().join("vault");
    write_file(&vault.join("a.md"), "- [ ] #task Plain task\n");
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run on emoji vault");
    assert_eq!(output.status.code(), Some(2));
}

fn seed_vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("a.md"),
        "- [ ] #task Buy milk [created:: 2026-09-29]\n\
        - [ ] #task Old bread [created:: 2026-09-01]\n\
        - [*] #task Next thing ^next-one\n",
    );
    write_file(
        &vault.join("b.md"),
        "---\ntask_refresh: 30\n---\n- [ ] #task Big project [created:: 2026-08-01]\n",
    );
    temp
}

fn seed_json(temp: &TempDir, extra: &[&str]) -> (std::process::Output, Value) {
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("seed")
        .env("BOB_DIR", vault_dir(temp))
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json");
    for arg in extra {
        command.arg(arg);
    }
    let output = command.output().expect("run bob freshness seed");
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("seed JSON");
    (output, value)
}

#[test]
fn seed_dry_run_writes_nothing_and_reports_buckets() {
    let temp = seed_vault("bob-cli-freshness-seed-dry");
    let before = fs::read_to_string(vault_dir(&temp).join("a.md")).unwrap();
    let (output, value) = seed_json(&temp, &["--dry-run"]);
    assert_success(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 4);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["stamped"]["ready"], 3);
    assert_eq!(value["stamped"]["other"], 1);
    assert_eq!(
        fs::read_to_string(vault_dir(&temp).join("a.md")).unwrap(),
        before,
        "dry run must not write"
    );
    let buckets = value["buckets"].as_array().expect("buckets");
    assert_eq!(buckets.len(), 7);
    let total: u64 = buckets
        .iter()
        .map(|bucket| bucket["count"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 3);
    // Bucket dates stagger across the last 7 days; nothing lands on
    // cutover day with a past due date.
    assert_eq!(buckets[0]["fresh"], "2026-10-02");
    assert_eq!(buckets[6]["fresh"], "2026-10-08");
    assert_eq!(value["skipped"]["already_stamped"], 0);
    assert_eq!(value["files"].as_array().unwrap().len(), 2);
}

#[test]
fn seed_applies_stamps_and_rerun_is_noop() {
    let temp = seed_vault("bob-cli-freshness-seed-apply");
    let (output, first) = seed_json(&temp, &[]);
    assert_success(&output);
    assert_eq!(first["dry_run"], false);

    let a_contents = fs::read_to_string(vault_dir(&temp).join("a.md")).unwrap();
    assert!(
        a_contents.contains("[fresh:: 2026-10-02]")
            && a_contents.contains("[created:: 2026-09-29]")
            && a_contents.contains("[fresh:: 2026-10-08] ^next-one"),
        "seed stamps before the suffix:\n{a_contents}"
    );
    let b_contents = fs::read_to_string(vault_dir(&temp).join("b.md")).unwrap();
    assert!(
        b_contents.contains("[fresh::")
            && b_contents.contains("[created:: 2026-08-01]"),
        "note task is stamped:\n{b_contents}"
    );

    // A same-day rerun is an idempotent no-op.
    let (output, second) = seed_json(&temp, &[]);
    assert_success(&output);
    assert_eq!(second["stamped"]["ready"], 0);
    assert_eq!(second["stamped"]["other"], 0);
    assert!(second["skipped"]["already_stamped"].as_u64().unwrap() > 0);
    assert_eq!(second["files"].as_array().unwrap().len(), 0);

    // Nothing is due on cutover day.
    let (_, list) = list_json(&temp, &[]);
    assert_eq!(list["counts"]["new"], 0);
    assert_eq!(list["counts"]["due"], 0);
}

#[test]
fn seed_guard_refuses_and_force_overrides() {
    let temp = seed_vault("bob-cli-freshness-seed-guard");
    write_file(
        &vault_dir(&temp).join("c.md"),
        "- [ ] #task Previously seen [fresh:: 2026-10-01]\n",
    );
    let (output, value) = seed_json(&temp, &["--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);

    let untouched = fs::read_to_string(vault_dir(&temp).join("a.md")).unwrap();
    assert!(
        !untouched.contains("[fresh::"),
        "guard refusal must not write:\n{untouched}"
    );

    let (output, forced) = seed_json(&temp, &["--dry-run", "--force"]);
    assert_success(&output);
    assert_eq!(forced["stamped"]["ready"], 3);
}

#[test]
fn seed_dry_run_lists_files_without_writing() {
    let temp = seed_vault("bob-cli-freshness-seed-race");
    let (output, value) = seed_json(&temp, &["--dry-run"]);
    assert_success(&output);
    let files = value["files"].as_array().expect("files");
    assert_eq!(files.len(), 2);
    let a_contents = fs::read_to_string(vault_dir(&temp).join("a.md")).unwrap();
    assert!(!a_contents.contains("[fresh::"));
}

#[test]
fn list_lane_rows_cover_pending_and_next() {
    let temp = TempDir::new("bob-cli-freshness-lanes");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("a.md"),
        "- [/] #task Pending chore [fresh:: 2026-10-07]\n\
        - [*] #task Next chore [fresh:: 2026-10-07]\n\
        - [*] #task Fresh next [fresh:: 2026-10-08]\n",
    );
    let (_, value) = list_json(&temp, &[]);
    assert_eq!(value["schema_version"], 4);
    let counts = &value["counts"];
    assert_eq!(counts["pending_due"], 1);
    assert_eq!(counts["next_due"], 1);
    assert_eq!(counts["walk"], 2);
    let queue = value["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0]["tier"], "pending");
    assert_eq!(queue[0]["lane"], "pending");
    assert!(queue[0]["state"].is_null());
    assert!(queue[0]["bucket"].is_null());
    assert_eq!(queue[0]["interval_source"], "pending");
    assert_eq!(queue[0]["due_on"], "2026-10-08");
    assert_eq!(queue[0]["days_overdue"], 0);
    assert_eq!(queue[1]["tier"], "next");
    assert_eq!(queue[1]["lane"], "next");
    assert!(queue[1]["state"].is_null());
    // Stamped-today lane tasks never queue.
    assert!(
        !queue.iter().any(|entry| entry["text"]
            .as_str()
            .is_some_and(|text| text.contains("Fresh next"))),
        "stamped-today lane task queued:\n{value}"
    );
}

#[test]
fn list_excludes_today_and_daily_lane_tasks() {
    let temp = TempDir::new("bob-cli-freshness-lane-scope");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("a.md"),
        "- [*] #task Lane today ^lane-today\n\
        - [*] #task Lane plain [fresh:: 2026-10-07]\n",
    );
    write_file(
        &vault.join("2026/20261008.md"),
        "- [*] #task Lane daily [fresh:: 2026-10-07]\n\
        \n\
        ## Pomodoros\n\
        \n\
        - [ ] (0900-0930) — REVIEW\n\
        \x20  - [[a#^lane-today]]\n",
    );
    let (_, value) = list_json(&temp, &[]);
    let queue = value["queue"].as_array().expect("queue array");
    let has = |needle: &str| {
        queue.iter().any(|entry| {
            entry["text"]
                .as_str()
                .is_some_and(|text| text.contains(needle))
        })
    };
    assert!(
        !has("Lane today"),
        "Today-linked lane task queued:\n{value}"
    );
    assert!(!has("Lane daily"), "daily-note lane task queued:\n{value}");
    assert!(has("Lane plain"), "plain lane task missing:\n{value}");
}

#[test]
fn list_lane_intervals_false_null_and_invalid() {
    // false turns the lane off: the never-stamped [*] leaves the walk.
    let temp = freshness_vault("bob-cli-freshness-lane-off");
    let config = temp.path().join("config.yml");
    write_file(&config, "freshness:\n  next_interval: false\n");
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json");
    let output = command.output().expect("run with lane off");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    assert_eq!(value["config"]["next_interval"], false);
    assert_eq!(value["counts"]["next_due"], 0);
    assert!(
        !value["queue"]
            .as_array()
            .expect("queue")
            .iter()
            .any(|entry| {
                entry["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Next thing"))
            }),
        "disabled lane still queued:\n{value}"
    );
    let human = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run lane-off human");
    assert_success(&human);
    assert!(
        stdout(&human).contains("next off"),
        "disabled lane header:\n{}",
        stdout(&human)
    );

    // null means the default 1: the lane still walks.
    write_file(&config, "freshness:\n  next_interval:\n");
    let (_, value) = {
        let mut command = bob_command();
        command
            .arg("freshness")
            .arg("list")
            .env("BOB_DIR", vault_dir(&temp))
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_NOW", NOW)
            .arg("-f")
            .arg("json");
        let output = command.output().expect("run with null lane");
        assert_success(&output);
        let value: Value =
            serde_json::from_str(stdout(&output).trim()).expect("list JSON");
        (output, value)
    };
    assert_eq!(value["config"]["next_interval"], 1);
    assert_eq!(value["counts"]["next_due"], 1);

    // anything else (including true) exits 2.
    for body in [
        "freshness:\n  next_interval: true\n",
        "freshness:\n  pending_interval: 0\n",
        "freshness:\n  next_interval: soon\n",
    ] {
        write_file(&config, body);
        let output = bob_command()
            .arg("freshness")
            .arg("list")
            .env("BOB_DIR", vault_dir(&temp))
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_NOW", NOW)
            .arg("-f")
            .arg("json")
            .output()
            .expect("run with invalid lane");
        assert_eq!(output.status.code(), Some(2), "body:\n{body}");
    }
}

#[test]
fn list_budget_meter_uses_upkeep() {
    let temp = freshness_vault("bob-cli-freshness-upkeep-meter");
    let config = temp.path().join("config.yml");
    write_file(&config, "freshness:\n  rotten_daily_budget: 15\n");
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run with budget");
    assert_success(&output);
    let human = stdout(&output);
    // Upkeep is 1 (the [x] stamp); the [*] stamp does not count.
    assert!(human.contains("✓ 1/15 today"), "upkeep meter:\n{human}");
}

fn keeps_vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("a.md"),
        "- [ ] #task At limit [fresh:: 2026-09-20] [keeps:: 3] [created:: 2026-09-01]\n\
        - [ ] #task Below limit [fresh:: 2026-09-20] [keeps:: 1]\n\
        - [ ] #task No streak [fresh:: 2026-09-20]\n\
        - [ ] #task Never kept\n",
    );
    temp
}

fn keeps_list_json(temp: &TempDir, now: &str, extra: &[&str]) -> Value {
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(temp))
        .env("BOB_NOW", now)
        .arg("-f")
        .arg("json");
    for arg in extra {
        command.arg(arg);
    }
    let output = command.output().expect("run keeps list");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("keeps list JSON")
}

#[test]
fn list_reports_keeps_and_decide_per_schema_4() {
    // After activation (2026-10-19), the at-limit rotten row decides.
    let temp = keeps_vault("bob-cli-freshness-keeps");
    let value = keeps_list_json(&temp, "2026-10-20", &[]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 4);
    assert_eq!(value["config"]["decay"]["enabled"], true);
    assert_eq!(value["config"]["decay"]["keeps"], 3);
    assert!(value["config"]["decay"]["enter"].is_null());
    assert_eq!(value["config"]["decay"]["active_from"], "2026-10-19");
    assert_eq!(value["config"]["decay"]["active"], true);

    let queue = value["queue"].as_array().expect("queue array");
    let at_limit = queue
        .iter()
        .find(|entry| entry["text"] == "At limit")
        .expect("at-limit row");
    assert_eq!(at_limit["tier"], "rotten");
    assert_eq!(at_limit["keeps"], 3);
    assert_eq!(at_limit["decide"], true);
    let below = queue
        .iter()
        .find(|entry| entry["text"] == "Below limit")
        .expect("below-limit row");
    assert_eq!(below["keeps"], 1);
    assert_eq!(below["decide"], false);
    let plain = queue
        .iter()
        .find(|entry| entry["text"] == "No streak")
        .expect("no-streak row");
    assert_eq!(plain["keeps"], 0);
    assert_eq!(plain["decide"], false);
    assert_eq!(value["counts"]["decide"], 1);
    assert_eq!(value["counts"]["walk"], 4);

    // Human rows show `kept N×` and `· decide`; the header shows the
    // active threshold.
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_NOW", "2026-10-20")
        .output()
        .expect("run keeps list human");
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("keeps 3"), "threshold header:\n{human}");
    assert!(human.contains("kept 3×"), "kept row:\n{human}");
    assert!(human.contains("decide"), "decide marker:\n{human}");
}

#[test]
fn list_pre_activation_counts_but_never_decides() {
    let temp = keeps_vault("bob-cli-freshness-keeps-trial");
    let value = keeps_list_json(&temp, NOW, &[]);
    assert_eq!(value["schema_version"], 4);
    assert_eq!(value["config"]["decay"]["active"], false);
    assert_eq!(value["counts"]["decide"], 0);
    let queue = value["queue"].as_array().expect("queue array");
    assert!(queue.iter().all(|entry| entry["decide"] == false));
    // The streak still reads: counting works before activation.
    let at_limit = queue
        .iter()
        .find(|entry| entry["text"] == "At limit")
        .expect("at-limit row");
    assert_eq!(at_limit["keeps"], 3);

    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_NOW", NOW)
        .output()
        .expect("run trial list human");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("asks from 2026-10-19"),
        "pre-activation header:\n{human}"
    );
    assert!(!human.contains("decide"), "no decision promise:\n{human}");
}

#[test]
fn list_decay_off_and_zero_limit() {
    let temp = keeps_vault("bob-cli-freshness-keeps-off");
    let config = temp.path().join("config.yml");
    write_file(&config, "freshness:\n  decay: false\n");
    let mut command = bob_command();
    command
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", "2026-10-20")
        .arg("-f")
        .arg("json");
    let output = command.output().expect("run with decay off");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("off JSON");
    assert_eq!(value["config"]["decay"]["enabled"], false);
    assert_eq!(value["counts"]["decide"], 0);

    // A zero limit asks on every due Ready re-confirmation.
    write_file(&config, "freshness:\n  decay:\n    keeps: 0\n");
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", "2026-10-20")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run with zero keeps");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("zero JSON");
    assert_eq!(value["config"]["decay"]["keeps"], 0);
    let queue = value["queue"].as_array().expect("queue array");
    let plain = queue
        .iter()
        .find(|entry| entry["text"] == "No streak")
        .expect("no-streak row");
    assert_eq!(plain["decide"], true);
}

#[test]
fn list_invalid_decay_exits_2() {
    let temp = keeps_vault("bob-cli-freshness-keeps-bad");
    let config = temp.path().join("config.yml");
    write_file(&config, "freshness:\n  decay:\n    keeps: soon\n");
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run with invalid decay");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn seed_preserves_existing_keeps() {
    // A keep streak without a fresh stamp is still a seed candidate;
    // the seed stamps the date but must never reset the streak.
    let temp = TempDir::new("bob-cli-freshness-seed-keeps");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("a.md"), "- [ ] #task Kept before [keeps:: 2]\n");
    let (output, value) = seed_json(&temp, &[]);
    assert_success(&output);
    assert_eq!(value["schema_version"], 4);
    assert_eq!(value["stamped"]["ready"], 1);
    let contents =
        fs::read_to_string(vault.join("a.md")).expect("read seeded line");
    assert!(
        contents.contains("[keeps:: 2]"),
        "seed must preserve keeps:\n{contents}"
    );
    assert!(
        contents.contains("[fresh::"),
        "seed must stamp:\n{contents}"
    );
}

#[test]
fn seed_invariance_abort_lists_the_line() {
    let temp = seed_vault("bob-cli-freshness-seed-invariance");
    // A misplaced, malformed fresh hides `created` from both parsers;
    // repairing the placement would change the parsed fields, so the
    // seed aborts with no writes.
    write_file(
        &vault_dir(&temp).join("c.md"),
        "- [ ] #task Tricky [created:: 2026-09-01] [fresh:: not-a-date]\n",
    );
    let (output, value) = seed_json(&temp, &["--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    let error = value["error"].as_str().unwrap();
    assert!(
        error.contains("c.md:1"),
        "abort must list the line:\n{error}"
    );
    let a_contents = fs::read_to_string(vault_dir(&temp).join("a.md")).unwrap();
    assert!(
        !a_contents.contains("[fresh::"),
        "invariance abort must not write:\n{a_contents}"
    );
}

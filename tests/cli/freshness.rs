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
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["date"], NOW);
    assert_eq!(value["config"]["interval"], 7);
    assert!(value["config"]["rotten_daily_budget"].is_null());

    let counts = &value["counts"];
    // NEW: New capture, Bad date, Future stamp. DUE: Stale bread,
    // Deferred pie (resurfaced), Quick note task (note interval 3),
    // plus Slow note task is FRESH (task interval 14).
    assert_eq!(counts["new"], 3);
    assert_eq!(counts["resurfaced"], 1);
    assert_eq!(counts["rotten"], 2);
    assert_eq!(counts["due"], 6);
    // FRESH: Fresh eggs, Slow note task, Root.
    assert_eq!(counts["fresh"], 3);
    // Refreshed today: the [*] and [x] stamped 2026-10-08.
    assert_eq!(counts["refreshed_today"], 2);
    assert_eq!(counts["budget_met"], false);

    let queue = value["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 6);
    // NEW by (path, line), then DUE by (due_on, path, line):
    // Stale bread due 2026-09-27, Quick note task due 2026-10-08,
    // Deferred pie resurfaced due 2026-10-07... wait: resurfaced due
    // 10-07 sorts before stale due 09-27? No: DUE sorts by due_on:
    // 09-27, then 10-07, then 10-08.
    let states: Vec<&str> = queue
        .iter()
        .map(|entry| entry["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        vec!["new", "new", "new", "rotten", "resurfaced", "rotten"]
    );
    // Schema 2 uses the rotten vocabulary for machine `state` names;
    // the `bucket` still carries the stable gating vocabulary: new →
    // new, rotten and resurfaced → rotten.
    let buckets: Vec<Option<&str>> =
        queue.iter().map(|entry| entry["bucket"].as_str()).collect();
    assert_eq!(
        buckets,
        vec![
            Some("new"),
            Some("new"),
            Some("new"),
            Some("rotten"),
            Some("rotten"),
            Some("rotten"),
        ]
    );
    let first = &queue[0];
    assert_eq!(first["rank"], 1);
    assert_eq!(first["tier"], "new");
    assert_eq!(first["path"], "a.md");
    assert_eq!(first["line"], 1);
    assert_eq!(first["text"], "New capture");
    assert_eq!(first["created"], "2026-09-30");
    assert!(first["fresh"].is_null());
    assert_eq!(first["interval"], 7);
    assert_eq!(first["interval_source"], "default");
    assert!(first["due_on"].is_null());

    let rotten = &queue[3];
    assert_eq!(rotten["path"], "a.md");
    assert_eq!(rotten["line"], 2);
    assert_eq!(rotten["fresh"], "2026-09-20");
    assert_eq!(rotten["due_on"], "2026-09-27");
    assert_eq!(rotten["days_overdue"], 11);

    let resurfaced = &queue[4];
    assert_eq!(resurfaced["state"], "resurfaced");
    assert_eq!(resurfaced["bucket"], "rotten");
    assert_eq!(resurfaced["due_on"], "2026-10-07");
    assert_eq!(resurfaced["days_overdue"], 1);

    let note_task = &queue[5];
    assert_eq!(note_task["path"], "b.md");
    assert_eq!(note_task["interval"], 3);
    assert_eq!(note_task["interval_source"], "note");

    // Slow note task (task interval 14) is FRESH, so absent.
    assert!(
        !queue.iter().any(|entry| entry["text"] == "Slow note task"),
        "task interval must beat the note interval:\n{value}"
    );
    // Out-of-scope tasks never queue.
    for missing in [
        "Hidden chore",
        "Water plants",
        "Waiting",
        "Next thing",
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
            && human.contains("REVIEW 6 due")
            && human.contains("3 new")
            && human.contains("1 resurfaced")
            && human.contains("2 rotten")
            && human.contains("✓ 2 today"),
        "expected a REVIEW summary:\n{human}"
    );
    assert!(
        human.contains("NEW") && human.contains("DUE"),
        "expected NEW and DUE sections:\n{human}"
    );
    assert!(
        human.contains("LINTS")
            && human.contains("fresh_malformed")
            && human.contains("fresh_future"),
        "expected lints last:\n{human}"
    );
    assert_text_order(&human, &["REVIEW", "NEW", "DUE", "LINTS"]);
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
    // The legacy key still supplies the budget under schema 2.
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
    assert_eq!(value["schema_version"], 2);
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

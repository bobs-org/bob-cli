//! Binary-level recovery and report regressions for direct Snoozed rows.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path, process::Output};

const NOW: &str = "2026-11-05 12:00:00";

fn reconcile(
    temp: &TempDir,
    vault: &Path,
    daily: &Path,
    dry_run: bool,
) -> Output {
    let mut command = bob_command();
    command
        .args(["task", "reconcile", "--format", "json", "-r", "0"])
        .arg("--bob-dir")
        .arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .env("BOB_DAY_FILE", daily)
        .env("BOB_CONFIG_FILE", vault.join("bob.yml"))
        .env("BOB_NOW", NOW)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_VAULT_SYNC_LOCK_FILE", temp.path().join("vault.lock"))
        .output()
        .expect("run task reconcile")
}

fn report(output: &Output) -> Value {
    assert_success(output);
    serde_json::from_str(stdout(output).trim()).expect("reconcile JSON")
}

fn task_line(contents: &str, block_id: &str) -> String {
    contents
        .lines()
        .find(|line| {
            line.starts_with("- [") && line.contains(&format!("^{block_id}"))
        })
        .unwrap_or_else(|| panic!("missing task ^{block_id}:\n{contents}"))
        .to_string()
}

#[test]
fn snoozed_daily_next_gets_grace_and_hooks_forecast_matches_plan() {
    let temp = TempDir::new("bob-cli-hooks-snoozed-daily-next");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20261105.md");
    let previous = vault.join("2026/20261104.md");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("bob.yml"), "timezone: America/New_York\n");
    write_file(
        &daily,
        concat!(
            "# 2026-11-05\n\n",
            "## Pomodoros\n\n",
            "## Snoozed\n\n",
            "- **1530** [[#^planned]]\n\n",
            "## Tasks\n",
            "- [*] #task Planned Next ^planned\n",
            "- [*] #task No row today ^unplanned\n",
        ),
    );
    // A previous-day row is history for that day and cannot preserve a
    // same-day task's daily Next status.
    let previous_text = concat!(
        "# 2026-11-04\n\n",
        "## Snoozed\n\n",
        "- **1530** [[2026/20261105#^unplanned]]\n",
    );
    write_file(&previous, previous_text);
    let before = fs::read(&daily).expect("daily bytes before dry-run");

    let dry = reconcile(&temp, &vault, &daily, true);
    let hooks = report(&dry);
    assert_eq!(hooks["kept_next"], 1);
    assert!(hooks["cleared"].as_array().unwrap().iter().any(|item| {
        item["path"] == "2026/20261105.md" && item["block_id"] == "unplanned"
    }));
    assert_eq!(hooks["plan_budget"]["links"]["count"], 0);
    assert_eq!(hooks["plan_budget"]["today"]["count"], 0);
    assert_eq!(hooks["plan_budget"]["snoozed"]["count"], 1);
    assert_eq!(hooks["plan_budget"]["snoozed"]["on_clock"], 1);
    assert_eq!(hooks["plan_budget"]["snoozed"]["forecast"]["count"], 1);
    assert_eq!(fs::read(&daily).unwrap(), before, "dry-run writes no bytes");

    let plan = bob_command()
        .args(["plan", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CONFIG_FILE", vault.join("bob.yml"))
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob plan");
    assert_success(&plan);
    let plan: Value = serde_json::from_str(stdout(&plan).trim()).unwrap();
    assert_eq!(hooks["plan_budget"]["snoozed"], plan["snoozed"]);
    assert_eq!(hooks["plan_budget"]["today"], plan["today"]);
    assert_eq!(hooks["plan_budget"]["links"], plan["links"]);

    let applied = report(&reconcile(&temp, &vault, &daily, false));
    let after = fs::read_to_string(&daily).unwrap();
    assert!(task_line(&after, "planned").starts_with("- [*]"));
    assert!(task_line(&after, "unplanned").starts_with("- [ ]"));
    assert_eq!(applied["kept_next"], 1);
    assert!(after.contains("- **1530** [[#^planned]]"));
    assert_eq!(fs::read_to_string(previous).unwrap(), previous_text);
}

#[test]
fn snoozed_blocked_rows_recover_by_direct_identity_without_promoting_ready_tasks(
) {
    let temp = TempDir::new("bob-cli-hooks-snoozed-blocked-recovery");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20261105.md");
    let previous = vault.join("2026/20261104.md");
    let tasks = vault.join("tasks.md");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("bob.yml"), "timezone: America/New_York\n");
    write_file(
        &daily,
        concat!(
            "# 2026-11-05\n\n",
            "## Pomodoros\n\n",
            "## Snoozed\n\n",
            "- **1300** [[tasks#^snoozed-next]]\n",
            "- **1330** [[tasks#^snoozed-strong]]\n",
        ),
    );
    write_file(
        &previous,
        concat!(
            "# 2026-11-04\n\n",
            "## Pomodoros\n\n",
            "- [x] (**0900-1000**) — WORK\n",
            "  - [[tasks#^strong-root]]\n",
        ),
    );
    write_file(
        &tasks,
        concat!(
            "- [/] #task Stronger recent root ^strong-root\n",
            "  - ⛓️ **DEPENDS ON:** [[#^snoozed-strong]]\n",
            "- [?] #task Snoozed Next [dependsOn:: tasks__next-pre] ^snoozed-next\n",
            "  - ⛓️ **DEPENDS ON:** [[#^next-pre]]\n",
            "- [ ] #task Next prerequisite ^next-pre\n",
            "- [?] #task Snoozed stronger [dependsOn:: tasks__strong-pre] ^snoozed-strong\n",
            "  - ⛓️ **DEPENDS ON:** [[#^strong-pre]]\n",
            "- [ ] #task Strong prerequisite ^strong-pre\n",
        ),
    );

    let dry = report(&reconcile(&temp, &vault, &daily, true));
    assert!(dry["unblocked"].as_array().unwrap().is_empty());
    assert!(dry["marked_next"].as_array().unwrap().iter().all(|item| {
        item["block_id"] != "next-pre" && item["block_id"] != "strong-pre"
    }));
    let before = fs::read_to_string(&tasks).unwrap();
    assert!(task_line(&before, "next-pre").starts_with("- [ ]"));
    assert!(task_line(&before, "strong-pre").starts_with("- [ ]"));

    // Remove the independent blocking reasons before the first live run so
    // the previous day's In Progress root remains available as a stronger
    // recovery rank for its dependency.
    let opened = before
        .replace(
            "- [ ] #task Next prerequisite",
            "- [x] #task Next prerequisite",
        )
        .replace(
            "- [ ] #task Strong prerequisite",
            "- [x] #task Strong prerequisite",
        );
    write_file(&tasks, &opened);
    let recovered = report(&reconcile(&temp, &vault, &daily, false));
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        task_line(&after, "snoozed-next").starts_with("- [*]"),
        "{after}"
    );
    assert!(
        task_line(&after, "snoozed-strong").starts_with("- [/]"),
        "stronger In Progress recovery wins:\n{after}"
    );
    assert!(recovered["unblocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "snoozed-next" && item["to"] == "*"
        }));
    assert!(recovered["unblocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "snoozed-strong" && item["to"] == "/"
        }));
    assert!(task_line(&after, "next-pre").starts_with("- [x]"));
    assert!(task_line(&after, "strong-pre").starts_with("- [x]"));
}

#[test]
fn snoozed_project_and_inherited_blocks_stay_blocked_then_recover_to_next() {
    let temp = TempDir::new("bob-cli-hooks-snoozed-project-recovery");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20261105.md");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("bob.yml"), "timezone: America/New_York\n");
    write_file(
        &daily,
        concat!(
            "# 2026-11-05\n\n",
            "## Pomodoros\n\n",
            "## Snoozed\n\n",
            "- **1300** [[child#^child-resident]]\n",
            "- **1330** [[own#^own-resident]]\n",
        ),
    );
    let parent = concat!(
        "---\n",
        "type: [[project]]\n",
        "status: wip\n",
        "---\n",
        "- [?] #task #prj Parent [scheduled:: 2026-11-10] ^prj\n",
        "- [?] #task #prj_is_blocked Parent resident ^parent-resident\n",
    );
    let child = concat!(
        "---\n",
        "type: [[project]]\n",
        "status: wip\n",
        "parent: [[parent]]\n",
        "---\n",
        "- [?] #task #prj #prj_is_blocked Child ^prj\n",
        "- [?] #task #prj_is_blocked Child resident ^child-resident\n",
    );
    let own = concat!(
        "---\n",
        "type: [[project]]\n",
        "status: wip\n",
        "---\n",
        "- [?] #task #prj Own [scheduled:: 2026-11-10] ^prj\n",
        "- [?] #task #prj_is_blocked Own resident ^own-resident\n",
    );
    write_file(&vault.join("parent.md"), parent);
    write_file(&vault.join("child.md"), child);
    write_file(&vault.join("own.md"), own);

    let before_parent = fs::read_to_string(vault.join("parent.md")).unwrap();
    let before_child = fs::read_to_string(vault.join("child.md")).unwrap();
    let before_own = fs::read_to_string(vault.join("own.md")).unwrap();
    let dry = report(&reconcile(&temp, &vault, &daily, true));
    assert!(dry["unblocked"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(vault.join("parent.md")).unwrap(),
        before_parent
    );
    assert_eq!(
        fs::read_to_string(vault.join("child.md")).unwrap(),
        before_child
    );
    assert_eq!(
        fs::read_to_string(vault.join("own.md")).unwrap(),
        before_own
    );

    for path in [vault.join("parent.md"), vault.join("own.md")] {
        let contents = fs::read_to_string(&path).unwrap();
        write_file(&path, &contents.replace("[scheduled:: 2026-11-10]", ""));
    }
    let recovered = report(&reconcile(&temp, &vault, &daily, false));
    let child_after = fs::read_to_string(vault.join("child.md")).unwrap();
    let own_after = fs::read_to_string(vault.join("own.md")).unwrap();
    assert!(task_line(&child_after, "child-resident").starts_with("- [*]"));
    assert!(task_line(&own_after, "own-resident").starts_with("- [*]"));
    assert!(
        !task_line(&child_after, "child-resident").contains("#prj_is_blocked")
    );
    assert!(!task_line(&own_after, "own-resident").contains("#prj_is_blocked"));
    for block_id in ["child-resident", "own-resident"] {
        assert!(recovered["unblocked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["block_id"] == block_id && item["to"] == "*" }));
    }
}

#[test]
fn ignored_event_ambiguous_and_history_rows_do_not_seed_recovery() {
    let temp = TempDir::new("bob-cli-hooks-snoozed-exclusions");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20261105.md");
    let tasks = vault.join("tasks.md");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("bob.yml"), "timezone: America/New_York\n");
    write_file(
        &daily,
        concat!(
            "# 2026-11-05\n\n",
            "## Events\n",
            "- [?] #task **1300-1330** Meeting ^event\n\n",
            "## Pomodoros\n\n",
            "## Snoozed\n",
            "- **1000** [[tasks#^blocked]]\n",
            "- **1100** [[tasks#^blocked]]\n",
            "- **1100** [[tasks#^done]]\n",
            "- **1100** [[tasks#^cancelled]]\n",
            "- **1100** [[#^event]]\n",
            "- **1100** [[jobs#^ambiguous]]\n",
            "- ~~**0900** [[tasks#^history]]~~ · woke 09:05\n",
            "\n## Tasks\n",
        ),
    );
    write_file(
        &tasks,
        concat!(
            "- [?] #task Blocked Snoozed ^blocked\n",
            "- [x] #task Done ^done\n",
            "- [-] #task Cancelled ^cancelled\n",
            "- [?] #task History row ^history\n",
        ),
    );
    write_file(
        &vault.join("one/jobs.md"),
        "- [?] #task Ambiguous one ^ambiguous\n",
    );
    write_file(
        &vault.join("two/jobs.md"),
        "- [?] #task Ambiguous two ^ambiguous\n",
    );

    let list = bob_command()
        .args(["snooze", "list", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CONFIG_FILE", vault.join("bob.yml"))
        .env("BOB_NOW", NOW)
        .output()
        .expect("classify Snoozed inclusion and exclusion rows");
    assert_success(&list);
    let list: Value = serde_json::from_str(stdout(&list).trim()).unwrap();
    let row_reason = |id: &str| {
        list["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["block_id"] == id)
            .unwrap_or_else(|| panic!("missing Snoozed row ^{id}: {list}"))
    };
    assert_eq!(row_reason("blocked")["state"], "blocked");
    assert_eq!(row_reason("done")["reason"], "done");
    assert_eq!(row_reason("cancelled")["reason"], "cancelled");
    assert_eq!(row_reason("event")["reason"], "event_task");
    assert_eq!(row_reason("ambiguous")["reason"], "ambiguous");
    assert_eq!(row_reason("history")["state"], "woken");
    assert!(list["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |row| row["block_id"] == "blocked" && row["reason"] == "duplicate"
        ));

    let before = fs::read_to_string(&tasks).unwrap();
    let dry = report(&reconcile(&temp, &vault, &daily, true));
    assert_eq!(dry["plan_budget"]["snoozed"]["count"], 1);
    let dry_unblocked = dry["unblocked"].as_array().unwrap();
    for id in ["history", "ambiguous"] {
        assert!(dry_unblocked
            .iter()
            .any(|item| { item["block_id"] == id && item["to"] == " " }));
    }
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);

    let first = report(&reconcile(&temp, &vault, &daily, false));
    let after_first = fs::read_to_string(&tasks).unwrap();
    assert!(
        task_line(&after_first, "blocked").starts_with("- [*]"),
        "{after_first}"
    );
    assert!(task_line(&after_first, "history").starts_with("- [ ]"));
    for route in ["one/jobs.md", "two/jobs.md"] {
        let ambiguous = fs::read_to_string(vault.join(route)).unwrap();
        assert!(ambiguous.starts_with("- [ ]"), "{route}: {ambiguous}");
    }
    assert!(task_line(&after_first, "done").starts_with("- [x]"));
    assert!(task_line(&after_first, "cancelled").starts_with("- [-]"));
    assert!(
        !first["marked_next"].as_array().unwrap().iter().any(|item| {
            matches!(
                item["block_id"].as_str(),
                Some("dep" | "history" | "ambiguous" | "event")
            )
        })
    );

    assert!(first["unblocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| { item["block_id"] == "blocked" && item["to"] == "*" }));
}

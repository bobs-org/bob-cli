//! Blocked status, schedules, recovery guard.

use crate::support::*;
use std::fs;

#[test]
fn task_status_hooks_reconciles_blocked_status_from_dataview_dependencies() {
    let temp = TempDir::new("bob-cli-task-status-hooks-blocked");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let tasks = vault.join("tasks.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Open root [id:: root] ^root\n",
            "- [ ] #task Ready parent [dependsOn:: root] ^ready\n",
            "- [*] #task Next parent [dependsOn:: root] ^next\n",
            "- [/] #task Working parent [dependsOn:: root] ^working\n",
            "- [?] #task Already blocked [dependsOn:: root] ^already\n",
            "- [x] #task Done parent [dependsOn:: root] ^done-parent\n",
            "- [-] #task Canceled parent [dependsOn:: root] ^cancel-parent\n",
            "- [ ] #task Missing dependency [dependsOn:: missing] ^missing\n",
            "- [ ] #task Self dependency [id:: self] [dependsOn:: self] ^self\n",
            "- [ ] #task Parenthesized metadata (id:: paren) (dependsOn:: root) ^paren\n",
            "- [x] #task Duplicate done [id:: duplicate] ^duplicate-done\n",
            "- [ ] #task Duplicate open [id:: duplicate] ^duplicate-open\n",
            "- [ ] #task Duplicate dependent [dependsOn:: duplicate] ^duplicate-parent\n",
            "- [x] #task Done target [id:: closed] ^closed\n",
            "- [ ] #task Closed dependency [dependsOn:: closed] ^closed-parent\n",
            "- [~] #task Non-task target [id:: non-task] ^non-task\n",
            "- [ ] #task Non-task dependency [dependsOn:: non-task] ^non-task-parent\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    let before = fs::read_to_string(&tasks).unwrap();

    let dry_run = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run dependency status reconciliation");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(json["marked_blocked"].as_array().unwrap().len(), 6);
    assert!(json["unblocked"].as_array().unwrap().is_empty());
    assert!(json["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "ready"
                && item["from"] == " "
                && item["to"] == "?"
                && item["open_dependency_ids"] == serde_json::json!(["root"])
        }));
    assert!(json["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "self"
                && item["open_dependency_ids"] == serde_json::json!(["self"])
        }));

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply dependency status reconciliation");
    assert_success(&applied);
    assert!(stdout(&applied).contains("6 blocked, 0 unblocked"));
    let contents = fs::read_to_string(&tasks).unwrap();
    for block_id in [
        "ready",
        "next",
        "working",
        "self",
        "paren",
        "duplicate-parent",
    ] {
        assert!(
            contents.lines().any(|line| line.contains("- [?]")
                && line.ends_with(&format!("^{block_id}"))),
            "missing blocked {block_id}:\n{contents}"
        );
    }
    for expected in [
        "- [x] #task Done parent",
        "- [-] #task Canceled parent",
        "- [ ] #task Missing dependency",
        "- [ ] #task Closed dependency",
        "- [ ] #task Non-task dependency",
    ] {
        assert!(
            contents.contains(expected),
            "missing {expected}:\n{contents}"
        );
    }

    let second = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun dependency status reconciliation");
    assert_success(&second);
    assert!(stdout(&second).contains("already in sync, no changes"));
}

#[test]
fn task_status_hooks_reconciles_future_schedules_and_combined_blocking_reasons()
{
    let temp = TempDir::new("bob-cli-task-status-hooks-scheduled-blocked");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let tasks = vault.join("tasks.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &tasks,
        concat!(
            "---\n",
            "type: [[project]]\n",
            "scheduled: 2099-01-01\n",
            "---\n",
            "- [ ] #task Open root [id:: root] ^root\n",
            "- [x] #task Closed root [id:: closed] ^closed\n",
            "- [ ] #task Yesterday [scheduled:: 2026-07-15] ^yesterday\n",
            "- [ ] #task Today [scheduled:: 2026-07-16] ^today\n",
            "- [ ] #task Future ready [scheduled:: 2026-07-17] ^future-ready\n",
            "- [*] #task Future next (scheduled:: 2026-07-18) ^future-next\n",
            "- [/] #task Future working [scheduled::   2026-07-19  ] ^future-working\n",
            "- [?] #task Already future [scheduled:: 2026-07-20] ^already\n",
            "- [x] #task Done future [scheduled:: 2026-07-20] ^done\n",
            "- [!] #task Unknown future [scheduled:: 2026-07-20] ^unknown\n",
            "- [ ] #task Impossible [scheduled:: 2026-02-30] ^impossible\n",
            "- [ ] #task Dependency only [dependsOn:: root] ^dependency\n",
            "- [ ] #task Combined [dependsOn:: root] [scheduled:: 2026-07-21] ^combined\n",
            "- [?] #task Mature but still dependent [dependsOn:: root] [scheduled:: 2026-07-16] ^still-dependent\n",
            "- [ ] #task Future with closed dependency [dependsOn:: closed] [scheduled:: 2026-07-22] ^future-closed\n",
            "- [?] #task Mature recovery [scheduled:: 2026-07-16] ^recover\n",
            "- [ ] #task Project inline schedule [scheduled:: 2026-07-23] ^prj\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    let before = fs::read_to_string(&tasks).unwrap();

    let dry_run = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2030-12-31 23:59:59")
        .output()
        .expect("dry-run scheduled status reconciliation");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(json["marked_blocked"].as_array().unwrap().len(), 7);
    assert_eq!(json["unblocked"].as_array().unwrap().len(), 1);

    let marked = json["marked_blocked"].as_array().unwrap();
    let scheduled_only = marked
        .iter()
        .find(|item| item["block_id"] == "future-ready")
        .expect("future schedule change");
    assert_eq!(scheduled_only["future_scheduled_date"], "2026-07-17");
    assert_eq!(scheduled_only["open_dependency_ids"], serde_json::json!([]));
    let dependency_only = marked
        .iter()
        .find(|item| item["block_id"] == "dependency")
        .expect("dependency-only change");
    assert!(dependency_only["future_scheduled_date"].is_null());
    assert_eq!(
        dependency_only["open_dependency_ids"],
        serde_json::json!(["root"])
    );
    let combined = marked
        .iter()
        .find(|item| item["block_id"] == "combined")
        .expect("combined change");
    assert_eq!(combined["future_scheduled_date"], "2026-07-21");
    assert_eq!(combined["open_dependency_ids"], serde_json::json!(["root"]));
    assert!(json["unblocked"][0]["future_scheduled_date"].is_null());
    assert_eq!(json["unblocked"][0]["block_id"], "recover");

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2030-12-31 23:59:59")
        .output()
        .expect("apply scheduled status reconciliation");
    assert_success(&applied);
    let report = stdout(&applied);
    assert!(
        report.contains("(local_schedule; scheduled: 2026-07-17)"),
        "{report}"
    );
    assert!(
        report.contains(
            "(local_dependency+local_schedule; scheduled: 2026-07-21; open: root)"
        ),
        "{report}"
    );
    assert!(report.contains("7 blocked, 1 unblocked"), "{report}");

    let contents = fs::read_to_string(&tasks).unwrap();
    for block_id in [
        "future-ready",
        "future-next",
        "future-working",
        "already",
        "dependency",
        "combined",
        "still-dependent",
        "future-closed",
        "prj",
    ] {
        assert!(
            contents.lines().any(|line| line.contains("- [?]")
                && line.ends_with(&format!("^{block_id}"))),
            "missing blocked {block_id}:\n{contents}"
        );
    }
    for expected in [
        "- [ ] #task Yesterday",
        "- [ ] #task Today",
        "- [x] #task Done future",
        "- [!] #task Unknown future",
        "- [ ] #task Impossible",
        "- [ ] #task Mature recovery",
    ] {
        assert!(
            contents.contains(expected),
            "missing {expected}:\n{contents}"
        );
    }

    let second = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun scheduled status reconciliation");
    assert_success(&second);
    assert!(stdout(&second).contains("already in sync, no changes"));
}

#[test]
fn task_status_hooks_unblocks_to_final_pomodoro_rank_and_ready() {
    let temp = TempDir::new("bob-cli-task-status-hooks-unblocked-ranks");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let tasks = vault.join("tasks.md");
    write_file(
        &daily,
        concat!(
            "## Pomodoros\r\n\r\n",
            "- [ ] Current (0900-0930)\r\n",
            "  - [[tasks#^next]]\r\n",
            "  - [[tasks#^seed]]\r\n",
        ),
    );
    write_file(
        &tasks,
        concat!(
            "- [?] #task Return next [dependsOn:: done] [scheduled:: 2026-07-16] ^next\r\n",
            "- [/] #task Working seed [dependsOn:: working] ^seed\r\n",
            "  - ![[#^working]]\r\n",
            "- [?] #task Return working (dependsOn:: missing) (scheduled:: 2026-07-16) ^working\r\n",
            "- [?] #task Return ready [dependsOn:: done] [scheduled:: 2026-07-16] ^ready\r\n",
            "- [?] #task Return ready without metadata ^no-metadata\r\n",
            "- [x] #task Terminal done [dependsOn:: done] ^terminal-done\r\n",
            "- [-] #task Terminal canceled [dependsOn:: done] ^terminal-canceled\r\n",
            "- [~] #task Terminal non-task [dependsOn:: done] ^terminal-non-task\r\n",
            "- [!] #task Unknown status [dependsOn:: done] ^unknown\r\n",
            "- [x] #task Done dependency [id:: done] ^done\r\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let output = bob_command()
        .args(["task", "reconcile"])
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("unblock dependency statuses");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(json["unblocked"].as_array().unwrap().len(), 4);
    assert!(json["unblocked"].as_array().unwrap().iter().any(|item| {
        item["block_id"] == "working"
            && item["from"] == "?"
            && item["to"] == "/"
            && item["unresolved_dependency_ids"]
                == serde_json::json!(["missing"])
            && item["future_scheduled_date"].is_null()
    }));
    assert!(json["marked_next"].as_array().unwrap().is_empty());
    assert!(json["marked_in_progress"].as_array().unwrap().is_empty());
    let contents = fs::read_to_string(&tasks).unwrap();
    assert!(contents.contains("- [*] #task Return next"));
    assert!(contents.contains("- [/] #task Return working"));
    assert!(contents.contains("- [ ] #task Return ready"));
    assert!(contents.contains("- [ ] #task Return ready without metadata"));
    assert!(contents.contains("- [x] #task Terminal done"));
    assert!(contents.contains("- [-] #task Terminal canceled"));
    assert!(contents.contains("- [~] #task Terminal non-task"));
    assert!(contents.contains("- [!] #task Unknown status"));
    assert!(contents.contains("\r\n"));
}

#[test]
fn task_status_hooks_uses_recent_ledgers_only_for_blocked_recovery() {
    let temp =
        TempDir::new("bob-cli-task-status-hooks-recovery-only-recent-rank");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let previous = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    write_file(
        &daily,
        concat!(
            "## Pomodoros\n\n",
            "- [x] Completed (0900-0930)\n",
            "  - [[tasks#^current|alias]]\n",
            "  - ![[tasks#^root]]\n",
        ),
    );
    let previous_before = concat!(
        "## Pomodoros\r\n\r\n",
        "- [x] Previous (0800-0830)\r\n",
        "  - ![[tasks#^previous]]\r\n",
        "  - [[tasks#^ordinary]]\r\n",
        "  - [[tasks#^future]]\r\n",
        "  - [[tasks#^dependent]]\r\n",
    );
    write_file(&previous, previous_before);
    write_file(
        &tasks,
        concat!(
            "- [?] #task Current completed reference [scheduled:: 2026-07-16] ^current\n",
            "- [?] #task Previous direct reference [scheduled:: 2026-07-15] ^previous\n",
            "- [?] #task No recent reference ^ready\n",
            "- [?] #task Root [scheduled:: 2026-07-16] [dependsOn:: working] ^root\n",
            "  - ![[#^working]]\n",
            "- [/] #task Stronger intermediate [dependsOn:: graph] ^working\n",
            "  - ![[#^graph]]\n",
            "- [?] #task Graph-derived recovery ^graph\n",
            "- [ ] #task Ordinary previous reference ^ordinary\n",
            "- [?] #task Future still blocks [scheduled:: 2026-07-17] ^future\n",
            "- [ ] #task Open dependency [id:: open] ^open\n",
            "- [?] #task Dependency still blocks [dependsOn:: open] ^dependent\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    let before = fs::read_to_string(&tasks).unwrap();

    let dry_run = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("preview recovery-only recent ranks");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(json["previous_daily_file"], "2026/20260710.md");
    assert_eq!(json["unblocked"].as_array().unwrap().len(), 5);
    assert!(json["marked_next"].as_array().unwrap().is_empty());
    assert!(json["marked_in_progress"].as_array().unwrap().is_empty());

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply recovery-only recent ranks");
    assert_success(&applied);
    assert_eq!(fs::read_to_string(&previous).unwrap(), previous_before);
    let contents = fs::read_to_string(&tasks).unwrap();
    assert!(contents.contains("- [*] #task Current completed reference"));
    assert!(contents.contains("- [*] #task Previous direct reference"));
    assert!(contents.contains("- [ ] #task No recent reference"));
    assert!(contents.contains("- [*] #task Root"));
    assert!(contents.contains("- [/] #task Graph-derived recovery"));
    assert!(contents.contains("- [ ] #task Ordinary previous reference"));
    assert!(contents.contains("- [?] #task Future still blocks"));
    assert!(contents.contains("- [?] #task Dependency still blocks"));

    let second = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun recovery-only recent ranks");
    assert_success(&second);
    assert!(stdout(&second).contains("already in sync, no changes"));
}

#[test]
fn task_status_hooks_blocked_status_guard_writes_nothing() {
    let scenarios = [
        ("missing", None),
        (
            "duplicate",
            Some(blocked_tasks_settings_json(concat!(
                ", {\"symbol\":\"?\",\"name\":\"Blocked\",",
                "\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,",
                "\"type\":\"ON_HOLD\"}",
            ))),
        ),
        (
            "incompatible",
            Some(blocked_tasks_settings_json("").replace(
                "\"nextStatusSymbol\":\" \"",
                "\"nextStatusSymbol\":\"x\"",
            )),
        ),
    ];
    for (name, settings) in scenarios {
        let temp = TempDir::new(&format!("bob-cli-blocked-guard-{name}"));
        let vault = temp.path().join("vault");
        let daily = vault.join("2026/20260716.md");
        let tasks = vault.join("tasks.md");
        let daily_before = "## Pomodoros\n\n- [ ] Current (0900-0930)\n";
        let tasks_before = concat!(
            "- [ ] #task Root [id:: root] ^root\n",
            "- [ ] #task Parent [dependsOn:: root] ^parent\n",
        );
        write_file(&daily, daily_before);
        write_file(&tasks, tasks_before);
        if let Some(settings) = settings {
            write_file(
                &vault
                    .join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
                &settings,
            );
        }

        let output = bob_command()
            .args(["task", "reconcile"])
            .arg("--bob-dir")
            .arg(&vault)
            .env("BOB_DAY_FILE", &daily)
            .output()
            .expect("run Blocked status guard");
        assert_eq!(
            output.status.code(),
            Some(1),
            "{name}: {}",
            format_output(&output)
        );
        assert!(stderr(&output).contains("cannot reconcile Blocked [?] tasks"));
        assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
        assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_before);
    }
}

// Project blocking from the `^prj` line (`docs/task-dependencies.md`
// §13.3, §13.7, PDF2): while `^prj` has an open prerequisite, its
// residents are Blocked and tagged; when the prerequisite closes they
// recover, lose the tag, and stamped residents gain `[unblocked::]`.
#[test]
fn reconcile_derives_project_blocking_tag_and_unblocked() {
    let temp = TempDir::new("bob-cli-task-status-hooks-project-blocking");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let proj = vault.join("proj.md");
    let tasks = vault.join("tasks.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &proj,
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Ship the thing ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
            "- [ ] #task Resident one [fresh:: 2026-07-16] ^r1\n",
            "- [ ] #task Resident two [fresh:: 2026-07-16] ^r2\n",
        ),
    );
    write_file(&tasks, "- [ ] #task Prerequisite work ^pre\n");
    write_blocked_tasks_settings(&vault);

    let dry_run = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run project blocking");
    assert_success(&dry_run);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    // `^prj` blocks on its own reason only (PDB2).
    assert!(json["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "prj"
                && item["from"] == " "
                && item["to"] == "?"
                && item["reason_kinds"]
                    == serde_json::json!(["local_dependency"])
                && item["project_path"].is_null()
        }));
    // Residents block on the inherited project reason (PDB1).
    for block_id in ["r1", "r2"] {
        assert!(json["marked_blocked"].as_array().unwrap().iter().any(
            |item| {
                item["block_id"] == block_id
                    && item["reason_kinds"]
                        == serde_json::json!(["project_dependency"])
                    && item["project_path"] == "proj.md"
                    && item["project_open_dependency_ids"]
                        == serde_json::json!(["tasks__pre"])
            }
        ));
    }
    assert_eq!(
        json["prj_is_blocked_tags_added"].as_array().unwrap().len(),
        2
    );
    assert!(json["prj_is_blocked_tags_stripped"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(json["unblocked_fields_written"]
        .as_array()
        .unwrap()
        .is_empty());

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply project blocking");
    assert_success(&applied);
    let contents = fs::read_to_string(&proj).unwrap();
    assert!(
        contents.contains(
            "- [?] #task Resident one [fresh:: 2026-07-16] #prj_is_blocked ^r1"
        ),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [?] #task Resident two [fresh:: 2026-07-16] #prj_is_blocked ^r2"
        ),
        "{contents}"
    );
    assert!(
        contents.lines().any(|line| line.contains("^prj")
            && line.contains("[dependsOn:: tasks__pre]")),
        "{contents}"
    );

    // The prerequisite closes: everyone recovers, tags strip, and only
    // the stamped residents gain `[unblocked::]` (the unstamped `^prj`
    // does not).
    write_file(&tasks, "- [x] #task Prerequisite work ^pre\n");
    let recovered = bob_command()
        .args(["task", "reconcile"])
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply project recovery");
    assert_success(&recovered);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&recovered).trim()).unwrap();
    assert_eq!(
        json["prj_is_blocked_tags_stripped"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let written = json["unblocked_fields_written"].as_array().unwrap();
    assert_eq!(written.len(), 2);
    assert!(
        written.iter().all(|item| item["date"] == "2026-07-16"),
        "{written:?}"
    );
    let contents = fs::read_to_string(&proj).unwrap();
    assert!(!contents.contains("#prj_is_blocked"), "{contents}");
    assert!(
        contents.contains("[unblocked:: 2026-07-16] ^r1"),
        "{contents}"
    );
    assert!(
        contents.contains("[unblocked:: 2026-07-16] ^r2"),
        "{contents}"
    );

    // Settled: the next run changes nothing (PDR10).
    let rerun = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun settled project");
    assert_success(&rerun);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&rerun).trim()).unwrap();
    for key in [
        "marked_blocked",
        "unblocked",
        "prj_is_blocked_tags_added",
        "prj_is_blocked_tags_stripped",
        "unblocked_fields_written",
        "unblocked_fields_pruned",
        "consumed_blocked_references",
    ] {
        assert_eq!(json[key], serde_json::json!([]), "key {key}");
    }
}

// PDR7: a `^prj` link to a task in its own note stays on the line,
// never projects into `[dependsOn::]`, and warns.
#[test]
fn reconcile_keeps_prj_internal_links_unprojected() {
    let temp = TempDir::new("bob-cli-task-status-hooks-prj-internal");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let proj = vault.join("proj.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &proj,
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Ship the thing ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[#^local]]\n",
            "- [ ] #task Local work ^local\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let dry_run = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run prj internal link");
    assert_success(&dry_run);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert!(
        json["dependency_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning["kind"] == "project_internal_dependency"
                    && warning["path"] == "proj.md"
            }),
        "{}",
        stdout(&dry_run)
    );
    assert!(
        json["marked_blocked"].as_array().unwrap().is_empty(),
        "{}",
        stdout(&dry_run)
    );
    let contents = fs::read_to_string(&proj).unwrap();
    assert!(contents.contains("[[#^local]]"), "{contents}");
    assert!(!contents.contains("[dependsOn::"), "{contents}");
}

// PDB7: a future inline `^prj` date blocks `^prj` on its own schedule
// and residents on the inherited project schedule, with the tag.
#[test]
fn reconcile_derives_project_schedule_blocking() {
    let temp = TempDir::new("bob-cli-task-status-hooks-project-schedule");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let proj = vault.join("proj.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &proj,
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Ship the thing [scheduled:: 2026-07-20] ^prj\n",
            "- [ ] #task Resident work ^r1\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let dry_run = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run project schedule blocking");
    assert_success(&dry_run);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert!(json["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "prj"
                && item["reason_kinds"] == serde_json::json!(["local_schedule"])
        }));
    assert!(json["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "r1"
                && item["reason_kinds"]
                    == serde_json::json!(["project_schedule"])
                && item["project_path"] == "proj.md"
                && item["project_scheduled_date"] == "2026-07-20"
        }));
    assert_eq!(
        json["prj_is_blocked_tags_added"].as_array().unwrap().len(),
        1
    );
}

// PDB4: a terminal project is not `project(T)` — residents keep their
// status and lose the tag.
#[test]
fn reconcile_ignores_terminal_projects() {
    let temp = TempDir::new("bob-cli-task-status-hooks-terminal-project");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let proj = vault.join("proj.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &proj,
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: done\n",
            "---\n",
            "- [?] #task #prj Ship the thing [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
            "- [ ] #task Resident work #prj_is_blocked ^r1\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Prerequisite ^pre\n");
    write_blocked_tasks_settings(&vault);

    let dry_run = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run terminal project");
    assert_success(&dry_run);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert!(
        json["marked_blocked"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["block_id"] != "r1"),
        "{}",
        stdout(&dry_run)
    );
    assert_eq!(
        json["prj_is_blocked_tags_stripped"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

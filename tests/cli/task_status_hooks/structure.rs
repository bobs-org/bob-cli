//! Empty pomodoros, guard rails, archive refs, in-place strike.

use crate::support::*;
use std::fs;

#[test]
fn task_status_hooks_removes_empty_pomodoros_and_reports_them() {
    let temp = TempDir::new("bob-cli-task-status-hooks-empty-pomodoros");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260717.md");
    let tasks = vault.join("tasks.md");
    let daily_before = concat!(
        "# Daily\n\n",
        "## Pomodoros\n\n",
        "- [ ] () — GTD\n",
        "  - [[tasks#^keep]]\n",
        "- [ ] () — GTD\n",
        "- [ ] Current (0900-0930)\n",
        "  - current child without a link\n",
        "- [x] Completed (0800-0830)\n",
        "  - completed child without a link\n",
    );
    let daily_after = concat!(
        "# Daily\n\n",
        "## Pomodoros\n\n",
        "- [ ] () — GTD\n",
        "  - [[tasks#^keep]]\n",
        "- [ ] Current (0900-0930)\n",
        "  - current child without a link\n",
        "- [x] Completed (0800-0830)\n",
        "  - completed child without a link\n",
    );
    write_file(&daily, daily_before);
    write_file(&tasks, "- [*] #task Keep ^keep\n");

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run empty Pomodoro cleanup");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    let json: serde_json::Value = serde_json::from_str(stdout(&dry_run).trim())
        .expect("empty Pomodoro dry-run JSON");
    assert_eq!(json["open_pomodoros"], 3);
    assert_eq!(json["references"], 1);
    assert_eq!(
        json["removed_empty_pomodoros"],
        serde_json::json!([
            {"line_number": 7, "line": "- [ ] () — GTD"}
        ])
    );

    let human_dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("human dry-run empty Pomodoro cleanup");
    assert_success(&human_dry_run);
    assert!(
        stdout(&human_dry_run).contains("would remove empty Pomodoros"),
        "unexpected empty Pomodoro dry-run report:\n{}",
        format_output(&human_dry_run)
    );
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply empty Pomodoro cleanup");
    assert_success(&applied);
    assert!(
        stdout(&applied).contains("removed empty Pomodoros")
            && stdout(&applied).contains("1 empty Pomodoros removed")
            && stdout(&applied).contains("recovery copies:"),
        "unexpected empty Pomodoro live report:\n{}",
        format_output(&applied)
    );
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_after);

    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun empty Pomodoro cleanup");
    assert_success(&second);
    assert!(
        stdout(&second).contains("already in sync, no changes"),
        "expected idempotent no-op:\n{}",
        format_output(&second)
    );
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_after);
}

#[test]
fn task_status_hooks_resolves_duplicate_fragments_by_explicit_note_path() {
    let temp = TempDir::new("bob-cli-task-status-hooks-duplicate-fragments");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260711.md");
    write_file(
        &daily,
        "# Daily\n\n## Pomodoros\n\n- [ ] Open session (0900-0930)\n  - [[Root#^root]]\n",
    );
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("Root.md"),
        "- [ ] #task Root [dependsOn:: Alpha__dep] ^root\n  - ![[Alpha#^dep]]\n",
    );
    write_file(
        &vault.join("Alpha.md"),
        "- [ ] #task Alpha [id:: Alpha__dep] ^dep\n",
    );
    write_file(&vault.join("Beta.md"), "- [ ] #task Beta ^dep\n");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("mark next duplicate fragments");
    assert_success(&output);
    // The explicit note path still selects Alpha over Beta for the edge,
    // while the field-covered open prerequisite derives Blocked on Root.
    assert!(fs::read_to_string(vault.join("Root.md"))
        .unwrap()
        .contains("- [?] #task Root [dependsOn:: Alpha__dep] ^root"));
    assert!(fs::read_to_string(vault.join("Alpha.md"))
        .unwrap()
        .contains("- [*] #task Alpha [id:: Alpha__dep] ^dep"));
    assert!(fs::read_to_string(vault.join("Beta.md"))
        .unwrap()
        .contains("- [ ] #task Beta ^dep"));
}

#[test]
fn task_status_hooks_guard_rails_leave_tasks_unchanged() {
    let temp = TempDir::new("bob-cli-task-status-hooks-guards");
    let vault = temp.path().join("vault");
    let task_file = vault.join("tasks.md");
    let missing_daily = vault.join("missing.md");
    write_file(&task_file, "- [*] #task Must remain next ^keep\n");

    let missing = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &missing_daily)
        .output()
        .expect("run with missing daily note");
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("daily note does not exist"));
    assert_eq!(
        fs::read_to_string(&task_file).unwrap(),
        "- [*] #task Must remain next ^keep\n"
    );

    let malformed_daily = vault.join("malformed.md");
    write_file(&malformed_daily, "# Daily note\n\nNo ledger here.\n");
    let malformed = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &malformed_daily)
        .output()
        .expect("run with malformed daily note");
    assert_eq!(malformed.status.code(), Some(1));
    assert!(stderr(&malformed).is_empty());
    let json: serde_json::Value =
        serde_json::from_str(stdout(&malformed).trim()).expect("guard JSON");
    assert_eq!(json["ok"], false);
    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("has no Pomodoros section"));
    assert_eq!(json["plan_budget"], serde_json::Value::Null);
    assert_eq!(
        fs::read_to_string(&task_file).unwrap(),
        "- [*] #task Must remain next ^keep\n"
    );

    let multiple_current = vault.join("multiple-current.md");
    write_file(
        &multiple_current,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] First (0900-0930)\n",
            "  - child\n",
            "- [ ] Second (0930-1000)\n",
            "  - child\n",
        ),
    );
    let multiple = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &multiple_current)
        .output()
        .expect("run with multiple current Pomodoros");
    assert_eq!(multiple.status.code(), Some(1));
    let multiple_stderr = stderr(&multiple);
    assert!(
        multiple_stderr.contains("multiple open timed Pomodoros"),
        "unexpected multiple-timed report:\n{}",
        format_output(&multiple)
    );
    assert!(
        multiple_stderr.contains("First (line 3, 0900-0930)")
            && multiple_stderr.contains("Second (line 5, 0930-1000)")
            && multiple_stderr.contains("bob capture -- =x"),
        "multiple-timed error should name each entry and suggest =x:\n{}",
        format_output(&multiple)
    );
    assert_eq!(
        fs::read_to_string(&task_file).unwrap(),
        "- [*] #task Must remain next ^keep\n"
    );

    let empty_timed = vault.join("empty-timed.md");
    write_file(
        &empty_timed,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] First (0900-0930)\n",
            "  - [[tasks#^keep]]\n",
            "- [ ] Second (0930-1000)\n",
        ),
    );
    let empty_timed_output = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &empty_timed)
        .output()
        .expect("run with empty timed Pomodoro");
    assert_success(&empty_timed_output);
    let empty_timed_json: serde_json::Value =
        serde_json::from_str(stdout(&empty_timed_output).trim())
            .expect("empty timed JSON");
    assert_eq!(
        empty_timed_json["removed_empty_pomodoros"],
        serde_json::json!([
            {"line_number": 5, "line": "- [ ] Second (0930-1000)"}
        ])
    );
    assert_eq!(
        fs::read_to_string(&task_file).unwrap(),
        "- [*] #task Must remain next ^keep\n"
    );
}

#[test]
fn task_status_hooks_uses_custom_done_status_and_completed_fallback() {
    let temp = TempDir::new("bob-cli-task-status-hooks-custom-done");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let settings =
        vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    write_file(
        &daily,
        concat!(
            "## Pomodoros\n\n",
            "- [x] Last completed\n",
            "  - existing completed child\n",
            "- [ ] Future\n",
            "    - Review [[tasks#^custom|custom done]]\n",
            "    - keep future entry\n",
        ),
    );
    write_file(&tasks, "- [D] #task Custom completion ^custom\n");
    write_file(
        &settings,
        r##"{
  "globalFilter": "#task",
  "statusSettings": {
    "customStatuses": [{"symbol": "D", "type": "DONE"}]
  }
}"##,
    );

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("run custom DONE fallback case");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("custom done JSON");
    assert_eq!(
        json["embedded_completed_references"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        json["struck_completed_references"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        json["moved_completed_references"].as_array().unwrap().len(),
        1
    );
    assert_eq!(json["marker_added_references"].as_array().unwrap().len(), 1);
    assert!(json["marker_removed_references"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        fs::read_to_string(&daily).unwrap(),
        concat!(
            "## Pomodoros\n\n",
            "- [x] Last completed\n",
            "  - existing completed child\n",
            "  - Review 🍅 ~~[[tasks#^custom|custom done]]~~\n",
            "- [ ] Future\n",
            "    - keep future entry\n",
        )
    );
}

#[test]
fn task_status_hooks_removes_canceled_open_pomodoro_references() {
    let temp = TempDir::new("bob-cli-task-status-hooks-canceled-links");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let tasks = vault.join("tasks.md");
    let daily_before = concat!(
        "# Daily\n\n",
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930) [[tasks#^standard]]\n",
        "  - standard [[tasks#^standard]]\n",
        "  - live [[tasks#^live]] custom ![[tasks#^custom|alias]] tail\n",
        "  - history 🍅 ~~[[tasks#^standard|old]]~~ and [[tasks#^done]]\n",
        "  - all [[tasks#^all-canceled]]\n",
        "  - mixed [[tasks#^mixed]]\n",
        "  - unresolved [[missing#^nope]]\n",
        "  ```md\n",
        "  - [[tasks#^standard]]\n",
        "  ```\n",
        "- [x] Completed\n",
        "  - 🍅 [[tasks#^standard]]\n",
        "- [-] Canceled\n",
        "  - [[tasks#^custom]]\n",
    );
    let tasks_before = concat!(
        "- [-] #task Standard canceled root ^standard\n",
        "  - ![[#^dependency-only]]\n",
        "- [C] #task Custom canceled root ^custom\n",
        "- [ ] #task Live root ^live\n",
        "- [x] #task Completed task ^done\n",
        "- [-] #task All canceled conventional ^all-canceled\n",
        "- [C] #task All canceled custom ^all-canceled\n",
        "- [-] #task Mixed duplicate canceled ^mixed\n",
        "- [ ] #task Mixed duplicate live ^mixed\n",
        "- [*] #task Reachable only through canceled root ^dependency-only\n",
    );
    write_file(&daily, daily_before);
    write_file(&tasks, tasks_before);
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        &blocked_tasks_settings_json(concat!(
            ",\n      {\"symbol\":\"C\",\"name\":\"Custom canceled\",",
            "\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,",
            "\"type\":\"CANCELLED\"}",
        )),
    );

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run canceled Pomodoro cleanup");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_before);
    let repeated_dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("repeat dry-run canceled Pomodoro cleanup");
    assert_success(&repeated_dry_run);
    assert_eq!(stdout(&repeated_dry_run), stdout(&dry_run));

    let json: serde_json::Value = serde_json::from_str(stdout(&dry_run).trim())
        .expect("canceled cleanup dry-run JSON");
    assert_eq!(json["references"], 7);
    assert_eq!(json["dependency_references"], 0);
    assert_eq!(json["marked_next"].as_array().unwrap().len(), 1);
    assert!(json["cleared"].as_array().unwrap().is_empty());
    assert_eq!(
        json["removed_canceled_references"],
        serde_json::json!([
            {
                "target": "tasks",
                "block_id": "standard",
                "line_number": 6,
                "pomodoro": "- [ ] Current (0900-0930) [[tasks#^standard]]"
            },
            {
                "target": "tasks",
                "block_id": "custom",
                "line_number": 7,
                "pomodoro": "- [ ] Current (0900-0930) [[tasks#^standard]]"
            },
            {
                "target": "tasks",
                "block_id": "standard",
                "line_number": 8,
                "pomodoro": "- [ ] Current (0900-0930) [[tasks#^standard]]"
            },
            {
                "target": "tasks",
                "block_id": "all-canceled",
                "line_number": 9,
                "pomodoro": "- [ ] Current (0900-0930) [[tasks#^standard]]"
            }
        ])
    );
    assert!(json["unresolved_references"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["block_id"] == "mixed"
            && item["reason"]
                .as_str()
                .unwrap()
                .contains("canceled-reference list-item removal was skipped")));

    let human_dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("human dry-run canceled Pomodoro cleanup");
    assert_success(&human_dry_run);
    assert!(
        stdout(&human_dry_run).contains(
            "would remove list items containing canceled task references"
        ) && stdout(&human_dry_run).contains("4 canceled-reference triggers"),
        "unexpected canceled cleanup dry-run report:\n{}",
        format_output(&human_dry_run)
    );
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_before);

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply canceled Pomodoro cleanup");
    assert_success(&applied);
    assert!(
        stdout(&applied)
            .contains("removed list items containing canceled task references")
            && stdout(&applied).contains("4 canceled-reference triggers"),
        "unexpected canceled cleanup report:\n{}",
        format_output(&applied)
    );
    assert_eq!(
        fs::read_to_string(&daily).unwrap(),
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930) [[tasks#^standard]]\n",
            "  - mixed [[tasks#^mixed]]\n",
            "  - unresolved [[missing#^nope]]\n",
            "  ```md\n",
            "  - [[tasks#^standard]]\n",
            "  ```\n",
            "- [x] Completed\n",
            "  - 🍅 [[tasks#^standard]]\n",
            "- [-] Canceled\n",
            "  - [[tasks#^custom]]\n",
        )
    );
    let task_contents = fs::read_to_string(&tasks).unwrap();
    for expected in [
        "- [-] #task Standard canceled root ^standard",
        "- [C] #task Custom canceled root ^custom",
        "- [ ] #task Live root ^live",
        "- [x] #task Completed task ^done",
        "- [-] #task All canceled conventional ^all-canceled",
        "- [C] #task All canceled custom ^all-canceled",
        "- [-] #task Mixed duplicate canceled ^mixed",
        "- [*] #task Mixed duplicate live ^mixed",
        "- [*] #task Reachable only through canceled root ^dependency-only",
    ] {
        assert!(
            task_contents.contains(expected),
            "missing {expected}:\n{task_contents}"
        );
    }

    let daily_after = fs::read_to_string(&daily).unwrap();
    let tasks_after = fs::read_to_string(&tasks).unwrap();
    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun canceled Pomodoro cleanup");
    assert_success(&second);
    let second_json: serde_json::Value =
        serde_json::from_str(stdout(&second).trim()).unwrap();
    assert!(second_json["removed_canceled_references"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(second_json["marked_next"].as_array().unwrap().is_empty());
    assert!(second_json["cleared"].as_array().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_after);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_after);
}

#[test]
fn task_status_hooks_resolves_archive_references_read_only() {
    let temp =
        TempDir::new("bob-cli-task-status-hooks-archive-references-readonly");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260827.md");
    let previous = vault.join("2026/20260826.md");
    let archive = vault.join("done/dev/dev_done.md");
    let daily_before = concat!(
        "## Pomodoros\n\n",
        "- [x] Done (0900-0930)\n",
        "  - ~~[[done/dev/dev_done#^lower-athena-disk-use]]~~\n",
    );
    let previous_before = concat!(
        "## Pomodoros\n\n",
        "- [x] Previous (0900-0930)\n",
        "  - [[done/dev/dev_done#^lower-athena-disk-use]]\n",
    );
    let archive_before =
        "- [x] #task Lower Athena disk use ^lower-athena-disk-use\n";
    write_file(&daily, daily_before);
    write_file(&previous, previous_before);
    write_file(&archive, archive_before);

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run archived Pomodoro references");
    assert_success(&dry_run);
    assert_eq!(
        fs::read_to_string(&daily).unwrap(),
        daily_before,
        "dry-run changed current daily"
    );
    assert_eq!(
        fs::read_to_string(&previous).unwrap(),
        previous_before,
        "dry-run changed previous daily"
    );
    assert_eq!(
        fs::read_to_string(&archive).unwrap(),
        archive_before,
        "dry-run changed archive"
    );
    assert!(
        stderr(&dry_run).is_empty(),
        "unexpected archive warning:\n{}",
        format_output(&dry_run)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(json["scanned_files"], 2);
    assert_eq!(json["previous_daily_file"], "2026/20260826.md");
    assert_eq!(json["previous_daily_references"], 1);
    assert_eq!(json["recent_activity_references"], 1);
    assert!(
        json["unresolved_references"].as_array().unwrap().is_empty(),
        "unexpected unresolved references: {}",
        json["unresolved_references"]
    );

    let human = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("human dry-run archived Pomodoro references");
    assert_success(&human);
    assert!(
        stderr(&human).is_empty(),
        "unexpected human warning:\n{}",
        format_output(&human)
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply archived Pomodoro references");
    assert_success(&applied);
    assert!(
        stderr(&applied).is_empty(),
        "unexpected apply warning:\n{}",
        format_output(&applied)
    );
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&previous).unwrap(), previous_before);
    assert_eq!(fs::read_to_string(&archive).unwrap(), archive_before);

    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun archived Pomodoro references");
    assert_success(&second);
    let second_json: serde_json::Value =
        serde_json::from_str(stdout(&second).trim()).unwrap();
    assert!(second_json["unresolved_references"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&previous).unwrap(), previous_before);
    assert_eq!(fs::read_to_string(&archive).unwrap(), archive_before);
}

#[test]
fn task_status_hooks_normalizes_live_archive_terminal_references() {
    let temp = TempDir::new("bob-cli-task-status-hooks-archive-terminal-links");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260827.md");
    let tasks = vault.join("tasks.md");
    let archive = vault.join("done/dev_done.md");
    let daily_before = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - complete ![[done/dev_done#^archived-done|done alias]]\n",
        "  - cancel [[done/dev_done#^archived-cancel]]\n",
        "    - child removed with canceled reference\n",
        "  - active [[tasks#^active]]\n",
    );
    let tasks_before = "- [ ] #task Active work ^active\n";
    let archive_before = concat!(
        "- [x] #task Archived done ^archived-done\n",
        "- [-] #task Archived canceled ^archived-cancel\n",
    );
    write_file(&daily, daily_before);
    write_file(&tasks, tasks_before);
    write_file(&archive, archive_before);

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run live archived terminal links");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_before);
    assert_eq!(fs::read_to_string(&archive).unwrap(), archive_before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(json["scanned_files"], 2);
    assert!(json["unresolved_references"].as_array().unwrap().is_empty());
    assert_eq!(json["marked_next"].as_array().unwrap().len(), 1);
    assert_eq!(json["marked_next"][0]["path"], "tasks.md");
    assert_eq!(
        json["struck_completed_references"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        json["struck_completed_references"][0]["target"],
        "done/dev_done"
    );
    assert_eq!(
        json["struck_completed_references"][0]["removed_embed"],
        true
    );
    assert_eq!(
        json["removed_canceled_references"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        json["removed_canceled_references"][0]["target"],
        "done/dev_done"
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply live archived terminal links");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&daily).unwrap(),
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - complete ~~[[done/dev_done#^archived-done|done alias]]~~\n",
            "  - active [[tasks#^active]]\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [*] #task Active work ^active\n"
    );
    assert_eq!(fs::read_to_string(&archive).unwrap(), archive_before);

    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun live archived terminal links");
    assert_success(&second);
    let second_json: serde_json::Value =
        serde_json::from_str(stdout(&second).trim()).unwrap();
    assert!(second_json["marked_next"].as_array().unwrap().is_empty());
    assert!(second_json["struck_completed_references"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(second_json["removed_canceled_references"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(second_json["unresolved_references"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(fs::read_to_string(&archive).unwrap(), archive_before);
}

#[test]
fn task_status_hooks_keeps_archive_out_of_active_dependency_sync() {
    let temp =
        TempDir::new("bob-cli-task-status-hooks-archive-dependency-scope");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260827.md");
    let tasks = vault.join("tasks.md");
    let archive = vault.join("done/dev_done.md");
    let daily_before = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - active [[tasks#^root]]\n",
        "  - archived [[done/dev_done#^archived-open]]\n",
        "  - missing [[done/missing#^ghost]]\n",
        "  - invalid [[done/../dev_done#^bad]]\n",
    );
    let tasks_before = "- [ ] #task Active root ^root\n  - ![[done/dev_done#^archived-open]]\n";
    let archive_before = "- [ ] #task Archived open ^archived-open\n";
    write_file(&daily, daily_before);
    write_file(&tasks, tasks_before);
    write_file(&archive, archive_before);

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run archive dependency scope");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&archive).unwrap(), archive_before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(json["scanned_files"], 2);
    assert_eq!(json["marked_next"].as_array().unwrap().len(), 1);
    assert_eq!(json["marked_next"][0]["path"], "tasks.md");
    let unresolved = json["unresolved_references"].as_array().unwrap();
    // Links into done/ are history, never edges and never warned
    // (`docs/task-dependencies.md` §§4.3, 5).
    assert_eq!(
        unresolved.len(),
        2,
        "unexpected unresolved: {unresolved:#?}"
    );
    assert!(unresolved.iter().any(|item| {
        item["target"] == "done/missing"
            && item["block_id"] == "ghost"
            && item["reason"]
                .as_str()
                .unwrap()
                .contains("archive target done/missing.md does not exist")
    }));
    assert!(unresolved.iter().any(|item| {
        item["target"] == "done/../dev_done"
            && item["block_id"] == "bad"
            && item["reason"] == "note target did not resolve uniquely"
    }));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply archive dependency scope");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [*] #task Active root ^root\n  - ![[done/dev_done#^archived-open]]\n"
    );
    assert_eq!(fs::read_to_string(&archive).unwrap(), archive_before);
}

#[test]
fn task_status_hooks_strikes_in_place_when_no_relocation_target_exists() {
    let temp = TempDir::new("bob-cli-task-status-hooks-no-target");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    write_file(
        &daily,
        concat!(
            "## Pomodoros\r\n\r\n",
            "- [ ] Future without a time\r\n",
            "  - [[tasks#^done|finished]]\r\n",
        ),
    );
    write_file(&tasks, "- [X] #task Finished ^done\n");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("run no-target case");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&daily).unwrap(),
        concat!(
            "## Pomodoros\r\n\r\n",
            "- [ ] Future without a time\r\n",
            "  - ~~[[tasks#^done|finished]]~~\r\n",
        )
    );
}

#[test]
fn task_status_hooks_composes_daily_status_and_structural_edits() {
    let temp = TempDir::new("bob-cli-task-status-hooks-daily-composition");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    write_file(
        &daily,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - current child\n",
            "- [ ] Future\n",
            "  - [[tasks#^done]]\n",
            "  - future child\n\n",
            "## Tasks\n\n",
            "- [*] #task Daily orphan ^daily-orphan\n",
        ),
    );
    write_file(&tasks, "- [x] #task Finished ^done\n");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("run composed daily-note edits");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&daily).unwrap(),
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - current child\n",
            "  - ~~[[tasks#^done]]~~\n",
            "- [ ] Future\n",
            "  - future child\n\n",
            "## Tasks\n\n",
            "- [ ] #task Daily orphan ^daily-orphan\n",
        )
    );
}

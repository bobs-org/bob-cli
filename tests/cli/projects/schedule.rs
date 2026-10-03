//! Schedule propagation, dash-blocked parity, per-file errors.

use crate::support::*;
use std::fs;
use std::process::Output;

#[test]
fn projects_sync_propagates_scheduled_task_properties_at_date_boundary() {
    let temp = TempDir::new("bob-cli-projects-scheduled-properties");
    let vault = temp.path().join("vault");
    let project = vault.join("Future.md");
    let original = "---\r\ntype: [[project]]\r\nstatus: wip\r\nscheduled: 2026-07-11\r\n---\r\n- [ ] #task Ship [p:: 1] ^prj\r\n  - [/] #task Nested work ^nested\r\n1. [x] Completed #hide\r\n- [-] Canceled #hidden\r\n```md\r\n- [ ] fenced example\r\n```\r\nThis mentions - [ ] checkbox prose\r\n";
    write_file(&project, original);

    let preview = bob_command()
        .args(["projects", "sync", "--dry-run", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-10 12:00:00")
        .output()
        .expect("preview future scheduled project");
    assert_success(&preview);
    assert_eq!(fs::read_to_string(&project).unwrap(), original);
    assert!(
        stdout(&preview).contains(
            "would schedule 1 task 2026-07-11  frontmatter scheduled is future"
        ) && stdout(&preview).contains(
            "would remove #hide from 1 task  task schedules replace #hide"
        ) && stdout(&preview)
            .contains("would normalize #hide on ^prj  scheduled 2026-07-11")
            && stdout(&preview).contains("1 task schedules updated")
            && stdout(&preview).contains("bob task-status-hooks"),
        "unexpected preview:\n{}",
        format_output(&preview)
    );

    let applied = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-10 12:00:00")
        .output()
        .expect("apply future scheduled project");
    assert_success(&applied);
    assert!(
        stdout(&applied).contains(
            "scheduled 1 task 2026-07-11  frontmatter scheduled is future"
        ),
        "unexpected sync output:\n{}",
        format_output(&applied)
    );
    assert_eq!(
        fs::read_to_string(&project).unwrap(),
        "---\r\ntype: [[project]]\r\nstatus: wip\r\nscheduled: 2026-07-11\r\n---\r\n- [ ] #task Ship [p:: 1] #hide ^prj\r\n  - [/] #task Nested work [scheduled:: 2026-07-11] ^nested\r\n1. [x] Completed\r\n- [-] Canceled #hidden\r\n```md\r\n- [ ] fenced example\r\n```\r\nThis mentions - [ ] checkbox prose\r\n"
    );

    let due = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-11")
        .output()
        .expect("advance to scheduled day");
    assert_success(&due);
    assert!(
        stdout(&due).contains("0 task schedules updated"),
        "unexpected due output:\n{}",
        format_output(&due)
    );
    assert_eq!(
        fs::read_to_string(&project).unwrap(),
        "---\r\ntype: [[project]]\r\nstatus: wip\r\nscheduled: 2026-07-11\r\n---\r\n- [ ] #task Ship [p:: 1] #hide ^prj\r\n  - [/] #task Nested work [scheduled:: 2026-07-11] ^nested\r\n1. [x] Completed\r\n- [-] Canceled #hidden\r\n```md\r\n- [ ] fenced example\r\n```\r\nThis mentions - [ ] checkbox prose\r\n"
    );

    let second = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-12")
        .output()
        .expect("rerun due scheduled project");
    assert_success(&second);
    assert!(
        stdout(&second).contains("0 task schedules updated"),
        "second sync should be a no-op:\n{}",
        format_output(&second)
    );
}

#[test]
fn projects_sync_then_task_status_hooks_blocks_and_recovers_propagated_tasks() {
    let temp = TempDir::new("bob-cli-projects-scheduled-status-e2e");
    let vault = temp.path().join("vault");
    let project = vault.join("Roadmap.md");
    let before_daily = vault.join("2026/20260716.md");
    let due_daily = vault.join("2026/20260717.md");
    write_file(
        &project,
        "---\ntype: [[project]]\nstatus: wip\nscheduled: 2026-07-17\n---\n- [ ] #task Ship roadmap ^prj\n- [/] #task Implement milestone #hide ^milestone\n",
    );
    write_file(&before_daily, "## Pomodoros\n");
    write_file(&due_daily, "## Pomodoros\n");
    write_blocked_tasks_settings(&vault);

    let synced = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-16")
        .output()
        .expect("propagate project schedule");
    assert_success(&synced);
    let propagated = fs::read_to_string(&project).unwrap();
    assert!(propagated.contains(
        "- [/] #task Implement milestone [scheduled:: 2026-07-17] ^milestone"
    ));
    assert!(!propagated.contains("milestone #hide"));

    let blocked = bob_command()
        .args(["task-status-hooks", "--bob-dir"])
        .arg(&vault)
        .env("BOB_DAY_FILE", &before_daily)
        .env("BOB_NOW", "2026-07-16")
        .output()
        .expect("derive future scheduled Blocked status");
    assert_success(&blocked);
    assert!(fs::read_to_string(&project).unwrap().contains(
        "- [?] #task Implement milestone [scheduled:: 2026-07-17] ^milestone"
    ));

    let mature_sync = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-17")
        .output()
        .expect("reconcile due project");
    assert_success(&mature_sync);

    let recovered = bob_command()
        .args(["task-status-hooks", "--bob-dir"])
        .arg(&vault)
        .env("BOB_DAY_FILE", &due_daily)
        .env("BOB_NOW", "2026-07-17")
        .output()
        .expect("recover matured project task");
    assert_success(&recovered);
    assert!(fs::read_to_string(&project).unwrap().contains(
        "- [ ] #task Implement milestone [scheduled:: 2026-07-17] ^milestone"
    ));
}

#[test]
fn project_schedule_tasks_flip_between_dash_and_blocked_queries_when_due() {
    let temp = TempDir::new("bob-cli-project-schedule-query-transition");
    let vault = temp.path().join("vault");
    let project = vault.join("Roadmap.md");
    let before_daily = vault.join("2026/20260716.md");
    let due_daily = vault.join("2026/20260717.md");
    write_file(
        &project,
        "---\ntype: [[project]]\nstatus: wip\nscheduled: 2026-07-17\n---\n- [ ] #task Ship roadmap ^prj\n- [ ] #task Implement query transition #hide ^transition\n",
    );
    write_file(&before_daily, "## Pomodoros\n");
    write_file(&due_daily, "## Pomodoros\n");
    write_file(
        &vault.join("dash.md"),
        include_str!("../../fixtures/tasks_parity/vault/dash.md"),
    );
    write_file(
        &vault.join("blocked.md"),
        include_str!("../../fixtures/tasks_parity/vault/blocked.md"),
    );
    write_blocked_tasks_settings(&vault);

    let sync = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-16")
        .output()
        .expect("propagate project schedule before query verification");
    assert_success(&sync);
    let hooks = bob_command()
        .args(["task-status-hooks", "--bob-dir"])
        .arg(&vault)
        .env("BOB_DAY_FILE", &before_daily)
        .env("BOB_NOW", "2026-07-16")
        .output()
        .expect("derive Blocked status before query verification");
    assert_success(&hooks);

    let run_tasks_note = |note: &str, now: &str| {
        bob_command()
            .args(["query", "--bob-dir"])
            .arg(&vault)
            .args(["--format", "json", "--tasks-note", note])
            .env("BOB_NOW", now)
            .output()
            .unwrap_or_else(|error| panic!("query {note}: {error}"))
    };
    let contains_transition = |output: &Output| {
        assert_success(output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(output).trim())
                .expect("parse tasks-note JSON");
        json["blocks"]
            .as_array()
            .expect("tasks-note blocks")
            .iter()
            .flat_map(|block| {
                block["result"]["tasks"]
                    .as_array()
                    .expect("tasks-note result tasks")
            })
            .any(|task| {
                task["description"].as_str().is_some_and(|description| {
                    description.starts_with("#task Implement query transition")
                })
            })
    };

    let future_dash = run_tasks_note("dash.md", "2026-07-16");
    let future_blocked = run_tasks_note("blocked.md", "2026-07-16");
    assert!(
        !contains_transition(&future_dash),
        "future-scheduled task must stay off dash.md:\n{}",
        format_output(&future_dash)
    );
    assert!(
        contains_transition(&future_blocked),
        "future-scheduled task must appear in blocked.md:\n{}",
        format_output(&future_blocked)
    );

    let mature_sync = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-17")
        .output()
        .expect("reconcile due project before query verification");
    assert_success(&mature_sync);
    let mature_hooks = bob_command()
        .args(["task-status-hooks", "--bob-dir"])
        .arg(&vault)
        .env("BOB_DAY_FILE", &due_daily)
        .env("BOB_NOW", "2026-07-17")
        .output()
        .expect("recover due task before query verification");
    assert_success(&mature_hooks);

    let due_dash = run_tasks_note("dash.md", "2026-07-17");
    let due_blocked = run_tasks_note("blocked.md", "2026-07-17");
    assert!(
        contains_transition(&due_dash),
        "matured task must appear in dash.md:\n{}",
        format_output(&due_dash)
    );
    assert!(
        !contains_transition(&due_blocked),
        "matured task must leave blocked.md:\n{}",
        format_output(&due_blocked)
    );
}

#[test]
fn projects_sync_shows_sole_prj_task_when_schedule_is_due() {
    let temp = TempDir::new("bob-cli-projects-scheduled-sole-prj");
    let vault = temp.path().join("vault");
    let project = vault.join("Sole.md");
    write_file(
        &project,
        "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship #hide ^prj\n",
    );

    let output = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-11")
        .output()
        .expect("sync due project whose only task is ^prj");
    assert_success(&output);
    assert!(
        stdout(&output)
            .contains("removed #hide from ^prj  no non-hidden open tasks")
            && stdout(&output).contains("0 task schedules updated"),
        "due sole ^prj must surface through the normal rule:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(project).unwrap(),
        "---\ntype: [[project]]\nscheduled: 2026-07-11\n---\n- [ ] #task Ship ^prj\n"
    );

    // A second run changes nothing.
    let again = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-11")
        .output()
        .expect("rerun sync is a no-op");
    assert_success(&again);
    assert!(
        stdout(&again).contains("0 ^prj edited"),
        "second run must be a no-op:\n{}",
        format_output(&again)
    );
}

#[test]
fn projects_sync_surfaces_due_scheduled_project_with_only_closed_tasks() {
    // The `sase_sites` shape: past frontmatter date, hidden `^prj`,
    // only closed tasks besides it.
    let temp = TempDir::new("bob-cli-projects-scheduled-sites");
    let vault = temp.path().join("vault");
    let project = vault.join("Sites.md");
    let original = "---\ntype: [[project]]\nscheduled: 2026-08-08\n---\n- [ ] #task Ship #hide ^prj\n- [x] #task Done\n";
    write_file(&project, original);

    let preview = bob_command()
        .args(["projects", "sync", "--dry-run", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-10-03")
        .output()
        .expect("preview due scheduled project");
    assert_success(&preview);
    assert_eq!(fs::read_to_string(&project).unwrap(), original);
    assert!(
        stdout(&preview).contains("would remove #hide from ^prj"),
        "dry run must surface the ^prj:\n{}",
        format_output(&preview)
    );

    let applied = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-10-03")
        .output()
        .expect("apply due scheduled surfacing");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&project).unwrap(),
        "---\ntype: [[project]]\nscheduled: 2026-08-08\n---\n- [ ] #task Ship ^prj\n- [x] #task Done\n"
    );

    let again = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-10-03")
        .output()
        .expect("rerun sync is a no-op");
    assert_success(&again);
    assert!(
        stdout(&again).contains("0 ^prj edited"),
        "second run must change nothing:\n{}",
        format_output(&again)
    );
}

#[test]
fn projects_schedule_errors_are_per_file_and_leave_invalid_file_untouched() {
    let temp = TempDir::new("bob-cli-projects-scheduled-errors");
    let vault = temp.path().join("vault");
    let invalid = vault.join("Invalid.md");
    let invalid_contents =
        "---\ntype: [[project]]\nscheduled: 2026-02-30\n---\n- [ ] #task Invalid ^prj\n";
    write_file(&invalid, invalid_contents);
    write_file(
        &vault.join("Quoted.md"),
        "---\ntype: [[project]]\nscheduled: \"2026-07-11\"\n---\n- [ ] #task Quoted ^prj\n",
    );

    let listed = bob_command()
        .args(["projects", "list", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-10")
        .output()
        .expect("list scheduled projects with one error");
    assert_eq!(listed.status.code(), Some(1));
    assert!(
        stdout(&listed).contains("Invalid")
            && stdout(&listed).contains("Quoted")
    );
    assert!(
        stderr(&listed).contains(
            "Invalid.md:3: scheduled is not a valid calendar date: 2026-02-30"
        ),
        "unexpected list error:\n{}",
        format_output(&listed)
    );

    let synced = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .env("BOB_NOW", "2026-07-10")
        .output()
        .expect("sync scheduled projects with one error");
    assert_eq!(synced.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&invalid).unwrap(), invalid_contents);
    assert!(
        fs::read_to_string(vault.join("Quoted.md"))
            .unwrap()
            .contains("#hide ^prj"),
        "valid quoted schedule should still sync"
    );
    assert!(stdout(&synced).contains("0 task schedules updated"));
}

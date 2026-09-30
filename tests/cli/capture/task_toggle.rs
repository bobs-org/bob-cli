//! Capture task toggle except ensure-next.

use crate::support::*;
use std::fs;

#[test]
fn capture_task_toggle_link_and_unlink_updates_notes_and_reports_json() {
    let temp = TempDir::new("bob-cli-capture-task-toggle-link-unlink");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &target,
        "- [ ] #task Finish packet [dependsOn::root] ^goog-exit\n",
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CODING\n",
            "  - context\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^goog-exit]]\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("explicitly toggle ready task next");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("toggle JSON");
    // The task starts linked (under LATER), so the first toggle unlinks:
    // the lane is kept and only the ledger changes.
    assert_eq!(json["kind"], "task_toggle");
    assert_eq!(json["placement"], "toggled");
    assert_eq!(json["toggle_direction"], "unlink");
    assert_eq!(json["status_changed"], false);
    assert!(json.get("toggle_behavior").is_none(), "{json}");
    assert!(json.get("pomodoro_link_action").is_none(), "{json}");
    assert_eq!(json["previous_status_symbol"], " ");
    assert_eq!(json["previous_status_name"], "Ready");
    assert_eq!(json["status_symbol"], " ");
    assert_eq!(json["status_name"], "Ready");
    assert_eq!(
        json["previous_task_line"],
        "- [ ] #task Finish packet [dependsOn::root] ^goog-exit"
    );
    assert_eq!(
        json["task_line"],
        "- [ ] #task Finish packet [dependsOn::root] ^goog-exit"
    );
    assert_eq!(json["block_id"], "goog-exit");
    assert_eq!(json["block_link"], "[[cash#^goog-exit]]");
    assert!(json.get("pomodoro_name").is_none(), "{json}");
    assert_eq!(json["creates_pomodoro"], false);
    assert_eq!(json["pomodoro_already_linked"], false);
    assert_eq!(json["removed_pomodoro_links"], 1);
    // No dependency warning: the status did not become Next.
    assert!(json.get("warnings").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(&target).expect("read unlinked target"),
        "- [ ] #task Finish packet [dependsOn::root] ^goog-exit\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read unlinked day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CODING\n",
            "  - context\n",
            "- [ ] () — LATER\n",
        )
    );

    // Now unlinked, the second toggle links: Ready rises to Next (with the
    // dependency warning) and the link lands under CODING.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:05:00")
        .output()
        .expect("explicitly link unlinked task");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("toggle JSON");
    assert_eq!(json["toggle_direction"], "link");
    assert_eq!(json["status_changed"], true);
    assert_eq!(json["previous_status_symbol"], " ");
    assert_eq!(json["previous_status_name"], "Ready");
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["status_name"], "Next");
    assert_eq!(json["pomodoro_name"], "CODING");
    assert_eq!(json["creates_pomodoro"], false);
    assert_eq!(json["pomodoro_already_linked"], false);
    assert_eq!(json["removed_pomodoro_links"], 0);
    assert_eq!(json["pomodoro_selector_unused"], false);
    assert!(json.get("toggle_behavior").is_none(), "{json}");
    assert!(json.get("pomodoro_link_action").is_none(), "{json}");
    assert!(
        json["warnings"][0]
            .as_str()
            .is_some_and(|warning| warning.contains("declares dependencies")),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read linked target"),
        "- [*] #task Finish packet [dependsOn::root] ^goog-exit\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read linked day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CODING\n",
            "  - context\n",
            "  - [[cash#^goog-exit]]\n",
            "- [ ] () — LATER\n",
        )
    );

    // Linked again, a third toggle unlinks while keeping Next.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:10:00")
        .output()
        .expect("explicitly unlink linked next task");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("toggle JSON");
    assert_eq!(json["toggle_direction"], "unlink");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["previous_status_symbol"], "*");
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["status_name"], "Next");
    assert_eq!(json["removed_pomodoro_links"], 1);
    assert!(json.get("removed_scheduled").is_none(), "{json}");
    assert!(json.get("schedule_log").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(&target).expect("read relinked target"),
        "- [*] #task Finish packet [dependsOn::root] ^goog-exit\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read relinked day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CODING\n",
            "  - context\n",
            "- [ ] () — LATER\n",
        )
    );
}

#[test]
fn capture_task_toggle_named_creation_dry_run_and_pull_forward() {
    let temp = TempDir::new("bob-cli-capture-task-toggle-pull-forward");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    let target_before = concat!(
        "- [?] #task Scheduled work [scheduled::2026-07-20] ^sched\n",
        "  - 🗓️ **SCHEDULE LOG**
",
        "    - *2026-07-01* — older\n",
        "- [?] #task No log [scheduled::2026-07-25] ^nolog\n",
    );
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
        "  - [[cash#^sched]]\n",
        "    - review notes\n",
        "  - [[cash#^nolog]]\n",
    );
    write_toggle_task_settings(&vault);
    write_file(&target, target_before);
    write_file(&day_file, day_before);

    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+sched#deep+work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("dry-run named toggle");
    assert_success(&dry_run);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["toggle_direction"], "next");
    assert_eq!(json["toggle_behavior"], "ensure_next");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["pomodoro_name"], "DEEP+WORK");
    assert_eq!(json["removed_scheduled"], "2026-07-20");
    assert_eq!(
        json["schedule_log"]["lines"][0],
        "    - _2026-07-20 → 2026-07-10_ — 🍅 pulled into today's Pomodoro"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read dry target"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read dry day"),
        day_before
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+sched#deep+work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run named toggle");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("toggle JSON");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(json["toggle_behavior"], "ensure_next");
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["removed_pomodoro_links"], 0);
    assert!(json.get("pomodoro_link_placement").is_some(), "{json}");
    assert_eq!(
        fs::read_to_string(&target).expect("read target"),
        concat!(
            "- [*] #task Scheduled work ^sched\n",
            "  - 🗓️ **SCHEDULE LOG**
",
            "    - _2026-07-20 → 2026-07-10_ — 🍅 pulled into today's Pomodoro\n",
            "    - *2026-07-01* — older\n",
            "- [?] #task No log [scheduled::2026-07-25] ^nolog\n",
        )
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+nolog#deep+work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:45:00")
        .output()
        .expect("run second named toggle");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("toggle JSON");
    assert_eq!(json["removed_scheduled"], "2026-07-25");
    assert!(json.get("schedule_log").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(&target).expect("read target"),
        concat!(
            "- [*] #task Scheduled work ^sched\n",
            "  - 🗓️ **SCHEDULE LOG**
",
            "    - _2026-07-20 → 2026-07-10_ — 🍅 pulled into today's Pomodoro\n",
            "    - *2026-07-01* — older\n",
            "- [*] #task No log ^nolog\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
            "- [ ] () — DEEP+WORK\n",
            "  - [[cash#^sched]]\n",
            "    - review notes\n",
            "  - [[cash#^nolog]]\n",
        )
    );
}

#[test]
fn capture_task_toggle_can_edit_task_in_daily_note() {
    let temp = TempDir::new("bob-cli-capture-task-toggle-same-file");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — DAILY\n",
            "## Tasks\n",
            "- [ ] #task Daily task ^daily\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@day+daily!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:05:00")
        .output()
        .expect("toggle task in daily note");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("toggle JSON");
    assert_eq!(json["relative_target"], "day.md");
    assert_eq!(json["block_link"], "[[day#^daily]]");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — DAILY\n",
            "\t- [[day#^daily]]\n",
            "## Tasks\n",
            "- [*] #task Daily task ^daily\n",
        )
    );
}

#[test]
fn capture_task_toggle_batch_uses_staged_snapshots_and_rolls_back() {
    let temp = TempDir::new("bob-cli-capture-task-toggle-batch");
    let vault = temp.path().join("vault");
    let cash = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&cash, "- [ ] #task Alpha ^alpha\n- [ ] #task Beta ^beta\n");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
            "  - [[cash#^alpha]]\n",
            "  - [[cash#^beta]]\n",
        ),
    );

    let draft = "New work @dev:first#deep-work\n\n@cash+alpha#deep-work\n\n@cash+beta#deep-work\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00"),
        draft,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 3);
    assert_eq!(captures[0]["kind"], "pomodoro_task");
    assert_eq!(captures[1]["kind"], "task_toggle");
    assert_eq!(captures[1]["toggle_behavior"], "ensure_next");
    assert_eq!(captures[2]["kind"], "task_toggle");
    assert_eq!(captures[2]["toggle_behavior"], "ensure_next");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
            "- [ ] () — DEEP-WORK\n",
            "  - [[dev#^first]]\n",
            "  - [[cash#^alpha]]\n",
            "  - [[cash#^beta]]\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&cash).expect("read cash"),
        "- [*] #task Alpha ^alpha\n- [*] #task Beta ^beta\n",
    );

    let rollback_temp = TempDir::new("bob-cli-capture-task-toggle-rollback");
    let vault = rollback_temp.path().join("vault");
    let cash = vault.join("cash.md");
    let day_file = vault.join("day.md");
    let cash_before = "- [ ] #task Alpha ^alpha\n";
    let day_before = "## Notes\n- nothing here\n";
    write_toggle_task_settings(&vault);
    write_file(&cash, cash_before);
    write_file(&day_file, day_before);
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00"),
        "ordinary @notes\n\n@cash+alpha!\n",
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("no Pomodoros section")),
        "{json}"
    );
    assert_eq!(fs::read_to_string(&cash).expect("read cash"), cash_before);
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), day_before);
    assert!(
        !vault.join("notes.md").exists(),
        "failed batch must not commit earlier ordinary captures"
    );
}

#[test]
fn capture_task_toggle_errors_are_actionable_without_writes() {
    struct ErrorCase<'a> {
        name: &'a str,
        target: &'a str,
        day: &'a str,
        args: Vec<&'a str>,
        exit: i32,
        expected: &'a str,
    }

    let cases = vec![
        ErrorCase {
            name: "done",
            target: "- [x] #task Done already ^done\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n",
            args: vec!["@cash+done!"],
            exit: 1,
            expected: "task ^done is Done; only Ready, Blocked, Next, and In Progress tasks can be toggled",
        },
        ErrorCase {
            name: "cancelled",
            target: "- [-] #task Dropped idea ^dropped\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n",
            args: vec!["@cash+dropped!"],
            exit: 1,
            expected: "only Ready, Blocked, Next, and In Progress tasks can be toggled",
        },
        ErrorCase {
            name: "missing-id",
            target: "- [ ] #task Parent ^parent\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n",
            args: vec!["@cash+missing!"],
            exit: 1,
            expected: "no task with block ID ^missing in cash.md",
        },
        ErrorCase {
            name: "not-task",
            target: "ordinary paragraph ^plain\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n",
            args: vec!["@cash+plain!"],
            exit: 1,
            expected: "^plain in cash.md is not a task",
        },
        ErrorCase {
            name: "duplicate-id",
            target: "- [ ] #task One ^dup\n- [ ] #task Two ^dup\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n",
            args: vec!["@cash+dup!"],
            exit: 1,
            expected: "block ID ^dup appears 2 times",
        },
        ErrorCase {
            name: "no-section",
            target: "- [ ] #task Alpha ^alpha\n",
            day: "## Notes\n- nothing here\n",
            args: vec!["@cash+alpha!"],
            exit: 1,
            expected: "Bob daily note has no Pomodoros section",
        },
        ErrorCase {
            name: "ambiguous-current",
            target: "- [ ] #task Alpha ^alpha\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — A\n- [ ] (**0930-1000**) — B\n",
            args: vec!["@cash+alpha!"],
            exit: 1,
            expected: "Bob daily note has multiple open timed Pomodoros",
        },
        ErrorCase {
            name: "clip-conflict",
            target: "- [ ] #task Alpha ^alpha\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n",
            args: vec!["--clip", "@cash+alpha!"],
            exit: 2,
            expected: "task toggle capture cannot be combined with --clip",
        },
    ];

    for case in cases {
        let temp = TempDir::new(&format!(
            "bob-cli-capture-toggle-error-{}",
            case.name
        ));
        let vault = temp.path().join("vault");
        let target = vault.join("cash.md");
        let day_file = vault.join("day.md");
        write_toggle_task_settings(&vault);
        write_file(&target, case.target);
        write_file(&day_file, case.day);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .args(&case.args)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:05:00")
            .output()
            .expect("run failing toggle");
        assert_eq!(
            output.status.code(),
            Some(case.exit),
            "{}: {}",
            case.name,
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(case.expected),
            "{}: expected {:?} in {}",
            case.name,
            case.expected,
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&target).expect("read target"),
            case.target,
            "{}",
            case.name
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("read day"),
            case.day,
            "{}",
            case.name
        );
    }
}

#[test]
fn capture_task_toggle_reports_inserted_link_block() {
    // A link-direction toggle that inserts a fresh Task Link is
    // auto-detected: no planner ref is pushed for toggles.
    let temp = TempDir::new("bob-cli-capture-toggle-blocks-insert");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&target, "- [ ] #task Fresh work ^fresh\n");
    let day_before =
        "## Pomodoros\n- [ ] (**0900-0930**) — CODING\n  - context\n";
    write_file(&day_file, day_before);

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-07-10 10:00:00",
        &["@cash+fresh!"],
    );
    assert_eq!(json["kind"], "task_toggle");
    assert_eq!(json["toggle_direction"], "link");
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 2,
                "name": "CODING",
                "time_range": "0900-0930",
                "status": "running",
                "created": false,
                "roles": ["changed"],
                "lines": [
                    {
                        "text": "- [ ] (**0900-0930**) — CODING",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - context",
                        "depth": 1,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - [[cash#^fresh]]",
                        "depth": 1,
                        "change": "added",
                    },
                ],
            },
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read toggled day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CODING\n",
            "  - context\n",
            "  - [[cash#^fresh]]\n",
        )
    );
    assert_pomodoro_blocks_cover_changes(day_before, &day_after, &json);
}

#[test]
fn capture_task_toggle_reports_created_named_entry() {
    // Ensure-Next onto a missing name creates the Pomodoro and moves the
    // queued link into it: the destination carries the `linked` ref with
    // `created: true`, the source the `unlinked` ref.
    let temp = TempDir::new("bob-cli-capture-toggle-blocks-created");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&target, "- [ ] #task Fresh work ^fresh\n");
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CODING\n",
        "- [ ] () — QUEUE\n",
        "  - [[cash#^fresh]]\n",
    );
    write_file(&day_file, day_before);

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-07-10 10:00:00",
        &["@cash+fresh#deep+work"],
    );
    assert_eq!(json["kind"], "task_toggle");
    assert_eq!(json["toggle_behavior"], "ensure_next");
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 3,
                "name": "DEEP+WORK",
                "status": "queued",
                "created": true,
                "roles": ["linked"],
                "lines": [
                    {
                        "text": "- [ ] () — DEEP+WORK",
                        "depth": 0,
                        "change": "added",
                    },
                    {
                        "text": "  - [[cash#^fresh]]",
                        "depth": 1,
                        "change": "added",
                    },
                ],
            },
            {
                "relative_target": "day.md",
                "line": 5,
                "name": "QUEUE",
                "status": "queued",
                "created": false,
                "roles": ["unlinked"],
                "lines": [
                    {
                        "text": "- [ ] () — QUEUE",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - [[cash#^fresh]]",
                        "depth": 1,
                        "change": "removed",
                    },
                ],
            },
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read toggled day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CODING\n",
            "- [ ] () — DEEP+WORK\n",
            "  - [[cash#^fresh]]\n",
            "- [ ] () — QUEUE\n",
        )
    );
    assert_pomodoro_blocks_cover_changes(day_before, &day_after, &json);
}

#[test]
fn capture_task_toggle_reports_unlink_removal_from_two_entries() {
    // Unlink-direction cleanup removes the link from every open entry while
    // keeping the lane, and auto-detection reports each touched block with
    // its removed line.
    let temp = TempDir::new("bob-cli-capture-toggle-blocks-removal");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&target, "- [*] #task Queued work ^done\n");
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — ONE\n",
        "  - [[cash#^done]]\n",
        "- [ ] () — TWO\n",
        "  - [[cash#^done]]\n",
    );
    write_file(&day_file, day_before);

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-07-10 10:00:00",
        &["@cash+done!"],
    );
    assert_eq!(json["kind"], "task_toggle");
    assert_eq!(json["toggle_direction"], "unlink");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["status_name"], "Next");
    assert_eq!(json["removed_pomodoro_links"], 2);
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 2,
                "name": "ONE",
                "time_range": "0900-0930",
                "status": "running",
                "created": false,
                "roles": ["changed"],
                "lines": [
                    {
                        "text": "- [ ] (**0900-0930**) — ONE",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - [[cash#^done]]",
                        "depth": 1,
                        "change": "removed",
                    },
                ],
            },
            {
                "relative_target": "day.md",
                "line": 3,
                "name": "TWO",
                "status": "queued",
                "created": false,
                "roles": ["changed"],
                "lines": [
                    {
                        "text": "- [ ] () — TWO",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - [[cash#^done]]",
                        "depth": 1,
                        "change": "removed",
                    },
                ],
            },
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read toggled day");
    assert_eq!(
        day_after,
        "## Pomodoros\n- [ ] (**0900-0930**) — ONE\n- [ ] () — TWO\n"
    );
    // The route note is untouched: unlinking keeps Next.
    assert_eq!(
        fs::read_to_string(&target).expect("read toggled target"),
        "- [*] #task Queued work ^done\n"
    );
    assert_pomodoro_blocks_cover_changes(day_before, &day_after, &json);
}

#[test]
fn capture_task_toggle_links_unlinked_lanes_without_status_change() {
    // Unlinked Next, In Progress, and Blocked tasks all link under the
    // implicit entry; only Ready/Blocked rise to Next.
    let temp = TempDir::new("bob-cli-capture-toggle-link-lanes");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &target,
        concat!(
            "- [*] #task Queued ^next\n",
            "- [/] #task Busy ^busy\n",
            "- [?] #task Waiting ^blocked\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — CURRENT\n");

    // Human dry run first: In Progress links with its lane kept.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("@cash+busy!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("dry-run link human");
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    let out = stdout(&human);
    assert!(out.contains("would toggle"), "{out}");
    assert!(
        out.contains("linked ^busy under CURRENT · stays In Progress"),
        "{out}"
    );

    for (arg, previous, current, name, changed) in [
        ("@cash+next!", "*", "*", "Next", false),
        ("@cash+busy!", "/", "/", "In Progress", false),
        ("@cash+blocked!", "?", "*", "Next", true),
    ] {
        let json = capture_json_dry_run_matches_real(
            &vault,
            &day_file,
            "2026-07-10 10:00:00",
            &[arg],
        );
        assert_eq!(json["toggle_direction"], "link", "{arg}");
        assert_eq!(
            json["status_changed"],
            serde_json::Value::Bool(changed),
            "{arg}"
        );
        assert_eq!(json["previous_status_symbol"], previous, "{arg}");
        assert_eq!(json["status_symbol"], current, "{arg}");
        assert_eq!(json["status_name"], name, "{arg}");
        assert_eq!(json["pomodoro_name"], "CURRENT", "{arg}");
        assert!(json.get("toggle_behavior").is_none(), "{json}");
    }
    assert_eq!(
        fs::read_to_string(&target).expect("read linked target"),
        concat!(
            "- [*] #task Queued ^next\n",
            "- [/] #task Busy ^busy\n",
            "- [*] #task Waiting ^blocked\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read linked day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] () — CURRENT\n",
            "\t- [[cash#^next]]\n",
            "\t- [[cash#^busy]]\n",
            "\t- [[cash#^blocked]]\n",
        )
    );
}

#[test]
fn capture_task_toggle_unlink_keeps_every_lane() {
    // Linked tasks of any open status unlink with the route note untouched.
    let temp = TempDir::new("bob-cli-capture-toggle-unlink-lanes");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    let target_before =
        concat!("- [/] #task Busy ^busy\n", "- [ ] #task Ready ^ready\n",);
    write_file(&target, target_before);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — CURRENT\n",
            "  - [[cash#^busy]]\n",
            "  - [[cash#^ready]]\n",
        ),
    );

    // Human dry run first: unlinking names the kept lane.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("@cash+ready!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("dry-run unlink human");
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    let out = stdout(&human);
    assert!(out.contains("would toggle"), "{out}");
    assert!(
        out.contains("unlinked ^ready from 1 open Pomodoro · stays Ready"),
        "{out}"
    );

    for (arg, symbol, name) in [
        ("@cash+busy!", "/", "In Progress"),
        ("@cash+ready!", " ", "Ready"),
    ] {
        let json = capture_json_dry_run_matches_real(
            &vault,
            &day_file,
            "2026-07-10 10:00:00",
            &[arg],
        );
        assert_eq!(json["toggle_direction"], "unlink", "{arg}");
        assert_eq!(json["status_changed"], false, "{arg}");
        assert_eq!(json["previous_status_symbol"], symbol, "{arg}");
        assert_eq!(json["status_symbol"], symbol, "{arg}");
        assert_eq!(json["status_name"], name, "{arg}");
        assert_eq!(json["removed_pomodoro_links"], 1, "{arg}");
        assert!(json.get("removed_scheduled").is_none(), "{json}");
        assert!(json.get("schedule_log").is_none(), "{json}");
    }
    assert_eq!(
        fs::read_to_string(&target).expect("read unlinked target"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read unlinked day"),
        "## Pomodoros\n- [ ] () — CURRENT\n"
    );
}

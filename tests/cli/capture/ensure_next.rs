//! Capture task toggle ensure-next tests.

use crate::support::*;
use std::fs;

#[test]
fn capture_task_toggle_ensure_next_moves_link_and_sets_next() {
    let temp = TempDir::new("bob-cli-capture-ensure-next-ready");
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
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
        ),
    );
    let day_before = fs::read_to_string(&day_file).expect("read day before");

    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--")
        .arg("@cash+goog-exit")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("dry-run ensure-Next");
    assert_success(&dry_run);
    let human = stdout(&dry_run);
    assert!(human.contains("would ensure"), "{human}");
    assert!(human.contains("[ ] → [*]"), "{human}");
    assert!(human.contains("moved Task Link"), "{human}");
    assert!(human.contains("LATER"), "{human}");
    assert!(human.contains("CURRENT"), "{human}");
    assert_eq!(
        fs::read_to_string(&target).expect("dry target"),
        "- [ ] #task Finish packet [dependsOn::root] ^goog-exit\n"
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("ensure-Next ready task");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["kind"], "task_toggle");
    assert_eq!(json["toggle_direction"], "next");
    assert_eq!(json["toggle_behavior"], "ensure_next");
    assert_eq!(json["status_changed"], true);
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["creates_pomodoro"], false);
    assert_eq!(json["pomodoro_already_linked"], false);
    assert_eq!(json["removed_pomodoro_links"], 0);
    assert_eq!(json["pomodoro_name"], "CURRENT");
    assert_eq!(json["pomodoro_link_source"]["name"], "LATER");
    assert_eq!(json["pomodoro_link_destination"]["name"], "CURRENT");
    assert_eq!(json["pomodoro_link_destination"]["time_range"], "0900-0930");
    assert!(json.get("pomodoro_link_placement").is_some(), "{json}");
    assert!(
        json["warnings"][0]
            .as_str()
            .is_some_and(|warning| warning.contains("declares dependencies")),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("target"),
        "- [*] #task Finish packet [dependsOn::root] ^goog-exit\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
            "- [ ] () — LATER\n",
        )
    );
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 2,
                "name": "CURRENT",
                "time_range": "0900-0930",
                "status": "running",
                "created": false,
                "roles": ["linked"],
                "lines": [
                    {
                        "text": "- [ ] (**0900-0930**) — CURRENT",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - context",
                        "depth": 1,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - [[cash#^goog-exit]]",
                        "depth": 1,
                        "change": "added",
                    },
                    {
                        "text": "    - review notes",
                        "depth": 2,
                        "change": "added",
                    },
                ],
            },
            {
                "relative_target": "day.md",
                "line": 6,
                "name": "LATER",
                "status": "queued",
                "created": false,
                "roles": ["unlinked"],
                "lines": [
                    {
                        "text": "- [ ] () — LATER",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - [[cash#^goog-exit]]",
                        "depth": 1,
                        "change": "removed",
                    },
                    {
                        "text": "    - review notes",
                        "depth": 2,
                        "change": "removed",
                    },
                ],
            },
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read day after");
    assert_pomodoro_blocks_cover_changes(&day_before, &day_after, &json);

    let noop = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:05:00")
        .output()
        .expect("total no-op ensure-Next");
    assert_success(&noop);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&noop).trim()).expect("json");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["pomodoro_already_linked"], true);
    assert!(json.get("pomodoro_link_placement").is_none(), "{json}");
}

#[test]
fn capture_task_toggle_ensure_next_covers_open_states_and_failures() {
    struct Case<'a> {
        name: &'a str,
        target: &'a str,
        day: &'a str,
        expected_status: &'a str,
        link_action: &'a str,
        status_changed: bool,
    }

    let cases = [
        Case {
            name: "blocked-move",
            target: "- [?] #task Waiting ^wait\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n- [ ] () — LATER\n  - [[cash#^wait]]\n",
            expected_status: "*",
            link_action: "moved",
            status_changed: true,
        },
        Case {
            name: "in-progress-move",
            target: "- [/] #task Busy ^busy\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n- [ ] () — LATER\n  - [[cash#^busy]]\n",
            expected_status: "/",
            link_action: "moved",
            status_changed: false,
        },
        Case {
            name: "already-next-move-only",
            target: "- [*] #task Planned ^plan\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n- [ ] () — LATER\n  - [[cash#^plan]]\n",
            expected_status: "*",
            link_action: "moved",
            status_changed: false,
        },
        Case {
            name: "status-only-already-current",
            target: "- [ ] #task Here ^here\n",
            day: "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n  - [[cash#^here]]\n",
            expected_status: "*",
            link_action: "already_current",
            status_changed: true,
        },
    ];

    for case in cases {
        let temp =
            TempDir::new(&format!("bob-cli-capture-ensure-next-{}", case.name));
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
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(format!(
                "@cash+{}",
                case.target.rsplit('^').next().unwrap().trim()
            ))
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 10:00:00")
            .output()
            .unwrap_or_else(|_| panic!("run {}", case.name));
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert_eq!(json["toggle_behavior"], "ensure_next", "{}", case.name);
        assert_eq!(
            json["status_symbol"], case.expected_status,
            "{}",
            case.name
        );
        assert_eq!(
            json["status_changed"], case.status_changed,
            "{}",
            case.name
        );
        assert_eq!(
            json["pomodoro_link_action"], case.link_action,
            "{}",
            case.name
        );
    }

    let sched_temp = TempDir::new("bob-cli-capture-ensure-next-schedule");
    let vault = sched_temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &target,
        concat!(
            "- [?] #task Scheduled [scheduled::2026-07-20] ^sched\n",
            "  - 🗓️ **SCHEDULE LOG**
",
            "    - *2026-07-01* — older\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**1330-1400**) — CURRENT\n  - [[cash#^sched]]\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+sched")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("schedule retirement");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["removed_scheduled"], "2026-07-20");
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert!(json.get("schedule_log").is_some(), "{json}");

    let failures = [
        (
            "missing-link",
            "- [ ] #task Alpha ^alpha\n",
            "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n",
            "no movable open Pomodoro Task Link",
        ),
        (
            "completed-only",
            "- [ ] #task Alpha ^alpha\n",
            "## Pomodoros\n- [x] (**0800-0830**) — DONE\n  - [[cash#^alpha]]\n- [ ] (**0900-0930**) — CURRENT\n",
            "no movable open Pomodoro Task Link",
        ),
        (
            "duplicate-link",
            "- [ ] #task Alpha ^alpha\n",
            "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n  - [[cash#^alpha]]\n- [ ] () — LATER\n  - [[cash#^alpha]]\n",
            "more than one movable open Pomodoro Task Link",
        ),
        (
            "done-task",
            "- [x] #task Done already ^done\n",
            "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n  - [[cash#^done]]\n",
            "only Ready, Blocked, In Progress, and Next tasks can be ensured Next",
        ),
    ];
    for (name, target_body, day_body, expected) in failures {
        let temp = TempDir::new(&format!("bob-cli-capture-ensure-next-{name}"));
        let vault = temp.path().join("vault");
        let target = vault.join("cash.md");
        let day_file = vault.join("day.md");
        write_toggle_task_settings(&vault);
        write_file(&target, target_body);
        write_file(&day_file, day_body);
        let id = target_body.rsplit('^').next().unwrap().trim();
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(format!("@cash+{id}"))
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 10:00:00")
            .output()
            .unwrap_or_else(|_| panic!("run {name}"));
        assert_eq!(
            output.status.code(),
            Some(1),
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert!(
            json["error"]
                .as_str()
                .is_some_and(|error| error.contains(expected)),
            "{name}: {json}"
        );
        if name == "missing-link" {
            assert!(
                json["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("use @cash+alpha!")),
                "{name}: {json}"
            );
        }
        assert_eq!(
            fs::read_to_string(&target).expect("target"),
            target_body,
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("day"),
            day_body,
            "{name}"
        );
    }
}

#[test]
fn capture_task_toggle_ensure_next_same_note_batch_and_rollback() {
    let temp = TempDir::new("bob-cli-capture-ensure-next-same-note");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — DAILY\n",
            "- [ ] () — LATER\n",
            "  - [[day#^daily]]\n",
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
        .arg("@day+daily")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:05:00")
        .output()
        .expect("same-note ensure-Next");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&day_file).expect("day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — DAILY\n",
            "  - [[day#^daily]]\n",
            "- [ ] () — LATER\n",
            "## Tasks\n",
            "- [*] #task Daily task ^daily\n",
        )
    );

    let batch_temp = TempDir::new("bob-cli-capture-ensure-next-batch");
    let vault = batch_temp.path().join("vault");
    let cash = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&cash, "- [ ] #task Alpha ^alpha\n- [ ] #task Beta ^beta\n");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**1330-1400**) — CURRENT\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^alpha]]\n",
            "  - [[cash#^beta]]\n",
        ),
    );
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00"),
        "@cash+alpha\n\n@cash+beta\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["pomodoro_link_action"], "moved");
    assert_eq!(captures[1]["pomodoro_link_action"], "moved");
    assert_eq!(
        fs::read_to_string(&day_file).expect("day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**1330-1400**) — CURRENT\n",
            "  - [[cash#^alpha]]\n",
            "  - [[cash#^beta]]\n",
            "- [ ] () — LATER\n",
        )
    );

    let rollback_temp = TempDir::new("bob-cli-capture-ensure-next-rollback");
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
        "ordinary @notes\n\n@cash+alpha\n",
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(fs::read_to_string(&cash).expect("cash"), cash_before);
    assert_eq!(fs::read_to_string(&day_file).expect("day"), day_before);
    assert!(
        !vault.join("notes.md").exists(),
        "failed batch must not commit earlier ordinary captures"
    );
}

#[test]
fn capture_task_toggle_named_ensure_next_moves_creates_and_noops() {
    let temp = TempDir::new("bob-cli-capture-named-ensure-next");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&target, "- [*] #task Finish packet ^goog-exit\n");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — CODING\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
        ),
    );

    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--")
        .arg("@cash+goog-exit#coding")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("dry-run named ensure-Next");
    assert_success(&dry_run);
    let human = stdout(&dry_run);
    assert!(human.contains("would ensure"), "{human}");
    assert!(human.contains("[*] already Next"), "{human}");
    assert!(human.contains("moved Task Link"), "{human}");
    assert!(human.contains("LATER"), "{human}");
    assert!(human.contains("CODING"), "{human}");
    assert!(!human.contains("would toggle"), "{human}");
    assert_eq!(
        fs::read_to_string(&day_file).expect("dry day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — CODING\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
        )
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit#cod")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("named prefix ensure-Next");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["toggle_behavior"], "ensure_next");
    assert_eq!(json["toggle_direction"], "next");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["creates_pomodoro"], false);
    assert_eq!(json["pomodoro_already_linked"], false);
    assert_eq!(json["removed_pomodoro_links"], 0);
    assert_eq!(json["pomodoro_name"], "CODING");
    assert_eq!(json["pomodoro_link_source"]["name"], "LATER");
    assert_eq!(json["pomodoro_link_destination"]["name"], "CODING");
    assert!(
        json.get("pomodoro_selector_unused") != Some(&serde_json::json!(true))
    );
    assert_eq!(
        fs::read_to_string(&target).expect("target"),
        "- [*] #task Finish packet ^goog-exit\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — CODING\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
            "- [ ] () — LATER\n",
        )
    );

    let noop = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--")
        .arg("@cash+goog-exit#coding")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:05:00")
        .output()
        .expect("named no-op");
    assert_success(&noop);
    let human = stdout(&noop);
    assert!(human.contains("already in CODING"), "{human}");
    assert!(!human.contains("current/next Pomodoro"), "{human}");

    let noop_json = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit#coding")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:05:00")
        .output()
        .expect("named no-op json");
    assert_success(&noop_json);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&noop_json).trim()).expect("json");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["pomodoro_already_linked"], true);
    assert_eq!(json["creates_pomodoro"], false);
    assert_eq!(json["pomodoro_name"], "CODING");
    assert!(json.get("pomodoro_link_placement").is_none(), "{json}");

    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0830**) — DEEP+WORK\n",
            "  - old\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
        ),
    );
    write_file(&target, "- [ ] #task Finish packet ^goog-exit\n");
    let created = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit#deep+work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:10:00")
        .output()
        .expect("named create");
    assert_success(&created);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&created).trim()).expect("json");
    assert_eq!(json["toggle_behavior"], "ensure_next");
    assert_eq!(json["status_changed"], true);
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(json["pomodoro_name"], "DEEP+WORK");
    assert_eq!(json["removed_pomodoro_links"], 0);
    assert_eq!(
        fs::read_to_string(&target).expect("target"),
        "- [*] #task Finish packet ^goog-exit\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day"),
        concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0830**) — DEEP+WORK\n",
            "  - old\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "- [ ] () — DEEP+WORK\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
        )
    );

    let created_human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--")
        .arg("@cash+goog-exit#fresh")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:15:00")
        .output()
        .expect("named create human");
    assert_success(&created_human);
    let human = stdout(&created_human);
    assert!(human.contains("created FRESH"), "{human}");
    assert!(human.contains("moved Task Link"), "{human}");

    write_file(&target, "- [ ] #task No link ^noleak\n");
    let missing = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+noleak#coding")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:20:00")
        .output()
        .expect("named missing link");
    assert_eq!(
        missing.status.code(),
        Some(1),
        "{}",
        format_output(&missing)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("json");
    assert!(
        json["error"].as_str().is_some_and(|error| error
            .contains("no movable open Pomodoro Task Link")
            && error.contains("use @cash+noleak!")),
        "{json}"
    );

    let bang = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+noleak!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:25:00")
        .output()
        .expect("explicit insert");
    assert_success(&bang);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&bang).trim()).expect("json");
    assert!(json.get("toggle_behavior").is_none(), "{json}");
    assert_eq!(json["toggle_direction"], "link");
    assert_eq!(json["status_changed"], true);
    assert!(json["removed_pomodoro_links"].as_u64().unwrap_or(0) == 0);
    assert!(
        fs::read_to_string(&day_file)
            .expect("day")
            .contains("- [[cash#^noleak]]"),
        "explicit toggle may insert a missing link"
    );
}

#[test]
fn capture_task_toggle_ensure_next_keeps_in_progress() {
    // Ensure Next on In Progress moves the link and keeps the lane.
    let temp = TempDir::new("bob-cli-capture-ensure-next-in-progress");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&target, "- [/] #task Busy ^busy\n");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^busy]]\n",
        ),
    );

    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--")
        .arg("@cash+busy")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("dry-run ensure-Next in progress");
    assert_success(&dry_run);
    assert_stdout_has_no_ansi(&dry_run);
    let human = stdout(&dry_run);
    assert!(human.contains("would ensure"), "{human}");
    assert!(human.contains("stays In Progress"), "{human}");
    assert!(human.contains("moved Task Link"), "{human}");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+busy")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("ensure-Next in progress task");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["toggle_direction"], "next");
    assert_eq!(json["toggle_behavior"], "ensure_next");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["status_symbol"], "/");
    assert_eq!(json["status_name"], "In Progress");
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(
        fs::read_to_string(&target).expect("target"),
        "- [/] #task Busy ^busy\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "  - [[cash#^busy]]\n",
            "- [ ] () — LATER\n",
        )
    );
}

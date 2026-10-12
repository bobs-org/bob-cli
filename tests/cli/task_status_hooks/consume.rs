//! Blocked-link consumption (`docs/task-dependencies.md` §13.5, PDL).

use crate::support::*;
use std::fs;

// PDF1 / PDL1: two resident links under an open entry that has other
// children are consumed; the entry is kept; the prerequisite is
// promoted; closed entries stay byte-identical (PDL4).
#[test]
fn reconcile_consumes_blocked_resident_links_and_keeps_the_entry() {
    let temp = TempDir::new("bob-cli-task-status-hooks-consume-pdl1");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let proj = vault.join("proj.md");
    let tasks = vault.join("tasks.md");
    let daily_before = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930) — JOB\n",
        "  - [[proj#^r1]]\n",
        "  - [[proj#^r2]]\n",
        "  - [[tasks#^other]]\n",
        "- [x] Closed (0800-0830)\n",
        "  - [[proj#^r1]]\n",
    );
    write_file(&daily, daily_before);
    write_file(
        &proj,
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Ship ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
            "- [*] #task Resident one ^r1\n",
            "- [*] #task Resident two ^r2\n",
        ),
    );
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Prerequisite work ^pre\n",
            "- [ ] #task Unrelated other ^other\n",
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
        .expect("dry-run blocked-link consumption");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    let consumed = json["consumed_blocked_references"].as_array().unwrap();
    assert_eq!(consumed.len(), 2, "{consumed:?}");
    for block_id in ["r1", "r2"] {
        assert!(
            consumed.iter().any(|item| {
                item["task"]["block_id"] == block_id
                    && item["task"]["path"] == "proj.md"
                    && item["reasons"]
                        == serde_json::json!(["project_dependency"])
                    && item["pomodoro"] == "- [ ] Current (0900-0930) — JOB"
            }),
            "missing {block_id}: {consumed:?}"
        );
    }
    assert!(json["removed_empty_pomodoros"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(json["marked_next"].as_array().unwrap().iter().any(|item| {
        item["block_id"] == "pre" && item["dependency"] == true
    }));

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply blocked-link consumption");
    assert_success(&applied);
    let daily_after = fs::read_to_string(&daily).unwrap();
    assert!(
        daily_after.contains("- [x] Closed (0800-0830)\n  - 🍅 [[proj#^r1]]"),
        "closed-entry history is kept; first-pass marker repair adds 🍅:\n{daily_after}"
    );
    assert!(daily_after.contains("[[tasks#^other]]"), "{daily_after}");
    let open_section = daily_after
        .split("- [x] Closed")
        .next()
        .unwrap_or(&daily_after);
    assert!(
        !open_section.contains("[[proj#^r1]]")
            && !open_section.contains("[[proj#^r2]]"),
        "{daily_after}"
    );
    let proj_after = fs::read_to_string(&proj).unwrap();
    assert!(proj_after.contains("#prj_is_blocked ^r1"), "{proj_after}");
    assert!(proj_after.contains("#prj_is_blocked ^r2"), "{proj_after}");
    let tasks_after = fs::read_to_string(&tasks).unwrap();
    assert!(
        tasks_after
            .lines()
            .any(|line| line.contains("- [*]") && line.ends_with("^pre")),
        "{tasks_after}"
    );

    let second = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("idempotent blocked-link consumption");
    assert_success(&second);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&second).trim()).unwrap();
    assert_eq!(json["consumed_blocked_references"], serde_json::json!([]));
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_after);
}

// PDL2: the consumed link was the entry's last child — entry deleted.
#[test]
fn reconcile_deletes_a_pomodoro_emptied_by_blocked_consumption() {
    let temp = TempDir::new("bob-cli-task-status-hooks-consume-pdl2");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let tasks = vault.join("tasks.md");
    let daily_before = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^blocked]]\n",
        "- [ ] Later\n",
        "  - later child\n",
    );
    write_file(&daily, daily_before);
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Open root [id:: root] ^root\n",
            "- [*] #task Blocked child [dependsOn:: root] ^blocked\n",
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
        .expect("dry-run emptied Pomodoro");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(
        json["consumed_blocked_references"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        json["removed_empty_pomodoros"],
        serde_json::json!([
            {"line_number": 3, "line": "- [ ] Current (0900-0930)"}
        ])
    );

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply emptied Pomodoro");
    assert_success(&applied);
    let daily_after = fs::read_to_string(&daily).unwrap();
    assert!(
        !daily_after.contains("Current (0900-0930)"),
        "{daily_after}"
    );
    assert!(
        daily_after.contains("- [ ] Later\n  - later child"),
        "{daily_after}"
    );
}

// PDL3: the target's last reason clears in the same run — link kept.
#[test]
fn reconcile_keeps_a_link_when_the_block_clears_in_the_same_run() {
    let temp = TempDir::new("bob-cli-task-status-hooks-consume-pdl3");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let tasks = vault.join("tasks.md");
    let daily_before = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^mature]]\n",
    );
    write_file(&daily, daily_before);
    write_file(
        &tasks,
        "- [?] #task Mature schedule [scheduled:: 2026-07-16] ^mature\n",
    );
    write_blocked_tasks_settings(&vault);

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply final-state keep");
    assert_success(&applied);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&applied).trim()).unwrap();
    assert_eq!(json["consumed_blocked_references"], serde_json::json!([]));
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    let tasks_after = fs::read_to_string(&tasks).unwrap();
    assert!(
        tasks_after
            .lines()
            .any(|line| line.contains("- [*]") && line.ends_with("^mature")),
        "{tasks_after}"
    );
}

// Daily-note exception (§13.5): a prerequisite in a canonical daily
// note promoted only by consumption may clear on the next run (R9).
#[test]
fn consumption_promoted_daily_note_prerequisite_may_clear_next_run() {
    let temp = TempDir::new("bob-cli-task-status-hooks-consume-daily");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let proj = vault.join("proj.md");
    write_file(
        &daily,
        concat!(
            "- [ ] #task Daily prerequisite [id:: pre] ^pre\n",
            "\n",
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[proj#^r1]]\n",
            "  - keep child\n",
        ),
    );
    write_file(
        &proj,
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Ship ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[2026/20260716#^pre]]\n",
            "- [*] #task Resident ^r1\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let first = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("first consumption run");
    assert_success(&first);
    let daily_mid = fs::read_to_string(&daily).unwrap();
    assert!(
        daily_mid
            .lines()
            .any(|line| line.contains("- [*]") && line.ends_with("^pre")),
        "first run should promote the daily prerequisite:\n{daily_mid}"
    );
    assert!(
        !daily_mid
            .split("## Pomodoros")
            .nth(1)
            .unwrap_or("")
            .contains("[[proj#^r1]]"),
        "{daily_mid}"
    );

    let second = bob_command()
        .args(["task", "reconcile"])
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("second consumption run");
    assert_success(&second);
    let daily_after = fs::read_to_string(&daily).unwrap();
    assert!(
        daily_after
            .lines()
            .any(|line| line.contains("- [ ]") && line.ends_with("^pre")),
        "R9 should clear the unlinked daily-note Next:\n{daily_after}"
    );
}

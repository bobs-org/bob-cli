//! Promotion through Depends-On lines.

use crate::support::*;
use std::fs;

#[test]
fn task_status_hooks_promotes_prerequisite_through_depends_on_line() {
    // A Pomodoro-linked dependent promotes its prerequisite through a
    // Depends-On line (`docs/task-dependencies.md` §5). The dependent
    // itself derives Blocked while the prerequisite is open; the
    // prerequisite rises to Next along the line edge.
    let temp = TempDir::new("bob-cli-task-status-hooks-dependency-lines");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let tasks = vault.join("tasks.md");
    write_file(
        &daily,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[tasks#^dep]]\n",
        ),
    );
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent [dependsOn:: tasks__pre] ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [ ] #task Prerequisite [id:: tasks__pre] ^pre\n",
        ),
    );
    write_blocked_tasks_settings(&vault);
    let before = fs::read_to_string(&tasks).unwrap();

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run Depends-On line promotion");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["unresolved_references"].as_array().unwrap().is_empty());
    let marked = json["marked_next"].as_array().unwrap();
    assert_eq!(marked.len(), 1);
    assert!(marked.iter().any(|item| {
        item["path"] == "tasks.md"
            && item["block_id"] == "pre"
            && item["dependency"] == true
    }));
    assert!(json["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["block_id"] == "dep"
                && item["from"] == " "
                && item["to"] == "?"
                && item["open_dependency_ids"]
                    == serde_json::json!(["tasks__pre"])
        }));
}

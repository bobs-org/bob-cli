//! Link-guard pull-forward behavior for scheduled project residents.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-10-12 09:20:00";

fn capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    draft: &str,
) -> Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn assert_pulled_forward(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    json: &Value,
) {
    assert!(json.get("link_guard").is_none(), "{json}");
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["removed_scheduled"], "2026-10-20");
    let task = fs::read_to_string(vault.join("cash.md")).expect("read project");
    assert!(task.contains("[*] #task Scheduled resident"), "{task}");
    assert!(!task.contains("[scheduled:: 2026-10-20]"), "{task}");
    let day = fs::read_to_string(day_file).expect("read daily note");
    assert!(day.contains("[[cash#^resident]]"), "{day}");
}

#[test]
fn link_toggle_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-toggle-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);

    let json = capture_json(&vault, &day_file, "@cash+resident!");
    assert_eq!(json["toggle_direction"], "link", "{json}");
    assert_pulled_forward(&vault, &day_file, &json);
}

#[test]
fn pomodoro_link_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-link-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);

    let json = capture_json(&vault, &day_file, "@cash:resident");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(json["pomodoro_link_action"], "linked", "{json}");
    assert_pulled_forward(&vault, &day_file, &json);
}

#[test]
fn ensure_next_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-ensure-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^resident]]\n",
        ),
    );

    let json = capture_json(&vault, &day_file, "@cash+resident");
    assert_eq!(json["toggle_behavior"], "ensure_next", "{json}");
    assert_pulled_forward(&vault, &day_file, &json);
}

#[test]
fn close_link_pulls_forward_a_scheduled_resident_of_an_unblocked_project() {
    let temp = TempDir::new("bob-cli-link-guard-pdg2-close-resident");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_unblocked_project_with_future_resident(&vault, &day_file);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
        ),
    );

    let json = capture_json(&vault, &day_file, "@cash:resident=x");
    assert!(json.get("pomodoro_close").is_some(), "{json}");
    assert!(json.get("link_guard").is_none(), "{json}");
    let task = fs::read_to_string(vault.join("cash.md")).expect("read project");
    assert!(!task.contains("[scheduled:: 2026-10-20]"), "{task}");
    let day = fs::read_to_string(day_file).expect("read daily note");
    assert!(day.contains("[[cash#^resident]]"), "{day}");
}

fn write_tagged_child_tree(
    vault: &std::path::Path,
    day_file: &std::path::Path,
) {
    write_toggle_task_settings(vault);
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Prep work [id:: job__prep] ^prep\n",
    );
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n\n",
            "- [ ] #task Parent #prj #hide [dependsOn:: job__prep] ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "parent: \"[[parent]]\"\n",
            "---\n\n",
            "- [?] #task Child #prj #hide #prj_is_blocked ^prj\n",
            "- [?] #task Work #prj_is_blocked ^work\n",
        ),
    );
    write_file(day_file, "## Pomodoros\n- [ ] () — CURRENT\n  - context\n");
}

#[test]
fn own_dependency_drop_is_refused_for_a_project_blocked_resident() {
    let temp = TempDir::new("bob-cli-link-guard-project-drop-refusal");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_tagged_child_tree(&vault, &day_file);
    write_file(
        &vault.join("local.md"),
        "- [ ] #task Local prerequisite [id:: local__prep] ^prep\n",
    );
    let child_path = vault.join("child.md");
    let child_before = concat!(
        "---\n",
        "type: \"[[project]]\"\n",
        "status: wip\n",
        "parent: \"[[parent]]\"\n",
        "---\n\n",
        "- [?] #task Child #prj #hide #prj_is_blocked ^prj\n",
        "- [?] #task Work #prj_is_blocked [id:: child__work] [dependsOn:: local__prep] ^work\n",
        "\t- ⛓️ **DEPENDS ON:** [[local#^prep]]\n",
    );
    write_file(&child_path, child_before);
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — CURRENT\n  - [[child#^work]]\n  - context\n",
    );

    let json = capture_json(&vault, &day_file, "@child:work");
    assert_eq!(json["kind"], "link_guard", "{json}");
    assert_eq!(
        json["link_guard"]["drop_refused"], "project_blocked",
        "{json}"
    );
    assert!(
        json["link_guard"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("dependencies kept: its project is blocked"),
        "{json}"
    );
    let after = fs::read_to_string(child_path).expect("read child");
    assert_eq!(after, child_before);
    let day = fs::read_to_string(day_file).expect("read day");
    assert!(!day.contains("[[child#^work]]"), "{day}");
    assert!(day.contains("context"), "{day}");
}

#[test]
fn own_dependency_drop_is_refused_for_a_project_task() {
    let temp = TempDir::new("bob-cli-link-guard-project-task-drop-refusal");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Prep [id:: job__prep] ^prep\n",
    );
    let project_before = concat!(
        "---\n",
        "type: \"[[project]]\"\n",
        "status: wip\n",
        "---\n\n",
        "- [?] #task Ship #prj #hide [id:: cash__prj] [dependsOn:: job__prep] ^prj\n",
        "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
    );
    write_file(&vault.join("cash.md"), project_before);
    write_file(&day_file, "## Pomodoros\n- [ ] () — CURRENT\n  - context\n");

    let json = capture_json(&vault, &day_file, "@cash:prj");
    assert_eq!(json["kind"], "link_guard", "{json}");
    assert_eq!(json["link_guard"]["drop_refused"], "project_task", "{json}");
    assert!(
        json["link_guard"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("edit a project's prerequisites from its ^prj Task Card"),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read project"),
        project_before
    );
}

// PDG15 (§13.15): a link gesture on a tagged child's `^prj` or resident
// does not link. Capture names the owning ancestor and promotes that
// ancestor's open projected prerequisites.
#[test]
fn capture_toggle_on_tagged_child_resident_promotes_ancestor() {
    let temp = TempDir::new("bob-cli-link-guard-pdg15-toggle");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_tagged_child_tree(&vault, &day_file);

    let json = capture_json(&vault, &day_file, "@child+work!");
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["project_dependency"]),
        "{json}"
    );
    assert_eq!(
        json["link_guard"]["promoted"],
        serde_json::json!(["job.md#^prep"]),
        "{json}"
    );
    let detail = json["link_guard"]["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("parent.md"), "{json}");
    assert!(!fs::read_to_string(&day_file)
        .expect("read day")
        .contains("[[child#^work]]"));
    assert!(
        fs::read_to_string(vault.join("job.md"))
            .expect("read job")
            .contains("- [*] #task Prep work"),
        "ancestor prerequisite rises to Next"
    );
}

#[test]
fn capture_toggle_on_tagged_child_prj_promotes_ancestor() {
    let temp = TempDir::new("bob-cli-link-guard-pdg15-prj");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_tagged_child_tree(&vault, &day_file);

    let json = capture_json(&vault, &day_file, "@child+prj!");
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["project_dependency"]),
        "{json}"
    );
    assert_eq!(
        json["link_guard"]["promoted"],
        serde_json::json!(["job.md#^prep"]),
        "{json}"
    );
    let detail = json["link_guard"]["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("parent.md"), "{json}");
}

#[test]
fn capture_pomodoro_link_on_tagged_child_is_not_linked() {
    let temp = TempDir::new("bob-cli-link-guard-pdg15-link");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_tagged_child_tree(&vault, &day_file);

    let json = capture_json(&vault, &day_file, "@child:work");
    assert_eq!(json["kind"], "link_guard", "{json}");
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["project_dependency"]),
        "{json}"
    );
    assert_eq!(
        json["link_guard"]["promoted"],
        serde_json::json!(["job.md#^prep"]),
        "{json}"
    );
    assert!(json.get("block_link").is_none(), "{json}");
}

// PDG16 (§13.15): a new `:` task captured into a tagged project lands
// `[?]` plus the tag, stays unlinked, and reports the ancestor reason.
#[test]
fn capture_new_task_into_tagged_project_is_unlinked() {
    let temp = TempDir::new("bob-cli-link-guard-pdg16");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_tagged_child_tree(&vault, &day_file);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Grind")
        .arg("@child:grind")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("capture into tagged child");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["project_dependency"]),
        "{json}"
    );
    assert_eq!(
        json["link_guard"]["promoted"],
        serde_json::json!(["job.md#^prep"]),
        "{json}"
    );
    let task_line = json["task_line"].as_str().unwrap_or_default();
    assert!(task_line.starts_with("- [?] "), "{json}");
    assert!(task_line.contains("#prj_is_blocked"), "{json}");
    assert!(json.get("block_link").is_none(), "{json}");
    assert!(!fs::read_to_string(&day_file)
        .expect("read day")
        .contains("[[child#^grind]]"));
}

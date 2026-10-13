//! Reconcile cascade vectors copied from `docs/task-dependencies.md` §13.19.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path};

fn reconcile_json(vault: &Path, daily: &Path) -> std::process::Output {
    bob_command()
        .args(["task", "reconcile", "--dry-run", "--format", "json"])
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_NOW", "2026-07-16")
        .output()
        .expect("run reconcile dry-run")
}

fn parse_json(output: &std::process::Output) -> Value {
    assert_success(output);
    serde_json::from_str(stdout(output).trim()).unwrap()
}

fn standard_vault(temp: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_blocked_tasks_settings(&vault);
    (vault, daily)
}

#[test]
fn reconcile_cascades_reasons_and_tags_through_multiple_project_levels() {
    let temp = TempDir::new("bob-cli-task-status-hooks-pds-cascade");
    let (vault, daily) = standard_vault(&temp);
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task #prj Parent #hide [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
            "\t- 🧩 **Sub-projects:** [[child]]\n",
            "- [ ] #task Parent resident ^parent-res\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "parent: \"[[parent|Parent alias]]\"\n",
            "---\n",
            "- [ ] #task #prj Child #hide ^prj\n",
            "  - 🧩 **Sub-projects:** [[grandchild]]\n",
            "- [ ] #task #ref Child ref resident ^child-ref\n",
            "  - [ ] #task Nested child resident ^nested\n",
        ),
    );
    write_file(
        &vault.join("grandchild.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "parent: \"[[child#Context]]\"\n",
            "---\n",
            "- [ ] #task #prj Grandchild #hide ^prj\n",
            "- [ ] #task Grandchild resident ^grandchild-res\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Prerequisite ^pre\n");

    // PDS1/PDS2/PDS15: ancestor dependency reasons name the owner all the way
    // down; nested and #ref residents receive the same derived block/tag.
    let reconcile = reconcile_json(&vault, &daily);
    let output = parse_json(&reconcile);
    let marked = output["marked_blocked"].as_array().unwrap();
    for (path, block_id) in [
        ("child.md", "prj"),
        ("child.md", "child-ref"),
        ("child.md", "nested"),
        ("grandchild.md", "prj"),
        ("grandchild.md", "grandchild-res"),
    ] {
        let row = marked
            .iter()
            .find(|row| row["path"] == path && row["block_id"] == block_id);
        let row = row.unwrap_or_else(|| {
            panic!("missing {path}#{block_id}: {}", stdout(&reconcile))
        });
        assert_eq!(
            row["reason_kinds"],
            serde_json::json!(["project_dependency"])
        );
        assert_eq!(row["project_path"], "parent.md");
        assert_eq!(
            row["project_open_dependency_ids"],
            serde_json::json!(["tasks__pre"])
        );
    }
    let added = output["prj_is_blocked_tags_added"].as_array().unwrap();
    for (path, block_id) in [
        ("child.md", "prj"),
        ("child.md", "child-ref"),
        ("child.md", "nested"),
        ("grandchild.md", "prj"),
        ("grandchild.md", "grandchild-res"),
    ] {
        assert!(
            added.iter().any(|row| {
                row["path"] == path && row["block_id"] == block_id
            }),
            "missing tag addition for {path}#{block_id}"
        );
    }

    let human = bob_command()
        .args(["task", "reconcile", "--dry-run"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2026-07-16")
        .output()
        .expect("run human reconcile dry-run");
    assert_success(&human);
    assert!(
        stdout(&human).contains("project: parent.md"),
        "{}",
        stdout(&human)
    );
}

#[test]
fn reconcile_recovers_subprojects_and_keeps_nearest_remaining_reason() {
    // PDS4: an unblocked ancestor recovers descendants and writes the
    // freshness tickler on eligible Ready landings.
    let ready_temp = TempDir::new("bob-cli-task-status-hooks-pds-recover");
    let (ready_vault, ready_daily) = standard_vault(&ready_temp);
    write_file(
        &ready_vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [?] #task #prj Parent [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
        ),
    );
    write_file(
        &ready_vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [?] #task #prj #prj_is_blocked Child [fresh:: 2026-07-15] ^prj\n",
            "- [?] #task #prj_is_blocked Resident [fresh:: 2026-07-15] ^resident\n",
        ),
    );
    write_file(
        &ready_vault.join("tasks.md"),
        "- [x] #task Prerequisite ^pre\n",
    );
    let recovered = parse_json(&reconcile_json(&ready_vault, &ready_daily));
    for block_id in ["prj", "resident"] {
        assert!(recovered["unblocked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["path"] == "child.md" && row["block_id"] == block_id
            }));
        assert!(recovered["prj_is_blocked_tags_stripped"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |row| row["path"] == "child.md" && row["block_id"] == block_id
            ));
        assert!(recovered["unblocked_fields_written"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |row| row["path"] == "child.md" && row["block_id"] == block_id
            ));
    }

    // PDS5: after the ancestor clears, the child's own Y reason remains.
    // The child's ^prj loses only the inherited tag; the grandchild keeps
    // the tag and reports the nearer child as its blocking project.
    let own_temp = TempDir::new("bob-cli-task-status-hooks-pds-nearest");
    let (own_vault, own_daily) = standard_vault(&own_temp);
    write_file(
        &own_vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [?] #task #prj Parent [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
        ),
    );
    write_file(
        &own_vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [?] #task #prj #prj_is_blocked Child [dependsOn:: tasks__own] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^own]]\n",
            "- [?] #task #prj_is_blocked Child resident ^child-res\n",
        ),
    );
    write_file(
        &own_vault.join("grandchild.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[child]]\n---\n",
            "- [ ] #task #prj Grandchild ^prj\n",
        ),
    );
    write_file(
        &own_vault.join("tasks.md"),
        "- [x] #task Ancestor prerequisite ^pre\n- [ ] #task Child prerequisite ^own\n",
    );
    let nearest = parse_json(&reconcile_json(&own_vault, &own_daily));
    assert!(nearest["prj_is_blocked_tags_stripped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["path"] == "child.md" && row["block_id"] == "prj"));
    assert!(!nearest["prj_is_blocked_tags_stripped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["path"] == "grandchild.md" && row["block_id"] == "prj"));
    assert!(nearest["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| {
            row["path"] == "grandchild.md"
                && row["block_id"] == "prj"
                && row["project_path"] == "child.md"
                && row["reason_kinds"]
                    == serde_json::json!(["project_dependency"])
        }));
}

#[test]
fn descendant_project_targets_stay_unprojected_and_warn() {
    let temp = TempDir::new("bob-cli-task-status-hooks-pds-internal");
    let (vault, daily) = standard_vault(&temp);
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [ ] #task #prj Parent [dependsOn:: child__work] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[child#^work]]\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [ ] #task #prj Child ^prj\n",
            "- [ ] #task Child work ^work\n",
        ),
    );

    // PDS8: a parent ^prj link into a descendant remains hand-authored,
    // never contributes an id field or Blocked reason, and warns by project.
    let output = parse_json(&reconcile_json(&vault, &daily));
    assert!(output["dependency_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| {
            row["kind"] == "project_internal_dependency"
                && row["detail"]
                    .as_str()
                    .unwrap()
                    .contains("inside a sub-project")
        }));
    assert!(output["marked_blocked"].as_array().unwrap().is_empty());
    let parent = fs::read_to_string(vault.join("parent.md")).unwrap();
    assert!(parent.contains("[[child#^work]]"));
}

#[test]
fn project_parent_cycles_warn_once_and_only_inherit_other_projects_reasons() {
    let temp = TempDir::new("bob-cli-task-status-hooks-pds-cycle");
    let (vault, daily) = standard_vault(&temp);
    write_file(
        &vault.join("A.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[B]]\n---\n",
            "- [ ] #task #prj A [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
        ),
    );
    write_file(
        &vault.join("B.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[A]]\n---\n",
            "- [ ] #task #prj B ^prj\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Prerequisite ^pre\n");

    // PDS9: A never inherits its own reason through the cycle; B inherits A.
    let output = parse_json(&reconcile_json(&vault, &daily));
    let warnings = output["dependency_warnings"].as_array().unwrap();
    let cycles = warnings
        .iter()
        .filter(|row| row["kind"] == "project_parent_cycle")
        .collect::<Vec<_>>();
    assert_eq!(
        cycles.len(),
        1,
        "{}",
        stdout(&reconcile_json(&vault, &daily))
    );
    let detail = cycles[0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("A.md") && detail.contains("B.md"),
        "{detail}"
    );
    assert!(output["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| {
            row["path"] == "B.md"
                && row["block_id"] == "prj"
                && row["project_path"] == "A.md"
                && row["reason_kinds"]
                    == serde_json::json!(["project_dependency"])
        }));
}

#[test]
fn inherited_edges_participate_in_r7_cycle_detection() {
    let temp = TempDir::new("bob-cli-task-status-hooks-pds-r7-cycle");
    let (vault, daily) = standard_vault(&temp);
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [ ] #task #prj Parent [dependsOn:: tasks__y] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^y]]\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [ ] #task #prj Child ^prj\n",
            "- [ ] #task Child resident ^resident\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Y [dependsOn:: child__resident] ^y\n",
            "  - ⛓️ **DEPENDS ON:** [[child#^resident]]\n",
        ),
    );

    // PDS11: child resident → ancestor prerequisite plus Y → child resident
    // forms an R7 dependency cycle.
    let output = parse_json(&reconcile_json(&vault, &daily));
    assert!(
        output["dependency_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row["kind"] == "dependency_cycle" }),
        "{}",
        stdout(&reconcile_json(&vault, &daily))
    );
}

#[test]
fn reconciled_cascade_tree_has_no_second_run_changes() {
    let temp = TempDir::new("bob-cli-task-status-hooks-pds-idempotent");
    let (vault, daily) = standard_vault(&temp);
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [?] #task #prj Parent [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
            "- [?] #task #prj_is_blocked Parent resident ^parent-res\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [?] #task #prj #prj_is_blocked Child ^prj\n",
            "- [?] #task #prj_is_blocked Child resident ^child-res\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Prerequisite ^pre\n");

    // PDS12: a tree already reconciled to the derived statuses and tags stays
    // unchanged on consecutive reconcile runs.
    for _ in 0..2 {
        let output = parse_json(&reconcile_json(&vault, &daily));
        for key in [
            "marked_blocked",
            "unblocked",
            "prj_is_blocked_tags_added",
            "prj_is_blocked_tags_stripped",
            "consumed_blocked_references",
        ] {
            assert_eq!(output[key], serde_json::json!([]), "{key}: {output}");
        }
    }
}

#[test]
fn blocked_subproject_links_are_consumed_and_promote_ancestor_prerequisites() {
    let temp = TempDir::new("bob-cli-task-status-hooks-pds-consume");
    let (vault, daily) = standard_vault(&temp);
    write_file(
        &daily,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[child#^prj]]\n",
            "  - [[child#^resident]]\n",
        ),
    );
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [ ] #task #prj Parent [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [*] #task #prj Child ^prj\n",
            "- [ ] #task Resident ^resident\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Ancestor prerequisite ^pre\n",
    );

    // PDS10/PDS13: both live links disappear; the Next ^prj loses its lane,
    // and the ancestor's prerequisite rises through the implicit edges.
    let output = parse_json(&reconcile_json(&vault, &daily));
    assert_eq!(
        output["consumed_blocked_references"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(output["marked_blocked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| {
            row["path"] == "child.md"
                && row["block_id"] == "prj"
                && row["from"] == "*"
        }));
    assert!(output["marked_next"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| { row["path"] == "tasks.md" && row["block_id"] == "pre" }));
    assert!(output["consumed_blocked_references"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| {
            row["reasons"] == serde_json::json!(["project_dependency"])
        }));
}

#[test]
fn projects_sync_ignores_cascade_tags_and_keeps_marker_state() {
    let temp = TempDir::new("bob-cli-task-status-hooks-pds-project-sync");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [?] #task #prj Parent #hide [dependsOn:: tasks__pre] ^prj\n",
            "  - ⛓️ **DEPENDS ON:** [[tasks#^pre]]\n",
            "\t- 🧩 **Sub-projects:** [[child]]\n",
            "- [?] #task #prj_is_blocked Parent resident ^parent-res\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [?] #task #prj #hide #prj_is_blocked Child ^prj\n",
            "- [?] #task #prj_is_blocked Child resident ^child-res\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Pre ^pre\n");
    let before = ["parent.md", "child.md"]
        .map(|name| fs::read_to_string(vault.join(name)).unwrap());

    // PDS14: project sync sees the same open marker and does not rewrite
    // descriptions or project visibility because the machine tag is present.
    let output = bob_command()
        .args(["projects", "sync", "--dry-run", "--bob-dir"])
        .arg(&vault)
        .output()
        .expect("run projects sync dry-run");
    assert_success(&output);
    assert!(!stdout(&output).contains("would "), "{}", stdout(&output));
    for (name, original) in ["parent.md", "child.md"].into_iter().zip(before) {
        assert_eq!(fs::read_to_string(vault.join(name)).unwrap(), original);
    }
}

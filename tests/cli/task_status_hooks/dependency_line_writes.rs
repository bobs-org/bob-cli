//! Writer-path Depends-On tests: DW adoption slots, the DW5/R9 label-only
//! removal, DW3/DW6 link order, and the dependency summary counts
//! (`docs/task-dependencies.md` §§11.2–11.3, DR vectors). Shared
//! fixtures live in the sibling `dependency_lines` module.

use super::dependency_lines::{
    assert_reconciled_clean, dry_run_json, warning_kinds, write_daily,
};
use crate::support::*;
use std::fs;

#[test]
fn reconcile_dw1_inserts_adopted_line_before_schedule_log() {
    // DW1 (`docs/task-dependencies.md` §11.2): adoption creates the line
    // as the first child — here before a `🗓️ **SCHEDULE LOG**` child —
    // with the existing child indent. The adopted links keep field order
    // (DW3 append order through the engine's create path).
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-dw1");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Adopter [dependsOn:: tasks__one, tasks__two] ^adopt\n",
            "  - 🗓️ **SCHEDULE LOG**\n",
            "- [ ] #task One [id:: tasks__one] ^one\n",
            "- [ ] #task Two [id:: tasks__two] ^two\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let adopted = json["adopted_dependency_lines"].as_array().unwrap();
    assert_eq!(adopted.len(), 1);
    assert_eq!(adopted[0]["path"], "tasks.md");
    assert_eq!(adopted[0]["line"], 2);
    assert_eq!(
        adopted[0]["detail"],
        "  - ⛓️ **DEPENDS ON:** [[#^one]] • [[#^two]]"
    );
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply DW1 adoption");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [?] #task Adopter [dependsOn:: tasks__one, tasks__two] ^adopt\n",
            "  - ⛓️ **DEPENDS ON:** [[#^one]] • [[#^two]]\n",
            "  - 🗓️ **SCHEDULE LOG**\n",
            "- [ ] #task One [id:: tasks__one] ^one\n",
            "- [ ] #task Two [id:: tasks__two] ^two\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_dw2_inserts_adopted_line_after_cancel_log() {
    // DW2 (`docs/task-dependencies.md` §11.2): with a `❌ **CANCEL LOG**`
    // child holding the first slot, the adopted line lands second — after
    // the log, before the prose child — reusing the child indent (the
    // engine's `child_slot` / `is_cancel_log_line` slot logic).
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-dw2");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Adopter [dependsOn:: tasks__one] ^adopt\n",
            "  - ❌ **CANCEL LOG**\n",
            "  - prose child\n",
            "- [ ] #task One [id:: tasks__one] ^one\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let adopted = json["adopted_dependency_lines"].as_array().unwrap();
    assert_eq!(adopted.len(), 1);
    assert_eq!(adopted[0]["path"], "tasks.md");
    assert_eq!(adopted[0]["line"], 3);
    assert_eq!(adopted[0]["detail"], "  - ⛓️ **DEPENDS ON:** [[#^one]]");
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply DW2 adoption");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [?] #task Adopter [dependsOn:: tasks__one] ^adopt\n",
            "  - ❌ **CANCEL LOG**\n",
            "  - ⛓️ **DEPENDS ON:** [[#^one]]\n",
            "  - prose child\n",
            "- [ ] #task One [id:: tasks__one] ^one\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_dw5_label_only_line_deletes_line_and_field() {
    // DW5/R9/DR16 (`docs/task-dependencies.md` §§4.2, 11.2): a label-only
    // line with nothing behind it is deleted with the field. The ghost id
    // matches no block anywhere, so there is nothing to adopt; the line
    // goes (`line_removed`) and the field goes with it (`dependsOn
    // removed`, warned as `dependency_field_ids_dropped`).
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-dw5");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Bare [dependsOn:: tasks__ghost] ^bare\n",
            "  - ⛓️ **DEPENDS ON:**\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let updates = json["dependency_projection_updates"].as_array().unwrap();
    assert!(updates.iter().any(|item| item["kind"] == "line_removed"));
    assert!(updates.iter().any(|item| {
        item["kind"] == "dependent_field"
            && item["detail"] == "dependsOn removed"
    }));
    assert_eq!(warning_kinds(&json), ["dependency_field_ids_dropped"]);

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply DW5 removal");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [ ] #task Bare ^bare\n"
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_dw5_label_only_line_with_adoptable_id_deletes_line_and_field() {
    // DW5/R9/DR16 (`docs/task-dependencies.md` §§4.2, 11.2): a label-only
    // line is the source of truth — with no legacy children the line is
    // deleted with the field even when the field id is adoptable (a `^a`
    // task exists). The hooks must not re-adopt onto the line.
    let temp =
        TempDir::new("bob-cli-task-status-hooks-reconcile-dw5-adoptable");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Bare [dependsOn:: tasks__a] ^bare\n",
            "  - ⛓️ **DEPENDS ON:**\n",
            "- [ ] #task A [id:: tasks__a] ^a\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let updates = json["dependency_projection_updates"].as_array().unwrap();
    assert!(updates.iter().any(|item| item["kind"] == "line_removed"));
    assert!(updates.iter().any(|item| {
        item["kind"] == "dependent_field"
            && item["detail"] == "dependsOn removed"
    }));
    assert_eq!(warning_kinds(&json), ["dependency_field_ids_dropped"]);
    assert!(json["adopted_dependency_lines"]
        .as_array()
        .unwrap()
        .is_empty());

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply DW5 removal with adoptable id");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [ ] #task Bare ^bare\n",
            "- [ ] #task A [id:: tasks__a] ^a\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_dw3_dw6_rewrite_keeps_link_order() {
    // DW3/DW6 through the reconcile writer (`docs/task-dependencies.md`
    // §11.2): a non-canonical two-link line canonicalises with its links
    // in line order — the writer renders the given order verbatim and
    // never re-sorts — and the field keeps that order.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-order");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent [dependsOn:: tasks__b, tasks__a] ^dep\n",
            "  - 🔗 **DEPENDENCIES:** [[#^b]], [[#^a]]\n",
            "- [ ] #task A [id:: tasks__a] ^a\n",
            "- [ ] #task B [id:: tasks__b] ^b\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let canonicalized =
        json["canonicalized_dependency_lines"].as_array().unwrap();
    assert_eq!(canonicalized.len(), 1);
    assert_eq!(
        canonicalized[0]["detail"],
        "  - ⛓️ **DEPENDS ON:** [[#^b]] • [[#^a]]"
    );
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply order-preserving canonicalisation");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [?] #task Dependent [dependsOn:: tasks__b, tasks__a] ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]] • [[#^a]]\n",
            "- [ ] #task A [id:: tasks__a] ^a\n",
            "- [ ] #task B [id:: tasks__b] ^b\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn summary_reports_dependency_counts() {
    // The `Dependencies:` and `Summary:` human lines carry the dependency
    // projection counts; the JSON report and the human report must agree.
    let temp = TempDir::new("bob-cli-task-status-hooks-summary-deps");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [ ] #task Prerequisite ^pre\n",
            "- [ ] #task Adopter [dependsOn:: tasks__one] ^adopt\n",
            "- [ ] #task One [id:: tasks__one] ^one\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(
        json["dependency_projection_updates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        json["adopted_dependency_lines"].as_array().unwrap().len(),
        1
    );
    assert!(json["healed_dependency_links"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(json["canonicalized_dependency_lines"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(json["legacy_dependency_children"], 0);
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .args(["task", "reconcile"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply summary-counts run");
    assert_success(&applied);
    let report = stdout(&applied);
    assert!(
        report.contains(
            "Dependencies: 2 projected, 1 adopted, 0 healed, 0 canonicalized, 0 legacy children, 0 warnings"
        ),
        "unexpected Dependencies line:\n{report}"
    );
    assert!(
        report.contains(
            "2 dependency projected, 1 adopted, 0 healed, 0 canonicalized, 0 legacy children, 0 warnings"
        ),
        "unexpected Summary line:\n{report}"
    );
}

//! Promotion through Depends-On lines, plus R1–R10 reconciliation
//! (`docs/task-dependencies.md` §§4.2–4.3, DR vectors).

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn dry_run_json(vault: &Path, daily: &Path) -> Value {
    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .output()
        .expect("dry-run task-status-hooks");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON")
}

fn write_daily(vault: &Path) -> std::path::PathBuf {
    let daily = vault.join("2026/20260716.md");
    write_file(
        &daily,
        concat!("## Pomodoros\n\n", "- [ ] Current (0900-0930)\n"),
    );
    daily
}

fn warning_kinds(json: &Value) -> Vec<String> {
    json["dependency_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["kind"].as_str().unwrap().to_string())
        .collect()
}

fn assert_reconciled_clean(json: &Value) {
    assert!(
        json["dependency_projection_updates"]
            .as_array()
            .unwrap()
            .is_empty(),
        "unexpected projection updates: {}",
        json["dependency_projection_updates"]
    );
    assert!(
        json["adopted_dependency_lines"]
            .as_array()
            .unwrap()
            .is_empty(),
        "unexpected adoptions: {}",
        json["adopted_dependency_lines"]
    );
    assert!(
        json["healed_dependency_links"]
            .as_array()
            .unwrap()
            .is_empty(),
        "unexpected heals: {}",
        json["healed_dependency_links"]
    );
    assert!(
        json["canonicalized_dependency_lines"]
            .as_array()
            .unwrap()
            .is_empty(),
        "unexpected canonicalizations: {}",
        json["canonicalized_dependency_lines"]
    );
}

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

#[test]
fn reconcile_projects_line_into_field_and_stamps_target_id() {
    // DR1/DR2: a well-formed line with a resolved open target projects
    // the field in line order; the target gains `[id::]`. The second
    // run is a no-op (DR22).
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r1");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [ ] #task Prerequisite ^pre\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [ ] #task Dependent ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [ ] #task Prerequisite ^pre\n",
        )
    );
    let updates = json["dependency_projection_updates"].as_array().unwrap();
    assert!(updates.iter().any(|item| {
        item["kind"] == "dependent_field"
            && item["path"] == "tasks.md"
            && item["detail"]
                .as_str()
                .unwrap()
                .contains("dependsOn := tasks__pre")
    }));
    assert!(updates.iter().any(|item| {
        item["kind"] == "target_id"
            && item["path"] == "tasks.md"
            && item["detail"] == "[id:: tasks__pre]"
    }));
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply R1 projection");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [?] #task Dependent [dependsOn:: tasks__pre] ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [ ] #task Prerequisite [id:: tasks__pre] ^pre\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_drops_stale_field_id() {
    // DR3: a field id no link or legacy child accounts for is dropped,
    // reported in the projection detail, and warned about
    // (`dependency_field_ids_dropped`, `docs/task-dependencies.md` §4).
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r1-drop");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent [dependsOn:: tasks__pre, tasks__ghost] ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [x] #task Prerequisite [id:: tasks__pre] ^pre\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let updates = json["dependency_projection_updates"].as_array().unwrap();
    assert!(updates.iter().any(|item| {
        item["kind"] == "dependent_field"
            && item["detail"]
                .as_str()
                .unwrap()
                .contains("dropped tasks__ghost")
    }));
    assert!(
        warning_kinds(&json)
            .contains(&"dependency_field_ids_dropped".to_string()),
        "dropped ids must warn: {}",
        json["dependency_warnings"]
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply stale-id drop");
    assert_success(&applied);
    assert!(fs::read_to_string(&tasks)
        .unwrap()
        .contains("- [ ] #task Dependent [dependsOn:: tasks__pre] ^dep\n"));

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_adopts_field_ids_into_canonical_line() {
    // DR4/DR20: field ids not covered by legacy children gain a
    // canonical first-child line; the covered legacy child is never
    // rewritten.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r2");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Adopter [dependsOn:: tasks__one, tasks__two] ^adopt\n",
            "  - ![[#^two]]\n",
            "- [ ] #task One [id:: tasks__one] ^one\n",
            "- [ ] #task Two [id:: tasks__two] ^two\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let adopted = json["adopted_dependency_lines"].as_array().unwrap();
    assert_eq!(adopted.len(), 1);
    assert_eq!(adopted[0]["path"], "tasks.md");
    assert_eq!(adopted[0]["detail"], "  - ⛓️ **DEPENDS ON:** [[#^one]]");
    assert_eq!(json["legacy_dependency_children"], 1);

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply R2 adoption");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [?] #task Adopter [dependsOn:: tasks__one, tasks__two] ^adopt\n",
            "  - ⛓️ **DEPENDS ON:** [[#^one]]\n",
            "  - ![[#^two]]\n",
            "- [ ] #task One [id:: tasks__one] ^one\n",
            "- [ ] #task Two [id:: tasks__two] ^two\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
}

#[test]
fn reconcile_warns_unadoptable_id_and_keeps_field() {
    // DR5: a field id with no `^block-id` anywhere stays in the field
    // with an `unadoptable_dependency_id` warning and never blocks.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r2-ghost");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = "- [ ] #task Lonely [dependsOn:: tasks__ghost] ^lonely\n";
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(json["adopted_dependency_lines"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(warning_kinds(&json), ["unadoptable_dependency_id"]);
    assert!(json["marked_blocked"].as_array().unwrap().is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply unadoptable run");
    assert_success(&applied);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
}

#[test]
fn reconcile_heals_moved_link() {
    // DR6: a link that no longer resolves heals when exactly one
    // scanned task carries its block id and an unaccounted field id.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r3");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let dependent = vault.join("a.md");
    let target = vault.join("b.md");
    write_file(
        &dependent,
        concat!(
            "- [ ] #task Dependent [dependsOn:: b__moved] ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[old#^moved]]\n",
        ),
    );
    write_file(&target, "- [ ] #task Moved [id:: b__moved] ^moved\n");
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let healed = json["healed_dependency_links"].as_array().unwrap();
    assert_eq!(healed.len(), 1);
    assert_eq!(healed[0]["path"], "a.md");
    assert!(healed[0]["detail"]
        .as_str()
        .unwrap()
        .contains("[[b#^moved]]"));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply R3 heal");
    assert_success(&applied);
    assert!(fs::read_to_string(&dependent)
        .unwrap()
        .contains("  - ⛓️ **DEPENDS ON:** [[b#^moved]]\n"));

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_keeps_unresolved_link_and_breadcrumbs() {
    // DR7/DR8: an unhealable link stays verbatim, never blocks, and the
    // unaccounted field ids stay on as heal breadcrumbs.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r4");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Cut [dependsOn:: tasks__pasted] ^cut\n",
        "  - ⛓️ **DEPENDS ON:** [[old#^pasted]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(warning_kinds(&json), ["unresolved_dependency_link"]);
    assert!(json["marked_blocked"].as_array().unwrap().is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply R4 keep");
    assert_success(&applied);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
}

#[test]
fn reconcile_warns_non_task_and_self_links_without_projecting() {
    // DR10/R5 + DR11/R6: non-task and self links are kept verbatim,
    // warned about, and never projected.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r5r6");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[#^ref]]\n",
            "- [ ] #task Loopy [dependsOn:: tasks__loopy] ^loopy\n",
            "  - ⛓️ **DEPENDS ON:** [[#^loopy]]\n",
            "\n",
            "Reference block only. ^ref\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let kinds = warning_kinds(&json);
    assert!(
        kinds.contains(&"non_task_dependency".to_string()),
        "{kinds:?}"
    );
    assert!(kinds.contains(&"self_dependency".to_string()), "{kinds:?}");
    assert!(json["marked_blocked"].as_array().unwrap().is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply R5/R6 run");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    // The self id is not projected, so the field is removed; both links
    // stay verbatim.
    assert!(after.contains("- [ ] #task Loopy ^loopy\n"), "{after}");
    assert!(after.contains("[[#^ref]]"), "{after}");
    assert!(after.contains("[[#^loopy]]"), "{after}");
}

#[test]
fn reconcile_warns_dependency_cycle_and_keeps_members_blocked() {
    // DR12/R7: two tasks linking to each other both stay Blocked with
    // one `dependency_cycle` warning carrying the path.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r7");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Alpha [dependsOn:: tasks__b] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
            "- [ ] #task Beta [dependsOn:: tasks__a] ^b\n",
            "  - ⛓️ **DEPENDS ON:** [[#^a]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let kinds = warning_kinds(&json);
    assert_eq!(kinds, ["dependency_cycle"]);
    let warning = &json["dependency_warnings"].as_array().unwrap()[0];
    assert!(
        warning["detail"].as_str().unwrap().contains("tasks.md#^a")
            && warning["detail"].as_str().unwrap().contains("tasks.md#^b"),
        "{}",
        warning["detail"]
    );
    assert_eq!(json["marked_blocked"].as_array().unwrap().len(), 2);
}

#[test]
fn reconcile_removes_empty_line_and_leaves_malformed_alone() {
    // DR16/R9: a label-only line with nothing behind it is deleted
    // with the field. DR17/R10: a malformed line warns and is left
    // alone.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-r9r10");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let messy = "  - ⛓️ **DEPENDS ON:** [[#^a]] needs review\n";
    write_file(
        &tasks,
        &format!(
            "{}{}{}{}",
            "- [ ] #task Bare ^bare\n",
            "  - ⛓️ **DEPENDS ON:**\n",
            "- [ ] #task Messy ^messy\n",
            messy,
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(warning_kinds(&json), ["malformed_dependency_line"]);
    let updates = json["dependency_projection_updates"].as_array().unwrap();
    assert!(updates.iter().any(|item| item["kind"] == "line_removed"));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply R9 removal");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(after.contains("- [ ] #task Bare ^bare\n"), "{after}");
    assert!(!after.contains("**DEPENDS ON:**\n"), "{after}");
    assert!(after.contains(messy), "{after}");

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    // The malformed line still warns; warnings are not changes.
    assert_eq!(warning_kinds(&second), ["malformed_dependency_line"]);
}

#[test]
fn reconcile_skips_closed_dependents() {
    // DR18: a closed dependent with a stale line is never touched.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-scope");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [x] #task Done [dependsOn:: tasks__ghost] ^done\n",
        "  - ⛓️ **DEPENDS ON:** [[old#^ghost]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&json);
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply scope run");
    assert_success(&applied);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
}

#[test]
fn reconcile_canonicalizes_legacy_line_variants() {
    // Canonicalise: legacy emoji, label, separator, embed, and alias
    // collapse to the writer form with the same targets.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-canon");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent [dependsOn:: tasks__pre] ^dep\n",
            "  - 🔗 **DEPENDENCIES:** ![[#^pre|alias]]\n",
            "- [ ] #task Prerequisite [id:: tasks__pre] ^pre\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    let canonicalized =
        json["canonicalized_dependency_lines"].as_array().unwrap();
    assert_eq!(canonicalized.len(), 1);
    assert_eq!(
        canonicalized[0]["detail"],
        "  - ⛓️ **DEPENDS ON:** [[#^pre]]"
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply canonicalisation");
    assert_success(&applied);
    assert!(fs::read_to_string(&tasks)
        .unwrap()
        .contains("  - ⛓️ **DEPENDS ON:** [[#^pre]]\n"));

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
}

#[test]
fn reconcile_keeps_archive_prerequisite_silently() {
    // DR19: a link into `done/` keeps its id, never warns, and never
    // blocks — and the explicit `done/` link form round-trips.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-archive");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let archive = vault.join("done/old.md");
    write_file(&archive, "- [x] #task Gone [id:: done__old__gone] ^gone\n");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task Dependent ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[done/old#^gone]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(warning_kinds(&json).is_empty());
    assert!(json["marked_blocked"].as_array().unwrap().is_empty());
    let updates = json["dependency_projection_updates"].as_array().unwrap();
    assert!(updates.iter().any(|item| {
        item["kind"] == "dependent_field"
            && item["detail"].as_str().unwrap().contains("done__old__gone")
    }));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply archive run");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(after.contains("[dependsOn:: done__old__gone]"), "{after}");
    assert!(after.contains("[[done/old#^gone]]"), "{after}");

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
    assert!(second["marked_blocked"].as_array().unwrap().is_empty());
}

#[test]
fn reconcile_same_index_insert_does_not_clobber_replace() {
    // Same-index edit ordering: an adopted Depends-On line inserted at
    // the same original index as a target `[id::]` stamp must land
    // before the rewritten line, not be overwritten by it. One live run
    // used to duplicate `^b` and lose A's adopted line.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-order");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task A [dependsOn:: tasks__c] ^a\n",
            "- [ ] #task B ^b\n",
            "- [ ] #task C [id:: tasks__c] ^c\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(
        json["adopted_dependency_lines"].as_array().unwrap().len(),
        1
    );
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply same-index reconcile");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [?] #task A [dependsOn:: tasks__c] ^a\n",
            "\t- ⛓️ **DEPENDS ON:** [[#^c]]\n",
            "- [ ] #task B [id:: tasks__b] ^b\n",
            "- [?] #task C [id:: tasks__c] [dependsOn:: tasks__b] ^c\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_dependency_chain_settles_in_one_run() {
    // Task-line rewrite merge: B is stamped with `[id::]` (as C's
    // prerequisite) and gains its own field (as A's dependent) in the
    // same run. The stamp used to be lost, leaving A unresolved until a
    // second run.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-chain");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task A [dependsOn:: tasks__b] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
            "- [ ] #task B ^b\n",
            "  - ⛓️ **DEPENDS ON:** [[#^c]]\n",
            "- [ ] #task C [id:: tasks__c] ^c\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply chain reconcile");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [?] #task A [dependsOn:: tasks__b] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
            "- [?] #task B [id:: tasks__b] [dependsOn:: tasks__c] ^b\n",
            "  - ⛓️ **DEPENDS ON:** [[#^c]]\n",
            "- [ ] #task C [id:: tasks__c] ^c\n",
        )
    );

    // One run settled the chain: the second run reports no new
    // Blocked transitions (A and B already are) and no warnings.
    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
    assert!(second["marked_blocked"].as_array().unwrap().is_empty());
}

#[test]
fn reconcile_daily_note_keeps_adoption_and_stamp() {
    // Current daily notes with no status change still keep their
    // reconcile edits: compose must write the reconciled daily
    // contents, not the pre-reconcile normalized ones.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-daily");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_file(
        &daily,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - notes here\n\n",
            "## Tasks\n\n",
            "- [?] #task Dep1 [dependsOn:: 2026__20260716__one] ^dep1\n",
            "- [ ] #task One [id:: 2026__20260716__one] ^one\n",
            "- [?] #task Dep2 ^dep2\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [ ] #task Pre ^pre\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(
        json["adopted_dependency_lines"].as_array().unwrap().len(),
        1
    );
    assert!(json["dependency_projection_updates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["kind"] == "target_id"));
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply daily reconcile");
    assert_success(&applied);
    let after = fs::read_to_string(&daily).unwrap();
    assert!(
        after.contains("\t- ⛓️ **DEPENDS ON:** [[#^one]]\n"),
        "{after}"
    );
    assert!(
        after.contains("- [ ] #task Pre [id:: 2026__20260716__pre] ^pre\n"),
        "{after}"
    );
    assert!(
        after.contains("[dependsOn:: 2026__20260716__pre]"),
        "{after}"
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_never_writes_previous_daily_target() {
    // `contract` §4.1: the previous daily snapshot is never written. A
    // target there keeps its link verbatim with a
    // `previous_daily_target` warning and never projects.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-prev");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let previous = vault.join("2026/20260715.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(&previous, "- [ ] #task Old ^t\n");
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Dep ^dep\n",
        "  - ⛓️ **DEPENDS ON:** [[20260715#^t]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(warning_kinds(&json), ["previous_daily_target"]);
    assert!(json["marked_blocked"].as_array().unwrap().is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply previous-daily run");
    assert_success(&applied);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    assert_eq!(
        fs::read_to_string(&previous).unwrap(),
        "- [ ] #task Old ^t\n"
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert_eq!(warning_kinds(&second), ["previous_daily_target"]);
}

#[test]
fn reconcile_field_writer_handles_trailing_tags() {
    // A `[dependsOn::]` before trailing tags is replaced in place,
    // never duplicated: the old trailing-only peeler appended a second
    // field after `#hide`, so the dependent never blocked and the edit
    // repeated every run.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-tags");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [ ] #task D [dependsOn:: tasks__old] #hide ^d\n",
            "  - ⛓️ **DEPENDS ON:** [[#^e]]\n",
            "- [ ] #task E [id:: tasks__e] ^e\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(
        warning_kinds(&json)
            .contains(&"dependency_field_ids_dropped".to_string()),
        "{}",
        json["dependency_warnings"]
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply trailing-tag run");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    assert_eq!(
        after.matches("[dependsOn::").count(),
        1,
        "exactly one dependsOn field: {after}"
    );
    assert!(
        after.contains("- [?] #task D #hide [dependsOn:: tasks__e] ^d\n"),
        "{after}"
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_archive_legacy_children_follow_archive_rule() {
    // Explicit `done/` legacy children follow §4.3 like any other
    // legacy child: their ids are kept, never warned about, and never
    // block — both for a line-owning and a field-only dependent.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-archleg");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let archive = vault.join("done/old.md");
    write_file(&archive, "- [x] #task Gone [id:: done__old__gone] ^gone\n");
    let before = concat!(
        "- [ ] #task Dep [dependsOn:: tasks__pre, done__old__gone] ^dep\n",
        "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
        "  - ![[done/old#^gone]]\n",
        "- [ ] #task Pre [id:: tasks__pre] ^pre\n",
        "- [ ] #task Lone [dependsOn:: done__old__gone] ^lone\n",
        "  - ![[done/old#^gone]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(json["legacy_dependency_children"], 2);
    assert!(
        warning_kinds(&json).is_empty(),
        "{}",
        json["dependency_warnings"]
    );
    assert!(
        json["marked_blocked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["block_id"] == "dep"
                    && item["open_dependency_ids"]
                        == serde_json::json!(["tasks__pre"])
            }),
        "{}",
        json["marked_blocked"]
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply archive-legacy run");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    // Only the Blocked checkbox changes; lines, fields, and legacy
    // children are byte-identical.
    assert_eq!(
        after,
        before.replacen("- [ ] #task Dep", "- [?] #task Dep", 1)
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_warns_unencodable_target_without_projecting() {
    // `contract` §3: a target whose path cannot encode (spaces) and
    // that has no `[id::]` keeps its link verbatim with an
    // `unencodable_dependency_target` warning and never projects.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-enc");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let target = vault.join("my notes/target.md");
    write_file(&target, "- [ ] #task Spaced ^t\n");
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Dep ^dep\n",
        "  - ⛓️ **DEPENDS ON:** [[my notes/target#^t]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(warning_kinds(&json), ["unencodable_dependency_target"]);
    assert!(json["marked_blocked"].as_array().unwrap().is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply unencodable run");
    assert_success(&applied);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "- [ ] #task Spaced ^t\n"
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert_eq!(warning_kinds(&second), ["unencodable_dependency_target"]);
}

#[test]
fn reconcile_unblocks_when_last_open_prerequisite_leaves() {
    // Removing the last open prerequisite's link unblocks the `[?]`
    // dependent in the same run: the stale field id drops and the
    // closed prerequisite never blocks.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-unblock");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    write_file(
        &tasks,
        concat!(
            "- [?] #task A [dependsOn:: tasks__pre, tasks__done] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^done]]\n",
            "- [ ] #task Pre [id:: tasks__pre] ^pre\n",
            "- [x] #task Done [id:: tasks__done] ^done\n",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(
        warning_kinds(&json)
            .contains(&"dependency_field_ids_dropped".to_string()),
        "{}",
        json["dependency_warnings"]
    );
    assert_eq!(json["unblocked"].as_array().unwrap().len(), 1);

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply unblock run");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [ ] #task A [dependsOn:: tasks__done] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^done]]\n",
            "- [ ] #task Pre [id:: tasks__pre] ^pre\n",
            "- [x] #task Done [id:: tasks__done] ^done\n",
        )
    );

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_breadcrumb_heals_when_target_returns() {
    // DR8/DR9: an unhealable link keeps its unaccounted field ids as
    // heal breadcrumbs, and the link heals once the target is pasted
    // back into the vault.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-heal");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Cut [dependsOn:: tasks__pasted] ^cut\n",
        "  - ⛓️ **DEPENDS ON:** [[old#^pasted]]\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(warning_kinds(&json), ["unresolved_dependency_link"]);

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply breadcrumb run");
    assert_success(&applied);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);

    write_file(
        &vault.join("b.md"),
        "- [ ] #task Pasted [id:: tasks__pasted] ^pasted\n",
    );
    let healed = dry_run_json(&vault, &daily);
    let links = healed["healed_dependency_links"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert!(
        links[0]["detail"]
            .as_str()
            .unwrap()
            .contains("[[b#^pasted]]"),
        "{}",
        links[0]["detail"]
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply heal run");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(
        after.contains("  - ⛓️ **DEPENDS ON:** [[b#^pasted]]\n"),
        "{after}"
    );
    assert!(after.contains("[dependsOn:: tasks__pasted]"), "{after}");

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_projection_only_write_defers_in_quiet_interval() {
    // A projection-only write is structural, so a note modified less
    // than the quiet interval ago defers through the guarded write
    // instead of racing the editor. A future mtime defers
    // deterministically; backdating lets the same write apply.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-quiet");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Dependent ^dep\n",
        "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
        "- [ ] #task Prerequisite ^pre\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);
    let touch = |spec: &str| {
        let status = std::process::Command::new("touch")
            .arg("-d")
            .arg(spec)
            .arg(&tasks)
            .status()
            .expect("touch tasks.md");
        assert!(status.success());
    };
    touch("+1 hour");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("-r")
        .arg("0")
        .env("BOB_DAY_FILE", &daily)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run deferred projection-only write");
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("deferral JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["reason"], "quiet_period");
    assert!(
        json["deferred_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str().unwrap().contains("tasks.md")),
        "{}",
        json["deferred_files"]
    );
    assert_eq!(fs::read_to_string(&tasks).unwrap(), before);

    touch("-1 hour");
    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("apply deferred projection");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    assert!(after.contains("[dependsOn:: tasks__pre]"), "{after}");
    assert!(after.contains("[id:: tasks__pre]"), "{after}");

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
}

#[test]
fn reconcile_stamps_cross_note_target_id() {
    // A cross-note target without `[id::]` is stamped in its own note
    // while the dependent's field projects in the same run.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-cross");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let dependent = vault.join("a.md");
    let target = vault.join("b.md");
    write_file(
        &dependent,
        concat!(
            "- [ ] #task Dep ^dep\n",
            "  - ⛓️ **DEPENDS ON:** [[b#^t]]\n",
        ),
    );
    write_file(&target, "- [ ] #task T ^t\n");
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert!(warning_kinds(&json).is_empty());
    assert!(json["dependency_projection_updates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["kind"] == "target_id" && item["path"] == "b.md"));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply cross-note run");
    assert_success(&applied);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "- [ ] #task T [id:: b__t] ^t\n"
    );
    assert!(fs::read_to_string(&dependent)
        .unwrap()
        .contains("- [?] #task Dep [dependsOn:: b__t] ^dep\n"));

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

#[test]
fn reconcile_legacy_window_with_line_and_children() {
    // The legacy window (R8): a well-formed line plus a plain
    // (un-embedded) legacy child project the field together. The child
    // is never rewritten and counts toward the legacy window.
    let temp = TempDir::new("bob-cli-task-status-hooks-reconcile-window");
    let vault = temp.path().join("vault");
    let daily = write_daily(&vault);
    let tasks = vault.join("tasks.md");
    let before = concat!(
        "- [ ] #task Dep [dependsOn:: tasks__one, tasks__two] ^dep\n",
        "  - ⛓️ **DEPENDS ON:** [[#^one]]\n",
        "  - [[#^two]]\n",
        "- [ ] #task One [id:: tasks__one] ^one\n",
        "- [ ] #task Two [id:: tasks__two] ^two\n",
    );
    write_file(&tasks, before);
    write_blocked_tasks_settings(&vault);

    let json = dry_run_json(&vault, &daily);
    assert_eq!(json["legacy_dependency_children"], 1);
    assert!(warning_kinds(&json).is_empty());

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply legacy-window run");
    assert_success(&applied);
    let after = fs::read_to_string(&tasks).unwrap();
    assert_eq!(
        after,
        before.replacen("- [ ] #task Dep", "- [?] #task Dep", 1)
    );
    assert!(after.contains("  - [[#^two]]\n"), "{after}");

    let second = dry_run_json(&vault, &daily);
    assert_reconciled_clean(&second);
    assert!(warning_kinds(&second).is_empty());
}

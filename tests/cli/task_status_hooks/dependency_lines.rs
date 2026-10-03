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
    // DR3: a field id no link or legacy child accounts for is dropped
    // and reported.
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

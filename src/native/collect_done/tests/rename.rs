//! Rename-following archive plans: canonical creates, inbound-link and
//! dependency-id repairs, guarded old-path removals, and recovery.
use super::*;

use serde_json::Value;

use super::super::command::{execute_standalone, Hooks};
use super::super::report::{render_json, Report};

fn archive_note_for(parent: &str, body: &str) -> String {
    format!("---\nparent: \"[[{parent}]]\"\ntype: \"[[done]]\"\n---\n\n{body}")
}

fn bound_source(archive_target: &str, body: &str) -> String {
    format!(
        "---\ntype: [[project]]\ndone_tasks: \"[[{archive_target}]]\"\n---\n\n{body}"
    )
}

/// A reciprocal project/archive pair whose archive name is old.
fn bound_pair_fixture(prefix: &str) -> TempDir {
    let vault = TempDir::new(prefix);
    write_file(
        &vault.path().join("work.md"),
        &bound_source(
            "done/work_old_done",
            "\
- [x] done one #task [completion:: 2026-01-01] ^one
- [x] done two #task [completion:: 2026-01-02]
- [ ] active #task
",
        ),
    );
    write_file(
        &vault.path().join("done/work_old_done.md"),
        &archive_note_for(
            "work",
            "- [x] older #task [completion:: 2025-12-01] ^old\n",
        ),
    );
    vault
}

fn json(report: &Report) -> Value {
    serde_json::from_str(&render_json(report)).expect("report is JSON")
}

fn lock_env(dir: &Path) -> bob_env::TestEnvGuard {
    bob_env::TestEnvGuard::set(&[(
        "BOB_VAULT_SYNC_LOCK_FILE",
        Some(dir.join("bob_sync.lock").as_os_str()),
    )])
}

fn report_for(vault: &TempDir, dry_run: bool) -> Report {
    Report::new(vault.path().to_path_buf(), 1, dry_run)
}

#[test]
fn rename_only_run_moves_bytes_and_second_run_is_noop() {
    let vault = TempDir::new("bob-cli-archive-rename-only");
    write_file(
        &vault.path().join("solo.md"),
        &bound_source("done/solo_old_done", "- [ ] active #task\n"),
    );
    write_file(
        &vault.path().join("done/solo_old_done.md"),
        &archive_note_for(
            "solo",
            "- [x] older #task [completion:: 2025-12-01] ^old\n",
        ),
    );

    // Threshold 10: no task extraction, but the rename is still planned.
    let plan = build_collection_plan(vault.path(), 10).expect("plan");
    assert_eq!(plan.renames.len(), 1);
    assert_eq!(plan.renames[0].source, PathBuf::from("solo.md"));
    assert_eq!(plan.renames[0].from, PathBuf::from("done/solo_old_done.md"));
    assert_eq!(plan.renames[0].to, PathBuf::from("done/solo_done.md"));
    assert!(!plan.is_empty());
    let scratch = TempDir::new("bob-cli-archive-rename-only-lock");
    let _env = lock_env(scratch.path());
    let mut preview = Report::new(vault.path().to_path_buf(), 10, true);
    execute_standalone(&mut preview, &mut Hooks::production());
    let value = json(&preview);
    assert_eq!(
        value["renames"],
        serde_json::json!([{
            "source": "solo.md",
            "from": "done/solo_old_done.md",
            "to": "done/solo_done.md",
        }])
    );
    assert_eq!(value["scan"]["archive_renames"], 1);
    let human = super::super::report::render_human(&preview);
    assert!(human.contains("renames:"), "{human}");
    assert!(
        human.contains("solo.md: done/solo_old_done.md -> done/solo_done.md"),
        "{human}"
    );

    apply_archive(vault.path(), 10);

    let canonical = fs::read_to_string(vault.path().join("done/solo_done.md"))
        .expect("canonical archive installed");
    assert!(canonical.contains("older #task"), "{canonical}");
    assert!(canonical.contains("^old"), "{canonical}");
    assert!(canonical.contains("parent: \"[[solo]]\""), "{canonical}");
    assert!(!vault.path().join("done/solo_old_done.md").exists());
    let source =
        fs::read_to_string(vault.path().join("solo.md")).expect("source");
    assert!(
        source.contains("done_tasks: \"[[done/solo_done]]\""),
        "{source}"
    );

    // A successful second run is a no-op.
    let rerun = build_collection_plan(vault.path(), 10).expect("replan");
    assert!(rerun.renames.is_empty(), "{:?}", rerun.renames);
    assert!(rerun.is_empty());
}

#[test]
fn rename_plus_task_move_keeps_old_body_and_appends_new_roots() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-move");

    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert_eq!(plan.renames.len(), 1);
    let file = plan
        .files
        .iter()
        .find(|file| file.relative_source_path == Path::new("work.md"))
        .expect("work file");
    assert_eq!(
        file.relative_archive_path,
        PathBuf::from("done/work_done.md")
    );
    assert_eq!(
        file.renamed_from,
        Some(PathBuf::from("done/work_old_done.md"))
    );
    assert_eq!(file.task_count, 2);
    let archive = file.archive_contents.as_deref().expect("archive");
    assert!(archive.contains("^old"), "{archive}");
    assert!(archive.contains("done one #task"), "{archive}");
    assert!(archive.contains("parent: \"[[work]]\""), "{archive}");

    apply_archive(vault.path(), 1);

    let canonical = fs::read_to_string(vault.path().join("done/work_done.md"))
        .expect("canonical");
    assert!(canonical.contains("older #task"), "{canonical}");
    assert!(canonical.contains("^old"), "{canonical}");
    assert!(canonical.contains("done one #task"), "{canonical}");
    assert!(!vault.path().join("done/work_old_done.md").exists());
    let source =
        fs::read_to_string(vault.path().join("work.md")).expect("source");
    assert!(!source.contains("done one"), "{source}");
    assert!(source.contains("- [ ] active #task"), "{source}");

    let rerun = build_collection_plan(vault.path(), 1).expect("replan");
    assert!(rerun.is_empty());
}

#[test]
fn legacy_daily_filename_follows_reciprocal_metadata() {
    let vault = TempDir::new("bob-cli-archive-rename-daily");
    write_file(
        &vault.path().join("2026/20260528.md"),
        "\
---
done_tasks: \"[[done/2026/20260528_day_done]]\"
---

# 2026-05-28
",
    );
    write_file(
        &vault.path().join("done/2026/20260528_day_done.md"),
        &archive_note_for(
            "2026/20260528",
            "- [x] older #task [completion:: 2025-12-01] ^old\n",
        ),
    );

    // Daily sources never feed task extraction, but keep maintenance.
    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert_eq!(plan.total_task_count(), 0);
    assert_eq!(plan.renames.len(), 1);
    assert_eq!(plan.renames[0].source, PathBuf::from("2026/20260528.md"));
    assert_eq!(
        plan.renames[0].from,
        PathBuf::from("done/2026/20260528_day_done.md")
    );
    assert_eq!(
        plan.renames[0].to,
        PathBuf::from("done/2026/20260528_done.md")
    );

    apply_archive(vault.path(), 1);
    assert!(vault.path().join("done/2026/20260528_done.md").exists());
    assert!(!vault.path().join("done/2026/20260528_day_done.md").exists());
}

#[test]
fn finished_project_keeps_maintenance_without_extraction_or_regrouping() {
    let vault = TempDir::new("bob-cli-archive-rename-finished");
    let before = "\
---
type: [[project]]
done_tasks: \"[[done/projects/ship_old_done]]\"
---

- [x] #task Ship #hide ^prj
- [x] leftover #task [completion:: 2026-01-01]
- [ ] active #task
";
    write_file(&vault.path().join("projects/ship.md"), before);
    write_file(
        &vault.path().join("done/projects/ship_old_done.md"),
        &archive_note_for(
            "projects/ship",
            "- [x] older #task [completion:: 2025-12-01] ^old\n",
        ),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert_eq!(plan.renames.len(), 1);
    assert_eq!(plan.total_task_count(), 0);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 0);
    // Only the managed done_tasks line may change on a finished note.
    let untouched = before
        .replace("done/projects/ship_old_done", "done/projects/ship_done");
    assert_eq!(file.source_contents, untouched);

    apply_archive(vault.path(), 1);
    let source = fs::read_to_string(vault.path().join("projects/ship.md"))
        .expect("source");
    assert!(source.contains("- [x] leftover #task"), "{source}");
    assert!(vault.path().join("done/projects/ship_done.md").exists());
    assert!(!vault.path().join("done/projects/ship_old_done.md").exists());
}

#[test]
fn nested_source_renames_within_its_parent() {
    let vault = TempDir::new("bob-cli-archive-rename-nested");
    write_file(
        &vault.path().join("foo/bar.md"),
        &bound_source(
            "done/foo/bar_old_done",
            "- [x] nested #task [completion:: 2026-01-01]\n- [ ] on #task\n",
        ),
    );
    write_file(
        &vault.path().join("done/foo/bar_old_done.md"),
        &archive_note_for(
            "foo/bar",
            "- [x] older #task [completion:: 2025-12-01] ^old\n",
        ),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert_eq!(plan.renames.len(), 1);
    assert_eq!(plan.renames[0].to, PathBuf::from("done/foo/bar_done.md"));

    apply_archive(vault.path(), 1);
    assert!(vault.path().join("done/foo/bar_done.md").exists());
    assert!(!vault.path().join("done/foo/bar_old_done.md").exists());
}

#[test]
fn inbound_note_heading_block_markdown_and_dependency_links_follow() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-links");
    write_file(
        &vault.path().join("notes.md"),
        &area_note(
            "\
Link forms that must follow the rename:

- note [[done/work_old_done]]
- short [[work_old_done]]
- case variant [[WORK_OLD_DONE]]
- alias [[work_old_done|label]]
- embed ![[work_old_done]]
- heading [[work_old_done#Overview]]
- block [[work_old_done#^old]]
- block alias [[done/work_old_done#^old|seen]]
- markdown [text](done/work_old_done.md#^old)
- markdown bare [text](work_old_done.md)
",
        ),
    );
    write_file(
        &vault.path().join("deps.md"),
        &area_note(
            "\
- [ ] blocked #task
  [dependsOn:: done__work_old_done__old]
- [ ] identified #task
  [id:: done__work_old_done__old]
",
        ),
    );
    // Skipped contexts keep the old target.
    write_file(
        &vault.path().join("skipped.md"),
        &area_note(
            "\
```
[[work_old_done]]
```

Inline `[[work_old_done]]` stays.

%%
[[work_old_done]]
%%
",
        ),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert_eq!(plan.renames.len(), 1);

    apply_archive(vault.path(), 1);

    let notes =
        fs::read_to_string(vault.path().join("notes.md")).expect("notes");
    for expected in [
        "[[done/work_done]]",
        "[[done/work_done|label]]",
        "![[done/work_done]]",
        "[[done/work_done#Overview]]",
        "[[done/work_done#^old]]",
        "[[done/work_done#^old|seen]]",
        "[text](done/work_done.md#^old)",
        "[text](done/work_done.md)",
    ] {
        assert!(notes.contains(expected), "missing {expected}:\n{notes}");
    }
    assert!(!notes.contains("work_old_done"), "stale target:\n{notes}");

    let deps = fs::read_to_string(vault.path().join("deps.md")).expect("deps");
    assert!(
        deps.contains("[dependsOn:: done__work_done__old]"),
        "{deps}"
    );
    assert!(deps.contains("[id:: done__work_done__old]"), "{deps}");
    assert!(!deps.contains("work_old_done"), "{deps}");

    let skipped =
        fs::read_to_string(vault.path().join("skipped.md")).expect("skipped");
    assert!(skipped.contains("[[work_old_done]]"), "{skipped}");
}

#[test]
fn ambiguous_basename_links_are_never_guessed() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-ambiguous");
    // A second stem twin makes the bare target ambiguous.
    write_file(
        &vault.path().join("misc/work_old_done.md"),
        &area_note("unrelated note\n"),
    );
    write_file(
        &vault.path().join("notes.md"),
        &area_note(
            "- bare [[work_old_done]] stays\n- exact [[done/work_old_done]] follows\n",
        ),
    );

    apply_archive(vault.path(), 1);

    let notes =
        fs::read_to_string(vault.path().join("notes.md")).expect("notes");
    assert!(notes.contains("[[work_old_done]]"), "{notes}");
    assert!(notes.contains("[[done/work_done]]"), "{notes}");
    assert!(vault.path().join("done/work_done.md").exists());
    assert!(!vault.path().join("done/work_old_done.md").exists());
}

#[test]
fn occupied_canonical_destination_refuses_without_deleting_a_copy() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-collision");
    // An unrelated occupant holds the canonical rename destination, even
    // with byte-identical contents: diagnosing the duplicate never licenses
    // deleting either copy.
    let old = fs::read_to_string(vault.path().join("done/work_old_done.md"))
        .expect("old archive");
    write_file(&vault.path().join("done/work_done.md"), &old);

    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert!(plan.renames.is_empty());
    assert!(plan.files.is_empty());

    apply_archive(vault.path(), 1);

    // Both copies survive; nothing was deleted or guessed.
    assert!(vault.path().join("done/work_old_done.md").exists());
    assert!(vault.path().join("done/work_done.md").exists());
}

#[test]
fn concurrent_canonical_appearance_fails_the_create() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-concurrent");
    let planned = plan_archive(vault.path(), 1).expect("plan");
    assert_eq!(planned.collection.renames.len(), 1);

    // A concurrent run installs the destination between planning and apply.
    write_file(
        &vault.path().join("done/work_done.md"),
        &archive_note_for("work", "- [x] rival #task\n"),
    );

    let state = TempDir::new("bob-cli-archive-rename-concurrent-state");
    let result = apply_plan(
        &planned.write_plan,
        &test_session(state.path(), vault.path()),
    );
    let error = result.expect_err("concurrent destination must fail apply");
    assert!(error.applied_files.is_empty(), "{:?}", error.applied_files);
    assert!(vault.path().join("done/work_old_done.md").exists());
}

#[test]
fn changed_old_archive_fails_the_preflight_with_nothing_installed() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-changed");
    let planned = plan_archive(vault.path(), 1).expect("plan");

    // The old archive changes underfoot: the guarded preflight refuses the
    // whole plan before anything is installed.
    let mut changed =
        fs::read_to_string(vault.path().join("done/work_old_done.md"))
            .expect("old archive");
    changed.push_str("\nconcurrent note\n");
    write_file(&vault.path().join("done/work_old_done.md"), &changed);

    let state = TempDir::new("bob-cli-archive-rename-changed-state");
    let result = apply_plan(
        &planned.write_plan,
        &test_session(state.path(), vault.path()),
    );
    let error = result.expect_err("stale inputs must fail");
    assert!(error.applied_files.is_empty(), "{:?}", error.applied_files);
    assert!(
        error
            .deferred_files
            .iter()
            .any(|path| path.ends_with("done/work_old_done.md")),
        "removal is deferred: {:?}",
        error.deferred_files
    );
    assert!(
        !vault.path().join("done/work_done.md").exists(),
        "nothing was installed"
    );
    assert!(vault.path().join("done/work_old_done.md").exists());

    // A fresh capture rebuilds the rename from the current bytes.
    let replan = build_collection_plan(vault.path(), 1).expect("replan");
    assert_eq!(replan.renames.len(), 1);
}

#[test]
fn partial_failure_before_removal_keeps_both_copies_and_recovery() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-partial");
    let scratch = TempDir::new("bob-cli-archive-rename-partial-scratch");
    let state = scratch.path().join("state");
    let _env = lock_env(scratch.path());
    let planned = plan_archive(vault.path(), 1).expect("plan");
    // The old path changes after its destination installed but before the
    // removal: the destination copy lands and the removal is deferred.
    let old_absolute = vault.path().join("done/work_old_done.md");
    let mut session = test_session(&state, vault.path());
    session.before_replace = Some(Box::new(move |dest| {
        if dest == old_absolute {
            let mut text = fs::read_to_string(dest).expect("read old");
            text.push_str("trailing concurrent note\n");
            fs::write(dest, text).expect("race the removal");
        }
    }));
    let result = apply_plan(&planned.write_plan, &session);
    let error = result.expect_err("raced removal must stop the apply");
    assert!(
        error
            .applied_files
            .iter()
            .any(|path| path.ends_with("done/work_done.md")),
        "{:?}",
        error.applied_files
    );
    assert!(
        error
            .deferred_files
            .iter()
            .any(|path| path.ends_with("done/work_old_done.md")),
        "{:?}",
        error.deferred_files
    );
    let recovery = error.recovery_directory.clone().expect("recovery");
    assert!(recovery.exists());
    let manifest = fs::read_to_string(recovery.join("manifest.json"))
        .expect("recovery manifest");
    let manifest: Value =
        serde_json::from_str(&manifest).expect("manifest JSON");
    let notes = manifest["notes"].as_array().expect("notes");
    let entry_for = |suffix: &str| {
        notes
            .iter()
            .find(|note| {
                note["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with(suffix))
            })
            .unwrap_or_else(|| panic!("no recovery note for {suffix}"))
    };
    // The create carries a missing before-image; the removal carries the
    // captured original bytes, so both directions stay recoverable.
    assert_eq!(
        entry_for("done/work_done.md")["original_missing"],
        serde_json::json!(true)
    );
    assert_eq!(
        entry_for("done/work_old_done.md")["proposed_missing"],
        serde_json::json!(true)
    );
    for entry in notes {
        if entry["original_missing"] == serde_json::json!(true) {
            continue;
        }
        let original = entry["original_file"].as_str().expect("original file");
        assert!(recovery.join(original).exists(), "{entry}");
    }
    assert!(vault.path().join("done/work_done.md").exists());
    assert!(vault.path().join("done/work_old_done.md").exists());

    // A later ordinary run refuses the unresolved duplicate claims instead
    // of guessing which copy is authoritative. The deletion did not
    // complete, so both copies and the recovery manifest stay available and
    // nothing retries on its own.
    let mut rerun = report_for(&vault, true);
    execute_standalone(&mut rerun, &mut Hooks::production());
    let rerun_value = json(&rerun);
    let codes: Vec<&str> = rerun_value["diagnostics"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|finding| finding["code"].as_str().expect("code"))
        .collect();
    assert!(
        codes.contains(&"archive_duplicate")
            || codes.contains(&"archive_binding_conflict"),
        "duplicate refused: {codes:?}"
    );
}

#[test]
fn dry_run_leaves_the_fixture_byte_for_byte_unchanged() {
    let vault = bound_pair_fixture("bob-cli-archive-rename-dry");
    let before: Vec<(PathBuf, String)> = ["work.md", "done/work_old_done.md"]
        .into_iter()
        .map(|relative| {
            (
                PathBuf::from(relative),
                fs::read_to_string(vault.path().join(relative)).expect("read"),
            )
        })
        .collect();

    let scratch = TempDir::new("bob-cli-archive-rename-dry-lock");
    let _env = lock_env(scratch.path());
    let mut report = report_for(&vault, true);
    execute_standalone(&mut report, &mut Hooks::production());

    assert_eq!(report.exit.outcome, super::super::report::Outcome::Preview);
    assert_eq!(report.exit_code(), 0);
    for (relative, contents) in before {
        assert_eq!(
            fs::read_to_string(vault.path().join(&relative)).expect("read"),
            contents
        );
    }
    assert!(!vault.path().join("done/work_done.md").exists());
    let value = json(&report);
    assert_eq!(value["renames"].as_array().expect("renames").len(), 1);
    assert_eq!(value["exit"]["outcome"], "preview");
}

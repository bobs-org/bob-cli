//! Acceptance tests for the captured archive-snapshot planner.
use super::*;

use std::time::{Duration, SystemTime};

use super::super::super::task_status_hooks_write::{
    apply_plan, new_run_id, ApplySession,
};
use super::super::plan::build_write_plan;

fn write_plan_outputs(vault: &Path) {
    let plan = build_write_plan(vault, 1).expect("write plan");
    assert!(!plan.outputs.is_empty(), "expected planned outputs");
    for output in &plan.outputs {
        if let Some(parent) = output.path.parent() {
            fs::create_dir_all(parent).expect("create output parent");
        }
        fs::write(&output.path, &output.proposed_bytes)
            .expect("install output");
    }
}

fn paired_archive(source: &str, body: &str) -> String {
    format!("---\nparent: \"[[{source}]]\"\ntype: \"[[done]]\"\n---\n\n{body}")
}

#[test]
fn records_selected_roots_with_final_ids() {
    let vault = TempDir::new("bob-cli-archive-snapshot-records");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] one #task ^a1
  child detail
- [x] two #task
- [ ] active #task
",
        ),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.moved_roots.len(), 2);
    let first = &file.moved_roots[0];
    assert_eq!(first.source_line, 5);
    assert_eq!(first.root_id.as_deref(), Some("a1"));
    assert_eq!(first.raw_bytes, "- [x] one #task ^a1\n  child detail\n");
    assert_eq!(first.contained_ids, vec!["a1".to_string()]);
    assert_eq!(first.final_ids, vec!["a1".to_string()]);
    assert!(!first.replayed);
    let second = &file.moved_roots[1];
    assert_eq!(second.source_line, 7);
    assert_eq!(second.root_id, None);
    assert!(!second.replayed);
}

#[test]
fn held_roots_carry_block_ids() {
    let vault = TempDir::new("bob-cli-archive-snapshot-holds");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] parent #task ^held
  - [ ] child #task ^kid
- [x] other #task
",
        ),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].task_count, 1);
    assert_eq!(plan.holds.len(), 1);
    assert_eq!(
        plan.holds[0].block_ids,
        vec!["held".to_string(), "kid".to_string()]
    );
}

#[test]
fn exact_replay_removes_source_block_without_appending() {
    let vault = TempDir::new("bob-cli-archive-snapshot-replay");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] one #task ^a1
- [ ] active #task
",
        ),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [x] one #task ^a1\n"),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 1);
    assert_eq!(file.moved_roots.len(), 1);
    assert!(file.moved_roots[0].replayed);
    assert_eq!(file.moved_roots[0].final_ids, vec!["a1".to_string()]);
    assert!(
        file.archive_contents.is_none(),
        "replay appends nothing:\n{:?}",
        file.archive_contents
    );

    write_plan_outputs(vault.path());
    let rerun = build_collection_plan(vault.path(), 1).expect("rerun plan");
    assert!(rerun.is_empty(), "second run must be a no-op");
}

#[test]
fn different_body_with_same_id_takes_rename_path() {
    let vault = TempDir::new("bob-cli-archive-snapshot-collision");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] new body #task ^a1
- [ ] active #task
",
        ),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [x] old body #task ^a1\n"),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.moved_roots.len(), 1);
    assert!(!file.moved_roots[0].replayed);
    assert_eq!(file.moved_roots[0].final_ids, vec!["a1-1".to_string()]);
    let archive = file.archive_contents.as_deref().expect("archive planned");
    assert!(
        archive.contains("- [x] new body #task ^a1-1\n"),
        "{archive}"
    );
    assert!(archive.contains("- [x] old body #task ^a1\n"), "{archive}");
}

#[test]
fn mixed_replay_and_new_blocks_append_only_new() {
    let vault = TempDir::new("bob-cli-archive-snapshot-mixed");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] replayed #task ^a1
- [x] fresh #task ^b2
- [ ] active #task
",
        ),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [x] replayed #task ^a1\n"),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.moved_roots.len(), 2);
    assert!(file.moved_roots[0].replayed);
    assert!(!file.moved_roots[1].replayed);
    let archive = file.archive_contents.as_deref().expect("archive planned");
    assert_eq!(
        archive.matches("- [x] replayed #task ^a1\n").count(),
        1,
        "replayed block must not duplicate:\n{archive}"
    );
    assert!(archive.contains("- [x] fresh #task ^b2\n"), "{archive}");

    write_plan_outputs(vault.path());
    let rerun = build_collection_plan(vault.path(), 1).expect("rerun plan");
    assert!(rerun.is_empty(), "second run must be a no-op");
}

#[test]
fn threshold_gates_replay_candidates() {
    let vault = TempDir::new("bob-cli-archive-snapshot-threshold");
    write_file(
        &vault.path().join("work.md"),
        "\
---
type: [[area]]
done_tasks: \"[[done/work_done]]\"
---

- [x] one #task ^a1
- [ ] active #task
",
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [x] one #task ^a1\n"),
    );

    let gated = build_collection_plan(vault.path(), 2).expect("gated plan");
    assert!(
        gated.is_empty(),
        "replay candidate below threshold stays put"
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");
    assert_eq!(plan.files.len(), 1);
    assert!(plan.files[0].moved_roots[0].replayed);
}

#[test]
fn crlf_replay_is_a_noop_on_second_run() {
    let vault = TempDir::new("bob-cli-archive-snapshot-crlf");
    let source = "---\r\ntype: [[area]]\r\n---\r\n\r\n\
- [x] one #task ^a1\r\n- [ ] active #task\r\n";
    write_file(&vault.path().join("work.md"), source);
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [x] one #task ^a1\r\n")
            .replace("---\n", "---\r\n"),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");
    assert_eq!(plan.files.len(), 1);
    assert!(plan.files[0].moved_roots[0].replayed);

    write_plan_outputs(vault.path());
    let rerun = build_collection_plan(vault.path(), 1).expect("rerun plan");
    assert!(rerun.is_empty(), "CRLF second run must be a no-op");
}

#[test]
fn grouping_rearranges_source_and_second_run_is_noop() {
    let vault = TempDir::new("bob-cli-archive-snapshot-grouping");
    let settings = super::super::super::task_status_hooks::read_tasks_settings(
        vault.path(),
    );
    let classification =
        super::super::super::task_status_hooks::task_group_classification(
            &settings,
        );
    let ungrouped = area_note(
        "\
## Tasks

- [x] done one #task ^gone1
- [x] done two #task ^gone2
- [ ] live task #task ^live
- [*] next thing #task ^nxt
",
    );
    let grouped = super::super::super::task_status_groups::transform(
        &ungrouped,
        &classification,
    )
    .contents;
    assert!(grouped.contains("### Done & Canceled"), "{grouped}");
    write_file(&vault.path().join("work.md"), &grouped);

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 2);
    assert!(file.source_structural, "grouping must mark the source");
    assert!(
        !file.source_contents.contains("### Done & Canceled"),
        "empty Done group goes away in the same postimage:\n{}",
        file.source_contents
    );
    assert!(
        file.source_contents.contains("- [ ] live task #task ^live"),
        "live task stays:\n{}",
        file.source_contents
    );

    let write_plan = build_write_plan(vault.path(), 1).expect("write plan");
    let source_output = write_plan
        .outputs
        .iter()
        .find(|output| output.path == vault.path().join("work.md"))
        .expect("source output");
    assert!(
        source_output.structural_regrouping,
        "grouped source writes carry the quiet-period flag"
    );
    let archive_output = write_plan
        .outputs
        .iter()
        .find(|output| output.path == vault.path().join("done/work_done.md"))
        .expect("archive output");
    assert!(
        !archive_output.structural_regrouping,
        "archive writes are never structural"
    );
    let archive_index = write_plan
        .outputs
        .iter()
        .position(|output| {
            output.path == vault.path().join("done/work_done.md")
        })
        .expect("archive position");
    let source_index = write_plan
        .outputs
        .iter()
        .position(|output| output.path == vault.path().join("work.md"))
        .expect("source position");
    assert!(
        archive_index < source_index,
        "archive installs precede source removals"
    );

    write_plan_outputs(vault.path());
    let rerun = build_collection_plan(vault.path(), 1).expect("rerun plan");
    assert!(rerun.is_empty(), "grouped second run must be a no-op");
}

#[test]
fn grandfathered_untyped_source_skips_grouping() {
    let vault = TempDir::new("bob-cli-archive-snapshot-grandfathered");
    write_file(
        &vault.path().join("loose.md"),
        "\
## Tasks

- [x] done one #task ^g1
- [x] done two #task ^g2
- [ ] live #task
",
    );
    write_file(
        &vault.path().join("done/loose_done.md"),
        &paired_archive("loose", "- [x] old #task ^old\n"),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 2);
    assert!(
        !file.source_structural,
        "grandfathered untyped sources never regroup"
    );
}

#[test]
fn repeated_bare_ids_in_unrelated_notes_stay_legal() {
    let vault = TempDir::new("bob-cli-archive-snapshot-bare-dup");
    write_file(
        &vault.path().join("a.md"),
        &area_note(
            "\
- [x] move me #task ^dup
- [x] move two #task
- [ ] live #task
",
        ),
    );
    write_file(&vault.path().join("b.md"), "- [ ] other #task ^dup\n");

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.relative_source_path, PathBuf::from("a.md"));
    let archive = file.archive_contents.as_deref().expect("archive planned");
    assert!(archive.contains("- [x] move me #task ^dup\n"), "{archive}");
}

#[test]
fn open_task_in_archive_postimage_is_refused_before_writes() {
    let vault = TempDir::new("bob-cli-archive-snapshot-open-refusal");
    let source_contents = area_note(
        "\
- [x] done #task ^j1
- [ ] live #task
",
    );
    write_file(&vault.path().join("work.md"), &source_contents);
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [ ] sneaky open #task\n"),
    );

    let error =
        build_collection_plan(vault.path(), 1).expect_err("must refuse");

    assert!(
        error.to_string().contains("open #task"),
        "refusal names the problem: {error}"
    );
    assert!(
        error.to_string().contains("done/work_done.md"),
        "refusal names the path: {error}"
    );
    assert_eq!(
        fs::read_to_string(vault.path().join("work.md")).expect("read source"),
        source_contents,
        "refusal happens before any write"
    );
}

fn stale_vault(prefix: &str, with_archive: bool) -> TempDir {
    let vault = TempDir::new(prefix);
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] one #task ^m1
- [x] two #task ^m2
- [ ] active #task
",
        ),
    );
    write_file(&vault.path().join("other.md"), "See [[work#^m1]].\n");
    if with_archive {
        write_file(
            &vault.path().join("done/work_done.md"),
            &paired_archive("work", "- [x] old #task ^old\n"),
        );
    }
    vault
}

fn live_scan_paths(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = markdown_files(vault)?;
    paths.extend(link_repair_markdown_files(vault)?);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn apply_session(state_dir: &Path, vault: PathBuf) -> ApplySession {
    ApplySession {
        tool: "task-archive",
        state_home: state_dir.to_path_buf(),
        quiet_period: Duration::from_secs(2),
        retention: Duration::from_secs(30 * 24 * 60 * 60),
        run_id: new_run_id(),
        now_system: Box::new(SystemTime::now),
        sleep: Box::new(|_| {}),
        rescan: Box::new(move || live_scan_paths(&vault)),
        before_preflight: None,
        after_staging: None,
        before_replace: None,
        fail_staging: None,
    }
}

fn stale_plan_and_session(
    vault: &Path,
    state_dir: &Path,
) -> (
    super::super::super::task_status_hooks_write::WritePlan,
    ApplySession,
) {
    let plan = build_write_plan(vault, 1).expect("write plan");
    assert!(!plan.outputs.is_empty(), "expected planned outputs");
    let session = apply_session(state_dir, vault.to_path_buf());
    (plan, session)
}

fn apply_write_plan(vault: &Path, state_dir: &Path) -> Result<(), String> {
    let (plan, session) = stale_plan_and_session(vault, state_dir);
    apply_plan(&plan, &session).map(|_| ()).map_err(|error| {
        format!("{} ({})", error.message, error.reason.as_str())
    })
}
#[test]
fn unchanged_inputs_apply_cleanly() {
    let vault = stale_vault("bob-cli-archive-snapshot-stale-ok", false);
    let state = TempDir::new("bob-cli-archive-snapshot-stale-ok-state");

    apply_write_plan(vault.path(), state.path())
        .unwrap_or_else(|error| panic!("baseline apply failed: {error}"));
}

#[test]
fn changed_source_rejects_the_stale_plan() {
    let vault = stale_vault("bob-cli-archive-snapshot-stale-source", false);
    let state = TempDir::new("bob-cli-archive-snapshot-stale-source-state");
    let (plan, session) = stale_plan_and_session(vault.path(), state.path());

    let before =
        fs::read_to_string(vault.path().join("work.md")).expect("read source");
    fs::write(
        vault.path().join("work.md"),
        format!("{before}- [ ] extra #task\n"),
    )
    .expect("mutate source");

    let error = apply_plan(&plan, &session)
        .map(|_| ())
        .map_err(|error| {
            format!("{} ({})", error.message, error.reason.as_str())
        })
        .expect_err("changed source must reject");
    assert!(
        error.contains("vault_changed"),
        "retryable refusal: {error}"
    );
}

#[test]
fn changed_archive_rejects_the_stale_plan() {
    let vault = stale_vault("bob-cli-archive-snapshot-stale-archive", true);
    let state = TempDir::new("bob-cli-archive-snapshot-stale-archive-state");
    let (plan, session) = stale_plan_and_session(vault.path(), state.path());

    let before = fs::read_to_string(vault.path().join("done/work_done.md"))
        .expect("read archive");
    fs::write(
        vault.path().join("done/work_done.md"),
        format!("{before}- [x] injected #task ^zzz\n"),
    )
    .expect("mutate archive");

    let error = apply_plan(&plan, &session)
        .map(|_| ())
        .map_err(|error| {
            format!("{} ({})", error.message, error.reason.as_str())
        })
        .expect_err("changed archive must reject");
    assert!(
        error.contains("vault_changed"),
        "retryable refusal: {error}"
    );
}

#[test]
fn changed_repair_note_rejects_the_stale_plan() {
    let vault = stale_vault("bob-cli-archive-snapshot-stale-repair", false);
    let state = TempDir::new("bob-cli-archive-snapshot-stale-repair-state");
    let (plan, session) = stale_plan_and_session(vault.path(), state.path());

    let before = fs::read_to_string(vault.path().join("other.md"))
        .expect("read repair note");
    fs::write(vault.path().join("other.md"), format!("{before}\nExtra.\n"))
        .expect("mutate repair note");

    let error = apply_plan(&plan, &session)
        .map(|_| ())
        .map_err(|error| {
            format!("{} ({})", error.message, error.reason.as_str())
        })
        .expect_err("changed repair note must reject");
    assert!(
        error.contains("vault_changed"),
        "retryable refusal: {error}"
    );
}

#[test]
fn changed_tasks_settings_reject_the_stale_plan() {
    let vault = stale_vault("bob-cli-archive-snapshot-stale-settings", false);
    let state = TempDir::new("bob-cli-archive-snapshot-stale-settings-state");
    let (plan, session) = stale_plan_and_session(vault.path(), state.path());

    let settings_dir =
        vault.path().join(".obsidian/plugins/obsidian-tasks-plugin");
    fs::create_dir_all(&settings_dir).expect("settings dir");
    fs::write(
        settings_dir.join("data.json"),
        "{\"globalFilter\": \"#todo\"}",
    )
    .expect("mutate settings");

    let error = apply_plan(&plan, &session)
        .map(|_| ())
        .map_err(|error| {
            format!("{} ({})", error.message, error.reason.as_str())
        })
        .expect_err("changed settings must reject");
    assert!(
        error.contains("vault_changed"),
        "retryable refusal: {error}"
    );
}

#[test]
fn changed_membership_rejects_the_stale_plan() {
    let vault = stale_vault("bob-cli-archive-snapshot-stale-membership", false);
    let state = TempDir::new("bob-cli-archive-snapshot-stale-membership-state");
    let (plan, session) = stale_plan_and_session(vault.path(), state.path());

    write_file(&vault.path().join("fresh.md"), "- [ ] new #task\n");

    let error = apply_plan(&plan, &session)
        .map(|_| ())
        .map_err(|error| {
            format!("{} ({})", error.message, error.reason.as_str())
        })
        .expect_err("changed membership must reject");
    assert!(
        error.contains("vault_changed"),
        "retryable refusal: {error}"
    );
}

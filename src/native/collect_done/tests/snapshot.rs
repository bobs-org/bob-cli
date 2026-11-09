//! Acceptance tests for the captured archive-snapshot planner.
use super::*;

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, SystemTime},
};

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
fn link_only_repairs_regroup_eligible_sources_and_preserve_notebook_catalog() {
    let vault = TempDir::new("bob-cli-archive-link-only-grouping");
    write_file(
        &vault.path().join("work.md"),
        &area_note("- [x] finished #task ^gone\n"),
    );
    let catalog_region = "<!-- bob:notebooks:begin v1 -->\n\n> [!notebooks]\n>\n> - [[notebooks/dev]]\n\n<!-- bob:notebooks:end -->";
    let observer = area_note(&format!(
        "## Notebooks\n\n{catalog_region}\n\n## Tasks\n\n- [x] completed #task ^stale\n- [ ] See [[work#^gone]] #task ^live\n"
    ));
    let settings = super::super::super::task_status_hooks::read_tasks_settings(
        vault.path(),
    );
    let classification =
        super::super::super::task_status_hooks::task_group_classification(
            &settings,
        );
    let grouped = super::super::super::task_status_groups::transform(
        &observer,
        &classification,
    )
    .contents;
    let stale_observer = grouped.replace("- [x] completed #task ^stale\n", "");
    assert!(stale_observer.contains("### Done & Canceled"));
    assert!(stale_observer.contains("🟢"));
    write_file(&vault.path().join("observer.md"), &stale_observer);

    std::fs::File::options()
        .write(true)
        .open(vault.path().join("observer.md"))
        .expect("open observer for mtime")
        .set_modified(SystemTime::now())
        .expect("set observer mtime");
    let now = fs::metadata(vault.path().join("observer.md"))
        .expect("observer metadata")
        .modified()
        .expect("observer mtime");

    let captured = super::super::snapshot::capture_inputs(vault.path())
        .expect("capture inputs");
    let planned = super::super::plan::plan_from_captured(&captured, 1)
        .expect("plan link-only repair");
    let repair = planned
        .collection
        .link_repairs
        .iter()
        .find(|repair| repair.relative_path == Path::new("observer.md"))
        .expect("observer link repair");
    assert_eq!(repair.link_count, 1);
    assert!(repair.structural_regrouping);
    assert!(repair.contents.contains("[[done/work_done#^gone]]"));
    assert!(!repair.contents.contains("### Done & Canceled"));
    assert!(!repair.contents.contains("🟢"));
    let region_start = repair
        .contents
        .find("<!-- bob:notebooks:begin v1 -->")
        .expect("catalog start");
    let region_end = repair
        .contents
        .find("<!-- bob:notebooks:end -->")
        .expect("catalog end")
        + "<!-- bob:notebooks:end -->".len();
    assert_eq!(
        &repair.contents[region_start..region_end],
        catalog_region,
        "grouping preserves the generated Notebooks region bytes"
    );
    let output = planned
        .write_plan
        .outputs
        .iter()
        .find(|output| output.path == vault.path().join("observer.md"))
        .expect("observer output");
    assert!(output.structural_regrouping);

    let state = TempDir::new("bob-cli-archive-link-only-grouping-state");
    let slept = Rc::new(Cell::new(None));
    let mut session = apply_session(state.path(), vault.path().to_path_buf());
    session.now_system = Box::new(move || now);
    session.sleep = Box::new({
        let slept = Rc::clone(&slept);
        move |duration| slept.set(Some(duration))
    });
    apply_plan(&planned.write_plan, &session).expect("apply archive plan");
    assert_eq!(slept.get(), Some(Duration::from_secs(2)));
    let rerun = build_collection_plan(vault.path(), 1).expect("rerun plan");
    assert!(rerun.is_empty(), "repair and grouping are idempotent");
}

#[test]
fn dependency_metadata_only_repairs_are_regrouped() {
    let vault = TempDir::new("bob-cli-archive-dependency-only-grouping");
    write_file(
        &vault.path().join("projects/Shared.md"),
        &area_note(
            "- [x] #task Review [id:: projects__Shared__review] ^review\n",
        ),
    );
    let observer = area_note(
        "## Tasks\n\n- [ ] #task A [dependsOn:: projects__Shared__review] ^a\n\n### Done & Canceled\n\n",
    );
    write_file(&vault.path().join("observer.md"), &observer);

    let plan = super::super::plan::plan_archive(vault.path(), 1)
        .expect("plan dependency metadata repair");
    let repair = plan
        .collection
        .link_repairs
        .iter()
        .find(|repair| repair.relative_path == Path::new("observer.md"))
        .expect("observer repair");
    assert_eq!(repair.link_count, 0, "this case changes metadata only");
    assert_eq!(repair.dependency_metadata_count, 1);
    assert!(repair.structural_regrouping);
    assert!(repair
        .contents
        .contains("[dependsOn:: done__projects__Shared_done__review]"));
    assert!(!repair.contents.contains("### Done & Canceled"));
    let output = plan
        .write_plan
        .outputs
        .iter()
        .find(|output| output.path == vault.path().join("observer.md"))
        .expect("observer output");
    assert!(output.structural_regrouping);
}

#[test]
fn link_repair_does_not_regroup_daily_reference_notebook_or_untyped_notes() {
    let vault = TempDir::new("bob-cli-archive-grouping-exclusions");
    write_file(
        &vault.path().join("work.md"),
        &area_note("- [x] finished #task ^gone\n"),
    );
    let section = "## Tasks\n\n- [ ] See [[work#^gone]] #task ^live\n\n### Done & Canceled\n\n";
    write_file(
        &vault.path().join("2026/20261109.md"),
        &format!("---\ntype: [[daily]]\n---\n\n{section}"),
    );
    write_file(
        &vault.path().join("ref/paper.md"),
        &format!("---\ntype: [[reference]]\n---\n\n{section}"),
    );
    write_file(
        &vault.path().join("notebooks/dev.md"),
        &format!("---\ntype: [[notebook]]\n---\n\n{section}"),
    );
    write_file(&vault.path().join("loose.md"), section);

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");
    for path in [
        "2026/20261109.md",
        "ref/paper.md",
        "notebooks/dev.md",
        "loose.md",
    ] {
        let repair = plan
            .link_repairs
            .iter()
            .find(|repair| repair.relative_path == Path::new(path))
            .unwrap_or_else(|| panic!("missing link repair for {path}"));
        assert!(repair.contents.contains("[[done/work_done#^gone]]"));
        assert!(
            repair.contents.contains("### Done & Canceled"),
            "excluded note {path} must retain its empty Done group"
        );
        assert!(!repair.structural_regrouping);
    }
}

#[test]
fn moved_identity_uses_original_id_when_destination_id_is_valid_at_source() {
    let vault = TempDir::new("bob-cli-archive-renamed-id-source");
    write_file(
        &vault.path().join("work.md"),
        &area_note("- [x] new body #task ^a1\n- [ ] active #task ^a1-1\n"),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [x] old body #task ^a1\n"),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");
    let file = &plan.files[0];
    assert_eq!(file.moved_roots[0].contained_ids, vec!["a1"]);
    assert_eq!(file.moved_roots[0].final_ids, vec!["a1-1"]);
    assert!(file.source_contents.contains("- [ ] active #task ^a1-1"));
    assert!(file
        .archive_contents
        .as_deref()
        .expect("archive")
        .contains("- [x] new body #task ^a1-1"));
}

#[test]
fn content_aware_moved_anchors_ignore_code_comments_and_keep_nested_ids() {
    let vault = TempDir::new("bob-cli-archive-content-aware-anchors");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "- [x] parent #task ^root\n  child detail ^child\n  ```text\n  example ^fenced\n  ```\n  %%\n  example ^percent\n  %%\n  <!--\n  example ^html\n  -->\n  inline `^inline` and prose ^inline-real\n- [ ] active #task\n",
        ),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive(
            "work",
            "- [x] old parent #task ^root\n  child detail ^child\n",
        ),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");
    let file = &plan.files[0];
    assert_eq!(
        file.moved_roots[0].contained_ids,
        vec!["root", "child", "inline-real"]
    );
    assert_eq!(
        file.moved_roots[0].final_ids,
        vec!["root-1", "child-1", "inline-real"]
    );
    let archive = file.archive_contents.as_deref().expect("archive");
    assert!(archive.contains("example ^fenced"));
    assert!(archive.contains("example ^percent"));
    assert!(archive.contains("example ^html"));
    assert!(archive.contains("inline `^inline` and prose ^inline-real"));
    assert!(archive.contains("child detail ^child-1"));
    assert!(!archive.contains("^fenced-1"));
    assert!(!archive.contains("^percent-1"));
    assert!(!archive.contains("^html-1"));
}

#[test]
fn nested_anchors_resolve_for_exact_replays() {
    let vault = TempDir::new("bob-cli-archive-nested-replay");
    let block = "- [x] parent #task ^root\n  child detail ^child\n";
    write_file(
        &vault.path().join("work.md"),
        &area_note(&format!("{block}- [ ] active #task\n")),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", block),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");
    let record = &plan.files[0].moved_roots[0];
    assert!(record.replayed);
    assert_eq!(record.contained_ids, vec!["root", "child"]);
    assert_eq!(record.final_ids, vec!["root", "child"]);
    assert!(plan.files[0].archive_contents.is_none());
}

#[test]
fn invalid_destination_anchor_refuses_before_any_write() {
    let vault = TempDir::new("bob-cli-archive-invalid-postimage");
    let source = area_note("- [x] new body #task ^a1\n- [ ] active #task\n");
    write_file(&vault.path().join("work.md"), &source);
    write_file(
        &vault.path().join("done/work_done.md"),
        &paired_archive("work", "- [x] old body #task ^a1\n"),
    );

    let captured = super::super::snapshot::capture_inputs(vault.path())
        .expect("capture inputs");
    let planned = super::super::plan::plan_from_captured(&captured, 1)
        .expect("valid plan");
    let mut overlay = captured.contents.clone();
    let file = &planned.collection.files[0];
    overlay.insert(
        file.relative_source_path.clone(),
        file.source_contents.clone(),
    );
    let archive = file.archive_contents.as_ref().expect("archive");
    overlay.insert(
        file.relative_archive_path.clone(),
        format!("{archive}- [x] duplicate #task ^a1-1\n"),
    );

    let error = super::super::plan::verify_moved_identities(
        &captured,
        &overlay,
        &planned.collection.files,
    )
    .expect_err("duplicate destination anchor must be rejected");
    assert!(
        error
            .to_string()
            .contains("occurs 2 times at its destination"),
        "refusal identifies the duplicate anchor: {error}"
    );
    assert_eq!(
        fs::read_to_string(vault.path().join("work.md")).expect("source"),
        source,
        "a rejected postimage leaves the source untouched"
    );
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

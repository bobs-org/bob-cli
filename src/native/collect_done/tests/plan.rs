//! Collection planning and link-repair tests.
use super::*;

#[test]
fn scans_markdown_files_with_exclusions_and_threshold() {
    let vault = TempDir::new("bob-cli-collect-done-vault");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] one #task
- [-] two #task
",
    );
    write_file(&vault.path().join("foo/bar.md"), "- [x] nested #task\n");
    write_file(&vault.path().join("foo/not-markdown.txt"), "#task\n");
    write_file(&vault.path().join("done/old.md"), "- [x] archived #task\n");
    write_file(
        &vault.path().join("_generated/topic/obsidian.md"),
        "- [x] generated #task\n",
    );
    write_file(
        &vault.path().join("_templates/template.md"),
        "- [x] template #task\n",
    );
    write_file(&vault.path().join(".git/config.md"), "- [x] git #task\n");
    write_file(
        &vault.path().join(".obsidian/settings.md"),
        "- [x] settings #task\n",
    );

    let plan = build_collection_plan(vault.path(), 2).expect("build plan");

    assert_eq!(plan.scanned_files, 2);
    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.relative_source_path, PathBuf::from("obsidian.md"));
    assert_eq!(
        file.relative_archive_path,
        PathBuf::from("done/obsidian_done.md")
    );
    assert_eq!(file.task_count, 2);
    assert_eq!(
        file.source_contents,
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

"
    );
    assert_eq!(
        file.archive_contents.as_deref(),
        Some(
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] one #task
- [-] two #task
"
        )
    );
    assert!(file.source_metadata_updated);
    assert!(!file.archive_metadata_updated);
}

#[test]
fn canceled_only_tasks_move_when_threshold_is_met() {
    let vault = TempDir::new("bob-cli-collect-done-canceled-only");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [-] canceled one #task
  detail
- [-] canceled two #task
",
    );

    let plan = build_collection_plan(vault.path(), 2).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 2);
    assert_eq!(
        file.source_contents,
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

"
    );
    assert_eq!(
        file.archive_contents.as_deref(),
        Some(
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [-] canceled one #task
  detail
- [-] canceled two #task
"
        )
    );
}

#[test]
fn canceled_only_tasks_below_threshold_remain_in_source() {
    let contents = "- [-] canceled #task\n";
    let transform = transform_markdown(contents);
    assert_eq!(transform.task_count, 1);

    let vault = TempDir::new("bob-cli-collect-done-canceled-below");
    let source = vault.path().join("obsidian.md");
    write_file(&source, contents);

    let plan = build_collection_plan(vault.path(), 2).expect("build plan");

    assert!(plan.files.is_empty());
    assert_eq!(fs::read_to_string(&source).expect("read source"), contents);
}

#[test]
fn task_moving_plan_writes_archive_with_nested_source_parent() {
    let vault = TempDir::new("bob-cli-collect-done-nested-parent");
    write_file(
        &vault.path().join("foo/bar.md"),
        "\
- [x] nested #task
- [ ] active #task
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(
        file.archive_contents.as_deref(),
        Some(
            "\
---
parent: \"[[foo/bar]]\"
type: \"[[done]]\"
---

- [x] nested #task
"
        )
    );
    assert!(!file.archive_metadata_updated);
}

#[test]
fn includes_nested_path_note_when_it_meets_threshold() {
    let vault = TempDir::new("bob-cli-collect-done-nested-vault");
    write_file(&vault.path().join("foo/bar.md"), "- [x] nested #task\n");

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    assert_eq!(
        plan.files[0].relative_source_path,
        PathBuf::from("foo/bar.md")
    );
    assert_eq!(
        plan.files[0].relative_archive_path,
        PathBuf::from("done/foo/bar_done.md")
    );
}

#[test]
fn task_moves_repair_dependency_ids_in_archive_and_all_dependents() {
    let vault = TempDir::new("bob-cli-collect-done-dependency-metadata");
    write_file(
        &vault.path().join("projects/Shared.md"),
        "- [x] #task Review [id:: projects__Shared__review] ^review\n",
    );
    write_file(
        &vault.path().join("A.md"),
        "- [ ] #task A [dependsOn:: projects__Shared__review]\n",
    );
    write_file(
        &vault.path().join("nested/B.md"),
        "- [ ] #task B [dependsOn:: projects__Shared__review, other]\n",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");
    assert_eq!(plan.dependency_metadata_repair_count(), 3);
    let moved = plan
        .files
        .iter()
        .find(|file| {
            file.relative_source_path == Path::new("projects/Shared.md")
        })
        .expect("moved source");
    assert!(moved
        .archive_contents
        .as_deref()
        .unwrap()
        .contains("[id:: done__projects__Shared_done__review] ^review"));
    assert_eq!(plan.link_repairs.len(), 2);
    for repair in &plan.link_repairs {
        assert!(repair
            .contents
            .contains("[dependsOn:: done__projects__Shared_done__review"));
    }

    for file in &plan.files {
        apply_file_plan(vault.path(), file).unwrap();
    }
    for repair in &plan.link_repairs {
        apply_link_repair_plan(vault.path(), repair).unwrap();
    }
    let rerun = build_collection_plan(vault.path(), 1).expect("rerun plan");
    assert_eq!(rerun.dependency_metadata_repair_count(), 0);
}

#[test]
fn collecting_tasks_adds_done_tasks_to_source() {
    let vault = TempDir::new("bob-cli-collect-done-source-link");
    write_file(
        &vault.path().join("foo/bar.md"),
        "\
- [x] nested #task
- [ ] active #task
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 1);
    assert!(file.source_metadata_updated);
    assert_eq!(
        file.source_contents,
        "\
---
done_tasks: \"[[done/foo/bar_done]]\"
---

- [ ] active #task
"
    );
}

#[test]
fn existing_archive_creates_metadata_only_source_update() {
    let vault = TempDir::new("bob-cli-collect-done-backfill");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] below threshold #task
- [ ] active #task
",
    );
    write_file(
        &vault.path().join("done/obsidian_done.md"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
",
    );

    let plan = build_collection_plan(vault.path(), 2).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 0);
    assert!(file.archive_contents.is_none());
    assert!(file.source_metadata_updated);
    assert!(!file.archive_metadata_updated);
    assert_eq!(
        file.source_contents,
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [x] below threshold #task
- [ ] active #task
"
    );
}

#[test]
fn existing_archive_with_stale_metadata_creates_archive_only_plan() {
    let vault = TempDir::new("bob-cli-collect-done-archive-repair");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
",
    );
    write_file(
        &vault.path().join("done/obsidian_done.md"),
        "\
---
parent: \"[[done]]\"
---

- [x] old #task
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    let file = &plan.files[0];
    assert_eq!(file.task_count, 0);
    assert!(!file.source_metadata_updated);
    assert!(file.archive_metadata_updated);
    assert!(!file.writes_source());
    assert_eq!(
        file.archive_contents.as_deref(),
        Some(
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
"
        )
    );
}

#[test]
fn already_linked_source_with_existing_archive_is_not_planned() {
    let vault = TempDir::new("bob-cli-collect-done-already-linked");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
",
    );
    write_file(
        &vault.path().join("done/obsidian_done.md"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert!(plan.files.is_empty());
}

#[test]
fn missing_archive_without_threshold_tasks_is_not_planned() {
    let vault = TempDir::new("bob-cli-collect-done-missing-archive");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] below threshold #task
- [ ] active #task
",
    );

    let plan = build_collection_plan(vault.path(), 2).expect("build plan");

    assert!(plan.files.is_empty());
}

#[test]
fn task_moving_plan_repairs_links_in_separate_notes() {
    let vault = TempDir::new("bob-cli-collect-done-link-plan");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] done #task ^abc123
- [ ] active #task
",
    );
    write_file(
        &vault.path().join("daily.md"),
        "References [[obsidian#^abc123]] and ![[obsidian#^abc123]].\n",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.link_repairs.len(), 1);
    assert_eq!(plan.moved_block_id_count(), 1);
    assert_eq!(plan.link_repair_count(), 2);
    assert_eq!(
        plan.link_repairs[0].relative_path,
        PathBuf::from("daily.md")
    );
    assert_eq!(
            plan.link_repairs[0].contents,
            "References [[done/obsidian_done#^abc123]] and ![[done/obsidian_done#^abc123]].\n"
        );
}

#[test]
fn planned_source_and_archive_contents_are_link_repaired() {
    let vault = TempDir::new("bob-cli-collect-done-planned-link-repair");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] done #task ^abc123
- [ ] active #task
  follow-up [[obsidian#^abc123]]
",
    );
    write_file(
        &vault.path().join("done/obsidian_done.md"),
        "\
# Archive
Old reference [[obsidian#^abc123]]
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    assert!(plan.link_repairs.is_empty());
    let file = &plan.files[0];
    assert_eq!(file.source_link_repair_count, 1);
    assert_eq!(file.archive_link_repair_count, 1);
    assert!(
        file.source_contents
            .contains("[[done/obsidian_done#^abc123]]"),
        "expected source link repair:\n{}",
        file.source_contents
    );
    let archive_contents =
        file.archive_contents.as_deref().expect("archive contents");
    assert!(
        archive_contents.contains("[[done/obsidian_done#^abc123]]"),
        "expected archive link repair:\n{archive_contents}"
    );
}

#[test]
fn link_repair_scan_includes_done_notes() {
    let vault = TempDir::new("bob-cli-collect-done-repair-done-notes");
    write_file(
        &vault.path().join("obsidian.md"),
        "- [x] done #task ^abc123\n",
    );
    write_file(
        &vault.path().join("done/old.md"),
        "Archive reference [[obsidian#^abc123]].\n",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.link_repairs.len(), 1);
    assert_eq!(
        plan.link_repairs[0].relative_path,
        PathBuf::from("done/old.md")
    );
    assert_eq!(
        plan.link_repairs[0].contents,
        "Archive reference [[done/obsidian_done#^abc123]].\n"
    );
}

#[test]
fn generated_tag_pages_do_not_make_source_basename_ambiguous() {
    let vault = TempDir::new("bob-cli-collect-done-generated-basename-link");
    write_file(
        &vault.path().join("sase.md"),
        "\
- [x] done #task ^auto-pair
- [ ] active #task
",
    );
    write_file(
        &vault.path().join("_generated/tag_pages/topic/sase.md"),
        "# Generated tag page\n",
    );
    write_file(
        &vault.path().join("2026/20260621.md"),
        "Reference [[sase#^auto-pair]].\n",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.link_repairs.len(), 1);
    assert_eq!(
        plan.link_repairs[0].relative_path,
        PathBuf::from("2026/20260621.md")
    );
    assert_eq!(
        plan.link_repairs[0].contents,
        "Reference [[done/sase_done#^auto-pair]].\n"
    );
}

#[test]
fn generated_and_template_directories_are_not_collected_or_repaired() {
    let vault = TempDir::new("bob-cli-collect-done-generated-template-skip");
    write_file(
        &vault.path().join("source.md"),
        "\
---
done_tasks: \"[[done/source_done]]\"
---

- [ ] active #task
",
    );
    write_file(
        &vault.path().join("done/source_done.md"),
        "\
---
parent: \"[[source]]\"
type: \"[[done]]\"
---

- [x] old #task ^old
",
    );
    write_file(
        &vault.path().join("_generated/tag_pages/topic/source.md"),
        "\
- [x] generated #task ^generated
Generated link [[source#^old]].
",
    );
    write_file(
        &vault.path().join("_templates/source.md"),
        "\
- [x] template #task ^template
Template link [[source#^old]].
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.scanned_files, 1);
    assert!(plan.is_empty());
    assert!(plan.link_repairs.is_empty());
}

#[test]
fn self_heals_preexisting_block_links_to_archive() {
    let vault = TempDir::new("bob-cli-collect-done-self-heal-links");
    write_file(
        &vault.path().join("sase.md"),
        "\
---
done_tasks: \"[[done/sase_done]]\"
---

- [ ] active #task
",
    );
    write_file(
        &vault.path().join("done/sase_done.md"),
        "\
---
parent: \"[[sase]]\"
type: \"[[done]]\"
---

- [x] archived #task ^auto-pair
",
    );
    write_file(
        &vault.path().join("daily.md"),
        "\
Wiki [[sase#^auto-pair]]
Embed ![[sase#^auto-pair]]
Alias [[sase#^auto-pair|Auto Pair]]
Markdown [task](sase.md#^auto-pair)
URL [task](https://example.com/sase#^auto-pair)
",
    );

    let plan = build_collection_plan(vault.path(), 10).expect("build plan");

    assert!(plan.files.is_empty());
    assert!(!plan.is_empty());
    assert_eq!(plan.link_repair_count(), 4);
    assert_eq!(plan.link_repairs.len(), 1);
    assert_eq!(
        plan.link_repairs[0].contents,
        "\
Wiki [[done/sase_done#^auto-pair]]
Embed ![[done/sase_done#^auto-pair]]
Alias [[done/sase_done#^auto-pair|Auto Pair]]
Markdown [task](done/sase_done.md#^auto-pair)
URL [task](https://example.com/sase#^auto-pair)
"
    );
}

#[test]
fn self_healing_is_idempotent_after_links_are_repaired() {
    let vault = TempDir::new("bob-cli-collect-done-self-heal-idempotent");
    write_file(
        &vault.path().join("sase.md"),
        "\
---
done_tasks: \"[[done/sase_done]]\"
---

- [ ] active #task
",
    );
    write_file(
        &vault.path().join("done/sase_done.md"),
        "\
---
parent: \"[[sase]]\"
type: \"[[done]]\"
---

- [x] archived #task ^auto-pair
",
    );
    write_file(
        &vault.path().join("daily.md"),
        "Reference [[sase#^auto-pair]].\n",
    );

    let first = build_collection_plan(vault.path(), 10).expect("first plan");
    assert_eq!(first.link_repair_count(), 1);
    write_file(
        &vault.path().join("daily.md"),
        &first.link_repairs[0].contents,
    );

    let second = build_collection_plan(vault.path(), 10).expect("second plan");

    assert!(second.is_empty());
    assert_eq!(second.link_repair_count(), 0);
}

#[test]
fn source_block_id_keeps_links_pointing_at_source() {
    let vault = TempDir::new("bob-cli-collect-done-source-id-stays-live");
    write_file(
        &vault.path().join("sase.md"),
        "\
---
done_tasks: \"[[done/sase_done]]\"
---

- [ ] active #task ^auto-pair
",
    );
    write_file(
        &vault.path().join("done/sase_done.md"),
        "\
---
parent: \"[[sase]]\"
type: \"[[done]]\"
---

- [x] archived #task ^auto-pair
",
    );
    write_file(
        &vault.path().join("daily.md"),
        "Reference [[sase#^auto-pair]].\n",
    );

    let plan = build_collection_plan(vault.path(), 10).expect("build plan");

    assert!(plan.is_empty());
    assert_eq!(plan.link_repair_count(), 0);
}

#[test]
fn duplicate_moved_block_ids_do_not_rewrite_links() {
    let vault = TempDir::new("bob-cli-collect-done-duplicate-link-plan");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] first #task ^dup
- [x] second #task ^dup
",
    );
    write_file(&vault.path().join("daily.md"), "[[obsidian#^dup]]\n");

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.moved_block_id_count(), 1);
    assert_eq!(plan.ambiguous_moved_block_id_count(), 1);
    assert_eq!(plan.moved_block_id_rename_count(), 1);
    assert_eq!(plan.link_repair_count(), 0);
    assert!(plan.link_repairs.is_empty());
    assert!(plan.files[0]
        .archive_contents
        .as_deref()
        .expect("archive contents")
        .contains("- [x] first #task ^dup\n- [x] second #task ^dup-1\n"));
}

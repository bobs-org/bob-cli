//! Unit tests for argument parsing and transforms.
use super::*;

#[test]
fn parses_default_threshold() {
    match parse_args(vec![]) {
        ParseResult::Run(args) => {
            assert_eq!(args.threshold, DEFAULT_THRESHOLD);
        }
        _ => panic!("expected runnable args"),
    }
}

#[test]
fn parses_threshold_option() {
    match parse_args(os_args(["--threshold", "15"])) {
        ParseResult::Run(args) => assert_eq!(args, Args { threshold: 15 }),
        _ => panic!("expected runnable args"),
    }
}

#[test]
fn dependency_ids_preserve_path_case_and_qualify_nested_notes() {
    assert_eq!(
        dependency_id(Path::new("projects/Shared.md"), "review").unwrap(),
        "projects__Shared__review"
    );
    assert!(dependency_id(Path::new("My Notes.md"), "review").is_err());
}

#[test]
fn dependency_metadata_repair_rewrites_exact_tokens_only() {
    let targets = moved_targets([(
        "projects/Shared.md",
        "review",
        "done/projects/Shared",
    )]);
    let repair = repair_dependency_metadata(
        "- [x] #task Target [id:: projects__Shared__review] ^review\n\
- [ ] #task A [dependsOn:: projects__Shared__review, unrelated]\n\
Prose projects__Shared__review\n",
        &targets,
    );
    assert_eq!(repair.count, 2);
    assert!(repair
        .contents
        .contains("[id:: done__projects__Shared__review] ^review"));
    assert!(repair
        .contents
        .contains("[dependsOn:: done__projects__Shared__review, unrelated]"));
    assert!(repair.contents.contains("Prose projects__Shared__review"));
}

#[test]
fn dependency_metadata_repair_supports_task_field_grammar_and_skips_code() {
    let targets = moved_targets([(
        "projects/Shared.md",
        "review",
        "done/projects/Shared",
    )]);
    let repair = repair_dependency_metadata(
            "(dependsOn:: projects__Shared__review)\n[ dependsOn :: projects__Shared__review]\n`[id:: projects__Shared__review]`\n```\n[dependsOn:: projects__Shared__review]\n```\n",
            &targets,
        );
    assert_eq!(repair.count, 2);
    assert!(repair
        .contents
        .contains("(dependsOn:: done__projects__Shared__review)"));
    assert!(repair
        .contents
        .contains("[ dependsOn :: done__projects__Shared__review]"));
    assert!(repair
        .contents
        .contains("`[id:: projects__Shared__review]`"));
    assert!(repair
        .contents
        .contains("```\n[dependsOn:: projects__Shared__review]\n```"));
}

#[test]
fn block_ids_are_only_end_of_line_obsidian_anchors() {
    assert_eq!(
        block_ids_in_markdown(
            "see ^c for details\n10 ^2 equals 100\nreal ^ok-id\n"
        ),
        vec!["ok-id"]
    );
    assert!(block_ids_in_markdown("underscore ^not_valid\n").is_empty());
}

#[test]
fn unqualifiable_paths_do_not_abort_identity_indexing() {
    let vault = TempDir::new("unsupported dependency path");
    write_file(
        &vault.path().join("Untitled 1.md"),
        "- [ ] #task Keep ^task\n",
    );
    let plan = build_collection_plan(vault.path(), 10)
        .expect("unsupported path is skipped");
    assert!(plan.is_empty());
}

#[test]
fn parses_short_threshold_option() {
    match parse_args(os_args(["-t", "15"])) {
        ParseResult::Run(args) => assert_eq!(args, Args { threshold: 15 }),
        _ => panic!("expected runnable args"),
    }
}

#[test]
fn parses_threshold_equals_option() {
    match parse_args(os_args(["--threshold=3"])) {
        ParseResult::Run(args) => assert_eq!(args, Args { threshold: 3 }),
        _ => panic!("expected runnable args"),
    }
}

#[test]
fn parses_short_threshold_equals_option() {
    match parse_args(os_args(["-t=3"])) {
        ParseResult::Run(args) => assert_eq!(args, Args { threshold: 3 }),
        _ => panic!("expected runnable args"),
    }
}

#[test]
fn parses_attached_short_threshold_option() {
    match parse_args(os_args(["-t3"])) {
        ParseResult::Run(args) => assert_eq!(args, Args { threshold: 3 }),
        _ => panic!("expected runnable args"),
    }
}

#[test]
fn rejects_zero_threshold() {
    match parse_args(os_args(["--threshold", "0"])) {
        ParseResult::Error(message) => {
            assert!(message.contains("at least 1"));
        }
        _ => panic!("expected parse error"),
    }
}

#[test]
fn recognizes_done_and_canceled_task_lines_only() {
    let transform = transform_markdown(
        "\
- [x] done #task
- [X] uppercase done #task
- [-] canceled #task
- [ ] active #task
- [/] in progress #task
- [x] done without task tag
- [x] not quite #tasks
",
    );

    assert_eq!(transform.task_count, 3);
    assert_eq!(
        transform.archive_append,
        "\
- [x] done #task
- [X] uppercase done #task
- [-] canceled #task
"
    );
    assert_eq!(
        transform.source_contents,
        "\
- [ ] active #task
- [/] in progress #task
- [x] done without task tag
- [x] not quite #tasks
"
    );
}

#[test]
fn extracts_nested_blocks_and_continuations() {
    let transform = transform_markdown(include_str!(
        "../../../../tests/fixtures/collect_done/nested_blocks.md"
    ));

    assert_eq!(transform.task_count, 1);
    assert_eq!(
        transform.source_contents,
        include_str!(
            "../../../../tests/fixtures/collect_done/nested_blocks_source.md"
        )
    );
    assert_eq!(
        transform.archive_append,
        include_str!(
            "../../../../tests/fixtures/collect_done/nested_blocks_archive.md"
        )
    );
}

#[test]
fn completed_child_moves_without_collecting_active_parent() {
    let transform = transform_markdown(
        "\
- [ ] active parent #task
  - [x] done child #task
    child continuation
  - [/] active child #task
",
    );

    assert_eq!(transform.task_count, 1);
    assert_eq!(
        transform.source_contents,
        "\
- [ ] active parent #task
  - [/] active child #task
"
    );
    assert_eq!(
        transform.archive_append,
        "  - [x] done child #task\n    child continuation\n"
    );
}

#[test]
fn preserves_line_endings_in_source_and_archive() {
    let transform = transform_markdown(
        "- [x] done #task\r\n  detail\r\n- [ ] keep #task\r\n",
    );

    assert_eq!(transform.task_count, 1);
    assert_eq!(transform.source_contents, "- [ ] keep #task\r\n");
    assert_eq!(transform.archive_append, "- [x] done #task\r\n  detail\r\n");
}

#[test]
fn extracts_block_ids_from_every_moved_task_block_line() {
    let transform = transform_markdown(
        "\
- [x] done #task ^top
  child continuation ^child-id
  linked block [[other#^not-this-one]]
- [ ] active #task ^active
",
    );

    assert_eq!(transform.task_count, 1);
    assert_eq!(transform.moved_block_ids, string_set(["top", "child-id"]));
    assert!(transform.ambiguous_moved_block_ids.is_empty());
}

#[test]
fn duplicate_moved_block_ids_are_ambiguous() {
    let transform = transform_markdown(
        "\
- [x] first #task ^dup
- [x] second #task ^dup
",
    );

    assert_eq!(transform.moved_block_ids, string_set(["dup"]));
    assert_eq!(transform.ambiguous_moved_block_ids, string_set(["dup"]));
}

#[test]
fn duplicate_moved_block_ids_become_unique_archive_ids() {
    let vault = TempDir::new("bob-cli-collect-done-duplicate-ids");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] first #task ^dup
- [x] second #task ^dup
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.moved_block_id_count(), 1);
    assert_eq!(plan.ambiguous_moved_block_id_count(), 1);
    assert_eq!(plan.moved_block_id_rename_count(), 1);
    assert_eq!(
        plan.files[0].archive_contents.as_deref(),
        Some(
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] first #task ^dup
- [x] second #task ^dup-1
"
        )
    );
}

#[test]
fn existing_archive_block_ids_reserve_original_ids() {
    let vault = TempDir::new("bob-cli-collect-done-existing-id");
    write_file(&vault.path().join("obsidian.md"), "- [x] new #task ^dup\n");
    write_file(
        &vault.path().join("done/obsidian_done.md"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task ^dup
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.moved_block_id_rename_count(), 1);
    assert_eq!(
        plan.files[0].archive_contents.as_deref(),
        Some(
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task ^dup
- [x] new #task ^dup-1
"
        )
    );
}

#[test]
fn block_id_suffix_selection_skips_existing_candidates() {
    let vault = TempDir::new("bob-cli-collect-done-existing-id-suffix");
    write_file(&vault.path().join("obsidian.md"), "- [x] new #task ^dup\n");
    write_file(
        &vault.path().join("done/obsidian_done.md"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task ^dup
- [x] old suffix #task ^dup-1
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.moved_block_id_rename_count(), 1);
    assert!(plan.files[0]
        .archive_contents
        .as_deref()
        .expect("archive contents")
        .contains("- [x] new #task ^dup-2\n"));
}

#[test]
fn block_id_suffix_selection_preserves_distinct_moved_ids() {
    let vault = TempDir::new("bob-cli-collect-done-preserve-distinct-id");
    write_file(
        &vault.path().join("obsidian.md"),
        "\
- [x] first #task ^dup
- [x] second #task ^dup
- [x] distinct #task ^dup-1
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.moved_block_id_rename_count(), 1);
    assert_eq!(
        plan.files[0].archive_contents.as_deref(),
        Some(
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] first #task ^dup
- [x] second #task ^dup-2
- [x] distinct #task ^dup-1
"
        )
    );
}

#[test]
fn block_id_deduplication_preserves_crlf_line_endings() {
    let vault = TempDir::new("bob-cli-collect-done-crlf-id-dedup");
    write_file(
        &vault.path().join("obsidian.md"),
        "- [x] first #task ^dup\r\n- [x] second #task ^dup\r\n",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.moved_block_id_rename_count(), 1);
    assert_eq!(
            plan.files[0].archive_contents.as_deref(),
            Some(
                "---\r\nparent: \"[[obsidian]]\"\r\ntype: \"[[done]]\"\r\n---\r\n\r\n- [x] first #task ^dup\r\n- [x] second #task ^dup-1\r\n"
            )
        );
}

#[test]
fn pathless_archive_links_gain_the_source_note_path() {
    // A pathless [[#^x]] inside an archived block whose target stayed
    // behind gains the source note path — on Depends-On lines and on
    // legacy children alike. Fenced code, inline code, moved blocks,
    // and already-pathed links are untouched.
    let stayed = BTreeSet::from(["open".to_string(), "held".to_string()]);
    let repair = repair_pathless_archive_links(
        "- [x] #task Archived ^moved\n  - ⛓️ **DEPENDS ON:** [[#^open]] • [[#^moved]]\n  - ![[#^held]]\n  - [[other#^open]]\n```\n- [[#^open]]\n```\n`[[#^open]]`\n",
        "projects/Work",
        &stayed,
    );
    assert_eq!(repair.link_count, 2);
    assert!(repair.contents.contains(
        "- ⛓️ **DEPENDS ON:** [[projects/Work#^open]] • [[#^moved]]"
    ));
    assert!(repair.contents.contains("- ![[projects/Work#^held]]"));
    assert!(repair.contents.contains("- [[other#^open]]"));
    assert!(repair.contents.contains("```\n- [[#^open]]\n```"));
    assert!(repair.contents.contains("`[[#^open]]`"));
}

#[test]
fn link_repair_uses_renamed_unique_moved_block_id() {
    let vault = TempDir::new("bob-cli-collect-done-renamed-link-plan");
    write_file(
        &vault.path().join("obsidian.md"),
        "- [x] done #task ^abc123\n",
    );
    write_file(
        &vault.path().join("daily.md"),
        "Reference [[obsidian#^abc123]].\n",
    );
    write_file(
        &vault.path().join("done/obsidian_done.md"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task ^abc123
",
    );

    let plan = build_collection_plan(vault.path(), 1).expect("build plan");

    assert_eq!(plan.moved_block_id_rename_count(), 1);
    assert_eq!(plan.link_repair_count(), 1);
    assert_eq!(
        plan.link_repairs[0].contents,
        "Reference [[done/obsidian_done#^abc123-1]].\n"
    );
    assert!(plan.files[0]
        .archive_contents
        .as_deref()
        .expect("archive contents")
        .contains("- [x] done #task ^abc123-1\n"));
}

#[test]
fn below_threshold_block_ids_do_not_trigger_link_repair() {
    let vault = TempDir::new("bob-cli-collect-done-below-threshold-links");
    write_file(
        &vault.path().join("obsidian.md"),
        "- [x] below threshold #task ^abc123\n",
    );
    write_file(
        &vault.path().join("daily.md"),
        "Still points at [[obsidian#^abc123]].\n",
    );

    let plan = build_collection_plan(vault.path(), 2).expect("build plan");

    assert!(plan.is_empty());
    assert_eq!(plan.moved_block_id_count(), 0);
    assert_eq!(plan.link_repair_count(), 0);
}

#[test]
fn repairs_wikilinks_embeds_and_aliases_to_moved_blocks() {
    let index =
        note_index(["obsidian.md", "daily.md", "done/obsidian_done.md"]);
    let moved_targets =
        moved_targets([("obsidian.md", "abc123", "done/obsidian_done")]);

    let repair = repair_links_in_note(
        "\
[[obsidian#^abc123]]
![[obsidian#^abc123]]
[[obsidian#^abc123|collect-done]]
🍅 ~~[[obsidian#^abc123|Pomodoro]]~~
[[obsidian#Heading]]
[[obsidian#^other]]
[[done/obsidian_done#^abc123]]
",
        Path::new("daily.md"),
        &index,
        &moved_targets,
    );

    assert_eq!(repair.link_count, 4);
    assert_eq!(
        repair.contents,
        "\
[[done/obsidian_done#^abc123]]
![[done/obsidian_done#^abc123]]
[[done/obsidian_done#^abc123|collect-done]]
🍅 ~~[[done/obsidian_done#^abc123|Pomodoro]]~~
[[obsidian#Heading]]
[[obsidian#^other]]
[[done/obsidian_done#^abc123]]
"
    );
}

#[test]
fn repairs_same_note_nested_and_unique_basename_links() {
    let index = note_index(["foo/bar.md", "done/foo/bar_done.md"]);
    let moved_targets =
        moved_targets([("foo/bar.md", "nested", "done/foo/bar_done")]);

    let repair = repair_links_in_note(
        "[[#^nested]] [[foo/bar#^nested]] [[bar#^nested]]\n",
        Path::new("foo/bar.md"),
        &index,
        &moved_targets,
    );

    assert_eq!(repair.link_count, 3);
    assert_eq!(
            repair.contents,
            "[[done/foo/bar_done#^nested]] [[done/foo/bar_done#^nested]] [[done/foo/bar_done#^nested]]\n"
        );
}

#[test]
fn leaves_ambiguous_basename_links_unchanged() {
    let index = note_index([
        "foo/obsidian.md",
        "bar/obsidian.md",
        "done/foo/obsidian_done.md",
    ]);
    let moved_targets = moved_targets([(
        "foo/obsidian.md",
        "abc123",
        "done/foo/obsidian_done",
    )]);

    let repair = repair_links_in_note(
        "[[obsidian#^abc123]]\n",
        Path::new("daily.md"),
        &index,
        &moved_targets,
    );

    assert_eq!(repair.link_count, 0);
    assert_eq!(repair.contents, "[[obsidian#^abc123]]\n");
}

#[test]
fn repairs_simple_markdown_inline_block_links() {
    let index = note_index(["obsidian.md", "done/obsidian_done.md"]);
    let moved_targets =
        moved_targets([("obsidian.md", "abc123", "done/obsidian_done")]);

    let repair = repair_links_in_note(
        "\
[md](obsidian.md#^abc123)
[same](#^abc123)
[wikiish](obsidian#^abc123)
[titled](obsidian.md#^abc123 \"title\")
",
        Path::new("obsidian.md"),
        &index,
        &moved_targets,
    );

    assert_eq!(repair.link_count, 3);
    assert_eq!(
        repair.contents,
        "\
[md](done/obsidian_done.md#^abc123)
[same](done/obsidian_done.md#^abc123)
[wikiish](done/obsidian_done#^abc123)
[titled](obsidian.md#^abc123 \"title\")
"
    );
}

#[test]
fn markdown_repair_skips_wikilink_spans() {
    let index = note_index(["obsidian.md", "done/obsidian_done.md"]);
    let moved_targets =
        moved_targets([("obsidian.md", "abc123", "done/obsidian_done")]);

    let repair = repair_links_in_note(
        "[[note|[alias](obsidian.md#^abc123)]] [real](obsidian.md#^abc123)\n",
        Path::new("daily.md"),
        &index,
        &moved_targets,
    );

    assert_eq!(repair.link_count, 1);
    assert_eq!(
            repair.contents,
            "[[note|[alias](obsidian.md#^abc123)]] [real](done/obsidian_done.md#^abc123)\n"
        );
}

#[test]
fn maps_source_notes_to_archive_notes() {
    assert_eq!(
        archive_relative_path(Path::new("obsidian.md")).unwrap(),
        PathBuf::from("done/obsidian_done.md")
    );
    assert_eq!(
        archive_relative_path(Path::new("foo/bar.md")).unwrap(),
        PathBuf::from("done/foo/bar_done.md")
    );
}

#[test]
fn maps_archive_notes_to_obsidian_wiki_links() {
    assert_eq!(
        archive_wiki_link(Path::new("done/obsidian_done.md")).unwrap(),
        "[[done/obsidian_done]]"
    );
    assert_eq!(
        archive_wiki_link(Path::new("done/foo/bar_done.md")).unwrap(),
        "[[done/foo/bar_done]]"
    );
}

#[test]
fn maps_source_notes_to_obsidian_wiki_links() {
    assert_eq!(
        source_wiki_link(Path::new("obsidian.md")).unwrap(),
        "[[obsidian]]"
    );
    assert_eq!(
        source_wiki_link(Path::new("foo/bar.md")).unwrap(),
        "[[foo/bar]]"
    );
}

#[test]
fn creates_archive_frontmatter_for_new_archive_note() {
    let contents = archive_contents(None, "- [x] done #task\n", "[[obsidian]]");

    assert_eq!(
        contents,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] done #task
"
    );
}

#[test]
fn adds_archive_parent_to_existing_frontmatter() {
    let contents = archive_contents(
        Some(
            "\
---
title: Existing archive
---

- [x] old #task
",
        ),
        "- [-] canceled #task\n",
        "[[obsidian]]",
    );

    assert_eq!(
        contents,
        "\
---
title: Existing archive
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
- [-] canceled #task
"
    );
}

#[test]
fn updates_existing_archive_parent_frontmatter() {
    let contents = archive_contents(
        Some(
            "\
---
parent: \"[[old]]\"
type: \"[[done]]\"
---
",
        ),
        "- [x] done #task\n",
        "[[obsidian]]",
    );

    assert_eq!(
        contents,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---
- [x] done #task
"
    );
}

#[test]
fn inserts_missing_archive_type_frontmatter() {
    let contents = archive_contents(
        Some(
            "\
---
parent: \"[[obsidian]]\"
---

- [x] old #task
",
        ),
        "",
        "[[obsidian]]",
    );

    assert_eq!(
        contents,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
"
    );
}

#[test]
fn replaces_stale_archive_type_frontmatter() {
    let contents = archive_contents(
        Some(
            "\
---
parent: \"[[obsidian]]\"
type: \"[[old]]\"
---
",
        ),
        "",
        "[[obsidian]]",
    );

    assert_eq!(
        contents,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---
"
    );
}

#[test]
fn leaves_correct_archive_frontmatter_unchanged() {
    let original = "\
---
title: Existing archive
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
";

    assert_eq!(
        archive_contents(Some(original), "", "[[obsidian]]"),
        original
    );
}

#[test]
fn preserves_crlf_when_repairing_archive_frontmatter() {
    let contents = archive_contents(
        Some("---\r\nparent: \"[[done]]\"\r\n---\r\n\r\n"),
        "",
        "[[obsidian]]",
    );

    assert_eq!(
        contents,
        "---\r\nparent: \"[[obsidian]]\"\r\ntype: \"[[done]]\"\r\n---\r\n\r\n"
    );
}

#[test]
fn creates_archive_frontmatter_with_nested_source_parent() {
    let contents = archive_contents(None, "- [x] done #task\n", "[[foo/bar]]");

    assert_eq!(
        contents,
        "\
---
parent: \"[[foo/bar]]\"
type: \"[[done]]\"
---

- [x] done #task
"
    );
}

#[test]
fn prepends_archive_frontmatter_when_existing_note_has_none() {
    let contents = archive_contents(
        Some("# Archive\n"),
        "- [x] done #task\n",
        "[[obsidian]]",
    );

    assert_eq!(
        contents,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

# Archive
- [x] done #task
"
    );
}

#[test]
fn adds_done_tasks_to_existing_source_frontmatter() {
    let contents = ensure_source_done_tasks_frontmatter(
        "\
---
title: Project
---

# Project
",
        "[[done/project_done]]",
    );

    assert_eq!(
        contents,
        "\
---
title: Project
done_tasks: \"[[done/project_done]]\"
---

# Project
"
    );
}

#[test]
fn creates_source_frontmatter_for_done_tasks() {
    let contents = ensure_source_done_tasks_frontmatter(
        "# Project\n",
        "[[done/project_done]]",
    );

    assert_eq!(
        contents,
        "\
---
done_tasks: \"[[done/project_done]]\"
---

# Project
"
    );
}

#[test]
fn replaces_stale_done_tasks_frontmatter() {
    let contents = ensure_source_done_tasks_frontmatter(
        "\
---
done_tasks: \"[[done/old_done]]\"
title: Project
---
",
        "[[done/project_done]]",
    );

    assert_eq!(
        contents,
        "\
---
done_tasks: \"[[done/project_done]]\"
title: Project
---
"
    );
}

#[test]
fn leaves_correct_done_tasks_frontmatter_unchanged() {
    let original = "\
---
done_tasks: \"[[done/project_done]]\"
title: Project
---

# Project
";

    assert_eq!(
        ensure_source_done_tasks_frontmatter(original, "[[done/project_done]]"),
        original
    );
}

#[test]
fn preserves_crlf_when_adding_done_tasks_frontmatter() {
    let contents = ensure_source_done_tasks_frontmatter(
        "---\r\ntitle: Project\r\n---\r\n\r\n# Project\r\n",
        "[[done/project_done]]",
    );

    assert_eq!(
            contents,
            "---\r\ntitle: Project\r\ndone_tasks: \"[[done/project_done]]\"\r\n---\r\n\r\n# Project\r\n"
        );
}

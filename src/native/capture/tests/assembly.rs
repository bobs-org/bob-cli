//! Capture assembly, managed logs, and Pomodoro selection tests.
use super::*;

#[test]
fn assembles_capture_block_with_clip_children_then_schedule_log() {
    let capture_line = "- [?] #task someday idea [created::2026-08-07] [priority::lowest] [scheduled::2026-11-02]";
    let clip_lines = vec![
        "\t- clip child one".to_string(),
        "\t- clip child two".to_string(),
    ];
    let schedule_log_lines = vec![
        "\t- 🗓️ **SCHEDULE LOG**".to_string(),
        "\t\t- *2026-11-02* — 🎲 P0 → P4 · in **91** (91–365) days".to_string(),
    ];

    let block = assemble_capture_block(
        capture_line,
        None,
        Some(&clip_lines),
        Some(&schedule_log_lines),
    );

    assert_eq!(
        block,
        [
            capture_line,
            "\t- clip child one",
            "\t- clip child two",
            "\t- 🗓️ **SCHEDULE LOG**",
            "\t\t- *2026-11-02* — 🎲 P0 → P4 · in **91** (91–365) days",
        ]
        .join("\n")
    );
}

#[test]
fn assembles_capture_block_with_sub_bullets_before_clip_and_schedule_log() {
    let capture_line = "- [ ] #task plan trip [created::2026-08-07]";
    let sub_bullet_lines = vec![
        "\t- book flights".to_string(),
        "\t- reserve hotel".to_string(),
    ];
    let clip_lines = vec!["\t- clip child".to_string()];
    let schedule_log_lines = vec!["\t- 🗓️ **SCHEDULE LOG**".to_string()];

    let block = assemble_capture_block(
        capture_line,
        Some(&sub_bullet_lines),
        Some(&clip_lines),
        Some(&schedule_log_lines),
    );

    assert_eq!(
        block,
        [
            capture_line,
            "\t- book flights",
            "\t- reserve hotel",
            "\t- clip child",
            "\t- 🗓️ **SCHEDULE LOG**",
        ]
        .join("\n")
    );
}

#[test]
fn recognizes_plugin_compatible_managed_log_markers() {
    let accepted = [
        ("\t- 🗓️ **SCHEDULE LOG**", ManagedTaskLogKind::Schedule),
        ("  * **SCHEDULE LOG**", ManagedTaskLogKind::Schedule),
        ("\t+ **SCHEDULE LOG:**", ManagedTaskLogKind::Schedule),
        ("  1. **Schedule log:**", ManagedTaskLogKind::Schedule),
        ("2) 🗓️ **Schedule log**", ManagedTaskLogKind::Schedule),
        ("\t- 🛠️ **WORK LOG**", ManagedTaskLogKind::Work),
        ("  * **WORK LOG**", ManagedTaskLogKind::Work),
        ("\t+ **Work log:**", ManagedTaskLogKind::Work),
        ("  10. **Work log**", ManagedTaskLogKind::Work),
        ("3) 🛠️ **WORK LOG:**", ManagedTaskLogKind::Work),
    ];
    for (line, kind) in accepted {
        assert_eq!(parse_managed_task_log_marker(line), Some(kind), "{line}");
    }

    for line in [
        "\t- **SCHEDULE LOG** trailing",
        "\t- **schedule log**",
        "\t- **Schedule Log**",
        "\t- **WORK LOG** extra",
        "\t- **work log**",
        "\t- **Work Log:**",
        "\t- SCHEDULE LOG",
        "\t- [ ] 🗓️ **SCHEDULE LOG**",
        "\t- 🗓️ **WORK LOG**",
        "\t- 🛠️ **SCHEDULE LOG**",
        "\t- 🗓️**SCHEDULE LOG**",
        "- [ ] #task 🗓️ **SCHEDULE LOG**",
        // Depends-On lines are never managed logs
        // (`docs/task-dependencies.md` §2.4).
        "\t- ⛓️ **DEPENDS ON:** [[#^a]]",
        "\t- **DEPENDS ON:** [[#^a]]",
        "\t- 🔗 **DEPENDENCIES:** [[#^a]]",
    ] {
        assert_eq!(parse_managed_task_log_marker(line), None, "{line}");
    }
}

#[test]
fn finds_the_earliest_direct_child_managed_log() {
    let both_logs = concat!(
        "- [ ] #task Parent\n",
        "\t- keep me\n",
        "\t- 🛠️ **WORK LOG**\n",
        "\t\t- *2026-08-15* — work\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — scheduled\n",
    );
    let lines = line_spans(both_logs);
    assert_eq!(
        first_direct_managed_log_start(&lines, 0, both_logs.len()),
        Some(both_logs.find("\t- 🛠️ **WORK LOG**").expect("work log")),
    );

    let schedule_first = concat!(
        "- [ ] #task Parent\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — scheduled\n",
        "\t- 🛠️ **WORK LOG**\n",
    );
    let lines = line_spans(schedule_first);
    assert_eq!(
        first_direct_managed_log_start(&lines, 0, schedule_first.len()),
        Some(lines[0].end),
    );

    let nested_only = concat!(
        "- [ ] #task Parent\n",
        "\t- child\n",
        "\t\t- 🗓️ **SCHEDULE LOG**\n",
        "\t- other\n",
    );
    let lines = line_spans(nested_only);
    assert_eq!(
        first_direct_managed_log_start(&lines, 0, nested_only.len()),
        None
    );

    let sibling_log = concat!(
        "- [ ] #task Parent\n",
        "\t- child\n",
        "- [ ] #task Other\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
    );
    let lines = line_spans(sibling_log);
    assert_eq!(
        first_direct_managed_log_start(&lines, 0, lines[1].end),
        None
    );

    let lookalike = concat!(
        "- [ ] #task Parent\n",
        "\t- **SCHEDULE LOG** trailing\n",
        "\t- **schedule log**\n",
    );
    let lines = line_spans(lookalike);
    assert_eq!(
        first_direct_managed_log_start(&lines, 0, lookalike.len()),
        None
    );
}

#[test]
fn sub_bullet_insertion_keeps_dependency_lines_first() {
    // A Depends-On line is neither a section title nor a managed log, so
    // capture inserts new sub-bullets after it: the managed-log anchor
    // skips the line, and without logs the block end is past it
    // (`docs/task-dependencies.md` §§2.1, 2.4).
    let with_log = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- ⛓️ **DEPENDS ON:** [[#^dep]]\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — scheduled\n",
    );
    let lines = line_spans(with_log);
    assert_eq!(
        first_direct_managed_log_start(&lines, 0, with_log.len()),
        Some(with_log.find("\t- 🗓️ **SCHEDULE LOG**").expect("log line")),
    );

    let line_only = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- ⛓️ **DEPENDS ON:** [[#^dep]]\n",
    );
    let lines = line_spans(line_only);
    assert_eq!(
        first_direct_managed_log_start(&lines, 0, line_only.len()),
        None,
    );
}

#[test]
fn formats_task_line() {
    assert_eq!(
        format_task_line("buy milk", "2026-06-15", None, None),
        "- [ ] #task buy milk [created::2026-06-15]"
    );
    assert_eq!(
        format_task_line("buy milk", "2026-06-15", None, Some("2026-06-16")),
        "- [?] #task buy milk [created::2026-06-15] [scheduled::2026-06-16]"
    );
    assert_eq!(
        format_task_line(
            "buy milk",
            "2026-06-15",
            Some(("priority", "high")),
            None,
        ),
        "- [ ] #task buy milk [created::2026-06-15] [priority::high]"
    );
    assert_eq!(
        format_task_line(
            "buy milk",
            "2026-06-15",
            Some(("priority", "high")),
            Some("2026-06-16"),
        ),
        "- [?] #task buy milk [created::2026-06-15] [priority::high] [scheduled::2026-06-16]"
    );
}

#[test]
fn formats_task_with_block_id_as_ordinary_task_with_final_block_id() {
    assert_eq!(
        format_task_with_block_id_line(
            "Some foobar task.",
            "2026-07-10",
            None,
            None,
            "foobar",
        ),
        "- [ ] #task Some foobar task. [created::2026-07-10] ^foobar"
    );
    assert_eq!(
        format_task_with_block_id_line(
            "Some foobar task.",
            "2026-07-10",
            Some(("priority", "lowest")),
            Some("2026-07-12"),
            "foobar",
        ),
        "- [?] #task Some foobar task. [created::2026-07-10] [priority::lowest] [scheduled::2026-07-12] ^foobar"
    );
}

#[test]
fn formats_pomodoro_task_with_block_id_as_final_token() {
    assert_eq!(
        format_pomodoro_task_line(
            "Some foobar task.",
            "2026-07-10",
            None,
            None,
            "foobar",
        ),
        "- [*] #task Some foobar task. [created::2026-07-10] ^foobar"
    );
    assert_eq!(
        format_pomodoro_task_line(
            "Some foobar task.",
            "2026-07-10",
            None,
            Some("2026-07-12"),
            "foobar",
        ),
        "- [?] #task Some foobar task. [created::2026-07-10] [scheduled::2026-07-12] ^foobar"
    );
    assert_eq!(
        format_pomodoro_task_line(
            "Some foobar task.",
            "2026-07-10",
            Some(("priority", "lowest")),
            Some("2026-07-12"),
            "foobar",
        ),
        "- [?] #task Some foobar task. [created::2026-07-10] [priority::lowest] [scheduled::2026-07-12] ^foobar"
    );
}

#[test]
fn pomodoro_link_prefers_the_single_timed_open_entry() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] Untimed first\n",
        "- [x] Completed (0800-0830)\n",
        "- [ ] (**0900-0930** [t:: 30m]) Timed\n",
        "  - existing child\n",
        "## Later\n",
        "- [ ] Outside (1000-1030)\n",
    );
    let insertion =
        insert_pomodoro_block_link(contents, "[[dev#^foobar]]", None)
            .expect("select timed Pomodoro");
    let updated = insertion.updated;
    let placement = insertion.placement;
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] Untimed first\n",
            "- [x] Completed (0800-0830)\n",
            "- [ ] (**0900-0930** [t:: 30m]) Timed\n",
            "  - existing child\n",
            "  - [[dev#^foobar]]\n",
            "## Later\n",
            "- [ ] Outside (1000-1030)\n",
        )
    );
}

#[test]
fn pomodoro_link_falls_back_to_first_open_and_ignores_nested_tasks() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Completed\n",
        "  - [ ] Nested (0800-0830)\n",
        "- [ ] First open\n",
        "- [ ] Second open\n",
    );
    let insertion =
        insert_pomodoro_block_link(contents, "[[dev#^fallback]]", None)
            .expect("select first open Pomodoro");
    let updated = insertion.updated;
    let placement = insertion.placement;
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] Completed\n",
            "  - [ ] Nested (0800-0830)\n",
            "- [ ] First open\n",
            "  - [[dev#^fallback]]\n",
            "- [ ] Second open\n",
        )
    );
}

#[test]
fn named_pomodoro_link_selects_placeholder_and_timed_entries() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
        "  - current child\n",
        "- [ ] () — BUGS\n",
    );
    let (updated, placement, selected, text) = insert_pomodoro_child_block(
        contents,
        "- [[dev#^fix]]",
        PomodoroSelection::NamedOrCreate("bugs"),
    )
    .expect("select named placeholder");
    assert_eq!(placement, Placement::Appended);
    assert_eq!(selected, 3);
    assert_eq!(text, "() — BUGS");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
            "  - current child\n",
            "- [ ] () — BUGS\n",
            "  - [[dev#^fix]]\n",
        )
    );

    let (updated, _, selected, text) = insert_pomodoro_child_block(
        contents,
        "- [[dev#^now]]",
        PomodoroSelection::NamedOrCreate("current"),
    )
    .expect("select named timed entry");
    assert_eq!(selected, 1);
    assert_eq!(text, "(**0900-0930** [t:: 30m]) — CURRENT");
    assert!(updated.contains("  - current child\n  - [[dev#^now]]\n"));
    assert!(!updated.contains("BUGS\n  - [[dev#^now]]"));
}

#[test]
fn named_pomodoro_link_first_duplicate_wins() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — MEMORY\n",
        "- [ ] () — MEMORY\n",
    );
    let (updated, _, selected, _) = insert_pomodoro_child_block(
        contents,
        "- [[dev#^dup]]",
        PomodoroSelection::NamedOrCreate("memory"),
    )
    .expect("first duplicate wins");
    assert_eq!(selected, 1);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — MEMORY\n",
            "  - [[dev#^dup]]\n",
            "- [ ] () — MEMORY\n",
        )
    );
}

#[test]
fn named_pomodoro_link_creates_placeholder_on_no_open_match() {
    let completed = concat!(
        "## Pomodoros\n",
        "- [x] () — BUGS\n",
        "  - old bug\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - current child\n",
        "- [ ] () — MEMORY\n",
    );
    let (updated, placement, selected, text) = insert_pomodoro_child_block(
        completed,
        "- [[dev#^id]]",
        PomodoroSelection::NamedOrCreate("bugs"),
    )
    .expect("completed-only match seeds a future Pomodoro");
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(selected, 5);
    assert_eq!(text, "() — BUGS");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] () — BUGS\n",
            "  - old bug\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - current child\n",
            "- [ ] () — BUGS\n",
            "  - [[dev#^id]]\n",
            "- [ ] () — MEMORY\n",
        )
    );

    let (updated, _, selected, text) = insert_pomodoro_child_block(
        completed,
        "- [[dev#^id]]",
        PomodoroSelection::NamedOrCreate("memry"),
    )
    .expect("nearby name creates rather than suggesting");
    assert_eq!(selected, 5);
    assert_eq!(text, "() — MEMRY");
    assert!(updated.contains("- [ ] () — MEMRY\n  - [[dev#^id]]\n"));

    let unnamed = "## Pomodoros\n- [ ] ()\n- [ ] (**0900-0930**)\n";
    let (updated, _, selected, text) = insert_pomodoro_child_block(
        unnamed,
        "- [[dev#^id]]",
        PomodoroSelection::NamedOrCreate("bugs"),
    )
    .expect("unnamed open Pomodoros still allow creation");
    assert_eq!(selected, 3);
    assert_eq!(text, "() — BUGS");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] ()\n",
            "- [ ] (**0900-0930**)\n",
            "- [ ] () — BUGS\n",
            "  - [[dev#^id]]\n",
        )
    );

    let closed = "## Pomodoros\n- [x] () — DONE\n";
    let (updated, _, selected, text) = insert_pomodoro_child_block(
        closed,
        "- [[dev#^id]]",
        PomodoroSelection::NamedOrCreate("bugs"),
    )
    .expect("last completed anchors a new Pomodoro");
    assert_eq!(selected, 2);
    assert_eq!(text, "() — BUGS");
    assert_eq!(
        updated,
        "## Pomodoros\n- [x] () — DONE\n- [ ] () — BUGS\n  - [[dev#^id]]\n"
    );

    let listed = concat!(
        "## Pomodoros\n",
        "- [ ] () — ALPHA\n",
        "- [ ] () — BRAVO\n",
        "- [ ] () — CHARLIE\n",
    );
    let (updated, _, selected, text) = insert_pomodoro_child_block(
        listed,
        "- [[dev#^id]]",
        PomodoroSelection::NamedOrCreate("zzzz"),
    )
    .expect("novel name inserts before first future entry");
    assert_eq!(selected, 1);
    assert_eq!(text, "() — ZZZZ");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — ZZZZ\n",
            "  - [[dev#^id]]\n",
            "- [ ] () — ALPHA\n",
            "- [ ] () — BRAVO\n",
            "- [ ] () — CHARLIE\n",
        )
    );
}

#[test]
fn named_pomodoro_link_creates_in_empty_and_crlf_sections() {
    let empty = concat!("## Pomodoros\n", "## Later\n");
    let (updated, placement, selected, text) = insert_pomodoro_child_block(
        empty,
        "- [[dev#^deep]]",
        PomodoroSelection::NamedOrCreate("deep-work"),
    )
    .expect("create in an empty Pomodoros section");
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(selected, 1);
    assert_eq!(text, "() — DEEP-WORK");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — DEEP-WORK\n",
            "  - [[dev#^deep]]\n",
            "## Later\n",
        )
    );

    let crlf = concat!(
        "## Pomodoros\r\n",
        "- [x] Done\r\n",
        "\t- old child\r\n",
        "## Later\r\n",
    );
    let (updated, placement, selected, text) = insert_pomodoro_child_block(
        crlf,
        "- [[dev#^crlf]]",
        PomodoroSelection::NamedOrCreate("crlf-name"),
    )
    .expect("create after completed CRLF block");
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(selected, 3);
    assert_eq!(text, "() — CRLF-NAME");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\r\n",
            "- [x] Done\r\n",
            "\t- old child\r\n",
            "- [ ] () — CRLF-NAME\r\n",
            "\t- [[dev#^crlf]]\r\n",
            "## Later\r\n",
        )
    );
}

#[test]
fn named_pomodoro_creation_ignores_cancelled_nested_and_fenced_entries() {
    let contents = concat!(
        "```md\n",
        "## Pomodoros\n",
        "- [x] Fenced done\n",
        "```\n",
        "## Pomodoros\n",
        "- [-] Cancelled ()\n",
        "  - [x] Nested done (0800-0830)\n",
        "- [ ] Future ()\n",
    );
    let (updated, _, selected, text) = insert_pomodoro_child_block(
        contents,
        "- [[dev#^real]]",
        PomodoroSelection::NamedOrCreate("real"),
    )
    .expect("ignored lookalikes do not anchor creation");
    assert_eq!(selected, 7);
    assert_eq!(text, "() — REAL");
    assert!(updated.ends_with(concat!(
        "- [-] Cancelled ()\n",
        "  - [x] Nested done (0800-0830)\n",
        "- [ ] () — REAL\n",
        "  - [[dev#^real]]\n",
        "- [ ] Future ()\n",
    )));
}

#[test]
fn named_pomodoro_link_bypasses_multiple_open_timed_guard() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (0800-0830) — ONE\n",
        "- [ ] (**0900-0930**) — TWO\n",
    );
    let error = insert_pomodoro_child_block(
        contents,
        "- [[dev#^id]]",
        PomodoroSelection::CurrentOrFuture,
    )
    .expect_err("implicit stays ambiguous");
    assert!(
        error.message.contains("multiple open timed Pomodoros"),
        "{error:?}"
    );

    let (updated, _, selected, text) = insert_pomodoro_child_block(
        contents,
        "- [[dev#^id]]",
        PomodoroSelection::NamedOrCreate("two"),
    )
    .expect("named selector is explicit");
    assert_eq!(selected, 2);
    assert_eq!(text, "(**0900-0930**) — TWO");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0800-0830) — ONE\n",
            "- [ ] (**0900-0930**) — TWO\n",
            "  - [[dev#^id]]\n",
        )
    );

    let error = insert_pomodoro_child_block(
        contents,
        "- [[dev#^new]]",
        PomodoroSelection::NamedOrCreate("three"),
    )
    .expect_err("named creation needs an unambiguous current anchor");
    assert!(
        error.message.contains("multiple open timed Pomodoros"),
        "{error:?}"
    );
}

#[test]
fn pomodoro_link_rejects_missing_section_target_and_timed_ambiguity() {
    for (contents, expected) in [
        ("## Notes\n- [ ] (0800-0830) Outside\n", "no Pomodoros"),
        ("## Pomodoros\n- [x] Complete\n", "no eligible"),
        (
            "## Pomodoros\n- [ ] (0800-0830) One\n- [ ] (**0900-0930**) Two\n",
            "multiple open timed",
        ),
    ] {
        let error = insert_pomodoro_block_link(contents, "[[dev#^id]]", None)
            .expect_err("invalid ledger should fail");
        assert!(error.message.contains(expected), "{error:?}");
    }
}

#[test]
fn pomodoro_link_preserves_crlf_and_reuses_nearby_child_indentation() {
    let contents = concat!(
        "## Pomodoros\r\n",
        "- [x] Old\r\n",
        "\t- old child\r\n",
        "- [ ] Next\r\n",
    );
    let insertion = insert_pomodoro_block_link(contents, "[[dev#^id]]", None)
        .expect("insert CRLF link");
    let updated = insertion.updated;
    let placement = insertion.placement;
    assert_eq!(placement, Placement::Appended);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\r\n",
            "- [x] Old\r\n",
            "\t- old child\r\n",
            "- [ ] Next\r\n",
            "\t- [[dev#^id]]\r\n",
        )
    );
}

#[test]
fn pomodoro_section_scan_ignores_fenced_lookalikes() {
    let contents = concat!(
        "```md\n",
        "## Pomodoros\n",
        "- [ ] (0800-0830) Example\n",
        "```\n",
        "## Pomodoros\n",
        "- [ ] Real\n",
    );
    let updated = insert_pomodoro_block_link(contents, "[[dev#^real]]", None)
        .expect("find real section")
        .updated;
    assert!(updated.ends_with("- [ ] Real\n  - [[dev#^real]]\n"));
    assert!(!updated.contains("Example\n  - [[dev#^real]]"));
}

#[test]
fn pomodoro_note_current_wins_over_a_completed_entry() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Done (0900-0930)\n",
        "- [ ] Focus (1000-1030)\n",
    );
    for selection in [
        PomodoroSelection::CurrentOrLastCompleted,
        PomodoroSelection::CurrentOrFuture,
    ] {
        let (updated, placement, selected, text) =
            insert_pomodoro_child_block(contents, "- note this", selection)
                .expect("select current Pomodoro");
        assert_eq!(placement, Placement::Appended);
        assert_eq!(selected, 2);
        assert_eq!(text, "Focus (1000-1030)");
        assert_eq!(
            updated,
            concat!(
                "## Pomodoros\n",
                "- [x] Done (0900-0930)\n",
                "- [ ] Focus (1000-1030)\n",
                "  - note this\n",
            )
        );
    }
}

#[test]
fn pomodoro_note_last_completed_wins_over_a_future_entry() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] First (0900-0930)\n",
        "- [x] Second (1000-1030)\n",
        "- [ ] Next ()\n",
    );
    let (updated, placement, selected, text) = insert_pomodoro_child_block(
        contents,
        "- note this",
        PomodoroSelection::CurrentOrLastCompleted,
    )
    .expect("select last completed Pomodoro");
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(selected, 2);
    assert_eq!(text, "Second (1000-1030)");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] First (0900-0930)\n",
            "- [x] Second (1000-1030)\n",
            "  - note this\n",
            "- [ ] Next ()\n",
        )
    );
}

#[test]
fn pomodoro_note_appends_after_completed_entry_children() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Done (0900-0930)\n",
        "    - existing child\n",
        "- [ ] Next ()\n",
    );
    let (updated, placement, selected, text) = insert_pomodoro_child_block(
        contents,
        "- note this",
        PomodoroSelection::CurrentOrLastCompleted,
    )
    .expect("append after completed children");
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(selected, 1);
    assert_eq!(text, "Done (0900-0930)");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] Done (0900-0930)\n",
            "    - existing child\n",
            "    - note this\n",
            "- [ ] Next ()\n",
        )
    );
}

#[test]
fn pomodoro_selection_policies_diverge_on_completed_plus_future() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Done (0900-0930)\n",
        "- [ ] Next ()\n",
    );
    let (_, _, note_selected, note_text) = insert_pomodoro_child_block(
        contents,
        "- note this",
        PomodoroSelection::CurrentOrLastCompleted,
    )
    .expect("note policy selects completed");
    let (_, _, link_selected, link_text) = insert_pomodoro_child_block(
        contents,
        "- [[dev#^id]]",
        PomodoroSelection::CurrentOrFuture,
    )
    .expect("link policy selects future");
    assert_eq!(note_selected, 1);
    assert_eq!(note_text, "Done (0900-0930)");
    assert_eq!(link_selected, 2);
    assert_eq!(link_text, "Next ()");
}

#[test]
fn pomodoro_note_first_future_when_nothing_is_completed() {
    let contents =
        concat!("## Pomodoros\n", "- [ ] Next ()\n", "- [ ] Later ()\n",);
    for selection in [
        PomodoroSelection::CurrentOrLastCompleted,
        PomodoroSelection::CurrentOrFuture,
    ] {
        let (updated, _, selected, text) =
            insert_pomodoro_child_block(contents, "- note this", selection)
                .expect("select first future Pomodoro");
        assert_eq!(selected, 1);
        assert_eq!(text, "Next ()");
        assert_eq!(
            updated,
            concat!(
                "## Pomodoros\n",
                "- [ ] Next ()\n",
                "  - note this\n",
                "- [ ] Later ()\n",
            )
        );
    }
}

#[test]
fn pomodoro_note_returned_text_comes_from_the_completed_parser() {
    let contents =
        concat!("## Heading\n", "## Pomodoros\n", "- [x] Done (0900-0930)\n",);
    let (_, _, selected, text) = insert_pomodoro_child_block(
        contents,
        "- note this",
        PomodoroSelection::CurrentOrLastCompleted,
    )
    .expect("select completed entry");
    assert_eq!(selected, 2);
    assert_eq!(text, "Done (0900-0930)");
    assert_eq!(
        pomodoro::completed_ledger_task("- [x] Done (0900-0930)"),
        Some(text.as_str())
    );
    assert!(pomodoro::open_ledger_task("- [x] Done (0900-0930)").is_none());
}

#[test]
fn pomodoro_note_ignores_cancelled_and_nested_completed_entries() {
    for contents in [
        concat!(
            "## Pomodoros\n",
            "- [-] Cancelled ()\n",
            "  - [x] Nested (0900-0930)\n",
        ),
        "## Pomodoros\n",
    ] {
        let error = insert_pomodoro_child_block(
            contents,
            "- note this",
            PomodoroSelection::CurrentOrLastCompleted,
        )
        .expect_err("ineligible ledger should fail");
        assert!(error.message.contains("no eligible Pomodoro"), "{error:?}");
        assert!(
            !error.message.contains("no eligible open Pomodoro"),
            "{error:?}"
        );
    }
}

#[test]
fn pomodoro_note_scan_ignores_fenced_completed_lookalikes() {
    let contents = concat!(
        "```md\n",
        "## Pomodoros\n",
        "- [x] Example (0800-0830)\n",
        "```\n",
        "## Pomodoros\n",
        "- [ ] Real ()\n",
    );
    let (updated, _, selected, text) = insert_pomodoro_child_block(
        contents,
        "- note this",
        PomodoroSelection::CurrentOrLastCompleted,
    )
    .expect("ignore fenced completed lookalike");
    assert_eq!(selected, 5);
    assert_eq!(text, "Real ()");
    assert!(updated.ends_with("- [ ] Real ()\n  - note this\n"));
    assert!(!updated.contains("Example (0800-0830)\n  - note this"));
}

#[test]
fn pomodoro_note_timed_ambiguity_wins_over_completed_fallback() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Done (0800-0830)\n",
        "- [ ] One (0900-0930)\n",
        "- [ ] Two (1000-1030)\n",
    );
    let error = insert_pomodoro_child_block(
        contents,
        "- note this",
        PomodoroSelection::CurrentOrLastCompleted,
    )
    .expect_err("two open timed entries remain an error");
    assert!(
        error.message.contains("multiple open timed Pomodoros"),
        "{error:?}"
    );
}

#[test]
fn pomodoro_note_preserves_crlf_under_a_completed_entry() {
    let contents = concat!(
        "## Pomodoros\r\n",
        "- [x] Done (0900-0930)\r\n",
        "- [ ] Next ()\r\n",
    );
    let (updated, placement, selected, text) = insert_pomodoro_child_block(
        contents,
        "- note this",
        PomodoroSelection::CurrentOrLastCompleted,
    )
    .expect("insert CRLF note under completed");
    assert_eq!(placement, Placement::Inserted);
    assert_eq!(selected, 1);
    assert_eq!(text, "Done (0900-0930)");
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\r\n",
            "- [x] Done (0900-0930)\r\n",
            "  - note this\r\n",
            "- [ ] Next ()\r\n",
        )
    );
}

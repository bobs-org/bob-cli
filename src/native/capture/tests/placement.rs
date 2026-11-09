//! Task/bullet placement and JSON contract tests.
use super::*;

#[test]
fn formats_scheduled_date_from_offset() {
    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("valid date");
    assert_eq!(
        scheduled_date_string(today, 0).expect("same day"),
        "2026-06-15"
    );
    assert_eq!(
        scheduled_date_string(today, 1).expect("tomorrow"),
        "2026-06-16"
    );

    let error = scheduled_date_string(today, 9_999_999_999)
        .expect_err("calendar overflow must fail");
    assert_eq!(error.kind, CaptureErrorKind::Usage);
}

#[test]
fn appends_to_empty_and_no_task_files() {
    assert_eq!(
        insert_task_line("", TASK),
        (format!("{TASK}\n"), Placement::Appended)
    );
    assert_eq!(
        insert_task_line("# Header", TASK),
        (format!("# Header\n{TASK}\n"), Placement::Appended)
    );
    assert_eq!(
        insert_task_line("# Header\n", TASK),
        (format!("# Header\n{TASK}\n"), Placement::Appended)
    );
}

#[test]
fn inserts_after_single_top_level_task() {
    let contents = "- [ ] #task old\nPlain paragraph\n";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!("- [ ] #task old\n{TASK}\nPlain paragraph\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn skips_indented_and_blank_then_indented_continuation_lines() {
    let contents = "- [ ] #task old\n  child\n\n\tdeep\n\nNext\n";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!("- [ ] #task old\n  child\n\n\tdeep\n{TASK}\n\nNext\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn inserts_after_last_of_many_task_blocks() {
    let contents = "- [ ] #task first\n- [x] #task second\n  note\nTail\n";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!(
                "- [ ] #task first\n- [x] #task second\n  note\n{TASK}\nTail\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn adds_leading_newline_when_inserting_after_non_newline_eof() {
    let contents = "- [*] #task old";
    assert_eq!(
        insert_task_line(contents, TASK),
        (format!("- [*] #task old\n{TASK}\n"), Placement::Inserted,)
    );
}

#[test]
fn inserts_multiline_capture_as_one_task_block() {
    let block = format!("{TASK}\n  - **CLIP:** hello");
    let contents = "- [ ] #task old\n  - old child\nTail\n";
    assert_eq!(
        insert_task_line(contents, &block),
        (
            format!("- [ ] #task old\n  - old child\n{block}\nTail\n"),
            Placement::Inserted,
        )
    );

    let crlf = "- [ ] #task old\r\nTail\r\n";
    assert_eq!(
        insert_task_line(crlf, &block).0,
        format!(
            "- [ ] #task old\r\n{}\r\nTail\r\n",
            block.replace('\n', "\r\n")
        )
    );
}

#[test]
fn inserts_after_final_continuation_running_to_eof() {
    let contents = "- [/] #task old\n  note";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!("- [/] #task old\n  note\n{TASK}\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn ignores_indented_task_lines_as_insertion_anchors() {
    let contents = "  - [ ] #task nested";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!("  - [ ] #task nested\n{TASK}\n"),
            Placement::Appended,
        )
    );
}

#[test]
fn tasks_section_wins_over_root_task_when_empty() {
    let contents = "# Project\n- [ ] #task root\n## Tasks\nNotes\n";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!("# Project\n- [ ] #task root\n## Tasks\n\n{TASK}\nNotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn tasks_section_inserts_after_last_task_block_in_section() {
    let contents = concat!(
        "# Project\n",
        "- [ ] #task root\n",
        "## Tasks\n",
        "Intro\n",
        "- [ ] #task old\n",
        "  detail\n",
        "\n",
        "\tmore\n",
        "After\n",
    );
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!(
                "{}{TASK}\nAfter\n",
                concat!(
                    "# Project\n",
                    "- [ ] #task root\n",
                    "## Tasks\n",
                    "Intro\n",
                    "- [ ] #task old\n",
                    "  detail\n",
                    "\n",
                    "\tmore\n",
                )
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn later_task_outside_tasks_section_does_not_win() {
    let contents =
        "## Tasks\n- [ ] #task in section\n## Other\n- [ ] #task outside\n";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!(
                "## Tasks\n- [ ] #task in section\n{TASK}\n## Other\n- [ ] #task outside\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn ignores_tasks_headings_in_frontmatter_and_fenced_code() {
    let contents = concat!(
        "---\n",
        "# Tasks\n",
        "---\n",
        "```md\n",
        "## Tasks\n",
        "```\n",
        "- [ ] #task old\n",
        "Tail\n",
    );
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!(
                "---\n\
                 # Tasks\n\
                 ---\n\
                 ```md\n\
                 ## Tasks\n\
                 ```\n\
                 - [ ] #task old\n\
                 {TASK}\n\
                 Tail\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn nested_heading_stops_empty_tasks_section_insertion() {
    let contents = "## Tasks\n### Later\n- [ ] #task later\n";
    assert_eq!(
        insert_task_line(contents, TASK),
        (
            format!("## Tasks\n\n{TASK}\n### Later\n- [ ] #task later\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn tasks_section_inserts_below_generated_status_badges() {
    const ROW: &str = "[`⚪ 0 open`](#Tasks) · [`🔵 1 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 0 done/canceled`](#Tasks#Done%20&%20Canceled)\n";
    const GROUP: &str = concat!(
        "\n",
        "### Next & In Progress\n",
        "\n",
        "- [*] #task active\n",
    );

    let marker_free = format!("## Tasks\n{ROW}{GROUP}");
    assert_eq!(
        insert_task_line(&marker_free, TASK),
        (
            format!("## Tasks\n{ROW}\n{TASK}\n{GROUP}"),
            Placement::Inserted,
        )
    );

    let legacy =
        format!("## Tasks\n<!-- bob:task-status-badges:v1 -->\n{ROW}{GROUP}");
    assert_eq!(
        insert_task_line(&legacy, TASK),
        (
            format!("## Tasks\n<!-- bob:task-status-badges:v1 -->\n{ROW}\n{TASK}\n{GROUP}"),
            Placement::Inserted,
        )
    );

    let legacy_prose =
        "## Tasks\n<!-- bob:task-status-badges:v1 -->\nProject context.\n";
    assert_eq!(
        insert_task_line(legacy_prose, TASK),
        (
            format!("## Tasks\n<!-- bob:task-status-badges:v1 -->\n\n{TASK}\nProject context.\n"),
            Placement::Inserted,
        )
    );

    let unlinked_row = "[`⚪ 0 open`](#Tasks) · `🔵 0 next/wip` · `🔴 0 blocked` · `🟢 1 done/canceled`\n";
    let unlinked_group = "\n### Done & Canceled\n\n- [x] #task done\n";
    let unlinked = format!("## Tasks\n{unlinked_row}{unlinked_group}");
    assert_eq!(
        insert_task_line(&unlinked, TASK),
        (
            format!("## Tasks\n{unlinked_row}\n{TASK}\n{unlinked_group}"),
            Placement::Inserted,
        )
    );
}

#[test]
fn tasks_heading_at_eof_inserts_after_blank_line() {
    assert_eq!(
        insert_task_line("## Tasks", TASK),
        (format!("## Tasks\n\n{TASK}\n"), Placement::Inserted,)
    );
    assert_eq!(
        insert_task_line("## Tasks ##\n", TASK),
        (format!("## Tasks ##\n\n{TASK}\n"), Placement::Inserted,)
    );
}

#[test]
fn json_success_shape_is_stable() {
    let result = CaptureResult::from_items(
        vec![CaptureItemResult {
            note_pull: None,
            record_note: None,
            task_theme: None,
            type_relocation: None,
            ok: true,
            dry_run: false,
            routed: true,
            route: Some("groceries".to_string()),
            route_label: "groceries.md".to_string(),
            relative_target: "groceries.md".to_string(),
            target: "/tmp/bob/groceries.md".to_string(),
            text: "buy milk".to_string(),
            task_line: "- [ ] #task buy milk [created::2026-06-15]".to_string(),
            kind: "task",
            fanout: None,
            created: "2026-06-15".to_string(),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Inserted,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            drawer: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            r#ref: None,
            gmail: Vec::new(),
            toggle_task_description: None,
            link_guard: None,
            dropped_dependencies: Vec::new(),
            blocks_line_updates: Vec::new(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            relation_merge_output: Default::default(),
            transfer_move_output: Default::default(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            list: None,
            relation_label_hides: Vec::new(),
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
        }],
        None,
        None,
        Vec::new(),
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        PullInJson::empty(),
        Vec::new(),
        InboxRouteReport::empty(),
        crate::native::capture::move_choice::MoveChoiceReport::empty(),
        Vec::new(),
    );

    let legacy_json = serde_json::to_string(&result)
        .expect("serialize legacy capture result");
    assert_eq!(success_json(&result), legacy_json);
    let value: serde_json::Value =
        serde_json::from_str(&success_json(&result)).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["dry_run"], false);
    assert_eq!(value["routed"], true);
    assert_eq!(value["route"], "groceries");
    assert_eq!(value["route_label"], "groceries.md");
    assert_eq!(value["relative_target"], "groceries.md");
    assert_eq!(value["target"], "/tmp/bob/groceries.md");
    assert_eq!(value["inbox_route"]["supported"], true);
    assert_eq!(value["inbox_route"]["candidate_count"], 0);
    assert_eq!(value["text"], "buy milk");
    assert_eq!(
        value["task_line"],
        "- [ ] #task buy milk [created::2026-06-15]"
    );
    assert_eq!(value["kind"], "task");
    assert_eq!(value["created"], "2026-06-15");
    assert!(value["scheduled"].is_null(), "{value}");
    assert_eq!(value["placement"], "inserted");
    assert!(value.get("clip").is_none(), "{value}");
    assert!(value.get("sub_bullets").is_none(), "{value}");
    assert!(value.get("captures").is_none(), "{value}");
    assert!(value.get("global_destination").is_none(), "{value}");
    assert!(value.get("section_blocks").is_none(), "{value}");
    assert_eq!(value["open_notes"], serde_json::json!([]));
    assert_eq!(value["pull_in"]["supported"], true);
    assert_eq!(value["pull_in"]["offer_count"], 0);
    assert!(value["pull_in"]["choice"].is_null(), "{value}");
    assert_eq!(value["pull_in"]["picked_count"], 0);
    assert!(value["pull_in"].get("groups").is_none(), "{value}");
    for special_field in [
        "priority",
        "priority_label",
        "schedule_log",
        "block_id",
        "day_file",
        "block_link",
        "pomodoro_link_placement",
        "parent_line",
        "parent_text",
        "parent_status_symbol",
        "parent_status_name",
        "toggle_direction",
        "previous_task_line",
        "status_symbol",
        "status_name",
        "previous_status_symbol",
        "previous_status_name",
        "pomodoro_name",
        "creates_pomodoro",
        "pomodoro_already_linked",
        "removed_pomodoro_links",
        "removed_scheduled",
        "pomodoro_selector_unused",
        "toggle_behavior",
        "status_changed",
        "pomodoro_link_action",
        "pomodoro_link_source",
        "pomodoro_link_destination",
        "project_note",
        "pomodoro_start",
        "plan_budget",
        "pomodoro_blocks",
        "task_blocks",
        "link_guard",
    ] {
        assert!(value.get(special_field).is_none(), "{value}");
    }
}

#[test]
fn parses_suffixed_route_token_as_bullet() {
    let cases = [
        (
            "Some note @foo#bar",
            "Some note",
            "foo",
            Some("bar"),
            "trailing route with section prefix",
        ),
        (
            "@foo#bar Some note",
            "Some note",
            "foo",
            Some("bar"),
            "leading route with section prefix",
        ),
        (
            "Some note @foo#",
            "Some note",
            "foo",
            None,
            "trailing bare bullet marker",
        ),
        (
            "@foo# Some note",
            "Some note",
            "foo",
            None,
            "leading bare bullet marker",
        ),
        (
            "Some note @Foo-Bar#R",
            "Some note",
            "foo-bar",
            Some("R"),
            "route lower-cases and prefix is preserved",
        ),
    ];

    for (raw, body, route, prefix, label) in cases {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{label}: {error:?}"));
        assert_eq!(parsed.body, body, "{label}");
        assert_eq!(parsed.route.as_deref(), Some(route), "{label}");
        assert_eq!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: prefix.map(str::to_string),
                exact: false,
                block_id: None,
            },
            "{label}"
        );
    }
}

#[test]
fn forced_section_forces_exact_bullet_with_forced_route() {
    let parsed = super::super::parse_capture_text(
        "Some note @other s:1",
        Some("Foo"),
        Some("Ideas"),
    )
    .expect("parse forced section");
    assert_eq!(parsed.body, "Some note @other");
    assert_eq!(parsed.route.as_deref(), Some("foo"));
    assert_eq!(parsed.scheduled_offset, Some(1));
    assert_eq!(
        parsed.kind,
        CaptureKind::Bullet {
            section_prefix: Some("Ideas".to_string()),
            exact: true,
            block_id: None,
        }
    );
}

#[test]
fn forced_section_requires_route_and_non_empty_title() {
    let error =
        super::super::parse_capture_text("Some note", None, Some("Ideas"))
            .expect_err("section without route must fail");
    assert_eq!(error.kind, CaptureErrorKind::Usage);
    assert!(
        error.message.contains("requires --route"),
        "unexpected error: {error:?}"
    );

    let error =
        super::super::parse_capture_text("Some note", Some("foo"), Some(""))
            .expect_err("empty section must fail");
    assert_eq!(error.kind, CaptureErrorKind::Usage);
    assert!(
        error.message.contains("must not be empty"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn suffixed_route_token_without_body_is_usage_error() {
    for raw in ["@foo#bar", "@foo#"] {
        let error = parse_capture_text(raw, None)
            .expect_err(&format!("{raw} should require body"));
        assert_eq!(error.kind, CaptureErrorKind::Usage, "{raw}");
    }
}

#[test]
fn edge_hash_markers_are_task_types_not_legacy_bullet_selectors() {
    for (raw, route) in [
        ("Some note #bar @foo", Some("foo")),
        ("Some note @foo #bar", Some("foo")),
        ("Some note #bar", None),
    ] {
        let parsed = parse_capture_text(raw, None)
            .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
        assert_eq!(parsed.body, "Some note", "{raw}");
        assert_eq!(parsed.route.as_deref(), route, "{raw}");
        assert_eq!(
            parsed
                .type_marker
                .as_ref()
                .map(|marker| marker.name.as_str()),
            Some("bar"),
            "{raw}"
        );
    }
}

#[test]
fn bare_trailing_hash_resolves_pomodoro_note() {
    let parsed = parse_capture_text("Some note %n", None)
        .expect("%n is the Pomodoro-note marker");
    assert_eq!(parsed.body, "Some note");
    assert_eq!(parsed.route, None);
    assert_eq!(parsed.kind, CaptureKind::PomodoroNote);
}

#[test]
fn forced_route_accepts_terminal_type_marker_but_keeps_middle_hashtag() {
    let typed = parse_capture_text("Some note #bar", Some("Work"))
        .expect("forced route accepts a terminal type marker");
    assert_eq!(typed.body, "Some note");
    assert_eq!(typed.route.as_deref(), Some("work"));
    assert_eq!(
        typed
            .type_marker
            .as_ref()
            .map(|marker| marker.name.as_str()),
        Some("bar")
    );

    let parsed = parse_capture_text("Some #tag note", Some("Work"))
        .expect("middle hashtag stays literal");
    assert_eq!(parsed.body, "Some #tag note");
    assert_eq!(parsed.route.as_deref(), Some("work"));
    assert_eq!(parsed.kind, CaptureKind::Task);
}

#[test]
fn marker_only_bullet_input_is_usage_error() {
    let error = parse_capture_text("%n", None).expect_err("marker only");
    assert_eq!(error.kind, CaptureErrorKind::Usage);

    let error =
        parse_capture_text("%n", Some("Work")).expect_err("forced marker only");
    assert_eq!(error.kind, CaptureErrorKind::Usage);
}

#[test]
fn formats_bullet_line() {
    assert_eq!(
        format_bullet_line("some idea", "2026-06-15", None, None),
        "- some idea [created::2026-06-15]"
    );
    assert_eq!(
        format_bullet_line("some idea", "2026-06-15", None, Some("2026-06-16")),
        "- some idea [created::2026-06-15] [scheduled::2026-06-16]"
    );
    assert_eq!(
        format_bullet_line(
            "some idea",
            "2026-06-15",
            Some(("priority", "medium")),
            Some("2026-06-16"),
        ),
        "- some idea [created::2026-06-15] [priority::medium] [scheduled::2026-06-16]"
    );
}

#[test]
fn formats_sub_bullet_line() {
    assert_eq!(
        format_sub_bullet_line("some idea", None, None),
        "- some idea"
    );
    assert_eq!(
        format_sub_bullet_line("some idea", None, Some("2026-06-16")),
        "- some idea [scheduled::2026-06-16]"
    );
    assert_eq!(
        format_sub_bullet_line(
            "some idea",
            Some(("priority", "low")),
            Some("2026-06-16"),
        ),
        "- some idea [priority::low] [scheduled::2026-06-16]"
    );
}

#[test]
fn bullet_inserts_after_matched_section_header() {
    let contents = "# Notes\n## Ideas\nNotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Ideas")),
        (
            format!("# Notes\n## Ideas\n\n{BULLET}\nNotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bullet_inserts_after_last_ordinary_bullet_block() {
    let contents = "## Ideas\n- first\n  detail\n\n\tmore\nAfter\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Ideas")),
        (
            format!("## Ideas\n- first\n  detail\n\n\tmore\n{BULLET}\nAfter\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bullet_treats_checkbox_only_section_as_empty() {
    let contents = "## Ideas\n- [ ] #task t\n- [x] done\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Ideas")),
        (
            format!("## Ideas\n\n{BULLET}\n- [ ] #task t\n- [x] done\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bullet_skips_tasks_section_matching_prefix() {
    let contents = "## Tasks\n- [ ] #task t\n## Ta-da\nNotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Ta")),
        (
            format!("## Tasks\n- [ ] #task t\n## Ta-da\n\n{BULLET}\nNotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bare_bullet_marker_selects_first_non_tasks_section() {
    let contents = "## Tasks\n- [ ] #task t\n## Ideas\nNotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, None),
        (
            format!("## Tasks\n- [ ] #task t\n## Ideas\n\n{BULLET}\nNotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn unmatched_prefix_falls_back_to_zeroth_section() {
    let contents = "Intro line\n## Tasks\n- [ ] #task t\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Ideas")),
        (
            format!("{BULLET}\nIntro line\n## Tasks\n- [ ] #task t\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bare_bullet_marker_skips_generated_status_group() {
    let contents = concat!(
        "# Title\n",
        "## Tasks\n",
        "### Next & In Progress\n",
        "- [/] #task now\n",
        "## Requirements\n",
        "notes\n",
    );
    assert_eq!(
        insert_bullet_line(contents, BULLET, None),
        (
            format!(
                "# Title\n## Tasks\n### Next & In Progress\n- [/] #task now\n## Requirements\n\n{BULLET}\nnotes\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn prefix_skips_generated_blocked_group_for_authored_match() {
    let contents = concat!(
        "# Title\n",
        "## Tasks\n",
        "### Blocked\n",
        "- [?] #task wait\n",
        "## Blog Ideas\n",
        "notes\n",
    );
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Bl")),
        (
            format!(
                "# Title\n## Tasks\n### Blocked\n- [?] #task wait\n## Blog Ideas\n\n{BULLET}\nnotes\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn generated_group_prefix_without_authored_match_falls_back() {
    let contents = concat!(
        "# Title\n",
        "## Tasks\n",
        "### Done & Canceled\n",
        "- [x] #task old\n",
    );
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Done")),
        (format!("{BULLET}\n{contents}"), Placement::Inserted,)
    );
}

#[test]
fn forced_section_does_not_target_generated_blocked_group() {
    let contents = concat!(
        "# Title\n",
        "## Tasks\n",
        "### Blocked\n",
        "- [?] #task wait\n",
        "## Requirements\n",
        "notes\n",
    );
    assert_eq!(
        super::super::insert_bullet_line(
            contents,
            BULLET,
            Some("Blocked"),
            true
        ),
        (format!("{BULLET}\n{contents}"), Placement::Inserted,)
    );
}

#[test]
fn zeroth_section_insertion_after_frontmatter() {
    let contents = "---\ntype: area\n---\nIntro\n## Tasks\n- [ ] #task t\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Ideas")),
        (
            format!("---\ntype: area\n---\n{BULLET}\nIntro\n## Tasks\n- [ ] #task t\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bullet_prefers_non_h1_match_over_earlier_h1_match() {
    let contents = "# Roadmap\nintro\n\n## Research\nnotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("R")),
        (
            format!("# Roadmap\nintro\n\n## Research\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bullet_uses_h1_match_when_no_non_h1_match_exists() {
    let contents = "# Research\nnotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("R")),
        (
            format!("# Research\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bare_bullet_marker_prefers_non_h1_section() {
    let contents = "# Title\nintro\n\n## Notes\nbody\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, None),
        (
            format!("# Title\nintro\n\n## Notes\n\n{BULLET}\nbody\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bullet_section_prefix_matches_case_insensitively() {
    let contents = "## Research\nnotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("r")),
        (
            format!("## Research\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bullet_ignores_headings_in_frontmatter_and_fences() {
    let contents = concat!(
        "---\n",
        "## Ideas\n",
        "---\n",
        "```md\n",
        "## Ideas\n",
        "```\n",
        "## Ideas\n",
        "Notes\n",
    );
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("Ideas")),
        (
            format!("---\n## Ideas\n---\n```md\n## Ideas\n```\n## Ideas\n\n{BULLET}\nNotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn exact_bullet_section_wins_over_prefix_sibling() {
    let contents = "## Ideas\nnotes\n## Idea\nnotes\n";
    assert_eq!(
        super::super::insert_bullet_line(contents, BULLET, Some("Idea"), true),
        (
            format!("## Ideas\nnotes\n## Idea\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn exact_bullet_section_keeps_non_h1_preference() {
    let contents = "# Idea\nintro\n## Idea\nnotes\n";
    assert_eq!(
        super::super::insert_bullet_line(contents, BULLET, Some("Idea"), true),
        (
            format!("# Idea\nintro\n## Idea\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn exact_bullet_section_matches_case_insensitively() {
    let contents = "## Research\nnotes\n";
    assert_eq!(
        super::super::insert_bullet_line(
            contents,
            BULLET,
            Some("research"),
            true
        ),
        (
            format!("## Research\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn exact_bullet_section_no_match_falls_back_to_zeroth_section() {
    let contents = "Intro\n## Ideas\nnotes\n";
    assert_eq!(
        super::super::insert_bullet_line(contents, BULLET, Some("Idea"), true),
        (
            format!("{BULLET}\nIntro\n## Ideas\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn bare_bullet_marker_ignores_exact_flag() {
    let contents = "# Title\nintro\n\n## Notes\nbody\n";
    assert_eq!(
        super::super::insert_bullet_line(contents, BULLET, None, true),
        insert_bullet_line(contents, BULLET, None)
    );
}

#[test]
fn slug_selector_reaches_spaced_heading() {
    let contents = "Intro\n## Future Work\nnotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("future-work")),
        (
            format!("Intro\n## Future Work\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn raw_prefix_beats_an_earlier_slug_match() {
    let contents = "## Future Work\nfirst\n## future-work-notes\nsecond\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("future-work")),
        (
            format!(
                "## Future Work\nfirst\n## future-work-notes\n\n{BULLET}\nsecond\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn whole_slug_beats_an_earlier_slug_prefix() {
    let contents = "## Future Workshop\nfirst\n## Future Work\nsecond\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("future-work")),
        (
            format!(
                "## Future Workshop\nfirst\n## Future Work\n\n{BULLET}\nsecond\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn exact_selector_never_slug_matches() {
    let contents = "Intro\n## Future Work\nnotes\n";
    assert_eq!(
        super::super::insert_bullet_line(
            contents,
            BULLET,
            Some("future-work"),
            true,
        ),
        (
            format!("{BULLET}\nIntro\n## Future Work\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn slug_selector_falls_back_to_h1_when_no_non_h1_matches() {
    let contents = "# Future Work\nnotes\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("future-work")),
        (
            format!("# Future Work\n\n{BULLET}\nnotes\n"),
            Placement::Inserted,
        )
    );
}

#[test]
fn article_slug_matches_after_raw_and_identity() {
    let contents = concat!(
        "---\n",
        "type: \"[[article]]\"\n",
        "---\n",
        "## Future Work\n",
        "notes\n",
    );
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("future-work")),
        (
            format!(
                "---\ntype: \"[[article]]\"\n---\n## Future Work\n\n{BULLET}\nnotes\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn section_completion_prefers_raw_then_slug_then_quoted() {
    let contents = concat!(
        "## Ideas\n",
        "notes\n",
        "## Inbox Ideas\n",
        "inbox\n",
        "## Future Work\n",
        "future\n",
        "## future-work\n",
        "hyphen\n",
        "## A \"Q\"\n",
        "quoted\n",
        "## a-\"q\"\n",
        "hyphen-quote\n",
    );
    let rows = super::super::section_completion_rows(contents, "");
    let by_title: std::collections::HashMap<&str, &str> = rows
        .iter()
        .map(|row| (row.title.as_str(), row.replacement.as_str()))
        .collect();
    assert_eq!(by_title["Ideas"], "Ideas");
    assert_eq!(by_title["Inbox Ideas"], "inbox-ideas");
    assert_eq!(by_title["Future Work"], "\"Future Work\"");
    assert_eq!(by_title["future-work"], "future-work");
    assert_eq!(by_title["A \"Q\""], "\"A \\\"Q\\\"\"");
    assert_eq!(by_title["a-\"q\""], "a-\"q\"");
}

const GENERATED_NOTEBOOKS: &str = concat!(
    "# Dev\n",
    "\n",
    "## Notebooks\n",
    "\n",
    "<!-- bob:notebooks:begin v1 -->\n",
    "\n",
    "> [!notebooks]\n",
    ">\n",
    "> - **[[git|Git]]**\n",
    "\n",
    "<!-- bob:notebooks:end -->\n",
    "\n",
    "## Ideas\n",
    "\n",
    "- one\n",
    "\n",
    "## Tasks\n",
);

#[test]
fn generated_notebooks_section_is_never_completed() {
    let titles: Vec<String> =
        super::super::section_completion_rows(GENERATED_NOTEBOOKS, "")
            .into_iter()
            .map(|row| row.title)
            .collect();
    assert_eq!(titles, vec!["Dev".to_string(), "Ideas".to_string()]);
    assert!(super::super::section_completion_rows(
        GENERATED_NOTEBOOKS,
        "notebooks"
    )
    .is_empty());
    let headings: Vec<String> =
        super::super::non_tasks_section_headings(GENERATED_NOTEBOOKS)
            .into_iter()
            .map(|heading| heading.title)
            .collect();
    assert_eq!(headings, vec!["Dev".to_string(), "Ideas".to_string()]);
}

#[test]
fn generated_notebooks_section_is_never_a_bullet_target() {
    // A neighbor selector still lands in its own section and leaves the
    // generated block untouched.
    let (updated, _) = super::super::insert_bullet_line(
        GENERATED_NOTEBOOKS,
        BULLET,
        Some("Ideas"),
        false,
    );
    assert!(updated.contains(&format!("- one\n{BULLET}")), "{updated}");
    let before_block = updated
        .split("<!-- bob:notebooks:begin v1 -->")
        .next()
        .unwrap();
    assert!(!before_block.contains(BULLET));
    let in_block = updated
        .split("<!-- bob:notebooks:begin v1 -->")
        .nth(1)
        .and_then(|rest| rest.split("<!-- bob:notebooks:end -->").next())
        .unwrap();
    assert!(!in_block.contains(BULLET));
}

#[test]
fn explicit_notebooks_selector_refuses_instead_of_falling_through() {
    for (selector, exact) in [
        ("notebooks", false),
        ("Notebooks", true),
        ("note", false),
        ("n", false),
    ] {
        let error = super::super::reject_generated_notebooks_selector(
            GENERATED_NOTEBOOKS,
            Some(selector),
            exact,
        )
        .expect_err(selector);
        assert!(
            error.message.contains("generated notebook catalog"),
            "{selector}: {}",
            error.message
        );
    }
    // Other selectors, an eligible match, bare `#`, and notes without the
    // generated heading are unaffected.
    for (contents, selector) in [
        (GENERATED_NOTEBOOKS, Some("Ideas")),
        (GENERATED_NOTEBOOKS, Some("zzz")),
        (GENERATED_NOTEBOOKS, None),
        ("## Ideas\n", Some("notebooks")),
    ] {
        super::super::reject_generated_notebooks_selector(
            contents, selector, false,
        )
        .expect("not refused");
    }
    // A user's own non-generated `Notebooks` heading elsewhere still wins
    // when it is an eligible match.
    super::super::reject_generated_notebooks_selector(
        "## Notebooks\n\n## Notebooks Extra\n",
        Some("notebooks"),
        false,
    )
    .expect("eligible heading exists");
}

#[test]
fn heading_matches_bullet_selector_is_raw_or_identity() {
    assert!(super::super::heading_matches_bullet_selector(
        "Ideas",
        Some("Id"),
        false,
        false,
    ));
    assert!(!super::super::heading_matches_bullet_selector(
        "Future Work",
        Some("future-work"),
        false,
        false,
    ));
}

#[test]
fn section_completion_slug_query_matches_spaced_title() {
    let contents = "## Future Work\nnotes\n## Ideas\nmore\n";
    let rows = super::super::section_completion_rows(contents, "future-w");
    assert_eq!(
        rows.iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Future Work"]
    );
    assert_eq!(rows[0].replacement, "future-work");
}

fn quoted_selector(title: &str) -> String {
    format!("\"{}\"", title.replace('\\', "\\\\").replace('"', "\\\""))
}

fn completion_pairs(contents: &str) -> Vec<(String, String)> {
    super::super::section_completion_rows(contents, "")
        .into_iter()
        .map(|row| (row.title, row.replacement))
        .collect()
}

#[test]
fn section_completion_omits_selectors_that_cannot_round_trip() {
    let quote_leading_spaced = concat!(
        "PREAMBLE\n",
        "## \"Future Work\"\n",
        "TARGET\n",
        "## future-work\n",
        "DECOY\n",
    );
    let quote_leading_word = concat!(
        "PREAMBLE\n",
        "## \"Quoted\"\n",
        "TARGET\n",
        "## quoted\n",
        "DECOY\n",
    );
    let curly_spaced = concat!(
        "PREAMBLE\n",
        "## “Future Work”\n",
        "TARGET\n",
        "## future-work\n",
        "DECOY\n",
    );
    let curly_word = concat!(
        "PREAMBLE\n",
        "## “Quoted”\n",
        "TARGET\n",
        "## quoted\n",
        "DECOY\n",
    );
    let escaped = concat!(
        "PREAMBLE\n",
        "## Say \"hi\" \\ there\n",
        "TARGET\n",
        "## say-\"hi\"-\\-there\n",
        "DECOY\n",
    );
    let duplicates = concat!(
        "PREAMBLE\n",
        "## Ideas\n",
        "FIRST\n",
        "## Ideas\n",
        "SECOND\n",
    );
    let case_variants = concat!(
        "PREAMBLE\n",
        "## Ideas\n",
        "FIRST\n",
        "## ideas\n",
        "SECOND\n",
    );
    let h1_shadowed =
        concat!("PREAMBLE\n", "# Ideas\n", "H1\n", "## Ideas\n", "H2\n",);

    assert_eq!(
        completion_pairs(quote_leading_spaced),
        vec![
            (
                "\"Future Work\"".to_string(),
                quoted_selector("\"Future Work\""),
            ),
            ("future-work".to_string(), "future-work".to_string()),
        ]
    );
    assert_eq!(
        completion_pairs(quote_leading_word),
        vec![
            ("\"Quoted\"".to_string(), quoted_selector("\"Quoted\"")),
            ("quoted".to_string(), "quoted".to_string()),
        ]
    );
    assert_eq!(
        completion_pairs(curly_spaced),
        vec![
            (
                "“Future Work”".to_string(),
                quoted_selector("“Future Work”"),
            ),
            ("future-work".to_string(), "future-work".to_string()),
        ]
    );
    assert_eq!(
        completion_pairs(curly_word),
        vec![
            ("“Quoted”".to_string(), quoted_selector("“Quoted”")),
            ("quoted".to_string(), "quoted".to_string()),
        ]
    );
    assert_eq!(
        completion_pairs(escaped),
        vec![
            (
                "Say \"hi\" \\ there".to_string(),
                quoted_selector("Say \"hi\" \\ there"),
            ),
            (
                "say-\"hi\"-\\-there".to_string(),
                "say-\"hi\"-\\-there".to_string(),
            ),
        ]
    );
    assert_eq!(
        completion_pairs(duplicates),
        vec![("Ideas".to_string(), "Ideas".to_string())]
    );
    assert_eq!(
        completion_pairs(case_variants),
        vec![("Ideas".to_string(), "Ideas".to_string())]
    );
    assert_eq!(
        completion_pairs(h1_shadowed),
        vec![("Ideas".to_string(), "Ideas".to_string())]
    );
}

const ARTICLE_SECTIONS: &str = concat!(
    "---\n",
    "type: \"[[article]]\"\n",
    "---\n",
    "\n",
    "# 1. Title\n",
    "\n",
    "## 1.1 Intro\n",
    "intro\n",
    "## 1.2 Skills\n",
    "skills\n",
    "## 1.3 Agent CLIs\n",
    "agents\n",
    "### 1.3.1 Nested\n",
    "nested\n",
    "## 1.4 Later\n",
    "later\n",
);

fn article_with_bullet_in_agent_clis() -> String {
    ARTICLE_SECTIONS.replace(
        "## 1.3 Agent CLIs\nagents\n",
        &format!("## 1.3 Agent CLIs\n\n{BULLET}\nagents\n"),
    )
}

#[test]
fn article_prefix_matches_de_numbered_identity() {
    assert_eq!(
        insert_bullet_line(ARTICLE_SECTIONS, BULLET, Some("agent")),
        (article_with_bullet_in_agent_clis(), Placement::Inserted)
    );
}

#[test]
fn article_prefix_prefers_raw_number_in_document_order() {
    assert_eq!(
        insert_bullet_line(ARTICLE_SECTIONS, BULLET, Some("1.3")),
        (article_with_bullet_in_agent_clis(), Placement::Inserted)
    );
}

#[test]
fn article_exact_section_matches_de_numbered_identity() {
    assert_eq!(
        super::super::insert_bullet_line(
            ARTICLE_SECTIONS,
            BULLET,
            Some("Agent CLIs"),
            true,
        ),
        (article_with_bullet_in_agent_clis(), Placement::Inserted)
    );
}

#[test]
fn article_exact_section_matches_stale_number() {
    assert_eq!(
        super::super::insert_bullet_line(
            ARTICLE_SECTIONS,
            BULLET,
            Some("1.4 Agent CLIs"),
            true,
        ),
        (article_with_bullet_in_agent_clis(), Placement::Inserted)
    );
}

#[test]
fn article_exact_raw_title_match_precedes_earlier_identity_match() {
    let contents = concat!(
        "---\n",
        "type: \"[[article]]\"\n",
        "---\n",
        "### 1.1.1 Examples\n",
        "first\n",
        "### 1.2.1 Examples\n",
        "second\n",
    );
    let expected = format!(
        "---\ntype: \"[[article]]\"\n---\n### 1.1.1 Examples\nfirst\n### 1.2.1 Examples\n\n{BULLET}\nsecond\n"
    );
    assert_eq!(
        super::super::insert_bullet_line(
            contents,
            BULLET,
            Some("1.2.1 Examples"),
            true,
        ),
        (expected, Placement::Inserted)
    );
}

#[test]
fn article_exact_section_matches_plain_identity() {
    let contents = concat!(
        "---\n",
        "type: \"[[article]]\"\n",
        "---\n",
        "## 1.3 **Agent CLIs**\n",
        "agents\n",
    );
    assert_eq!(
        super::super::insert_bullet_line(
            contents,
            BULLET,
            Some("Agent CLIs"),
            true,
        ),
        (
            format!(
                "---\ntype: \"[[article]]\"\n---\n## 1.3 **Agent CLIs**\n\n{BULLET}\nagents\n"
            ),
            Placement::Inserted,
        )
    );
}

#[test]
fn non_article_numbered_heading_keeps_raw_title_matching() {
    let contents = "# Title\n## 1.3 Agent CLIs\nagents\n";
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("agent")),
        (
            format!("{BULLET}\n# Title\n## 1.3 Agent CLIs\nagents\n"),
            Placement::Inserted,
        )
    );
    assert_eq!(
        super::super::insert_bullet_line(
            contents,
            BULLET,
            Some("Agent CLIs"),
            true,
        ),
        (
            format!("{BULLET}\n# Title\n## 1.3 Agent CLIs\nagents\n"),
            Placement::Inserted,
        )
    );
    assert_eq!(
        insert_bullet_line(contents, BULLET, Some("1.3")),
        (
            format!("# Title\n## 1.3 Agent CLIs\n\n{BULLET}\nagents\n"),
            Placement::Inserted,
        )
    );
}

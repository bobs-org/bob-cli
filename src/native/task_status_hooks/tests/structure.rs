//! Pomodoro, duplicate, strike, move, cancel, and empty-block tests.
use super::super::structure::BlockedConsumption;
use super::*;

#[test]
fn extracts_only_block_links_under_open_pomodoros() {
    let lines = [
        "- [ ] Open (0900-0930)",
        "  - [[dev#^one]] and [[Projects/Alpha.md#^two|alias]]",
        "  - ignore [[note]], [[note#Heading]], and [[note|alias #^fake]]",
        "- [x] Closed (0930-1000)",
        "  - [[dev#^closed]]",
    ];
    let model = scan_pomodoros(&lines, 0..lines.len());
    assert_eq!(model.open_pomodoros, 1);
    assert_eq!(
        model.raw_references,
        BTreeSet::from([
            RawReference {
                target: "Projects/Alpha.md".to_string(),
                block_id: "two".to_string(),
            },
            RawReference {
                target: "dev".to_string(),
                block_id: "one".to_string(),
            },
        ])
    );
}

#[test]
fn duplicate_lines_use_canonical_task_identity_and_first_open_owner() {
    let contents = concat!(
        "- [ ] First\n",
        "  - [[Projects/Alpha#^ship]]\n",
        "  - [[Alpha#^ship|same owner repeat]]\n",
        "  - [[#^daily]]\n",
        "- [ ] Second\n",
        "  - ![[Alpha.md#^ship|embedded duplicate]]\n",
        "  - [[2026/Today#^daily|same-note duplicate]]\n",
        "- [ ] Third\n",
        "  - ~~[[Projects/Alpha.md#^ship]]~~\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved_paths(&[
        ("Projects/Alpha", "ship", "Projects/Alpha.md", vec![' ']),
        ("Alpha", "ship", "Projects/Alpha.md", vec![' ']),
        ("Alpha.md", "ship", "Projects/Alpha.md", vec![' ']),
        ("Projects/Alpha.md", "ship", "Projects/Alpha.md", vec![' ']),
        ("", "daily", "2026/Today.md", vec![' ']),
        ("2026/Today", "daily", "2026/Today.md", vec![' ']),
    ]);

    let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
    assert_eq!(
        removals
            .iter()
            .map(|item| item.line_number)
            .collect::<Vec<_>>(),
        vec![6, 7, 9]
    );
    assert!(removals.iter().all(|item| item.duplicate_tasks.len() == 1));
    assert_eq!(
        removals[0].duplicate_tasks[0],
        DuplicateTaskIdentity {
            path: "Projects/Alpha.md".to_string(),
            block_id: "ship".to_string(),
        }
    );
}

#[test]
fn deleted_conflict_line_cannot_claim_an_unrelated_task() {
    let contents = concat!(
        "- [ ] First\n",
        "  - [[tasks#^alpha]]\n",
        "- [ ] Second\n",
        "  - [[tasks#^alpha]] and [[tasks#^beta]]\n",
        "- [ ] Third\n",
        "  - [[tasks#^beta]]\n",
        "- [ ] Fourth\n",
        "  - [[tasks#^beta]] and [[tasks#^alpha]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[
        ("tasks", "alpha", vec![' ']),
        ("tasks", "beta", vec![' ']),
    ]);

    let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
    assert_eq!(
        removals
            .iter()
            .map(|item| item.line_number)
            .collect::<Vec<_>>(),
        vec![4, 8]
    );
    assert_eq!(removals[1].duplicate_tasks.len(), 2);
}

#[test]
fn duplicate_cleanup_ignores_distinct_unresolved_and_ineligible_links() {
    let contents = concat!(
        "- [ ] First with [[Alpha#^same]] on its top-level line\n",
        "  - [[Alpha#^same]]\n",
        "  - ~~[[Alpha#^same]]~~\n",
        "- [ ] Second\n",
        "  - [[Beta#^same]] and [[missing#^same]]\n",
        "  ```md\n",
        "  - [[Alpha#^same]]\n",
        "  ```\n",
        "- [x] Closed\n",
        "  - [[Alpha#^same]]\n",
        "- [-] Cancelled\n",
        "  - [[Alpha#^same]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved_paths(&[
        ("Alpha", "same", "Alpha.md", vec![' ']),
        ("Beta", "same", "Beta.md", vec![' ']),
    ]);

    assert!(plan_duplicate_line_removals(&lines, &model, &resolved).is_empty());
}

#[test]
fn full_line_deletion_preserves_children_crlf_and_final_line_ending() {
    let contents = concat!(
        "- [ ] First\r\n",
        "  - [[tasks#^alpha]] and [[tasks#^beta]]\r\n",
        "- [ ] Second\r\n",
        "  - authored [[tasks#^alpha]] plus [[tasks#^beta]]\r\n",
        "    - retained child\r\n",
        "- [ ] Third\r\n",
        "  - [[tasks#^gamma]]",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[
        ("tasks", "alpha", vec![' ']),
        ("tasks", "beta", vec![' ']),
        ("tasks", "gamma", vec![' ']),
    ]);
    let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
    assert_eq!(removals.len(), 1);
    assert_eq!(removals[0].duplicate_tasks.len(), 2);
    let deleted_lines = BTreeSet::from([removals[0].line_number - 1]);
    let plan = plan_structural_changes(
        &model,
        &resolved,
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &deleted_lines,
        false,
    );

    assert_eq!(
        apply_structural_plan(contents, &model, &plan),
        concat!(
            "- [ ] First\r\n",
            "  - [[tasks#^alpha]] and [[tasks#^beta]]\r\n",
            "- [ ] Second\r\n",
            "    - retained child\r\n",
            "- [ ] Third\r\n",
            "  - [[tasks#^gamma]]",
        )
    );
}

#[test]
fn deleted_completed_duplicate_is_not_retired_moved_or_reinserted() {
    let contents = concat!(
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^done]]\n",
        "- [ ] Later\n",
        "  - ![[tasks#^done|duplicate]]\n",
        "    - retained child\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[("tasks", "done", vec!['x'])]);
    let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
    let deleted_lines = removals
        .iter()
        .map(|item| item.line_number - 1)
        .collect::<BTreeSet<_>>();
    let plan = plan_structural_changes(
        &model,
        &resolved,
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &deleted_lines,
        false,
    );

    assert_eq!(plan.struck.len(), 1);
    assert!(plan.moved.is_empty());
    assert!(!plan.token_edits.contains_key(&3));
    assert_eq!(
        apply_structural_plan(contents, &model, &plan),
        concat!(
            "- [ ] Current (0900-0930)\n",
            "  - ~~[[tasks#^done]]~~\n",
            "- [ ] Later\n",
            "    - retained child\n",
        )
    );
}

#[test]
fn struck_references_are_retired_and_spans_are_paired() {
    let lines = [
        "- [ ] Current",
        "  - ~~[[dev#^old]]~~",
        "  - ~~before~~[[dev#^live]]~~after~~",
    ];
    let model = scan_pomodoros(&lines, 0..lines.len());
    assert!(!model.raw_references.contains(&reference("dev", "old")));
    assert!(model.raw_references.contains(&reference("dev", "live")));
    let live = &model.bullets[1].links[0];
    assert!(!live.struck);
    assert!(live.retired_unmarked_token.starts_with(" ~~"));
    assert_eq!(live.retired_marked_token, " 🍅 ~~[[dev#^live]]~~ ");
}

#[test]
fn completed_fallback_does_not_take_mixed_live_bullets() {
    let lines = [
        "- [x] Completed (0800-0830)",
        "- [ ] Untimed",
        "  - [[dev#^done]] and [[dev#^live]]",
    ];
    let model = scan_pomodoros(&lines, 0..lines.len());
    let plan = plan_structural_changes(
        &model,
        &resolved(&[("dev", "done", vec!['x']), ("dev", "live", vec![' '])]),
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &BTreeSet::new(),
        false,
    );
    assert!(plan.moves.is_empty());
    assert_eq!(plan.struck.len(), 1);
}

#[test]
fn parses_embedded_alias_and_mixed_block_links() {
    let links = block_link_occurrences(
        "  - ![[dev#^done|Done alias]] and [[dev#^todo]]",
    );
    assert_eq!(links.len(), 2);
    assert!(links[0].embedded);
    assert_eq!(links[0].reference, reference("dev", "done"));
    assert!(!links[1].embedded);
    assert_eq!(links[1].reference, reference("dev", "todo"));
}

#[test]
fn parses_and_normalizes_pomodoro_marker_prefixes_per_link() {
    let links = block_link_occurrences(
        "  - 🍅   ![[dev#^embedded|Alias]] and 🍅 🍅 ~~[[dev#^done]]~~",
    );
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].marker_count, 1);
    assert_eq!(
        links[0].preserved_marked_token,
        "🍅 ![[dev#^embedded|Alias]]"
    );
    assert_eq!(links[1].marker_count, 2);
    assert_eq!(links[1].retired_marked_token, "🍅 ~~[[dev#^done]]~~");
}

#[test]
fn dependency_lines_are_not_legacy_children() {
    // Depends-On lines route through the line parser, never the R8
    // legacy-child recogniser (`docs/task-dependencies.md` §§2, 4.2).
    for line in [
        "  - ⛓️ **DEPENDS ON:** [[#^dep]]",
        "  - 🔗 **DEPENDENCIES:** ![[#^dep]]",
        "  - **DEPENDS ON:**",
        "  - ⛓️ **DEPENDS ON:** [[#^a",
    ] {
        assert_eq!(task_dependencies::legacy_child_reference(line), None);
        assert!(task_dependencies::is_dependency_line(line));
    }
}

#[test]
fn both_managed_dependency_lines_are_not_pomodoro_task_links() {
    let lines = [
        "- [ ] Open (0900-0930)",
        "  - ⛓️ **DEPENDS ON:** [[#^prerequisite]]",
        "  - 🧱 **BLOCKS:** [[#^dependent]]",
        "  - 🪢 **LINKS:** [[#^related]]",
        "  - 🪢 **LINKED FROM:** [[#^source]]",
        "  - [[#^worked]]",
    ];
    let model = scan_pomodoros(&lines, 0..lines.len());
    assert_eq!(
        model.raw_references,
        BTreeSet::from([reference("", "worked")])
    );
}

#[test]
fn blocks_lines_are_recognized_as_managed_not_legacy_children() {
    for line in [
        "  - 🧱 **BLOCKS:** [[#^dependent]]",
        "  - **BLOCKING:** ![[#^dependent]]",
        "  - 🧱 **BLOCKS:** [[#^dependent",
    ] {
        assert_eq!(task_dependencies::legacy_child_reference(line), None);
        assert!(task_dependencies::blocks::is_blocks_line(line));
        assert!(task_dependencies::is_managed_dependency_line(line));
        assert!(task_dependencies::is_managed_link_line(line));
    }
}

#[test]
fn relation_lines_are_managed_link_lines_not_dependency_lines() {
    for line in [
        "  - 🪢 **LINKS:** [[#^related]]",
        "  - 🪢 **LINKED FROM:** [[#^source]]",
        "  - **BACKLINKS:** [[#^source]]",
        "  - 🪢 **LINKS:** [[#^a]] extra",
    ] {
        assert_eq!(task_dependencies::legacy_child_reference(line), None);
        assert!(
            !task_dependencies::is_managed_dependency_line(line),
            "{line}"
        );
        assert!(task_dependencies::is_managed_link_line(line), "{line}");
    }
}

#[test]
fn blocks_parser_does_not_claim_other_managed_child_lines() {
    for line in [
        "  - ❌ **CANCEL LOG**",
        "  - 🧩 **Sub-projects:** [[Child]]",
        "  - 🗓️ **SCHEDULE LOG**",
        "  - 🛠️ **WORK LOG**",
    ] {
        assert!(!task_dependencies::blocks::is_blocks_line(line), "{line}");
        assert!(
            !task_dependencies::is_managed_dependency_line(line),
            "{line}"
        );
    }
}

#[test]
fn moves_completed_mixed_bullet_subtree_to_current_and_strikes_only_done() {
    let contents = concat!(
        "- [ ] Current (0900-0930)\n",
        "  - Existing child\n",
        "- [ ] Future\n",
        "    - [[dev#^done|Done]] and [[dev#^todo]]\n",
        "      - Nested detail\n",
        "  ```\n",
        "  - [[dev#^fenced]]\n",
        "  ```\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    assert!(!model.raw_references.contains(&reference("dev", "fenced")));
    let plan = plan_structural_changes(
        &model,
        &resolved(&[("dev", "done", vec!['x']), ("dev", "todo", vec![' '])]),
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &BTreeSet::new(),
        false,
    );
    let updated = apply_structural_plan(contents, &model, &plan);
    assert_eq!(
        updated,
        concat!(
            "- [ ] Current (0900-0930)\n",
            "  - Existing child\n",
            "  - ~~[[dev#^done|Done]]~~ and [[dev#^todo]]\n",
            "    - Nested detail\n",
            "- [ ] Future\n",
            "  ```\n",
            "  - [[dev#^fenced]]\n",
            "  ```\n",
        )
    );
    assert_eq!(plan.struck.len(), 1);
    assert_eq!(plan.moved.len(), 1);
}

#[test]
fn repairs_completed_pomodoro_links_in_place_and_is_idempotent() {
    let contents = concat!(
        "- [x] Historical (0800-0830)\r\n",
        "  - 🍅 ![[dev#^done|Embedded]] and [[dev#^done|Plain]]\r\n",
        "  - 🍅 ~~![[dev#^done|Stale]]~~ and ~~[[dev#^done|Canonical]]~~\r\n",
        "- [ ] Current (0900-0930)\r\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    assert!(model.raw_references.is_empty());
    assert_eq!(model.all_references.len(), 1);
    let plan = plan_structural_changes(
        &model,
        &resolved(&[("dev", "done", vec!['x'])]),
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &BTreeSet::new(),
        false,
    );
    assert!(plan.moves.is_empty());
    assert_eq!(plan.struck.len(), 3);
    let updated = apply_structural_plan(contents, &model, &plan);
    assert_eq!(
        updated,
        concat!(
            "- [x] Historical (0800-0830)\r\n",
            "  - ~~[[dev#^done|Embedded]]~~ and 🍅 ~~[[dev#^done|Plain]]~~\r\n",
            "  - ~~[[dev#^done|Stale]]~~ and ~~[[dev#^done|Canonical]]~~\r\n",
            "- [ ] Current (0900-0930)\r\n",
        )
    );
    assert_eq!(plan.marker_added.len(), 1);
    assert_eq!(plan.marker_removed.len(), 2);
    let updated_lines = logical_lines(&updated);
    let updated_model = scan_pomodoros(&updated_lines, 0..updated_lines.len());
    let second = plan_structural_changes(
        &updated_model,
        &resolved(&[("dev", "done", vec!['x'])]),
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &BTreeSet::new(),
        false,
    );
    assert!(
        second.token_edits.is_empty(),
        "unexpected second-pass edits: {:?}",
        second.token_edits
    );
    assert!(second.marker_added.is_empty());
    assert!(second.marker_removed.is_empty());
    assert_eq!(
        apply_structural_plan(&updated, &updated_model, &second),
        updated
    );
}

#[test]
fn repairs_markers_by_owner_and_marks_completed_fallback_moves() {
    let contents = concat!(
        "- [x] Done\n",
        "  - [[dev#^live]] and 🍅 🍅 ~~[[dev#^done]]~~\n",
        "- [ ] Future\n",
        "  - 🍅 [[dev#^open]]\n",
        "  - [[dev#^done]]\n",
        "- [-] Cancelled\n",
        "  - 🍅 🍅 [[dev#^cancelled]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let plan = plan_structural_changes(
        &model,
        &resolved(&[
            ("dev", "live", vec![' ']),
            ("dev", "done", vec!['x']),
            ("dev", "open", vec![' ']),
        ]),
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &BTreeSet::new(),
        false,
    );
    assert_eq!(plan.marker_added.len(), 2);
    assert_eq!(plan.marker_removed.len(), 2);
    let updated = apply_structural_plan(contents, &model, &plan);
    assert_eq!(
        updated,
        concat!(
            "- [x] Done\n",
            "  - 🍅 [[dev#^live]] and 🍅 ~~[[dev#^done]]~~\n",
            "  - 🍅 ~~[[dev#^done]]~~\n",
            "- [ ] Future\n",
            "  - [[dev#^open]]\n",
            "- [-] Cancelled\n",
            "  - 🍅 🍅 [[dev#^cancelled]]\n",
        )
    );
}

#[test]
fn conflicting_duplicate_statuses_are_not_normalized() {
    let contents = "- [ ] Future\n  - [[dev#^duplicate]]\n";
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let plan = plan_structural_changes(
        &model,
        &resolved(&[("dev", "duplicate", vec!['x', ' '])]),
        &BTreeSet::from(['x', 'X']),
        &test_settings().status_types,
        &BTreeSet::new(),
        false,
    );
    assert!(plan.token_edits.is_empty());
    assert!(plan.moves.is_empty());
    assert_eq!(apply_structural_plan(contents, &model, &plan), contents);
}

#[test]
fn canceled_reference_removal_deletes_complete_mixed_content_items() {
    let contents = concat!(
        "- [ ] Open (0900-0930) with [[tasks#^plain]]\r\n",
        "  - start [[tasks#^plain]] middle ![[tasks#^custom|Alias]] end\r\n",
        "  - 🍅 [[tasks#^marked]] and ~~[[tasks#^struck]]~~ and [[tasks#^done]] and [[tasks#^live]]\r\n",
        "  - [[tasks#^all-canceled]] and [[tasks#^mixed]] and [[missing#^unknown]]\r\n",
        "  ```md\r\n",
        "  - [[tasks#^plain]]\r\n",
        "  ```\r\n",
        "- [x] Completed\r\n",
        "  - 🍅 [[tasks#^plain]]\r\n",
        "- [-] Canceled\r\n",
        "  - [[tasks#^custom]]",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[
        ("tasks", "plain", vec!['-']),
        ("tasks", "custom", vec!['C']),
        ("tasks", "marked", vec!['-']),
        ("tasks", "struck", vec!['-']),
        ("tasks", "done", vec!['x']),
        ("tasks", "live", vec![' ']),
        ("tasks", "all-canceled", vec!['-', 'C']),
        ("tasks", "mixed", vec!['-', ' ']),
    ]);
    let mut settings = test_settings();
    settings.status_types.insert('C', TaskStatusType::Cancelled);
    let plan = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );

    assert_eq!(
        plan.removed_canceled,
        vec![
            RemovedCanceledReference {
                target: "tasks".to_string(),
                block_id: "plain".to_string(),
                line_number: 2,
                pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                    .to_string(),
            },
            RemovedCanceledReference {
                target: "tasks".to_string(),
                block_id: "custom".to_string(),
                line_number: 2,
                pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                    .to_string(),
            },
            RemovedCanceledReference {
                target: "tasks".to_string(),
                block_id: "marked".to_string(),
                line_number: 3,
                pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                    .to_string(),
            },
            RemovedCanceledReference {
                target: "tasks".to_string(),
                block_id: "struck".to_string(),
                line_number: 3,
                pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                    .to_string(),
            },
            RemovedCanceledReference {
                target: "tasks".to_string(),
                block_id: "all-canceled".to_string(),
                line_number: 4,
                pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                    .to_string(),
            },
        ]
    );
    assert!(plan.struck.is_empty());
    assert!(plan.moved.is_empty());
    assert!(plan.token_edits.is_empty());
    assert!(plan.marker_added.is_empty());
    assert!(plan.marker_removed.is_empty());
    let updated = apply_structural_plan(contents, &model, &plan);
    assert_eq!(
        updated,
        concat!(
            "- [ ] Open (0900-0930) with [[tasks#^plain]]\r\n",
            "  ```md\r\n",
            "  - [[tasks#^plain]]\r\n",
            "  ```\r\n",
            "- [x] Completed\r\n",
            "  - 🍅 [[tasks#^plain]]\r\n",
            "- [-] Canceled\r\n",
            "  - [[tasks#^custom]]",
        )
    );
    let updated_lines = logical_lines(&updated);
    let updated_model = scan_pomodoros(&updated_lines, 0..updated_lines.len());
    assert!(!updated_model
        .raw_references
        .contains(&reference("tasks", "plain")));
    assert!(!updated_model
        .raw_references
        .contains(&reference("tasks", "live")));
    assert!(!updated_model
        .raw_references
        .contains(&reference("tasks", "mixed")));
    let second = plan_structural_changes(
        &updated_model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );
    assert!(second.removed_canceled.is_empty());
    assert!(second.token_edits.is_empty());
    assert_eq!(
        apply_structural_plan(&updated, &updated_model, &second),
        updated
    );
}

#[test]
fn canceled_subtrees_compose_with_nested_and_moving_bullets() {
    let contents = concat!(
        "- [x] Completed\r\n",
        "  - existing completed child\r\n",
        "- [ ] Future\r\n",
        "  - surviving [[tasks#^live]]\r\n",
        "    - canceled child [[tasks#^canceled]]\r\n",
        "      - nested detail [[tasks#^done]]\r\n",
        "    - surviving child [[tasks#^other]]\r\n",
        "  - canceled parent [[tasks#^canceled]]\r\n",
        "    - redundant [[tasks#^custom]]\r\n",
        "  - moving parent [[tasks#^done]]\r\n",
        "    - omitted [[tasks#^custom]]\r\n",
        "      - omitted detail\r\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[
        ("tasks", "live", vec![' ']),
        ("tasks", "other", vec![' ']),
        ("tasks", "canceled", vec!['-']),
        ("tasks", "custom", vec!['C']),
        ("tasks", "done", vec!['x']),
    ]);
    let mut settings = test_settings();
    settings.status_types.insert('C', TaskStatusType::Cancelled);

    let plan = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );

    assert_eq!(
        plan.removed_canceled
            .iter()
            .map(|item| (item.block_id.as_str(), item.line_number))
            .collect::<Vec<_>>(),
        vec![("canceled", 5), ("canceled", 8), ("custom", 11)]
    );
    assert_eq!(plan.struck.len(), 1);
    assert_eq!(plan.moved.len(), 1);
    assert_eq!(
        apply_structural_plan(contents, &model, &plan),
        concat!(
            "- [x] Completed\r\n",
            "  - existing completed child\r\n",
            "  - moving parent 🍅 ~~[[tasks#^done]]~~\r\n",
            "- [ ] Future\r\n",
            "  - surviving [[tasks#^live]]\r\n",
            "    - surviving child [[tasks#^other]]\r\n",
        )
    );
}

#[test]
fn canceled_subtree_deletion_preserves_crlf_and_no_final_newline() {
    let contents = concat!(
        "- [ ] Open\r\n",
        "  - keep [[tasks#^live]]\r\n",
        "  - remove [[tasks#^canceled]]\r\n",
        "    - nested detail\r\n",
        "  - final [[tasks#^other]]",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[
        ("tasks", "live", vec![' ']),
        ("tasks", "canceled", vec!['-']),
        ("tasks", "other", vec![' ']),
    ]);
    let settings = test_settings();
    let plan = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );

    assert_eq!(
        apply_structural_plan(contents, &model, &plan),
        concat!(
            "- [ ] Open\r\n",
            "  - keep [[tasks#^live]]\r\n",
            "  - final [[tasks#^other]]",
        )
    );
}

#[test]
fn duplicate_deleted_lines_do_not_report_canceled_reference_edits() {
    let contents = concat!(
        "- [ ] First\n",
        "  - [[tasks#^canceled]]\n",
        "- [ ] Second\n",
        "  - duplicate [[tasks#^canceled]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[("tasks", "canceled", vec!['-'])]);
    let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
    let deleted_lines = removals
        .iter()
        .map(|item| item.line_number - 1)
        .collect::<BTreeSet<_>>();
    let settings = test_settings();
    let plan = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &deleted_lines,
        false,
    );

    assert_eq!(removals.len(), 1);
    assert_eq!(plan.removed_canceled.len(), 1);
    assert_eq!(plan.removed_canceled[0].line_number, 2);
    assert!(plan.token_edits.is_empty());
    assert_eq!(
        apply_structural_plan(contents, &model, &plan),
        "- [ ] First\n- [ ] Second\n"
    );
}

#[test]
fn direct_child_scan_counts_plain_children_but_ignores_fences() {
    let contents = concat!(
        "- [ ] Empty\n",
        "- [ ] Plain child\n",
        "  - authored note without a link\n",
        "    - [[tasks#^nested]]\n",
        "- [ ] Fenced only\n",
        "  ```md\n",
        "  - [[tasks#^fenced]]\n",
        "  ```\n",
        "- [ ] Direct link\n",
        "  - [[tasks#^direct]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());

    assert!(!model.entries[0].has_child);
    assert!(model.entries[1].has_child);
    assert!(!model.entries[2].has_child);
    assert!(model.entries[3].has_child);
    assert_eq!(
        model.raw_references,
        BTreeSet::from([
            reference("tasks", "direct"),
            reference("tasks", "nested"),
        ])
    );
}

#[test]
fn empty_pomodoro_deletion_removes_full_blocks_and_preserves_crlf_eof() {
    let contents = concat!(
        "## Pomodoros\r\n\r\n",
        "- [ ] ()\r\n",
        "  continuation text\r\n",
        "- [ ] (**0900-0930** [t:: 30m])\r\n",
        "- [x] Done (0800-0830)\r\n",
        "- [ ] Kept\r\n",
        "  - child without a link"
    );
    let lines = logical_lines(contents);
    let section = native_pomodoro::pomodoros_section_range(&lines).unwrap();
    let model = scan_pomodoros(&lines, section);
    let plan = plan_empty_pomodoro_removals(contents, &model);

    assert_eq!(
        plan.removed,
        vec![
            RemovedEmptyPomodoro {
                line_number: 3,
                line: "- [ ] ()".to_string(),
            },
            RemovedEmptyPomodoro {
                line_number: 5,
                line: "- [ ] (**0900-0930** [t:: 30m])".to_string(),
            },
            RemovedEmptyPomodoro {
                line_number: 6,
                line: "- [x] Done (0800-0830)".to_string(),
            },
        ]
    );
    assert_eq!(
        apply_empty_pomodoro_plan(contents, &plan),
        "## Pomodoros\r\n\r\n- [ ] Kept\r\n  - child without a link"
    );
}

#[test]
fn entries_emptied_by_duplicate_cleanup_are_removed_in_same_pass() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [ ] First\n",
        "  - [[tasks#^alpha]]\n",
        "- [ ] Second\n",
        "  - duplicate [[tasks#^alpha]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let resolved = resolved(&[("tasks", "alpha", vec![' '])]);
    let duplicate_removals =
        plan_duplicate_line_removals(&lines, &model, &resolved);
    let deleted_lines = duplicate_removals
        .iter()
        .map(|item| item.line_number - 1)
        .collect::<BTreeSet<_>>();
    let settings = test_settings();
    let structural = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &deleted_lines,
        false,
    );
    let structurally_updated =
        apply_structural_plan(contents, &model, &structural);
    let empty = plan_empty_pomodoro_removals(&structurally_updated, &model);

    assert_eq!(
        duplicate_removals
            .iter()
            .map(|item| item.line_number)
            .collect::<Vec<_>>(),
        vec![6]
    );
    assert_eq!(
        empty.removed,
        vec![RemovedEmptyPomodoro {
            line_number: 5,
            line: "- [ ] Second".to_string(),
        }]
    );
    assert_eq!(
        apply_empty_pomodoro_plan(&structurally_updated, &empty),
        "## Pomodoros\n\n- [ ] First\n  - [[tasks#^alpha]]\n"
    );
}

#[test]
fn moving_last_child_removes_source_but_retains_destination() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [x] Done\n",
        "  - existing child\n",
        "- [ ] Future\n",
        "  - [[tasks#^done]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let settings = test_settings();
    let structural = plan_structural_changes(
        &model,
        &resolved(&[("tasks", "done", vec!['x'])]),
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );
    let structurally_updated =
        apply_structural_plan(contents, &model, &structural);
    let empty = plan_empty_pomodoro_removals(&structurally_updated, &model);

    assert_eq!(structural.moved.len(), 1);
    assert_eq!(
        empty.removed,
        vec![RemovedEmptyPomodoro {
            line_number: 5,
            line: "- [ ] Future".to_string(),
        }]
    );
    assert_eq!(
        apply_empty_pomodoro_plan(&structurally_updated, &empty),
        concat!(
            "## Pomodoros\n\n",
            "- [x] Done\n",
            "  - existing child\n",
            "  - 🍅 ~~[[tasks#^done]]~~\n",
        )
    );
}

#[test]
fn empty_timed_entries_are_not_current_targets_or_ambiguity_inputs() {
    let contents = concat!(
        "- [ ] Empty current (0900-0930)\n",
        "- [ ] Real current (0930-1000)\n",
        "  - real child\n",
        "- [ ] Empty later (1000-1030)\n",
        "- [ ] Future\n",
        "  - [[tasks#^done]]\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let non_empty_timed = model
        .entries
        .iter()
        .filter(|entry| entry.open && entry.timed && entry.has_child)
        .count();
    let settings = test_settings();
    let structural = plan_structural_changes(
        &model,
        &resolved(&[("tasks", "done", vec!['x'])]),
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );

    assert_eq!(non_empty_timed, 1);
    assert_eq!(structural.moved.len(), 1);
    assert_eq!(
        structural.moved[0].destination_pomodoro,
        "- [ ] Real current (0930-1000)"
    );
}

#[test]
fn two_non_empty_timed_entries_still_match_the_ambiguity_guard() {
    let contents = concat!(
        "- [ ] First (0900-0930)\n",
        "  - child\n",
        "- [ ] Second (0930-1000)\n",
        "  - child\n",
    );
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());

    assert_eq!(
        model
            .entries
            .iter()
            .filter(|entry| entry.open && entry.timed && entry.has_child)
            .count(),
        2
    );
}

#[test]
fn completion_classification_accepts_conventional_and_custom_done_only() {
    let done = BTreeSet::from(['x', 'X', 'D']);
    for status in ['x', 'X', 'D'] {
        assert!(is_done_status(status, &done));
    }
    for status in [' ', '*', '/', '-', '?'] {
        assert!(!is_done_status(status, &done));
    }
}

#[test]
fn cancellation_classification_uses_recognized_tasks_status_types() {
    let mut status_types = test_settings().status_types;
    status_types.insert('C', TaskStatusType::Cancelled);
    status_types.insert('Q', TaskStatusType::Todo);

    for status in ['-', 'C'] {
        assert!(is_canceled_status(status, &status_types));
    }
    for status in [' ', 'x', '*', '/', '?', 'Q', '!'] {
        assert!(!is_canceled_status(status, &status_types));
    }
}

fn blocked_set(ids: &[&str]) -> BTreeSet<(PathBuf, String)> {
    ids.iter()
        .map(|id| (PathBuf::from("tasks.md"), (*id).to_string()))
        .collect()
}

fn consume_blocked(
    contents: &str,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    ids: &[&str],
) -> BlockedConsumption {
    apply_blocked_consumption(contents, resolved, &blocked_set(ids))
}

// PDL1: live link to a Blocked task under an open entry that has other
// children — link consumed; entry kept.
#[test]
fn consumes_blocked_live_link_and_keeps_the_entry() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^blocked]]\n",
        "  - [[tasks#^keep]]\n",
    );
    let resolved = resolved(&[
        ("tasks", "blocked", vec!['?']),
        ("tasks", "keep", vec![' ']),
    ]);
    let consumption = consume_blocked(contents, &resolved, &["blocked"]);

    assert_eq!(consumption.consumed.len(), 1);
    assert_eq!(consumption.consumed[0].block_id, "blocked");
    assert_eq!(consumption.consumed[0].line_number, 4);
    assert_eq!(
        consumption.contents,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[tasks#^keep]]\n",
        )
    );
}

// PDL2: that consumed link was the entry's last child — entry deleted.
#[test]
fn consuming_last_child_empties_the_pomodoro() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^blocked]]\n",
        "- [ ] Later\n",
        "  - [[tasks#^keep]]\n",
    );
    let resolved = resolved(&[
        ("tasks", "blocked", vec!['?']),
        ("tasks", "keep", vec![' ']),
    ]);
    let consumption = apply_blocked_consumption(
        contents,
        &resolved,
        &blocked_set(&["blocked"]),
    );

    assert_eq!(consumption.consumed.len(), 1);
    assert_eq!(
        consumption.removed_empty,
        vec![RemovedEmptyPomodoro {
            line_number: 3,
            line: "- [ ] Current (0900-0930)".to_string(),
        }]
    );
    assert_eq!(
        consumption.contents,
        concat!("## Pomodoros\n\n", "- [ ] Later\n", "  - [[tasks#^keep]]\n",)
    );
}

// PDL4: closed entries in the same day file stay byte-identical.
// PDL5: a struck link under an open entry is untouched.
#[test]
fn consumption_skips_closed_entries_and_struck_links() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [ ] Open (0900-0930)\n",
        "  - ~~[[tasks#^blocked]]~~\n",
        "  - [[tasks#^keep]]\n",
        "- [x] Closed (0800-0830)\n",
        "  - [[tasks#^blocked]]\n",
    );
    let resolved = resolved(&[
        ("tasks", "blocked", vec!['?']),
        ("tasks", "keep", vec![' ']),
    ]);
    let consumption = consume_blocked(contents, &resolved, &["blocked"]);

    assert!(consumption.consumed.is_empty());
    assert_eq!(consumption.contents, contents);
}

// PDL6: prose bullet whose only Task Link token is the Blocked target —
// token removed; the rest of the bullet kept.
#[test]
fn consumption_strips_the_token_from_a_prose_bullet() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - notes [[tasks#^blocked]] kept\n",
    );
    let resolved = resolved(&[("tasks", "blocked", vec!['?'])]);
    let consumption = consume_blocked(contents, &resolved, &["blocked"]);

    assert_eq!(consumption.consumed.len(), 1);
    assert_eq!(
        consumption.contents,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - notes  kept\n",
        )
    );
}

// PDL7: planned link-only bullet with authored child notes — whole
// subtree removed.
#[test]
fn consumption_removes_link_only_subtree_with_child_notes() {
    let contents = concat!(
        "## Pomodoros\r\n\r\n",
        "- [ ] Current (0900-0930)\r\n",
        "  - [[tasks#^blocked]]\r\n",
        "    - authored child note\r\n",
        "  - [[tasks#^keep]]\r\n",
    );
    let resolved = resolved(&[
        ("tasks", "blocked", vec!['?']),
        ("tasks", "keep", vec![' ']),
    ]);
    let consumption = consume_blocked(contents, &resolved, &["blocked"]);

    assert_eq!(consumption.consumed.len(), 1);
    assert_eq!(
        consumption.contents,
        concat!(
            "## Pomodoros\r\n\r\n",
            "- [ ] Current (0900-0930)\r\n",
            "  - [[tasks#^keep]]\r\n",
        )
    );
}

#[test]
fn capture_consumption_keeps_link_only_notes_as_clean_task_text() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^blocked]]\n",
        "    - authored note\n",
    );
    let resolved = resolved(&[("tasks", "blocked", vec!['?'])]);
    let options = BlockedConsumptionOptions {
        keep_notes: true,
        only_emptied: true,
        display_texts: BTreeMap::from([(
            (PathBuf::from("tasks.md"), "blocked".to_string()),
            "Task with plain linked detail".to_string(),
        )]),
    };
    let consumption = apply_blocked_consumption_with_options(
        contents,
        &resolved,
        &blocked_set(&["blocked"]),
        &options,
    );

    assert_eq!(
        consumption.contents,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - Task with plain linked detail\n",
            "    - authored note\n",
        )
    );
    assert_eq!(consumption.consumed[0].action, BlockedLinkAction::NotesKept);
    assert!(consumption.removed_empty.is_empty());
}

#[test]
fn capture_only_removes_pomodoros_emptied_by_this_consumption() {
    let contents = concat!(
        "## Pomodoros\n\n",
        "- [ ] Unrelated placeholder\n",
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^blocked]]\n",
    );
    let resolved = resolved(&[("tasks", "blocked", vec!['?'])]);
    let options = BlockedConsumptionOptions {
        keep_notes: true,
        only_emptied: true,
        display_texts: BTreeMap::new(),
    };
    let consumption = apply_blocked_consumption_with_options(
        contents,
        &resolved,
        &blocked_set(&["blocked"]),
        &options,
    );

    assert_eq!(
        consumption.contents,
        concat!("## Pomodoros\n\n", "- [ ] Unrelated placeholder\n",)
    );
    assert_eq!(consumption.removed_empty.len(), 1);
    assert_eq!(
        consumption.removed_empty[0].line,
        "- [ ] Current (0900-0930)"
    );
}

// Never key removal off `'?'`: an empty Blocked set leaves `[?]` links
// in place so `task_complete` retirement (`Live{'?'}`) is unchanged.
#[test]
fn empty_blocked_set_does_not_consume_question_status_links() {
    let contents =
        concat!("- [ ] Current (0900-0930)\n", "  - [[tasks#^blocked]]\n",);
    let resolved = resolved(&[("tasks", "blocked", vec!['?'])]);
    let lines = logical_lines(contents);
    let model = scan_pomodoros(&lines, 0..lines.len());
    let settings = test_settings();
    let plan = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );

    assert!(plan.removed_blocked.is_empty());
    assert_eq!(apply_structural_plan(contents, &model, &plan), contents);
}

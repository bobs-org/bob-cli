//! Unit tests for task status grouping.
use super::*;

fn grouped(input: &str) -> TransformOutput {
    transform(input, &TaskClassification::standard("#task"))
}

fn assert_idempotent(input: &str) {
    let once = grouped(input);
    let twice =
        transform(&once.contents, &TaskClassification::standard("#task"));
    assert_eq!(once.contents, twice.contents, "second pass changed bytes");
    assert!(!twice.changed, "second pass reported a change");
}

fn assert_unchanged_outside_tasks(input: &str, output: &str) {
    let input_prefix = input.split("## Tasks").next().unwrap_or(input);
    let output_prefix = output.split("## Tasks").next().unwrap_or(output);
    assert_eq!(input_prefix, output_prefix);
}

fn assert_text_order(contents: &str, needles: &[&str]) {
    let mut cursor = 0;
    for needle in needles {
        let offset = contents[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{contents}"));
        cursor += offset + needle.len();
    }
}

fn root_task_lines(contents: &str) -> Vec<String> {
    contents
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("- [") && trimmed.contains("#task")
        })
        .map(str::to_string)
        .collect()
}

const GOLDEN_INPUT: &str = "\
## Tasks

Short context for this project.

- [ ] #task An idea to pick up later ^idea
- [/] #task Finish the design ^design
  - Keep the keyboard interaction simple.
- [*] #task Review the implementation ^review
- [?] #task Ship when the dependency is ready ^ship
- [x] #task Agree on the scope ^scope
- [-] #task Superseded experiment ^experiment
";

const GOLDEN_OUTPUT: &str = "\
## Tasks
[`⚪ 1 open`](#Tasks) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 1 blocked`](#Tasks#Blocked) · [`🟢 2 done/canceled`](#Tasks#Done%20&%20Canceled)

Short context for this project.

- [ ] #task An idea to pick up later ^idea

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [/] #task Finish the design ^design
  - Keep the keyboard interaction simple.
- [*] #task Review the implementation ^review

### Blocked
<!-- bob:task-status-group:v1:blocked -->

- [?] #task Ship when the dependency is ready ^ship

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->

- [x] #task Agree on the scope ^scope
- [-] #task Superseded experiment ^experiment
";

#[test]
fn golden_layout_groups_every_status_bucket_and_keeps_ready_intake() {
    let fixture_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/task_status_groups");
    let fixture_in = std::fs::read_to_string(fixture_dir.join("layout.md"))
        .expect("golden input fixture");
    let fixture_out =
        std::fs::read_to_string(fixture_dir.join("layout.grouped.md"))
            .expect("golden output fixture");
    assert_eq!(fixture_in, GOLDEN_INPUT);
    assert_eq!(fixture_out, GOLDEN_OUTPUT);

    let result = grouped(GOLDEN_INPUT);
    assert_eq!(result.contents, GOLDEN_OUTPUT);
    assert!(result.changed);
    assert_eq!(result.grouped_sections.len(), 1);
    let section = &result.grouped_sections[0];
    assert_eq!(section.original_heading_line, 1);
    assert_eq!(section.heading_ancestry, ["Tasks"]);
    assert_eq!(section.open, 1);
    assert_eq!(section.next_and_in_progress, 2);
    assert_eq!(section.blocked, 1);
    assert_eq!(section.done_and_canceled, 2);
    assert_eq!(section.moved_block_count, 5);
    assert_eq!(
        section
            .moved_blocks
            .iter()
            .map(|block| (block.original_line, block.destination))
            .collect::<Vec<_>>(),
        [
            (6, DestinationLabel::NextAndInProgress),
            (8, DestinationLabel::NextAndInProgress),
            (9, DestinationLabel::Blocked),
            (10, DestinationLabel::DoneAndCanceled),
            (11, DestinationLabel::DoneAndCanceled),
        ]
    );
    assert_idempotent(GOLDEN_INPUT);
}

#[test]
fn badge_counts_refresh_when_membership_changes() {
    let grouped_once = grouped(GOLDEN_INPUT).contents;
    let reopened = grouped_once.replace(
        "- [*] #task Review the implementation ^review",
        "- [ ] #task Review the implementation ^review",
    );

    let result = grouped(&reopened);

    assert!(result.changed);
    assert!(result.contents.contains("[`⚪ 2 open`](#Tasks)"));
    assert!(result
        .contents
        .contains("[`🔵 1 next/wip`](#Tasks#Next%20&%20In%20Progress)"));
    let intake = result.contents.split("### Next").next().unwrap();
    assert!(intake.contains("- [ ] #task Review the implementation ^review"));
}

#[test]
fn authored_child_containers_get_independent_badges_and_anchors() {
    let input = "\
# Alpha

## Tasks

### Backend

- [*] #task API
- [ ] #task Later

### Frontend

- [?] #task UI
";
    let result = grouped(input);

    assert_eq!(result.grouped_sections.len(), 2);
    assert!(result.contents.contains(
            "[`⚪ 1 open`](#Alpha#Tasks#Backend) · [`🔵 1 next/wip`](#Alpha#Tasks#Backend#Next%20&%20In%20Progress)"
        ));
    assert!(result.contents.contains(
            "[`⚪ 0 open`](#Alpha#Tasks#Frontend) · [`🔵 0 next/wip`](#Alpha#Tasks#Frontend#Next%20&%20In%20Progress) · [`🔴 1 blocked`](#Alpha#Tasks#Frontend#Blocked)"
        ));
    assert_idempotent(input);
}

#[test]
fn is_badge_row_accepts_linked_unlinked_and_mixed_rows() {
    let linked = "[`⚪ 1 open`](#Tasks) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 3 done/canceled`](#Tasks#Done%20&%20Canceled)";
    let unlinked =
        "`⚪ 1 open` · `🔵 2 next/wip` · `🔴 0 blocked` · `🟢 3 done/canceled`";
    let mixed = "[`⚪ 1 open`](#Tasks) · `🔵 2 next/wip` · [`🔴 0 blocked`](#Tasks#Blocked) · `🟢 3 done/canceled`";
    assert!(is_badge_row(linked));
    assert!(is_badge_row(unlinked));
    assert!(is_badge_row(mixed));
    assert!(is_badge_row(&format!("{linked}   ")));
    assert!(is_badge_row(&format!("{linked}\r")));

    let wrong_order = "[`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`⚪ 1 open`](#Tasks) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 3 done/canceled`](#Tasks#Done%20&%20Canceled)";
    assert!(!is_badge_row(wrong_order));
    let wrong_label = "[`⚪ 1 opened`](#Tasks) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 3 done/canceled`](#Tasks#Done%20&%20Canceled)";
    assert!(!is_badge_row(wrong_label));
    let missing_chip = "[`⚪ 1 open`](#Tasks) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked)";
    assert!(!is_badge_row(missing_chip));
    let fifth_chip = format!("{linked} · `⚪ 1 open`");
    assert!(!is_badge_row(&fifth_chip));
    let non_digit = "[`⚪ x open`](#Tasks) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 3 done/canceled`](#Tasks#Done%20&%20Canceled)";
    assert!(!is_badge_row(non_digit));
    let anchor_space = "[`⚪ 1 open`](#Ta sks) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 3 done/canceled`](#Tasks#Done%20&%20Canceled)";
    assert!(!is_badge_row(anchor_space));
    let anchor_paren = "[`⚪ 1 open`](#a)b) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 3 done/canceled`](#Tasks#Done%20&%20Canceled)";
    assert!(!is_badge_row(anchor_paren));
    let anchor_missing_hash = "[`⚪ 1 open`](Tasks) · [`🔵 2 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 3 done/canceled`](#Tasks#Done%20&%20Canceled)";
    assert!(!is_badge_row(anchor_missing_hash));
    assert!(!is_badge_row(&format!("- {linked}")));
    assert!(!is_badge_row(&format!("Counts: {unlinked}")));
    assert!(!is_badge_row("<!-- bob:task-status-badges:v1 -->"));
    assert!(!is_badge_row("[`stale`](#Tasks)"));

    assert!(is_legacy_badge_marker("<!-- bob:task-status-badges:v1 -->"));
    assert!(is_legacy_badge_marker("<!-- bob:task-status-badges:v2 -->"));
    assert!(!is_legacy_badge_marker(linked));
    assert!(!is_legacy_badge_marker(
        "<!-- bob:task-status-group:v1:active -->"
    ));
}

#[test]
fn marker_free_output_emits_row_directly_under_tasks() {
    let result = grouped(GOLDEN_INPUT);
    assert!(!result.contents.contains("task-status-badges"));
    let mut lines = result.contents.lines();
    assert_eq!(lines.next(), Some("## Tasks"));
    let row = lines.next().expect("badge row directly under heading");
    assert!(is_badge_row(row));
    assert_idempotent(GOLDEN_INPUT);
}

#[test]
fn legacy_marker_migrates_to_a_marker_free_row_in_the_slot() {
    let stale = "[`⚪ 9 open`](#Tasks) · [`🔵 9 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 9 blocked`](#Tasks#Blocked) · [`🟢 9 done/canceled`](#Tasks#Done%20&%20Canceled)";
    let input = format!(
        "\
## Tasks
<!-- bob:task-status-badges:v1 -->
{stale}

- [*] #task next
"
    );
    let result = grouped(&input);

    assert!(result.changed);
    assert!(!result.contents.contains("task-status-badges"));
    assert!(!result.contents.contains("9 open"));
    assert!(result.contents.contains("[`⚪ 0 open`](#Tasks)"));
    let mut lines = result.contents.lines();
    assert_eq!(lines.next(), Some("## Tasks"));
    assert!(is_badge_row(lines.next().expect("row in the slot")));
    assert_idempotent(&result.contents);

    let crlf = input.replace('\n', "\r\n");
    let result = grouped(&crlf);
    assert!(!result.contents.contains("task-status-badges"));
    assert!(result.contents.contains("[`⚪ 0 open`](#Tasks)"));
    assert_idempotent(&crlf);
}

#[test]
fn legacy_marker_never_eats_following_prose() {
    let input = "\
## Tasks
<!-- bob:task-status-badges:v1 -->
Project context.

- [*] #task next
";
    let result = grouped(input);

    assert!(result.changed);
    assert!(!result.contents.contains("task-status-badges"));
    assert!(result.contents.contains("Project context."));
    assert!(result.contents.contains("[`⚪ 0 open`](#Tasks)"));
    assert_idempotent(&result.contents);
}

#[test]
fn duplicate_badge_rows_self_heal_into_the_slot() {
    let stale = "[`⚪ 9 open`](#Tasks) · [`🔵 9 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 9 blocked`](#Tasks#Blocked) · [`🟢 9 done/canceled`](#Tasks#Done%20&%20Canceled)";
    let input = format!(
        "\
## Tasks
{stale}

Intro.

{stale}

- [*] #task next
"
    );
    let result = grouped(&input);

    assert!(result.changed);
    assert!(result.warnings.is_empty());
    assert!(!result.contents.contains("9 open"));
    assert_eq!(
        result
            .contents
            .lines()
            .filter(|line| is_badge_row(line))
            .count(),
        1
    );
    let mut lines = result.contents.lines();
    assert_eq!(lines.next(), Some("## Tasks"));
    assert!(is_badge_row(lines.next().expect("single row in the slot")));
    assert_idempotent(&result.contents);
}

#[test]
fn badge_marker_in_intake_is_relocated_to_the_slot() {
    let stale = "[`⚪ 9 open`](#Tasks) · [`🔵 9 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 9 blocked`](#Tasks#Blocked) · [`🟢 9 done/canceled`](#Tasks#Done%20&%20Canceled)";
    let input = format!(
        "\
## Tasks

Intro.

- [ ] #task ready

<!-- bob:task-status-badges:v1 -->
{stale}

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [*] #task next

### Blocked
<!-- bob:task-status-group:v1:blocked -->

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->
"
    );
    let result = grouped(&input);

    assert!(result.changed);
    assert_text_order(
        &result.contents,
        &[
            "## Tasks",
            "[`⚪ 1 open`](#Tasks)",
            "Intro.",
            "- [ ] #task ready",
            "### Next & In Progress",
        ],
    );
    assert!(!result.contents.contains("9 open"));
    assert!(!result.contents.contains("task-status-badges"));
    assert_idempotent(&result.contents);
}

#[test]
fn badge_row_is_not_a_task_or_ambiguous_boundary() {
    let stale = "[`⚪ 9 open`](#Tasks) · [`🔵 9 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 9 blocked`](#Tasks#Blocked) · [`🟢 9 done/canceled`](#Tasks#Done%20&%20Canceled)";
    let input = format!(
        "\
## Tasks
<!-- bob:task-status-badges:v1 -->
{stale}

- [*] #task next
"
    );
    let result = grouped(&input);

    assert!(result.changed);
    assert!(result.warnings.is_empty());
    assert!(result.contents.contains("[`⚪ 0 open`](#Tasks)"));
    let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
    assert!(active.contains("- [*] #task next"));
}

#[test]
fn orphaned_badge_block_is_removed_without_creating_groups() {
    let row = "[`⚪ 1 open`](#Tasks) · [`🔵 0 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 0 done/canceled`](#Tasks#Done%20&%20Canceled)";
    let input = format!(
        "\
## Tasks
{row}

- [ ] #task ready
"
    );
    let result = grouped(&input);

    assert!(result.changed);
    assert_eq!(result.grouped_sections, Vec::new());
    assert!(!result.contents.contains("task-status-badges"));
    assert!(!result.contents.lines().any(is_badge_row));
    assert!(!result.contents.contains("Next & In Progress"));
    assert!(result.contents.contains("- [ ] #task ready"));

    let bare_marker = "\
## Tasks
<!-- bob:task-status-badges:v1 -->

- [ ] #task ready
";
    let result = grouped(bare_marker);
    assert!(result.changed);
    assert!(!result.contents.contains("task-status-badges"));
    assert!(result.contents.contains("- [ ] #task ready"));
}

#[test]
fn misplaced_badge_rows_fail_closed() {
    let row = "[`⚪ 1 open`](#Tasks) · [`🔵 1 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 0 done/canceled`](#Tasks#Done%20&%20Canceled)";
    let in_group = format!(
        "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

{row}

- [*] #task next

### Blocked
<!-- bob:task-status-group:v1:blocked -->

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->
"
    );
    let result = grouped(&in_group);
    assert_eq!(result.contents, in_group);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.code == GroupingSkipCode::MisplacedBadgeRow));

    let marker_in_group = "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

<!-- bob:task-status-badges:v1 -->

- [*] #task next

### Blocked
<!-- bob:task-status-group:v1:blocked -->

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->
";
    let result = grouped(marker_in_group);
    assert_eq!(result.contents, marker_in_group);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.code == GroupingSkipCode::MisplacedBadgeRow));
}

#[test]
fn prose_lookalike_is_preserved_as_intake_prose() {
    let input = "\
## Tasks

Counts: `⚪ 1 open`

- [*] #task next
";
    let result = grouped(input);

    assert!(result.changed);
    let intake = result.contents.split("### Next").next().unwrap();
    assert!(intake.contains("Counts: `⚪ 1 open`"));
    assert!(
        result
            .contents
            .lines()
            .filter(|line| is_badge_row(line))
            .count()
            >= 1
    );
    assert_idempotent(&result.contents);
}

#[test]
fn heading_hash_in_ancestry_renders_unlinked_badges() {
    let input = "\
# Alpha #1

## Tasks

- [*] #task next
";
    let result = grouped(input);
    let row = result
        .contents
        .lines()
        .find(|line| line.contains("next/wip"))
        .expect("badge row");

    assert!(row.contains("`⚪ 0 open`"));
    assert!(row.contains("`🔵 1 next/wip`"));
    assert!(!row.contains("]("));
}

#[test]
fn badge_anchors_percent_encode_the_obsidian_path_segments() {
    assert_eq!(
        encode_anchor_segment(r#"A B(50%) <tag> "quote" \ slash café &"#),
        "A%20B%2850%25%29%20%3Ctag%3E%20%22quote%22%20%5C%20slash%20café%20&"
    );

    let input = "\
# Alpha (50%)

## Tasks

- [*] #task next
";
    let result = grouped(input);
    assert!(result
        .contents
        .contains("#Alpha%20%2850%25%29#Tasks#Next%20&%20In%20Progress"));
}

#[test]
fn ready_only_and_empty_sections_are_not_decorated() {
    let ready = "## Tasks\n\nIntro.\n\n- [ ] #task later\n";
    let result = grouped(ready);
    assert!(!result.changed);
    assert_eq!(result.contents, ready);
    assert!(result.grouped_sections.is_empty());

    let empty = "## Tasks\n\nJust context.\n";
    let result = grouped(empty);
    assert!(!result.changed);
    assert_eq!(result.contents, empty);
}

#[test]
fn blockless_and_duplicate_ids_still_group() {
    let input = "\
## Tasks

- [*] #task First
- [*] #task Second ^dup
- [?] #task Third ^dup
";
    let result = grouped(input);
    assert!(result.contents.contains(TITLE_ACTIVE));
    assert!(result
        .contents
        .contains("- [*] #task First\n- [*] #task Second ^dup"));
    assert!(result.contents.contains("- [?] #task Third ^dup"));
    assert_idempotent(input);
}

#[test]
fn custom_terminal_statuses_and_registry_precedence() {
    let classification = TaskClassification::from_status_types(
        "#task",
        [
            (' ', "TODO"),
            ('*', "ON_HOLD"),
            ('/', "IN_PROGRESS"),
            ('?', "ON_HOLD"),
            ('x', "DONE"),
            ('D', "DONE"),
            ('C', "CANCELLED"),
            ('Q', "TODO"),
            ('N', "NON_TASK"),
            ('E', "EMPTY"),
        ],
        '*',
        '/',
        '?',
        ' ',
    );
    let input = "\
## Tasks

- [ ] #task ready
- [*] #task next
- [D] #task custom-done
- [C] #task custom-cancelled
- [Q] #task other-open
- [N] #task non-task
- [E] #task empty
- [Z] #task unknown
";
    let result = transform(input, &classification);
    assert!(result.contents.contains("- [D] #task custom-done"));
    assert!(result.contents.contains("- [C] #task custom-cancelled"));
    let closed = result.contents.split(TITLE_CLOSED).nth(1).unwrap();
    assert!(closed.contains("custom-done"));
    assert!(closed.contains("custom-cancelled"));
    assert!(!closed.contains("non-task"));
    assert!(!closed.contains("unknown"));
    assert!(result.contents.contains("- [Q] #task other-open"));
    assert!(result.contents.contains("- [N] #task non-task"));
    assert!(result.contents.contains("- [Z] #task unknown"));
}

#[test]
fn empty_global_filter_accepts_all_checkbox_tasks() {
    let classification = TaskClassification::standard("");
    let input = "## Tasks\n\n- [*] no tag needed\n- [ ] ready also\n";
    let result = transform(input, &classification);
    assert!(result.contents.contains(TITLE_ACTIVE));
    assert!(result.contents.contains("- [*] no tag needed"));
    assert!(result.contents.contains("- [ ] ready also"));
}

#[test]
fn global_filter_rejects_non_matching_lines() {
    let input = "## Tasks\n\n- [*] not filtered\n- [*] #task real\n";
    let result = grouped(input);
    assert!(result.contents.contains("- [*] not filtered"));
    assert!(result.contents.contains("- [*] #task real"));
    let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
    assert!(active.contains("#task real"));
    assert!(!active.contains("not filtered"));
}

#[test]
fn nested_children_travel_with_parent_status() {
    let input = "\
## Tasks

- [*] #task Parent
  - [x] #task Child done
    - note
  - [?] #task Child blocked

- [ ] #task Ready
";
    let result = grouped(input);
    let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
    let active = active.split("### ").next().unwrap();
    assert!(active.contains("- [*] #task Parent"));
    assert!(active.contains("- [x] #task Child done"));
    assert!(active.contains("- [?] #task Child blocked"));
    assert!(!result
        .contents
        .split(TITLE_CLOSED)
        .nth(1)
        .unwrap()
        .contains("Child done"));
    assert_eq!(result.grouped_sections[0].next_and_in_progress, 1);
    assert_eq!(result.grouped_sections[0].done_and_canceled, 0);
    assert_idempotent(input);
}

#[test]
fn tabs_internal_blanks_and_fences_stay_in_the_task_block() {
    let input = "\
## Tasks

\t- [*] #task Tabbed
\t\t- child

- [/] #task Fenced
  ```
  - [ ] #task inside fence
  ```
  still in item

- [?] #task After
";
    let result = grouped(input);
    let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
    assert!(active.contains("\t- [*] #task Tabbed"));
    assert!(active.contains("\t\t- child"));
    assert!(active.contains("```\n  - [ ] #task inside fence\n  ```"));
    assert!(active.contains("still in item"));
    assert_idempotent(input);
}

#[test]
fn multiple_tasks_headings_and_heading_syntax_variants() {
    let input = "\
# Outer

## Tasks ##

- [*] #task First

## Other

Unrelated.

### tasks

- [?] #task Second

Tasks
-----

- [x] #task Third
";
    let result = grouped(input);
    assert_eq!(result.grouped_sections.len(), 3);
    assert!(result.contents.contains("# Outer"));
    assert!(result.contents.contains("## Other\n\nUnrelated."));
    assert!(result.contents.contains("- [*] #task First"));
    assert!(result.contents.contains("- [?] #task Second"));
    assert!(result.contents.contains("- [x] #task Third"));
    assert_unchanged_outside_tasks(input, &result.contents);
    assert_idempotent(input);
}

#[test]
fn authored_topics_receive_local_groups() {
    let input = "\
## Tasks

### Backend

- [*] #task API
- [ ] #task later

### Frontend

- [?] #task UI
";
    let result = grouped(input);
    assert!(result.contents.contains("### Backend"));
    assert!(result.contents.contains("#### Next & In Progress"));
    assert!(result.contents.contains("### Frontend"));
    let backend = result.contents.split("### Backend").nth(1).unwrap();
    let backend = backend.split("### Frontend").next().unwrap();
    assert!(backend.contains("- [*] #task API"));
    assert!(backend.contains("- [ ] #task later"));
    assert!(!backend.contains("- [?] #task UI"));
    assert_eq!(result.grouped_sections.len(), 2);
    assert_idempotent(input);
}

#[test]
fn nested_tasks_is_processed_once() {
    let input = "\
## Tasks

- [*] #task Outer

### Tasks

- [?] #task Inner
";
    let result = grouped(input);
    assert!(result.contents.contains("- [*] #task Outer"));
    assert!(result.contents.contains("- [?] #task Inner"));
    let outer_record = result
        .grouped_sections
        .iter()
        .find(|section| section.heading_ancestry == ["Tasks"])
        .expect("outer");
    let inner_record = result
        .grouped_sections
        .iter()
        .find(|section| section.heading_ancestry == ["Tasks", "Tasks"])
        .expect("inner");
    assert_eq!(outer_record.next_and_in_progress, 1);
    assert_eq!(inner_record.blocked, 1);
    assert_idempotent(input);
}

#[test]
fn excluded_markdown_contexts_are_not_task_roots_or_headings() {
    let input = "\
---
title: note
---

## Tasks

- [*] #task Real

```md
## Tasks
- [*] #task Fenced
```

<!--
## Tasks
- [*] #task Comment
-->

> ## Tasks
> - [*] #task Quoted

    ## Tasks
    - [*] #task Indented
";
    let result = grouped(input);
    assert_eq!(result.grouped_sections.len(), 1);
    assert!(result.contents.contains("- [*] #task Fenced"));
    assert!(result.contents.contains("- [*] #task Comment"));
    assert!(result.contents.contains("- [*] #task Quoted"));
    assert!(result.contents.contains("- [*] #task Indented"));
    let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
    assert!(active.contains("- [*] #task Real"));
    assert!(!active.contains("Fenced"));
    assert!(!active.contains("Comment"));
    assert!(!active.contains("Quoted"));
    assert!(!active.contains("Indented"));
    assert_idempotent(input);
}

#[test]
fn h6_tasks_is_skipped_with_a_diagnostic() {
    let input = "###### Tasks\n\n- [*] #task Stuck\n";
    let result = grouped(input);
    assert!(!result.changed);
    assert_eq!(result.contents, input);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].code, GroupingSkipCode::H6Container);
}

#[test]
fn crlf_mixed_endings_unicode_and_missing_final_newline() {
    let input = "## Tasks\r\n\r\n- [*] #task café 日本語 ^id\n- [?] #task other\r\n- [x] #task done";
    let result = grouped(input);
    assert!(result.contents.contains("café 日本語 ^id"));
    assert!(result.contents.contains("\r\n"));
    assert!(result.contents.contains("[`⚪ 0 open`](#Tasks)"));
    assert!(!result.contents.contains("task-status-badges"));
    assert!(!result.contents.ends_with('\n') || input.ends_with('\n'));
    assert!(!result.contents.ends_with('\n'));
    assert!(result.contents.contains("- [*] #task café 日本語 ^id\n"));
    assert_idempotent(input);
}

#[test]
fn safe_legacy_adoption_completes_a_partial_set() {
    let input = "\
## Tasks

- [*] #task Next

### Blocked

- [?] #task Already
";
    let result = grouped(input);
    assert!(result.contents.contains(MARKER_ACTIVE));
    assert!(result.contents.contains(MARKER_BLOCKED));
    assert!(result.contents.contains(MARKER_CLOSED));
    assert!(result.contents.contains("- [?] #task Already"));
    assert_idempotent(input);
}

#[test]
fn unmarked_group_title_with_prose_is_a_collision() {
    let input = "\
## Tasks

- [*] #task Next

### Next & In Progress

This is my own notes section.

- some bullet
";
    let result = grouped(input);
    assert!(!result.changed);
    assert_eq!(result.contents, input);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.code == GroupingSkipCode::OwnershipCollision));
}

#[test]
fn prose_inside_managed_groups_is_preserved() {
    let input = "\
## Tasks

- [ ] #task ready

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

Focus on these first.

- [*] #task next
- [ ] #task reopened

### Blocked
<!-- bob:task-status-group:v1:blocked -->

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->
";
    let result = grouped(input);
    assert!(result.contents.contains("Focus on these first."));
    let intake = result.contents.split("### Next").next().unwrap();
    assert!(intake.contains("- [ ] #task ready"));
    assert!(intake.contains("- [ ] #task reopened"));
    assert!(!intake.contains("- [*] #task next"));
    assert_idempotent(&result.contents);
}

#[test]
fn malformed_and_duplicate_markers_fail_closed() {
    let malformed = "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:ACTIV -->

- [*] #task next
";
    let result = grouped(malformed);
    assert_eq!(result.contents, malformed);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.code == GroupingSkipCode::MalformedOwnership));

    let duplicate = "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [*] #task a

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [/] #task b
";
    let result = grouped(duplicate);
    assert_eq!(result.contents, duplicate);
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code
                == GroupingSkipCode::DuplicateGroupHeading)
    );

    let renamed = "\
## Tasks

### My WIP
<!-- bob:task-status-group:v1:active -->

- [*] #task next
";
    let result = grouped(renamed);
    assert_eq!(result.contents, renamed);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.code == GroupingSkipCode::RenamedMarkedHeading));
}

#[test]
fn authored_heading_inside_a_managed_group_fails_closed() {
    let input = "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [*] #task next

#### Notes
details
";
    let result = grouped(input);
    assert_eq!(result.contents, input);
    assert!(result.warnings.iter().any(
        |warning| warning.code == GroupingSkipCode::AuthoredHeadingInGroup
    ));
}

#[test]
fn empty_groups_are_retained_once_created() {
    let input = GOLDEN_OUTPUT
        .replace("- [?] #task Ship when the dependency is ready ^ship\n", "");
    let result = grouped(&input);
    assert!(result.contents.contains("[`🔴 0 blocked`](#Tasks#Blocked)"));
    assert!(result
        .contents
        .contains("### Blocked\n<!-- bob:task-status-group:v1:blocked -->"));
    assert_idempotent(&input);
}

#[test]
fn reopening_a_task_to_ready_appends_to_intake() {
    let input = "\
## Tasks

Intro.

- [ ] #task existing

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [ ] #task reopened
- [*] #task still-next

### Blocked
<!-- bob:task-status-group:v1:blocked -->

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->
";
    let result = grouped(input);
    let intake = result.contents.split("### Next").next().unwrap();
    assert!(intake.contains("Intro."));
    assert!(intake.contains("- [ ] #task existing"));
    assert!(intake.contains("- [ ] #task reopened"));
    let existing_at = intake.find("- [ ] #task existing").unwrap();
    let reopened_at = intake.find("- [ ] #task reopened").unwrap();
    assert!(existing_at < reopened_at);
    assert_idempotent(&result.contents);
}

#[test]
fn ordered_list_roots_and_nested_ordinary_items_are_reported() {
    let ordered = "## Tasks\n\n1. [*] #task numbered\n- [?] #task sibling\n";
    let result = grouped(ordered);
    assert!(result.contents.contains("1. [*] #task numbered"));
    assert!(result.warnings.iter().any(
        |warning| warning.code == GroupingSkipCode::UnsupportedOrderedList
    ));
    let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap_or("");
    assert!(!active.contains("numbered"));

    let nested =
        "## Tasks\n\n- context\n  - [*] #task nested\n- [?] #task root\n";
    let result = grouped(nested);
    assert!(result.contents.contains("  - [*] #task nested"));
    assert!(result.warnings.iter().any(
        |warning| warning.code == GroupingSkipCode::NestedUnderOrdinaryItem
    ));
}

#[test]
fn lazy_continuation_skips_the_container() {
    let input = "## Tasks\n\n- [*] #task title\nlazy continuation\n";
    let result = grouped(input);
    assert_eq!(result.contents, input);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.code == GroupingSkipCode::AmbiguousBoundary));
}

#[test]
fn standalone_prose_is_not_attached_to_a_task() {
    let input = "## Tasks\n\n- [*] #task title\n\nA query follows.\n";
    let result = grouped(input);
    assert!(result.changed);
    let intake = result.contents.split("### Next").next().unwrap();
    assert!(intake.contains("A query follows."));
    assert!(!intake.contains("- [*] #task title"));
    assert_idempotent(input);
}

#[test]
fn skip_codes_are_stable() {
    assert_eq!(GroupingSkipCode::H6Container.as_str(), "h6_container");
    assert_eq!(
        GroupingSkipCode::AmbiguousBoundary.as_str(),
        "ambiguous_boundary"
    );
    assert_eq!(
        GroupingSkipCode::MisplacedBadgeRow.as_str(),
        "misplaced_badge_row"
    );
}

#[test]
fn conservation_and_source_records() {
    let result = grouped(GOLDEN_INPUT);
    let input_tasks = root_task_lines(GOLDEN_INPUT);
    let output_tasks = root_task_lines(&result.contents);
    assert_eq!(input_tasks.len(), output_tasks.len());
    let mut input_sorted = input_tasks.clone();
    let mut output_sorted = output_tasks.clone();
    input_sorted.sort();
    output_sorted.sort();
    assert_eq!(input_sorted, output_sorted);
    assert_eq!(result.grouped_sections[0].original_heading_line, 1);
}

#[test]
fn out_of_scope_spans_are_byte_identical() {
    let input = "\
# Project

Keep this.

## Tasks

- [*] #task grouped

## Log

Do not touch.
";
    let result = grouped(input);
    assert!(result.contents.starts_with("# Project\n\nKeep this.\n\n"));
    assert!(result.contents.ends_with("## Log\n\nDo not touch.\n"));
    assert_idempotent(input);
}

#[test]
fn heading_only_first_setup_is_a_change() {
    let input = "\
## Tasks

### Blocked

- [?] #task already
";
    let result = grouped(input);
    assert!(result.changed);
    assert!(result.contents.contains(MARKER_ACTIVE));
    assert!(result.contents.contains(MARKER_CLOSED));
    assert!(
        result.grouped_sections[0].moved_block_count == 0
            || result.contents.contains("- [?] #task already")
    );
}

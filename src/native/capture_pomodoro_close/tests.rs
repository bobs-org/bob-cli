use chrono::{NaiveDate, NaiveDateTime};

use super::super::capture::parse_adjustment_range;
use super::ledger::*;

fn at(hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 9, 28)
        .expect("valid date")
        .and_hms_opt(hour, minute, 0)
        .expect("valid time")
}

fn note(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn close_at(contents: &str, hour: u32, minute: u32) -> LedgerClosePlan {
    let entry = find_running_pomodoro(contents).expect("running pomodoro");
    plan_ledger_close(contents, &entry, at(hour, minute))
}

fn worked_example() -> String {
    note(&[
        "## Pomodoros",
        "",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN",
        "\t- 🍅 [[bob#^capture-stop]]",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^capture-stop]]",
        "\t\t- Designed the `=x` grammar",
        "\t\t\t- chose `x` for done",
        "\t\t- Wrote the plan",
        "\t- [[bob#^web-capture]]#",
        "\t- ~~[[sase#^axe-restart]]~~",
        "\t\t- Restarted axe",
        "\t- quick note",
        "- [ ] () — SASE",
        "\t- [[sase#^recovery-panel]]",
    ])
}

fn worked_example_closed() -> String {
    note(&[
        "## Pomodoros",
        "",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN",
        "\t- 🍅 [[bob#^capture-stop]]",
        "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
        "\t- 🍅 [[bob#^capture-stop]]",
        "\t\t- Designed the `=x` grammar",
        "\t\t\t- chose `x` for done",
        "\t\t- Wrote the plan",
        "\t- ~~[[sase#^axe-restart]]~~",
        "\t\t- Restarted axe",
        "\t- quick note",
        "- [ ] () — CAPTURE",
        "\t- [[bob#^capture-stop]]",
        "\t- [[bob#^web-capture]]",
        "- [ ] () — SASE",
        "\t- [[sase#^recovery-panel]]",
    ])
}

fn timing_for(line: &str, hour: u32, minute: u32) -> CloseTiming {
    let range = parse_adjustment_range(line).expect("range");
    close_timing(&range, at(hour, minute))
}

#[test]
fn worked_example_ledger_is_byte_for_byte() {
    let plan = close_at(&worked_example(), 9, 37);
    assert_eq!(plan.contents, worked_example_closed());
    let timing = plan.timing.expect("timing");
    assert_eq!(timing.planned_start, 9 * 60 + 20);
    assert_eq!(timing.planned_end, 9 * 60 + 50);
    assert_eq!(timing.planned_duration, 30);
    assert_eq!(timing.closed_end, 9 * 60 + 40);
    assert_eq!(timing.closed_duration, 20);
    assert_eq!(timing.closed_at, 9 * 60 + 37);
    assert_eq!(timing.remaining_minutes, 13);
    assert_eq!(timing.decremented_minutes, 10);
    assert_eq!(
        plan.classified_links
            .iter()
            .map(|link| {
                (link.line, link.role, link.block_id.as_str(), link.carried)
            })
            .collect::<Vec<_>>(),
        vec![
            (6, LedgerLinkRole::Worked, "capture-stop", true),
            (10, LedgerLinkRole::Deferred, "web-capture", true),
            (11, LedgerLinkRole::Struck, "axe-restart", false),
        ]
    );
    assert_eq!(
        plan.carried_lines,
        vec![
            "\t- [[bob#^capture-stop]]".to_string(),
            "\t- [[bob#^web-capture]]".to_string(),
        ]
    );
    assert_eq!(plan.notes, vec!["quick note".to_string()]);
    let next = plan.next_pomodoro.expect("next");
    assert_eq!(next.line, 13);
    assert_eq!(next.name.as_deref(), Some("CAPTURE"));
    assert!(next.created);
    assert_eq!(
        plan.startable_targets
            .iter()
            .map(|target| target.block_id.as_str())
            .collect::<Vec<_>>(),
        vec!["capture-stop"]
    );
    assert!(plan.embedded_targets.is_empty());
    assert_eq!(plan.sub_bullet_range, 5..13);
    assert_eq!(
        plan.work_log_groups
            .iter()
            .map(|group| group.block_id.as_str())
            .collect::<Vec<_>>(),
        vec!["capture-stop", "axe-restart"]
    );
    assert_eq!(
        plan.work_log_groups[0].descendant_roots,
        vec![
            WorkLogNode {
                marker: "-".to_string(),
                body_text: "Designed the `=x` grammar".to_string(),
                children: vec![WorkLogNode {
                    marker: "-".to_string(),
                    body_text: "chose `x` for done".to_string(),
                    children: Vec::new(),
                    source_line: None,
                }],
                source_line: Some(7),
            },
            WorkLogNode {
                marker: "-".to_string(),
                body_text: "Wrote the plan".to_string(),
                children: Vec::new(),
                source_line: Some(9),
            },
        ]
    );
    assert_eq!(
        plan.work_log_groups[1].descendant_roots,
        vec![WorkLogNode {
            marker: "-".to_string(),
            body_text: "Restarted axe".to_string(),
            children: Vec::new(),
            source_line: Some(12),
        }]
    );
}

#[test]
fn no_decrement_when_fewer_than_five_minutes_remain() {
    let plan = close_at(&worked_example(), 9, 49);
    assert!(plan.contents.contains("(**0920-0950** [t:: 30m])"));
    let timing = plan.timing.expect("timing");
    assert_eq!(timing.remaining_minutes, 1);
    assert_eq!(timing.decremented_minutes, 0);
    assert_eq!(timing.new_range_text, None);
    assert_eq!(timing.closed_duration, 30);
}

#[test]
fn start_in_the_future_clamps_to_zero_minutes() {
    let timing = timing_for("- [ ] (**1000-1030** [t:: 30m]) — FUTURE", 9, 37);
    assert_eq!(timing.closed_duration, 0);
    assert_eq!(timing.closed_end, 10 * 60);
    assert_eq!(
        timing.new_range_text.as_deref(),
        Some("(**1000-1000** [t:: 0m])")
    );
}

#[test]
fn midnight_crossing_range_uses_signed_remaining() {
    let timing = timing_for("- [ ] (**2330-0030** [t:: 60m]) — LATE", 23, 50);
    assert_eq!(timing.remaining_minutes, 40);
    assert_eq!(timing.closed_duration, 20);
    assert_eq!(timing.closed_end, 23 * 60 + 50);
    assert_eq!(
        timing.new_range_text.as_deref(),
        Some("(**2330-2350** [t:: 20m])")
    );
}

#[test]
fn unnamed_empty_last_entry_creates_placeholder_and_stub() {
    let contents = note(&["## Pomodoros", "- [ ] (**0920-0950** [t:: 30m])"]);
    let plan = close_at(&contents, 9, 37);
    assert_eq!(
        plan.contents,
        note(&[
            "## Pomodoros",
            "- [x] (**0920-0940** [t:: 20m])",
            "- [ ] ()",
            "\t- ",
        ])
    );
    let next = plan.next_pomodoro.expect("next");
    assert!(next.created);
    assert_eq!(next.name, None);
    assert_eq!(next.line, 3);
}

#[test]
fn nothing_carried_with_later_entry_creates_nothing() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "- [ ] () — SASE",
    ]);
    let plan = close_at(&contents, 9, 37);
    assert_eq!(
        plan.contents,
        note(&[
            "## Pomodoros",
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "- [ ] () — SASE",
        ])
    );
    assert!(plan.carried_lines.is_empty());
    let next = plan.next_pomodoro.expect("next");
    assert!(!next.created);
    assert_eq!(next.name.as_deref(), Some("SASE"));
    assert_eq!(next.line, 3);
}

#[test]
fn deferred_lookalikes_are_not_removed() {
    let lookalikes = [
        "\t- ![[bob#^embedded]]#",
        "\t- ~~[[bob#^struck]]~~#",
        "\t- [[bob#^spaced]] #",
        "\t- [[bob#^double]]##",
        "\t- [[bob#^tagged]] #tag",
        "\t- prose [[bob#^mixed]]#",
    ];
    let mut lines =
        vec!["## Pomodoros", "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE"];
    lines.extend(lookalikes);
    let contents = note(&lines);
    let plan = close_at(&contents, 9, 37);
    assert!(plan.contents.contains("![[bob#^embedded]]#"));
    assert!(plan.contents.contains("~~[[bob#^struck]]~~#"));
    assert!(plan.contents.contains("[[bob#^spaced]] #"));
    assert!(plan.contents.contains("[[bob#^double]]##"));
    assert!(plan.contents.contains("[[bob#^tagged]] #tag"));
    assert!(plan.contents.contains("[[bob#^mixed]]#"));
    assert!(plan
        .classified_links
        .iter()
        .all(|link| link.role != LedgerLinkRole::Deferred));
}

#[test]
fn true_deferred_hash_is_removed_and_carried_without_hash() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^web-capture]]#",
    ]);
    let plan = close_at(&contents, 9, 37);
    assert!(!plan.contents.contains(
        "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n\t- [[bob#^web-capture]]#"
    ));
    assert!(plan.contents.contains("\t- [[bob#^web-capture]]\n"));
    assert_eq!(
        plan.carried_lines,
        vec!["\t- [[bob#^web-capture]]".to_string()]
    );
    assert_eq!(plan.classified_links[0].role, LedgerLinkRole::Deferred);
}

#[test]
fn struck_markers_collapse_and_embedded_drop() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- ~~[[bob#^unmarked]]~~",
        "\t- 🍅 ~~[[bob#^marked]]~~",
        "\t- 🍅 🍅 [[bob#^collapse]]",
        "\t- 🍅🍅 [[bob#^nospace]]",
        "\t- 🍅 ![[bob#^embed]]",
        "- [ ] () — LATER",
    ]);
    let plan = close_at(&contents, 9, 37);
    assert!(plan.contents.contains("\t- ~~[[bob#^unmarked]]~~"));
    assert!(plan.contents.contains("\t- 🍅 ~~[[bob#^marked]]~~"));
    assert!(plan.contents.contains("\t- 🍅 [[bob#^collapse]]"));
    assert!(!plan.contents.contains("🍅 🍅 [[bob#^collapse]]"));
    // No space between the pair: only the last 🍅 counts as a marker,
    // so the line is already canonical and keeps its bytes.
    assert!(plan.contents.contains("\t- 🍅🍅 [[bob#^nospace]]"));
    assert!(plan.contents.contains("\t- ![[bob#^embed]]"));
    assert!(!plan.contents.contains("🍅 ![[bob#^embed]]"));
    assert_eq!(
        plan.classified_links
            .iter()
            .map(|link| (link.role, link.block_id.as_str(), link.carried))
            .collect::<Vec<_>>(),
        vec![
            (LedgerLinkRole::Struck, "unmarked", false),
            (LedgerLinkRole::Struck, "marked", false),
            (LedgerLinkRole::Worked, "collapse", true),
            (LedgerLinkRole::Mentioned, "nospace", true),
            (LedgerLinkRole::Embedded, "embed", false),
        ]
    );
}

#[test]
fn nested_worked_on_links_keep_their_indent_when_carried() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^parent]]",
        "\t\t- [[bob#^child]]",
    ]);
    let plan = close_at(&contents, 9, 37);
    assert_eq!(
        plan.carried_lines,
        vec![
            "\t- [[bob#^parent]]".to_string(),
            "\t\t- [[bob#^child]]".to_string(),
        ]
    );
    assert!(plan.contents.contains("\t\t- [[bob#^child]]"));
}

#[test]
fn fenced_lines_are_untouched() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^keep]]",
        "```md",
        "\t- [[bob#^fenced]]#",
        "```",
        "- [ ] () — SASE",
    ]);
    let plan = close_at(&contents, 9, 37);
    assert!(plan.contents.contains("```md\n\t- [[bob#^fenced]]#\n```"));
    assert!(plan
        .classified_links
        .iter()
        .all(|link| link.block_id != "fenced"));
}

#[test]
fn range_cuts_at_a_blank_line() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^keep]]",
        "",
        "\t- [[bob#^after-blank]]#",
        "- [ ] () — SASE",
    ]);
    let plan = close_at(&contents, 9, 37);
    assert_eq!(plan.sub_bullet_range, 2..3);
    assert!(plan.contents.contains("\t- [[bob#^after-blank]]#"));
    assert!(plan
        .classified_links
        .iter()
        .all(|link| link.block_id != "after-blank"));
}

#[test]
fn deferred_line_leaves_orphaned_children() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "\t- [[bob#^keep]]",
        "\t- [[bob#^defer]]#",
        "\t\t- orphan note",
        "\t- [[bob#^other]]",
    ]);
    let plan = close_at(&contents, 9, 37);
    assert_eq!(
        plan.contents,
        note(&[
            "## Pomodoros",
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "\t- 🍅 [[bob#^keep]]",
            "\t\t- orphan note",
            "\t- 🍅 [[bob#^other]]",
            "- [ ] () — CAPTURE",
            "\t- [[bob#^keep]]",
            "\t- [[bob#^other]]",
            "\t- [[bob#^defer]]",
        ])
    );
}

#[test]
fn preserves_crlf() {
    let contents = worked_example().replace('\n', "\r\n");
    let plan = close_at(&contents, 9, 37);
    let expected = worked_example_closed().replace('\n', "\r\n");
    assert_eq!(plan.contents, expected);
    assert!(plan.contents.contains("\r\n"));
    assert!(!plan.contents.replace("\r\n", "").contains('\n'));
}

#[test]
fn preserves_missing_final_newline() {
    let mut contents = worked_example();
    assert!(contents.pop() == Some('\n'));
    let plan = close_at(&contents, 9, 37);
    let mut expected = worked_example_closed();
    assert!(expected.pop() == Some('\n'));
    assert_eq!(plan.contents, expected);
    assert!(!plan.contents.ends_with('\n'));
}

#[test]
fn multiple_open_timed_entries_are_an_error() {
    let contents = note(&[
        "## Pomodoros",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
        "- [ ] (**1000-1030** [t:: 30m]) — SASE",
    ]);
    let error = find_running_pomodoro(&contents).expect_err("multiple");
    match error {
        FindRunningError::Multiple(entries) => {
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| (entry.name.as_deref(), entry.line))
                    .collect::<Vec<_>>(),
                vec![(Some("CAPTURE"), 2), (Some("SASE"), 3)]
            );
        }
        other => panic!("expected multiple, got {other:?}"),
    }
}

#[test]
fn no_open_timed_entry_reports_next_placeholder() {
    let contents = note(&[
        "## Pomodoros",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN",
        "- [ ] () — CAPTURE",
    ]);
    let error = find_running_pomodoro(&contents).expect_err("none");
    match error {
        FindRunningError::NoneRunning {
            next_name,
            next_line,
        } => {
            assert_eq!(next_name.as_deref(), Some("CAPTURE"));
            assert_eq!(next_line, Some(3));
        }
        other => panic!("expected none running, got {other:?}"),
    }
}

#[test]
fn missing_section_is_an_error() {
    let contents = note(&["# Daily", "- [ ] (**0920-0950** [t:: 30m])"]);
    assert_eq!(
        find_running_pomodoro(&contents),
        Err(FindRunningError::NoSection)
    );
}

use super::*;
use chrono::NaiveDate;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
}

fn planned(outcome: LinkInsertionOutcome) -> LinkInsertionPlan {
    match outcome {
        LinkInsertionOutcome::Planned(plan) => plan,
        other => panic!("expected Planned, got {other:?}"),
    }
}

#[test]
fn pull_forward_entry_text_matches_vault_fixture() {
    // Byte-for-byte parity with the recorded example in sase.md, per the
    // plan's design doc.
    assert_eq!(
        pull_forward_entry_text("2026-09-01", "2026-08-25"),
        "_2026-09-01 → 2026-08-25_ — 🍅 pulled into today's Pomodoro"
    );
}

#[test]
fn sets_next_without_schedule_field() {
    let contents = "- [ ] Buy milk ^task1\n";
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    assert_eq!(plan.previous_status_symbol, ' ');
    assert_eq!(plan.new_status_symbol, '*');
    assert_eq!(plan.removed_scheduled, None);
    assert!(plan.schedule_log.is_none());
    assert_eq!(plan.content, "- [*] Buy milk ^task1\n");
}

#[test]
fn blocked_is_forced_to_next() {
    let contents = "- [?] Waiting ^task1\n";
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    assert_eq!(plan.previous_status_symbol, '?');
    assert_eq!(plan.new_status_symbol, '*');
    assert_eq!(plan.content, "- [*] Waiting ^task1\n");
}

#[test]
fn removes_single_future_scheduled_field() {
    let contents = "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n";
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    assert_eq!(plan.removed_scheduled.as_deref(), Some("2026-11-02"));
    assert_eq!(plan.content, "- [*] Ship it ^task1\n");
    assert!(plan.schedule_log.is_none());
}

#[test]
fn two_scheduled_fields_retire_nothing() {
    let contents = "- [ ] Ship it [scheduled:: 2026-11-02] [scheduled:: 2026-12-01] ^task1\n";
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    assert_eq!(plan.removed_scheduled, None);
    assert_eq!(plan.new_status_symbol, '*');
    assert!(plan.content.contains("[scheduled:: 2026-11-02]"));
    assert!(plan.content.contains("[scheduled:: 2026-12-01]"));
}

#[test]
fn past_or_today_schedule_retires_nothing() {
    let contents = "- [ ] Ship it [scheduled:: 2026-06-15] ^task1\n";
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    assert_eq!(plan.removed_scheduled, None);
    assert!(plan.content.contains("[scheduled:: 2026-06-15]"));
}

#[test]
fn writes_pull_forward_entry_reusing_existing_indentation() {
    let contents = concat!(
        "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01 → 2026-07-01* — some other reason\n",
    );
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    let schedule_log = plan.schedule_log.expect("schedule log");
    assert_eq!(schedule_log.reason, PULL_FORWARD_REASON);
    assert_eq!(
        schedule_log.lines,
        vec![
            "\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro"
                .to_string()
        ]
    );
    assert_eq!(
        plan.content,
        concat!(
            "- [*] Ship it ^task1\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro\n",
            "\t\t- *2026-08-01 → 2026-07-01* — some other reason\n",
        )
    );
}

#[test]
fn schedule_log_falls_back_to_marker_indent_plus_tab_with_no_existing_entries()
{
    let contents = concat!(
        "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
    );
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    let schedule_log = plan.schedule_log.expect("schedule log");
    assert_eq!(
        schedule_log.lines,
        vec![
            "\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro"
                .to_string()
        ]
    );
}

#[test]
fn no_schedule_log_marker_means_no_entry_even_when_field_removed() {
    let contents = "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\n";
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    assert_eq!(plan.removed_scheduled.as_deref(), Some("2026-11-02"));
    assert!(plan.schedule_log.is_none());
}

#[test]
fn preserves_crlf_line_endings() {
    let contents = "- [ ] Ship it [scheduled:: 2026-11-02] ^task1\r\n\t- 🗓️ **SCHEDULE LOG**\r\n";
    let plan = plan_task_next(contents, 0, date(2026, 6, 15)).expect("plan");
    assert_eq!(
        plan.content,
        "- [*] Ship it ^task1\r\n\t- 🗓️ **SCHEDULE LOG**\r\n\t\t- _2026-11-02 → 2026-06-15_ — 🍅 pulled into today's Pomodoro\r\n"
    );
}

#[test]
fn returns_none_for_non_task_or_out_of_range_lines() {
    assert!(plan_task_next("plain text\n", 0, date(2026, 6, 15)).is_none());
    assert!(plan_task_next("- [ ] Task\n", 5, date(2026, 6, 15)).is_none());
}

#[test]
fn implicit_insertion_targets_single_open_timed_entry() {
    let contents = "## Pomodoros\n- [ ] (0900-0930) — CURRENT\n";
    let plan = planned(
        plan_link_insertion(contents, "[[cash#^goog-exit]]", None)
            .expect("plan"),
    );
    assert!(!plan.already_linked);
    assert_eq!(plan.removed_links, 0);
    assert!(plan.has_changes);
    assert_eq!(
        plan.content,
        "## Pomodoros\n- [ ] (0900-0930) — CURRENT\n\t- [[cash#^goog-exit]]\n"
    );
}

#[test]
fn implicit_insertion_falls_back_to_first_open_entry_without_timed() {
    let contents = "## Pomodoros\n- [ ] () — PLANNED\n- [ ] () — LATER\n";
    let plan = planned(
        plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
    );
    assert_eq!(
        plan.content,
        "## Pomodoros\n- [ ] () — PLANNED\n\t- [[cash#^id]]\n- [ ] () — LATER\n"
    );
}

#[test]
fn errors_when_no_eligible_open_entry() {
    let contents = "## Pomodoros\n- [x] (0900-0930) — DONE\n";
    let error =
        plan_link_insertion(contents, "[[cash#^id]]", None).unwrap_err();
    assert_eq!(error, LinkPlanError::NoEligibleOpenEntry);
}

#[test]
fn errors_on_multiple_open_timed_entries() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — ONE\n",
        "- [ ] (0935-1005) — TWO\n",
    );
    let error =
        plan_link_insertion(contents, "[[cash#^id]]", None).unwrap_err();
    assert_eq!(error, LinkPlanError::MultipleOpenTimedEntries);
}

#[test]
fn errors_when_no_pomodoros_section() {
    let error =
        plan_link_insertion("# Day\n", "[[cash#^id]]", None).unwrap_err();
    assert_eq!(error, LinkPlanError::NoPomodorosSection);
}

#[test]
fn named_selection_targets_existing_open_entry() {
    let contents = "## Pomodoros\n- [ ] () — MEMORY\n- [ ] () — FOCUS\n";
    let plan = planned(
        plan_link_insertion(contents, "[[cash#^id]]", Some("memory"))
            .expect("plan"),
    );
    assert_eq!(plan.pomodoro_name.as_deref(), Some("MEMORY"));
    assert!(plan
        .content
        .contains("- [ ] () — MEMORY\n\t- [[cash#^id]]\n"));
}

#[test]
fn named_selection_reports_creation_needed() {
    let contents = "## Pomodoros\n- [ ] () — FOCUS\n";
    let outcome = plan_link_insertion(contents, "[[cash#^id]]", Some("memory"))
        .expect("plan");
    assert_eq!(
        outcome,
        LinkInsertionOutcome::NeedsPomodoroCreation {
            canonical_name: "MEMORY".to_string()
        }
    );
}

#[test]
fn invalid_pomodoro_name_is_rejected() {
    let contents = "## Pomodoros\n- [ ] () — FOCUS\n";
    let error = plan_link_insertion(contents, "[[cash#^id]]", Some("bad_id"))
        .unwrap_err();
    assert_eq!(error, LinkPlanError::InvalidPomodoroName);
}

#[test]
fn idempotent_insertion_skips_when_already_linked() {
    let contents =
        "## Pomodoros\n- [ ] (0900-0930) — CURRENT\n\t- [[cash#^id]]\n";
    let plan = planned(
        plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
    );
    assert!(plan.already_linked);
    assert!(!plan.has_changes);
    assert_eq!(plan.content, contents);
}

#[test]
fn insertion_removes_duplicate_from_later_open_entry() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — CURRENT\n",
        "- [ ] () — LATER\n",
        "\t- [[cash#^id]]\n",
    );
    let plan = planned(
        plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
    );
    assert!(!plan.already_linked);
    assert_eq!(plan.removed_links, 1);
    assert!(plan.has_changes);
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — CURRENT\n",
            "\t- [[cash#^id]]\n",
            "- [ ] () — LATER\n",
        )
    );
}

#[test]
fn removal_never_touches_completed_pomodoros() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] (0900-0930) — DONE\n",
        "\t- [[cash#^id]]\n",
        "- [ ] () — OPEN\n",
    );
    let plan = plan_link_removal(contents, "[[cash#^id]]");
    assert_eq!(plan.removed_links, 0);
    assert!(!plan.has_changes);
    assert_eq!(plan.content, contents);
}

#[test]
fn removal_deletes_whole_subtree_for_sole_content_bullet() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — OPEN\n",
        "\t- [[cash#^id]]\n",
        "\t\t- note about it\n",
        "- [ ] () — NEXT\n",
    );
    let plan = plan_link_removal(contents, "[[cash#^id]]");
    assert_eq!(plan.removed_links, 1);
    assert_eq!(
        plan.content,
        concat!("## Pomodoros\n", "- [ ] () — OPEN\n", "- [ ] () — NEXT\n",)
    );
}

#[test]
fn removal_only_strips_the_link_when_bullet_has_other_text() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — OPEN\n",
        "\t- see also [[cash#^id]] for context\n",
    );
    let plan = plan_link_removal(contents, "[[cash#^id]]");
    assert_eq!(plan.removed_links, 1);
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — OPEN\n",
            "\t- see also  for context\n",
        )
    );
}

#[test]
fn removal_reports_no_changes_when_nothing_matches() {
    let contents = "## Pomodoros\n- [ ] () — OPEN\n";
    let plan = plan_link_removal(contents, "[[cash#^id]]");
    assert_eq!(plan.removed_links, 0);
    assert!(!plan.has_changes);
    assert_eq!(plan.content, contents);
}

#[test]
fn link_operations_preserve_crlf() {
    let contents = "## Pomodoros\r\n- [ ] (0900-0930) — CURRENT\r\n";
    let plan = planned(
        plan_link_insertion(contents, "[[cash#^id]]", None).expect("plan"),
    );
    assert_eq!(
        plan.content,
        "## Pomodoros\r\n- [ ] (0900-0930) — CURRENT\r\n\t- [[cash#^id]]\r\n"
    );

    let removal = plan_link_removal(&plan.content, "[[cash#^id]]");
    assert_eq!(removal.content, contents);
    assert_eq!(removal.removed_links, 1);
}

fn relocated(contents: &str, block_link: &str) -> LinkRelocationPlan {
    plan_link_relocation(contents, block_link, None).expect("relocate")
}

fn relocated_named(
    contents: &str,
    block_link: &str,
    name: &str,
) -> LinkRelocationPlan {
    plan_link_relocation(contents, block_link, Some(name)).expect("relocate")
}

#[test]
fn relocation_moves_a_later_link_into_the_timed_current_pomodoro() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - keep\n",
        "- [ ] () — LATER\n",
        "  - [[cash#^id]]\n",
    );
    let plan = relocated(contents, "[[cash#^id]]");
    assert_eq!(plan.action, LinkRelocationAction::Moved);
    assert!(plan.has_changes);
    assert_eq!(plan.source.name.as_deref(), Some("LATER"));
    assert_eq!(plan.destination.name.as_deref(), Some("CURRENT"));
    assert_eq!(plan.destination.time_range.as_deref(), Some("0900-0930"));
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - keep\n",
            "  - [[cash#^id]]\n",
            "- [ ] () — LATER\n",
        )
    );
}

#[test]
fn relocation_moves_an_earlier_link_into_a_later_destination() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — EARLIER\n",
        "  - [[cash#^id]]\n",
        "- [ ] (**1330-1400**) — CURRENT\n",
        "  - context\n",
    );
    let plan = relocated(contents, "[[cash#^id]]");
    assert_eq!(plan.action, LinkRelocationAction::Moved);
    assert_eq!(plan.source.name.as_deref(), Some("EARLIER"));
    assert_eq!(plan.destination.name.as_deref(), Some("CURRENT"));
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — EARLIER\n",
            "- [ ] (**1330-1400**) — CURRENT\n",
            "  - context\n",
            "  - [[cash#^id]]\n",
        )
    );
}

#[test]
fn relocation_falls_back_to_the_first_open_entry_without_timed() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — PLANNED\n",
        "- [ ] () — LATER\n",
        "  - [[cash#^id]]\n",
    );
    let plan = relocated(contents, "[[cash#^id]]");
    assert_eq!(plan.destination.name.as_deref(), Some("PLANNED"));
    assert!(plan
        .content
        .contains("- [ ] () — PLANNED\n  - [[cash#^id]]\n"));
    assert!(plan.content.contains("- [ ] () — LATER\n"));
    assert!(!plan.content.contains("- [ ] () — LATER\n  - [[cash#^id]]"));
}

#[test]
fn relocation_reports_an_unnamed_endpoint() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**)\n",
        "- [ ] () — LATER\n",
        "  - [[cash#^id]]\n",
    );
    let plan = relocated(contents, "[[cash#^id]]");
    assert!(plan.destination.name.is_none());
    assert_eq!(plan.destination.time_range.as_deref(), Some("0900-0930"));
    assert_eq!(plan.destination.line, 2);
}

#[test]
fn relocation_moves_a_descendant_bearing_task_link_as_a_subtree() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "- [ ] () — LATER\n",
        "  - [[cash#^id]]\n",
        "    - review notes\n",
        "      - nested\n",
    );
    let plan = relocated(contents, "[[cash#^id]]");
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[cash#^id]]\n",
            "    - review notes\n",
            "      - nested\n",
            "- [ ] () — LATER\n",
        )
    );
}

#[test]
fn relocation_uses_the_destination_child_indentation() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "\t- existing\n",
        "- [ ] () — LATER\n",
        "  - [[cash#^id]]\n",
        "    - note\n",
    );
    let plan = relocated(contents, "[[cash#^id]]");
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "\t- existing\n",
            "\t- [[cash#^id]]\n",
            "\t  - note\n",
            "- [ ] () — LATER\n",
        )
    );
}

#[test]
fn relocation_preserves_crlf_and_a_missing_final_newline() {
    let contents =
        "## Pomodoros\r\n- [ ] (**0900-0930**) — CURRENT\r\n- [ ] () — LATER\r\n  - [[cash#^id]]";
    let plan = relocated(contents, "[[cash#^id]]");
    assert_eq!(
        plan.content,
        "## Pomodoros\r\n- [ ] (**0900-0930**) — CURRENT\r\n  - [[cash#^id]]\r\n- [ ] () — LATER\r\n"
    );
    assert_eq!(plan.placement, Some(LinkPlacement::Inserted));
}

#[test]
fn relocation_ignores_completed_history_and_mixed_text_lookalikes() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] (**0800-0830**) — DONE\n",
        "  - [[cash#^id]]\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - see [[cash#^id]] later\n",
        "  - context\n",
        "    - [[cash#^id]]\n",
    );
    let error =
        plan_link_relocation(contents, "[[cash#^id]]", None).unwrap_err();
    assert_eq!(error, LinkRelocationError::NoMovableLink);
}

#[test]
fn relocation_errors_when_the_link_is_missing() {
    let contents = "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n";
    assert_eq!(
        plan_link_relocation(contents, "[[cash#^id]]", None).unwrap_err(),
        LinkRelocationError::NoMovableLink
    );
}

#[test]
fn relocation_errors_on_duplicate_movable_links() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - [[cash#^id]]\n",
        "- [ ] () — LATER\n",
        "  - [[cash#^id]]\n",
    );
    assert_eq!(
        plan_link_relocation(contents, "[[cash#^id]]", None).unwrap_err(),
        LinkRelocationError::MultipleMovableLinks
    );
}

#[test]
fn relocation_is_a_noop_when_the_link_is_already_current() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - [[cash#^id]]\n",
        "    - notes\n",
        "- [ ] () — LATER\n",
    );
    let plan = relocated(contents, "[[cash#^id]]");
    assert_eq!(plan.action, LinkRelocationAction::AlreadyCurrent);
    assert!(!plan.has_changes);
    assert_eq!(plan.content, contents);
    assert_eq!(plan.source.name.as_deref(), Some("CURRENT"));
    assert_eq!(plan.destination.name.as_deref(), Some("CURRENT"));
    assert!(plan.placement.is_none());
}

#[test]
fn relocation_reuses_implicit_selection_errors() {
    assert_eq!(
        plan_link_relocation("# Day\n", "[[cash#^id]]", None).unwrap_err(),
        LinkRelocationError::NoPomodorosSection
    );
    assert_eq!(
        plan_link_relocation(
            "## Pomodoros\n- [x] (**0900-0930**) — DONE\n",
            "[[cash#^id]]",
            None
        )
        .unwrap_err(),
        LinkRelocationError::NoMovableLink
    );
    assert_eq!(
        plan_link_relocation(
            "## Pomodoros\n- [ ] (**0900-0930**) — A\n- [ ] (**0930-1000**) — B\n  - [[cash#^id]]\n",
            "[[cash#^id]]",
            None
        )
        .unwrap_err(),
        LinkRelocationError::MultipleOpenTimedEntries
    );
}

#[test]
fn named_relocation_moves_to_an_exact_open_match() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - [[cash#^id]]\n",
        "    - notes\n",
        "- [ ] () — DEEP+WORK\n",
        "  - keep\n",
    );
    let plan = relocated_named(contents, "[[cash#^id]]", "deep+work");
    assert_eq!(plan.action, LinkRelocationAction::Moved);
    assert!(!plan.creates_pomodoro);
    assert_eq!(plan.source.name.as_deref(), Some("CURRENT"));
    assert_eq!(plan.destination.name.as_deref(), Some("DEEP+WORK"));
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "- [ ] () — DEEP+WORK\n",
            "  - keep\n",
            "  - [[cash#^id]]\n",
            "    - notes\n",
        )
    );
}

#[test]
fn named_relocation_prefix_match_loses_to_a_whole_slug() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — CODE\n",
        "  - [[cash#^id]]\n",
        "- [ ] () — CODING\n",
    );
    let plan = relocated_named(contents, "[[cash#^id]]", "cod");
    assert_eq!(plan.destination.name.as_deref(), Some("CODE"));
    assert!(plan.content.contains("- [ ] () — CODE\n  - [[cash#^id]]\n"));
    assert!(!plan.content.contains("- [ ] () — CODING\n  - [[cash#^id]]"));

    let exact = relocated_named(contents, "[[cash#^id]]", "coding");
    assert_eq!(exact.destination.name.as_deref(), Some("CODING"));
    assert!(exact
        .content
        .contains("- [ ] () — CODING\n  - [[cash#^id]]\n"));
}

#[test]
fn named_relocation_is_a_noop_when_already_at_the_named_destination() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "- [ ] () — DEEP+WORK\n",
        "  - [[cash#^id]]\n",
        "    - notes\n",
    );
    let plan = relocated_named(contents, "[[cash#^id]]", "deep+work");
    assert_eq!(plan.action, LinkRelocationAction::AlreadyCurrent);
    assert!(!plan.has_changes);
    assert!(!plan.creates_pomodoro);
    assert_eq!(plan.content, contents);
    assert_eq!(plan.source.name.as_deref(), Some("DEEP+WORK"));
    assert_eq!(plan.destination.name.as_deref(), Some("DEEP+WORK"));
    assert!(plan.placement.is_none());
}

#[test]
fn named_relocation_creates_on_completed_only_and_missing_names() {
    let completed_only = concat!(
        "## Pomodoros\n",
        "- [x] () — BUGS\n",
        "  - old bug\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - [[cash#^id]]\n",
        "    - review\n",
        "- [ ] () — MEMORY\n",
    );
    let plan = relocated_named(completed_only, "[[cash#^id]]", "bugs");
    assert_eq!(plan.action, LinkRelocationAction::Moved);
    assert!(plan.creates_pomodoro);
    assert_eq!(plan.destination.name.as_deref(), Some("BUGS"));
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [x] () — BUGS\n",
            "  - old bug\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "- [ ] () — BUGS\n",
            "  - [[cash#^id]]\n",
            "    - review\n",
            "- [ ] () — MEMORY\n",
        )
    );
    assert_eq!(plan.destination.line, 5);

    let missing = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - [[cash#^id]]\n",
    );
    let created = relocated_named(missing, "[[cash#^id]]", "deep+work");
    assert!(created.creates_pomodoro);
    assert_eq!(created.destination.name.as_deref(), Some("DEEP+WORK"));
    assert_eq!(
        created.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "- [ ] () — DEEP+WORK\n",
            "  - [[cash#^id]]\n",
        )
    );
    assert_eq!(created.source.name.as_deref(), Some("CURRENT"));
    assert_eq!(created.destination.line, 3);
}

#[test]
fn named_relocation_inserts_before_the_first_future_entry() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — ALPHA\n",
        "  - [[cash#^id]]\n",
        "- [ ] () — BRAVO\n",
    );
    let plan = relocated_named(contents, "[[cash#^id]]", "zzzz");
    assert!(plan.creates_pomodoro);
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — ZZZZ\n",
            "  - [[cash#^id]]\n",
            "- [ ] () — ALPHA\n",
            "- [ ] () — BRAVO\n",
        )
    );
    assert_eq!(plan.destination.line, 2);
    assert_eq!(plan.source.name.as_deref(), Some("ALPHA"));
}

#[test]
fn named_relocation_selects_an_existing_name_despite_multiple_timed() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (0800-0830) — ONE\n",
        "  - [[cash#^id]]\n",
        "- [ ] (**0900-0930**) — TWO\n",
    );
    let plan = relocated_named(contents, "[[cash#^id]]", "two");
    assert!(!plan.creates_pomodoro);
    assert_eq!(plan.destination.name.as_deref(), Some("TWO"));
    assert!(plan
        .content
        .contains("- [ ] (**0900-0930**) — TWO\n  - [[cash#^id]]\n"));
}

#[test]
fn named_relocation_rejects_creation_with_multiple_timed_entries() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (0800-0830) — ONE\n",
        "  - [[cash#^id]]\n",
        "- [ ] (**0900-0930**) — TWO\n",
    );
    assert_eq!(
        plan_link_relocation(contents, "[[cash#^id]]", Some("three"))
            .unwrap_err(),
        LinkRelocationError::MultipleOpenTimedEntries
    );
}

#[test]
fn named_relocation_rejects_an_invalid_name() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - [[cash#^id]]\n",
    );
    assert_eq!(
        plan_link_relocation(contents, "[[cash#^id]]", Some("bad_id"))
            .unwrap_err(),
        LinkRelocationError::InvalidPomodoroName
    );
}

#[test]
fn named_relocation_preserves_descendants_and_destination_indent() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "\t- existing\n",
        "\t- [[cash#^id]]\n",
        "\t  - nested\n",
        "- [ ] () — LATER\n",
    );
    let plan = relocated_named(contents, "[[cash#^id]]", "later");
    assert_eq!(
        plan.content,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "\t- existing\n",
            "- [ ] () — LATER\n",
            "\t- [[cash#^id]]\n",
            "\t  - nested\n",
        )
    );
}

#[test]
fn named_relocation_preserves_crlf_when_creating() {
    let contents = "## Pomodoros\r\n- [x] Done\r\n\t- old child\r\n- [ ] (**0900-0930**) — CURRENT\r\n  - [[cash#^id]]\r\n";
    let plan = relocated_named(contents, "[[cash#^id]]", "crlf-name");
    assert!(plan.creates_pomodoro);
    assert_eq!(
        plan.content,
        "## Pomodoros\r\n- [x] Done\r\n\t- old child\r\n- [ ] (**0900-0930**) — CURRENT\r\n- [ ] () — CRLF-NAME\r\n\t- [[cash#^id]]\r\n"
    );
}

#[test]
fn named_relocation_same_location_noop_does_not_edit_bytes() {
    let contents = "## Pomodoros\n- [ ] () — DEEP+WORK\n  - [[cash#^id]]\n";
    let plan = relocated_named(contents, "[[cash#^id]]", "DEEP+WORK");
    assert_eq!(plan.action, LinkRelocationAction::AlreadyCurrent);
    assert_eq!(plan.content, contents);
}

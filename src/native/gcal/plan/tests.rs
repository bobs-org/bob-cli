use super::*;
use chrono::{NaiveDate, NaiveDateTime};

use crate::native::gcal::stamp;

const ACCOUNT: &str = "you@example.com";

fn note_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 30).expect("date")
}

fn now_at(hour: u32, minute: u32) -> NaiveDateTime {
    note_day().and_hms_opt(hour, minute, 0).expect("time")
}

fn stamp_line(hour: u32, minute: u32) -> String {
    stamp::render(ACCOUNT, now_at(hour, minute), note_day())
}

fn occurrence(
    event_id: &str,
    title: &str,
    task: bool,
    time_label: &str,
    start_min: i32,
) -> PlannedOccurrence {
    PlannedOccurrence {
        calendar_id: "primary".to_string(),
        event_id: event_id.to_string(),
        ical_uid: None,
        series_id: None,
        id_hint: title.to_string(),
        task,
        time_label: time_label.to_string(),
        title: title.to_string(),
        html_link: Some(format!(
            "https://calendar.google.com/event?eid={event_id}"
        )),
        join_link: None,
        location: None,
        root: None,
        source: None,
        qualifier: None,
        spans: BTreeMap::new(),
        needs_type: false,
        link_eligible: false,
        all_day: time_label == "all day",
        start_min,
    }
}

fn run(contents: &str, occurrences: Vec<PlannedOccurrence>) -> SyncPlan {
    run_at(contents, occurrences, ACCOUNT, now_at(12, 0), false)
}

fn run_at(
    contents: &str,
    occurrences: Vec<PlannedOccurrence>,
    account: &str,
    now: NaiveDateTime,
    is_today: bool,
) -> SyncPlan {
    plan_sync(&SyncPlanInput {
        contents,
        is_today,
        occurrences,
        journal: &BTreeMap::new(),
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account,
        now,
    })
}

#[test]
fn new_section_has_gap_below_heading_only() {
    let plan = run(
        "# Day\n\n## Pomodoros\n",
        vec![
            occurrence("holiday", "School holiday", false, "all day", 0),
            occurrence(
                "interview",
                "Product interview",
                true,
                "0900-0930",
                540,
            ),
        ],
    );

    let stamp = stamp_line(12, 0);
    assert_eq!(
        plan.contents,
        format!(
            concat!(
                "# Day\n",
                "## Events\n",
                "\n",
                "{stamp}\n",
                "- **all day** School holiday ",
                "[📅](https://calendar.google.com/event?eid=holiday \"Open in Google Calendar\")\n",
                "- [ ] #task **0900-0930** Product interview ",
                "[📅](https://calendar.google.com/event?eid=interview \"Open in Google Calendar\") ^product-interview\n",
                "\n",
                "## Pomodoros\n",
            ),
            stamp = stamp,
        )
    );
    assert!(plan.stamp_refreshed);
    assert_eq!(
        plan.stamp.as_ref().map(|stamp| stamp.account.as_str()),
        Some(ACCOUNT)
    );
}

#[test]
fn earlier_row_joins_a_well_formed_list_below_its_gap() {
    let plan = run(
        concat!(
            "## Events\n\n",
            "- [ ] #task **0900-0930** Existing [📅](https://calendar.google.com/event?eid=existing \"Open in Google Calendar\") ^existing\n",
            "\n## Pomodoros\n",
        ),
        vec![occurrence("earlier", "Earlier", true, "0800-0830", 480)],
    );

    let events = plan.contents.split("## Events\n").nth(1).expect("Events");
    let stamp = stamp_line(12, 0);
    assert!(
        events.starts_with(&format!(
            "\n{stamp}\n- [ ] #task **0800-0830** Earlier"
        )),
        "{events}"
    );
    assert!(
        events.contains("^earlier\n- [ ] #task **0900-0930** Existing"),
        "{events}"
    );
    assert!(events.contains("Existing [📅]"), "{events}");
    assert!(events.contains("Existing [📅](https://calendar.google.com/event?eid=existing \"Open in Google Calendar\") ^existing\n\n## Pomodoros"), "{events}");
}

#[test]
fn gap_only_repair_changes_contents_without_records() {
    let input = "## Events\n- [ ] #task **0900-0930** Existing ^existing\n\n## Pomodoros\n";
    let plan = run(input, Vec::new());
    let stamp = stamp_line(12, 0);

    assert!(plan.changed);
    assert_eq!(
        plan.contents,
        format!(
            "## Events\n\n{stamp}\n- [ ] #task **0900-0930** Existing ^existing\n\n## Pomodoros\n"
        )
    );
    assert!(plan.rows_added.is_empty());
    assert!(plan.rows_adopted.is_empty());
    assert!(plan.rows_gone.is_empty());
    assert!(plan.rows_removed.is_empty());
    assert!(plan.links_added.is_empty());
    assert!(plan.stamp_refreshed);
}

#[test]
fn existing_canonical_stamp_is_unchanged() {
    let stamp = stamp_line(12, 0);
    for contents in [
        format!("## Events\n\n{stamp}\n- row\n\n## Pomodoros\n"),
        format!("## Events\n\n{stamp}\n## Pomodoros\n"),
        format!("## Events\n\n\n{stamp}\n- row\n\n## Pomodoros\n"),
    ] {
        let plan = run(&contents, Vec::new());
        assert!(!plan.changed, "{contents:?}");
        assert_eq!(plan.contents, contents);
        assert!(!plan.stamp_refreshed);
        assert_eq!(
            plan.stamp.as_ref().map(|stamp| stamp.account.as_str()),
            Some(ACCOUNT)
        );
    }
}

#[test]
fn new_section_plan_is_idempotent() {
    let first = run(
        "# Day\n\n## Pomodoros\n",
        vec![occurrence("event", "Planning", true, "0900-0930", 540)],
    );
    let second = run(
        &first.contents,
        vec![occurrence("event", "Planning", true, "0900-0930", 540)],
    );

    assert_eq!(second.contents, first.contents);
    assert!(!second.stamp_refreshed);
}

#[test]
fn on_change_does_not_refresh_for_age() {
    let first = run(
        "# Day\n\n## Pomodoros\n",
        vec![occurrence("event", "Planning", true, "0900-0930", 540)],
    );
    let later = run_at(
        &first.contents,
        vec![occurrence("event", "Planning", true, "0900-0930", 540)],
        ACCOUNT,
        now_at(18, 0),
        false,
    );

    assert_eq!(later.contents, first.contents);
    assert!(!later.stamp_refreshed);
    assert!(
        later.contents.contains(&stamp_line(12, 0)),
        "{}",
        later.contents
    );
}

#[test]
fn future_stamp_is_rewritten() {
    let future = stamp_line(12, 1);
    let input = format!(
        "## Events\n\n{future}\n- [ ] #task **0900-0930** Existing ^existing\n\n## Pomodoros\n"
    );
    let plan = run(&input, Vec::new());
    let canonical = stamp_line(12, 0);
    assert!(plan.changed);
    assert!(plan.stamp_refreshed);
    assert!(plan.contents.contains(&canonical), "{}", plan.contents);
    assert!(!plan.contents.contains(&future), "{}", plan.contents);
}

#[test]
fn edited_misplaced_and_duplicated_stamps_normalize() {
    let canonical = stamp_line(12, 0);
    let edited = canonical.replace("12:00", "noon");
    let input = format!(
        "## Events\n\n- [ ] #task **0900-0930** Existing ^existing\n{edited}\n{canonical}\n\n## Pomodoros\n"
    );
    let plan = run(&input, Vec::new());
    let events = plan.contents.split("## Events\n").nth(1).expect("Events");
    assert!(
        events.starts_with(&format!(
            "\n{canonical}\n- [ ] #task **0900-0930** Existing"
        )),
        "{events}"
    );
    assert_eq!(events.matches("↻ synced").count(), 1, "{events}");
    assert!(plan.stamp_refreshed);
}

#[test]
fn new_earliest_row_lands_below_the_stamp() {
    let stamp = stamp_line(12, 0);
    let input = format!(
        concat!(
            "## Events\n\n",
            "{stamp}\n",
            "- [ ] #task **0900-0930** Existing [📅](https://calendar.google.com/event?eid=existing \"Open in Google Calendar\") ^existing\n",
            "\n## Pomodoros\n",
        ),
        stamp = stamp,
    );
    let plan = run(
        &input,
        vec![occurrence("earlier", "Earlier", true, "0800-0830", 480)],
    );
    let events = plan.contents.split("## Events\n").nth(1).expect("Events");
    assert!(
        events.contains(&format!("{stamp}\n- [ ] #task **0800-0830** Earlier")),
        "{events}"
    );
}

#[test]
fn stamp_above_a_paragraph_gets_a_blank_line() {
    let input = "## Events\n\nNotes from the day.\n\n## Pomodoros\n";
    let plan = run(input, Vec::new());
    let stamp = stamp_line(12, 0);
    assert_eq!(
        plan.contents,
        format!(
            "## Events\n\n{stamp}\n\nNotes from the day.\n\n## Pomodoros\n"
        )
    );
}

#[test]
fn pomodoro_entry_line_shifts_when_stamp_is_inserted() {
    let mut meeting = occurrence("meet", "Standup", true, "0900-0930", 540);
    meeting.link_eligible = true;
    let plan = run_at(
        "# Day\n\n## Pomodoros\n",
        vec![meeting],
        ACCOUNT,
        now_at(12, 0),
        true,
    );
    let meet_line = plan
        .contents
        .lines()
        .position(|line| line.contains("— MEET"))
        .expect("MEET entry")
        + 1;
    assert_eq!(plan.pomodoro_entry_line, Some(meet_line));
    assert!(
        plan.contents.contains(&stamp_line(12, 0)),
        "{}",
        plan.contents
    );
    assert!(plan.contents.contains("[[#^standup]]"), "{}", plan.contents);
}

#[test]
fn handwritten_section_gains_a_stamp() {
    let input = "## Events\n\n- leftover\n\n## Pomodoros\n";
    let plan = run(input, Vec::new());
    let stamp = stamp_line(12, 0);
    assert!(plan.changed);
    assert!(plan.stamp_refreshed);
    assert_eq!(
        plan.contents,
        format!("## Events\n\n{stamp}\n- leftover\n\n## Pomodoros\n")
    );
}

#[test]
fn no_section_and_no_events_gains_nothing() {
    let input = "# Day\n\n## Pomodoros\n";
    let plan = run(input, Vec::new());
    assert!(!plan.changed);
    assert_eq!(plan.contents, input);
    assert!(plan.stamp.is_none());
    assert!(!plan.stamp_refreshed);
}

#[test]
fn stamp_below_pomodoros_does_not_shift_meet_line() {
    let mut meeting = occurrence("meet", "Standup", true, "0900-0930", 540);
    meeting.link_eligible = true;
    let input = concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] () — MEET\n",
        "\n",
        "## Events\n",
        "\n",
        "- leftover\n",
    );
    let plan = run_at(input, vec![meeting], ACCOUNT, now_at(12, 0), true);
    let meet_line = plan
        .contents
        .lines()
        .position(|line| line.contains("— MEET"))
        .expect("MEET entry")
        + 1;
    assert_eq!(plan.pomodoro_entry_line, Some(meet_line));
    let events = plan.contents.split("## Events\n").nth(1).expect("Events");
    let stamp = stamp_line(12, 0);
    assert!(
        events.starts_with(&format!(
            "\n{stamp}\n- [*] #task **0900-0930** Standup"
        )),
        "{events}"
    );
    assert!(events.contains("- leftover\n"), "{events}");
}

#[test]
fn canonical_stamp_directly_above_prose_gains_one_separator() {
    let old = stamp_line(12, 0);
    let input = format!(
        "## Events\n\n{old}\nPersonal notes for the day.\n\n## Pomodoros\n"
    );
    let plan = run_at(&input, Vec::new(), ACCOUNT, now_at(12, 20), false);
    let fresh = stamp_line(12, 20);
    assert!(plan.changed, "{input:?}");
    assert!(plan.stamp_refreshed);
    assert_eq!(
        plan.contents,
        format!("## Events\n\n{fresh}\n\nPersonal notes for the day.\n\n## Pomodoros\n")
    );
    let idle =
        run_at(&plan.contents, Vec::new(), ACCOUNT, now_at(13, 0), false);
    assert!(!idle.changed, "{}", idle.contents);
    assert_eq!(idle.contents, plan.contents);
    assert!(!idle.stamp_refreshed);
}

#[test]
fn stamp_separator_is_reused_across_forced_refreshes() {
    for heading_blanks in ["\n\n", "\n\n\n"] {
        let future = stamp_line(12, 1);
        let input = format!("## Events{heading_blanks}{future}\n\nPersonal notes.\n\n## Pomodoros\n");
        let plan = run_at(&input, Vec::new(), ACCOUNT, now_at(12, 0), false);
        let canonical = stamp_line(12, 0);
        let expected =
            format!("## Events{heading_blanks}{canonical}\n\nPersonal notes.\n\n## Pomodoros\n");
        assert_eq!(
            plan.contents, expected,
            "heading_blanks={heading_blanks:?}"
        );
        assert!(plan.stamp_refreshed);
        let repeat =
            run_at(&expected, Vec::new(), ACCOUNT, now_at(12, 0), false);
        assert!(!repeat.changed, "{}", repeat.contents);
        assert_eq!(repeat.contents, expected);
        assert!(!repeat.stamp_refreshed);
        let earlier = stamp_line(11, 0);
        let forced =
            run_at(&expected, Vec::new(), ACCOUNT, now_at(11, 0), false);
        let expected_forced =
            format!("## Events{heading_blanks}{earlier}\n\nPersonal notes.\n\n## Pomodoros\n");
        assert_eq!(
            forced.contents, expected_forced,
            "heading_blanks={heading_blanks:?}"
        );
        assert!(forced.stamp_refreshed);
    }
}

#[test]
fn stamp_blank_before_list_is_preserved_on_refresh() {
    let future = stamp_line(12, 1);
    let input =
        format!("## Events\n\n{future}\n\n- leftover\n\n## Pomodoros\n");
    let plan = run_at(&input, Vec::new(), ACCOUNT, now_at(12, 0), false);
    let canonical = stamp_line(12, 0);
    assert_eq!(
        plan.contents,
        format!("## Events\n\n{canonical}\n\n- leftover\n\n## Pomodoros\n"),
        "{}",
        plan.contents
    );
    assert!(plan.stamp_refreshed);
}

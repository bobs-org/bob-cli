use super::*;
use chrono::NaiveDate;

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
    plan_sync(&SyncPlanInput {
        contents,
        is_today: false,
        occurrences,
        journal: &BTreeMap::new(),
        day_string: "2026-10-30".to_string(),
        today: NaiveDate::from_ymd_opt(2026, 10, 30).expect("date"),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
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

    assert_eq!(
        plan.contents,
        concat!(
            "# Day\n",
            "## Events\n",
            "\n",
            "- **all day** School holiday ",
            "[📅](https://calendar.google.com/event?eid=holiday \"Open in Google Calendar\")\n",
            "- [ ] #task **0900-0930** Product interview ",
            "[📅](https://calendar.google.com/event?eid=interview \"Open in Google Calendar\") ^product-interview\n",
            "\n",
            "## Pomodoros\n",
        )
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
    assert!(
        events.starts_with("\n- [ ] #task **0800-0830** Earlier"),
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

    assert!(plan.changed);
    assert_eq!(
        plan.contents,
        "## Events\n\n- [ ] #task **0900-0930** Existing ^existing\n\n## Pomodoros\n"
    );
    assert!(plan.rows_added.is_empty());
    assert!(plan.rows_adopted.is_empty());
    assert!(plan.rows_gone.is_empty());
    assert!(plan.rows_removed.is_empty());
    assert!(plan.links_added.is_empty());
}

#[test]
fn existing_gap_empty_section_and_extra_gaps_are_unchanged() {
    for contents in [
        "## Events\n\n- row\n\n## Pomodoros\n",
        "## Events\n## Pomodoros\n",
        "## Events\n\n## Pomodoros\n",
        "## Events\n\n\n- row\n\n## Pomodoros\n",
    ] {
        let plan = run(contents, Vec::new());
        assert!(!plan.changed, "{contents:?}");
        assert_eq!(plan.contents, contents);
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
}

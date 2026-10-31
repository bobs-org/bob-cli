use super::*;
use chrono::{FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use std::collections::{BTreeMap, HashSet};

use crate::native::gcal::{
    journal::{occurrence_key, span_hash, OccurrenceState, RowRecord},
    render, stamp,
};

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
    let html_link =
        Some(format!("https://calendar.google.com/event?eid={event_id}"));
    let badges = render::badge_text(html_link.as_deref(), None, None);
    let mut spans = BTreeMap::new();
    spans.insert("time".to_string(), span_hash(time_label));
    spans.insert("title".to_string(), span_hash(title));
    spans.insert("badges".to_string(), span_hash(&badges));
    PlannedOccurrence {
        calendar_id: "primary".to_string(),
        event_id: event_id.to_string(),
        ical_uid: None,
        series_id: None,
        id_hint: title.to_string(),
        task,
        time_label: time_label.to_string(),
        title: title.to_string(),
        html_link,
        join_link: None,
        location: None,
        root: None,
        source: None,
        qualifier: None,
        spans,
        needs_type: false,
        link_phase: None,
        all_day: time_label == "all day",
        start_min,
        end_min: end_min_from_label(time_label, start_min),
    }
}

fn end_min_from_label(time_label: &str, start_min: i32) -> i32 {
    if time_label.len() == 9 && time_label.as_bytes().get(4) == Some(&b'-') {
        let hour: i32 = time_label[5..7].parse().unwrap_or(0);
        let minute: i32 = time_label[7..9].parse().unwrap_or(0);
        let mut end = hour * 60 + minute;
        if end < start_min {
            end += 1440;
        }
        return end;
    }
    start_min
}

fn with_phase(
    mut occurrence: PlannedOccurrence,
    phase: LinkPhase,
) -> PlannedOccurrence {
    occurrence.link_phase = Some(phase);
    occurrence
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
    meeting.link_phase = Some(LinkPhase::Future);
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
    assert_eq!(plan.pomodoro_entry_line(), Some(meet_line));
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
    meeting.link_phase = Some(LinkPhase::Future);
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
    assert_eq!(plan.pomodoro_entry_line(), Some(meet_line));
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

fn empty_ledger() -> &'static str {
    "# Day\n\n## Pomodoros\n"
}

fn journal_task(
    event_id: &str,
    block_id: &str,
    url: Option<&str>,
    link: bool,
) -> ((String, String), OccurrenceState) {
    let ts = FixedOffset::east_opt(0)
        .expect("offset")
        .with_ymd_and_hms(2026, 10, 30, 12, 0, 0)
        .single()
        .expect("ts");
    (
        (
            occurrence_key("primary", event_id),
            "2026-10-30".to_string(),
        ),
        OccurrenceState {
            row: Some(RowRecord {
                block_id: block_id.to_string(),
                row_kind: "task".to_string(),
                spans: BTreeMap::new(),
                root: None,
                source: None,
                series_id: None,
                url: url.map(str::to_string),
                ts,
                host: "test".to_string(),
                path: "2026/20261030.md".to_string(),
            }),
            link,
            gone: false,
            flags: HashSet::new(),
        },
    )
}

#[test]
fn past_and_future_share_closed_then_placeholder() {
    let plan = run_at(
        empty_ledger(),
        vec![
            with_phase(
                occurrence("past", "Standup", true, "0800-0830", 480),
                LinkPhase::Past,
            ),
            with_phase(
                occurrence("later", "Dentist", true, "1830-1900", 1110),
                LinkPhase::Future,
            ),
        ],
        ACCOUNT,
        now_at(12, 0),
        true,
    );
    let pomodoros = plan.contents.split("## Pomodoros\n").nth(1).expect("P");
    assert!(
        pomodoros.contains("- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{pomodoros}"
    );
    assert!(pomodoros.contains("\t- 🍅 [[#^standup]]"), "{pomodoros}");
    assert!(pomodoros.contains("- [ ] () — MEET"), "{pomodoros}");
    assert!(pomodoros.contains("\t- [[#^dentist]]"), "{pomodoros}");
    let closed_at = pomodoros.find("(**0800-0830**").expect("closed");
    let open_at = pomodoros.find("- [ ] () — MEET").expect("placeholder");
    assert!(closed_at < open_at, "{pomodoros}");
    let standup = plan
        .contents
        .lines()
        .find(|line| line.contains("^standup"))
        .expect("standup");
    assert!(standup.starts_with("- [/] #task"), "{standup}");
    let dentist = plan
        .contents
        .lines()
        .find(|line| line.contains("^dentist"))
        .expect("dentist");
    assert!(dentist.starts_with("- [*] #task"), "{dentist}");
    assert_eq!(plan.links_added.len(), 2);
    assert_eq!(plan.pomodoro_entries.len(), 2);
    assert_eq!(plan.pomodoro_entries[0].state, MeetEntryState::Closed);
    assert_eq!(plan.pomodoro_entries[1].state, MeetEntryState::Planned);
}

#[test]
fn past_clusters_touching_meetings_and_splits_on_a_gap() {
    let plan = run_at(
        empty_ledger(),
        vec![
            with_phase(
                occurrence("a", "One", true, "0800-0830", 480),
                LinkPhase::Past,
            ),
            with_phase(
                occurrence("b", "Two", true, "0830-0900", 510),
                LinkPhase::Past,
            ),
            with_phase(
                occurrence("c", "Three", true, "1000-1030", 600),
                LinkPhase::Past,
            ),
        ],
        ACCOUNT,
        now_at(12, 0),
        true,
    );
    let pomodoros = plan.contents.split("## Pomodoros\n").nth(1).expect("P");
    assert!(
        pomodoros.contains("- [x] (**0800-0900** [t:: 60m]) — MEET"),
        "{pomodoros}"
    );
    assert!(
        pomodoros.contains("- [x] (**1000-1030** [t:: 30m]) — MEET"),
        "{pomodoros}"
    );
    let first = pomodoros.find("(**0800-0900**").expect("first");
    let second = pomodoros.find("(**1000-1030**").expect("second");
    assert!(first < second, "{pomodoros}");
    assert!(pomodoros.contains("\t- 🍅 [[#^one]]"), "{pomodoros}");
    assert!(pomodoros.contains("\t- 🍅 [[#^two]]"), "{pomodoros}");
    assert!(pomodoros.contains("\t- 🍅 [[#^three]]"), "{pomodoros}");
    assert_eq!(plan.pomodoro_entries.len(), 2);
}

#[test]
fn closed_entry_lands_between_completed_sessions() {
    let input = concat!(
        "# Day\n\n## Pomodoros\n",
        "- [x] (**0700-0730** [t:: 30m]) — WORK\n",
        "\t- [[#^early]]\n",
        "- [x] (**1100-1130** [t:: 30m]) — WORK\n",
        "\t- [[#^late]]\n",
        "- [ ] () — LATER\n",
    );
    let plan = run_at(
        input,
        vec![with_phase(
            occurrence("mid", "Standup", true, "0800-0830", 480),
            LinkPhase::Past,
        )],
        ACCOUNT,
        now_at(12, 0),
        true,
    );
    let pomodoros = plan.contents.split("## Pomodoros\n").nth(1).expect("P");
    let early = pomodoros.find("— WORK").expect("early");
    let meet = pomodoros
        .find("(**0800-0830** [t:: 30m]) — MEET")
        .expect("meet");
    let late = pomodoros.rfind("— WORK").expect("late");
    let later = pomodoros.find("— LATER").expect("later");
    assert!(early < meet && meet < late && late < later, "{pomodoros}");
}

#[test]
fn current_meeting_creates_or_reuses_running_meet() {
    let meeting = with_phase(
        occurrence("now", "Weekly sync", true, "1400-1500", 840),
        LinkPhase::Current,
    );
    let created = run_at(
        empty_ledger(),
        vec![meeting.clone()],
        ACCOUNT,
        now_at(14, 30),
        true,
    );
    assert!(
        created
            .contents
            .contains("- [ ] (**1400-1500** [t:: 60m]) — MEET"),
        "{}",
        created.contents
    );
    assert!(
        created.contents.contains("\t- [[#^weekly-sync]]"),
        "{}",
        created.contents
    );
    assert_eq!(created.pomodoro_entries.len(), 1);
    assert_eq!(created.pomodoro_entries[0].state, MeetEntryState::Running);
    assert!(created.pomodoro_created());

    let reused = run_at(
        concat!(
            "# Day\n\n## Pomodoros\n",
            "- [ ] (**1400-1500** [t:: 60m]) — MEET\n",
        ),
        vec![meeting],
        ACCOUNT,
        now_at(14, 30),
        true,
    );
    assert_eq!(reused.contents.matches("— MEET").count(), 1);
    assert!(
        reused.contents.contains("\t- [[#^weekly-sync]]"),
        "{}",
        reused.contents
    );
    assert!(!reused.pomodoro_created());
}

#[test]
fn current_during_running_work_uses_placeholder_and_warns() {
    let plan = run_at(
        concat!(
            "# Day\n\n## Pomodoros\n",
            "- [ ] (**1400-1500** [t:: 60m]) — WORK\n",
        ),
        vec![with_phase(
            occurrence("now", "Weekly sync", true, "1400-1500", 840),
            LinkPhase::Current,
        )],
        ACCOUNT,
        now_at(14, 30),
        true,
    );
    assert!(
        plan.contents.contains("- [ ] () — MEET"),
        "{}",
        plan.contents
    );
    assert!(
        plan.contents.contains("\t- [[#^weekly-sync]]"),
        "{}",
        plan.contents
    );
    assert_eq!(
        plan.contents
            .lines()
            .filter(|line| line.contains("(**") && line.contains("[ ]"))
            .count(),
        1,
        "no second open timed entry:\n{}",
        plan.contents
    );
    assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
    assert!(
        plan.warnings[0].contains("Weekly sync is running now, but WORK"),
        "{:?}",
        plan.warnings
    );
}

#[test]
fn one_open_never_leaves_two_open_meets() {
    let current_and_future = run_at(
        empty_ledger(),
        vec![
            with_phase(
                occurrence("now", "Weekly sync", true, "1400-1500", 840),
                LinkPhase::Current,
            ),
            with_phase(
                occurrence("later", "Dentist", true, "1830-1900", 1110),
                LinkPhase::Future,
            ),
        ],
        ACCOUNT,
        now_at(14, 30),
        true,
    );
    assert_eq!(
        current_and_future.contents.matches("— MEET").count(),
        1,
        "{}",
        current_and_future.contents
    );
    assert!(
        current_and_future
            .contents
            .contains("- [ ] (**1400-1500** [t:: 60m]) — MEET"),
        "{}",
        current_and_future.contents
    );
    assert!(
        current_and_future.contents.contains("[[#^weekly-sync]]"),
        "{}",
        current_and_future.contents
    );
    assert!(
        current_and_future.contents.contains("[[#^dentist]]"),
        "{}",
        current_and_future.contents
    );

    let with_placeholder = run_at(
        concat!("# Day\n\n## Pomodoros\n", "- [ ] () — MEET\n",),
        vec![with_phase(
            occurrence("now", "Weekly sync", true, "1400-1500", 840),
            LinkPhase::Current,
        )],
        ACCOUNT,
        now_at(14, 30),
        true,
    );
    assert_eq!(
        with_placeholder.contents.matches("— MEET").count(),
        1,
        "{}",
        with_placeholder.contents
    );
    assert!(
        with_placeholder.contents.contains("- [ ] () — MEET"),
        "{}",
        with_placeholder.contents
    );
    assert!(
        !with_placeholder.contents.contains("(**1400-1500**"),
        "{}",
        with_placeholder.contents
    );
}

#[test]
fn cross_midnight_past_meeting_wraps_end() {
    let plan = run_at(
        empty_ledger(),
        vec![with_phase(
            occurrence("late", "Late meeting", true, "2330-0030", 1410),
            LinkPhase::Past,
        )],
        ACCOUNT,
        now_at(12, 0),
        true,
    );
    assert!(
        plan.contents
            .contains("- [x] (**2330-0030** [t:: 60m]) — MEET"),
        "{}",
        plan.contents
    );
}

#[test]
fn existing_ended_row_gets_no_closed_entry() {
    let input = concat!(
        "## Events\n\n",
        "- [ ] #task **0800-0830** Standup [📅](https://calendar.google.com/event?eid=past \"Open in Google Calendar\") ^standup\n",
        "\n## Pomodoros\n",
    );
    let (key, state) = journal_task(
        "past",
        "standup",
        Some("https://calendar.google.com/event?eid=past"),
        false,
    );
    let mut journal = BTreeMap::new();
    journal.insert(key, state);
    let plan = plan_sync(&SyncPlanInput {
        contents: input,
        is_today: true,
        occurrences: vec![with_phase(
            occurrence("past", "Standup", true, "0800-0830", 480),
            LinkPhase::Past,
        )],
        journal: &journal,
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account: ACCOUNT,
        now: now_at(12, 0),
    });
    assert!(
        !plan.contents.contains("— MEET"),
        "journaled ended row must not mint a closed entry:\n{}",
        plan.contents
    );
    assert!(plan.links_added.is_empty());
}

#[test]
fn adopted_unended_row_still_gets_open_meet_link() {
    let input = concat!(
        "## Events\n\n",
        "- [ ] #task **1830-1900** Dentist [📅](https://calendar.google.com/event?eid=later \"Open in Google Calendar\") ^dentist\n",
        "\n## Pomodoros\n",
    );
    let plan = run_at(
        input,
        vec![with_phase(
            occurrence("later", "Dentist", true, "1830-1900", 1110),
            LinkPhase::Future,
        )],
        ACCOUNT,
        now_at(12, 0),
        true,
    );
    assert!(
        plan.contents.contains("- [ ] () — MEET"),
        "{}",
        plan.contents
    );
    assert!(plan.contents.contains("[[#^dentist]]"), "{}", plan.contents);
    assert_eq!(plan.rows_adopted.len(), 1);
}

#[test]
fn upgraded_ended_maybe_gets_closed_meet() {
    let occ = with_phase(
        occurrence("past", "Standup", true, "0800-0830", 480),
        LinkPhase::Past,
    );
    let input = concat!(
        "## Events\n\n",
        "- **0800-0830** Standup · *maybe* [📅](https://calendar.google.com/event?eid=past \"Open in Google Calendar\")\n",
        "\n## Pomodoros\n",
    );
    let ts = FixedOffset::east_opt(0)
        .expect("offset")
        .with_ymd_and_hms(2026, 10, 30, 12, 0, 0)
        .single()
        .expect("ts");
    let mut journal = BTreeMap::new();
    journal.insert(
        (occurrence_key("primary", "past"), "2026-10-30".to_string()),
        OccurrenceState {
            row: Some(RowRecord {
                block_id: String::new(),
                row_kind: "context".to_string(),
                spans: occ.spans.clone(),
                root: None,
                source: None,
                series_id: None,
                url: occ.html_link.clone(),
                ts,
                host: "test".to_string(),
                path: "2026/20261030.md".to_string(),
            }),
            link: false,
            gone: false,
            flags: HashSet::new(),
        },
    );
    let plan = plan_sync(&SyncPlanInput {
        contents: input,
        is_today: true,
        occurrences: vec![occ],
        journal: &journal,
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account: ACCOUNT,
        now: now_at(12, 0),
    });
    let standup = plan
        .contents
        .lines()
        .find(|line| line.contains("^standup"))
        .expect("standup");
    assert!(standup.starts_with("- [/] #task"), "{standup}");
    assert!(
        plan.contents
            .contains("- [x] (**0800-0830** [t:: 30m]) — MEET"),
        "{}",
        plan.contents
    );
    assert!(
        plan.contents.contains("\t- 🍅 [[#^standup]]"),
        "{}",
        plan.contents
    );
    assert_eq!(plan.rows_upgraded.len(), 1);
    assert_eq!(plan.links_added.len(), 1);
}

#[test]
fn replanning_output_with_journal_is_idempotent() {
    let first = run_at(
        empty_ledger(),
        vec![
            with_phase(
                occurrence("past", "Standup", true, "0800-0830", 480),
                LinkPhase::Past,
            ),
            with_phase(
                occurrence("later", "Dentist", true, "1830-1900", 1110),
                LinkPhase::Future,
            ),
        ],
        ACCOUNT,
        now_at(12, 0),
        true,
    );
    let mut journal = BTreeMap::new();
    for row in &first.rows_added {
        let linked = first.links_added.iter().any(|link| {
            link.calendar_id == row.calendar_id && link.event_id == row.event_id
        });
        let (key, mut state) = journal_task(
            &row.event_id,
            &row.block_id,
            row.url.as_deref(),
            linked,
        );
        if let Some(record) = state.row.as_mut() {
            record.spans.clone_from(&row.spans);
        }
        journal.insert(key, state);
    }
    let second = plan_sync(&SyncPlanInput {
        contents: &first.contents,
        is_today: true,
        occurrences: vec![
            with_phase(
                occurrence("past", "Standup", true, "0800-0830", 480),
                LinkPhase::Past,
            ),
            with_phase(
                occurrence("later", "Dentist", true, "1830-1900", 1110),
                LinkPhase::Future,
            ),
        ],
        journal: &journal,
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account: ACCOUNT,
        now: now_at(12, 0),
    });
    assert_eq!(second.contents, first.contents);
    assert!(!second.changed);
}

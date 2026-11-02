use super::*;
use chrono::{FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use std::collections::{BTreeMap, HashSet};

use crate::native::gcal::{
    description::bullets::{Bullet, DescriptionBullets},
    journal::{
        occurrence_key, span_hash, DescriptionState, OccurrenceState, RowRecord,
    },
    render,
    root::RootSourceKind,
    stamp,
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
    occurrence_off_clock(event_id, title, task, time_label, start_min, false)
}

fn occurrence_off_clock(
    event_id: &str,
    title: &str,
    task: bool,
    time_label: &str,
    start_min: i32,
    off_clock: bool,
) -> PlannedOccurrence {
    let html_link =
        Some(format!("https://calendar.google.com/event?eid={event_id}"));
    let buffer = crate::native::event_task::EventBuffer::default();
    let badges = render::badge_text(
        html_link.as_deref(),
        None,
        None,
        off_clock && task,
        buffer,
    );
    let mut spans = BTreeMap::new();
    spans.insert("time".to_string(), span_hash(time_label));
    spans.insert("title".to_string(), span_hash(title));
    spans.insert("badges".to_string(), span_hash(&badges));
    let end_min = end_min_from_label(time_label, start_min);
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
        root_kind: RootSourceKind::None,
        qualifier: None,
        spans,
        needs_type: false,
        buffer,
        off_clock,
        description: None,
        link_phase: None,
        all_day: time_label == "all day",
        start_min,
        end_min,
    }
}

/// Bullets for one occurrence, as `build_occurrences` renders them.
fn with_description(
    mut occurrence: PlannedOccurrence,
    bodies: &[&str],
) -> PlannedOccurrence {
    occurrence.description = Some(DescriptionBullets {
        bullets: bodies
            .iter()
            .map(|body| Bullet {
                depth: 1,
                body: (*body).to_string(),
            })
            .collect(),
    });
    occurrence
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

fn with_root(
    mut occurrence: PlannedOccurrence,
    root: &str,
    source: &str,
    kind: RootSourceKind,
    needs_type: bool,
) -> PlannedOccurrence {
    occurrence.root = Some(root.to_string());
    occurrence.source = Some(source.to_string());
    occurrence.root_kind = kind;
    occurrence.needs_type = needs_type;
    occurrence
}

fn run_with_journal(
    contents: &str,
    occurrences: Vec<PlannedOccurrence>,
    journal: &BTreeMap<(String, String), OccurrenceState>,
) -> SyncPlan {
    plan_sync(&SyncPlanInput {
        contents,
        is_today: false,
        occurrences,
        journal,
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account: ACCOUNT,
        now: now_at(12, 0),
    })
}

fn journal_from_added(
    plan: &SyncPlan,
) -> BTreeMap<(String, String), OccurrenceState> {
    let mut journal = BTreeMap::new();
    for row in &plan.rows_added {
        let (key, mut state) = journal_task(
            &row.event_id,
            &row.block_id,
            row.url.as_deref(),
            false,
        );
        if let Some(record) = state.row.as_mut() {
            record.spans.clone_from(&row.spans);
            record.root.clone_from(&row.root);
            record.source.clone_from(&row.source);
            record.series_id.clone_from(&row.series_id);
            record.row_kind.clone_from(&row.row_kind);
            record.description.clone_from(&row.description);
        }
        journal.insert(key, state);
    }
    journal
}

fn overlay_updated(
    mut journal: BTreeMap<(String, String), OccurrenceState>,
    plan: &SyncPlan,
) -> BTreeMap<(String, String), OccurrenceState> {
    for row in &plan.rows_updated {
        let key = (
            occurrence_key(&row.calendar_id, &row.event_id),
            "2026-10-30".to_string(),
        );
        if let Some(record) =
            journal.get_mut(&key).and_then(|state| state.row.as_mut())
        {
            record.spans.clone_from(&row.spans);
            record.root.clone_from(&row.root);
            record.source.clone_from(&row.source);
            record.description.clone_from(&row.description);
        }
    }
    for notice in &plan.kept {
        let key = (
            occurrence_key(&notice.calendar_id, &notice.event_id),
            "2026-10-30".to_string(),
        );
        if let Some(state) = journal.get_mut(&key) {
            state.flags.insert(notice.flag.clone());
        }
    }
    journal
}

fn row_line(contents: &str, block_id: &str) -> String {
    let needle = format!("^{block_id}");
    contents
        .lines()
        .find(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("no row {needle} in:\n{contents}"))
        .to_string()
}

fn seed_task(
    occ: PlannedOccurrence,
) -> (
    String,
    BTreeMap<(String, String), OccurrenceState>,
    PlannedOccurrence,
    String,
) {
    let first = run(empty_ledger(), vec![occ.clone()]);
    let block_id = first.rows_added[0].block_id.clone();
    let journal = journal_from_added(&first);
    (first.contents, journal, occ, block_id)
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
fn every_run_refreshes_an_older_stamp() {
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

    assert!(later.changed);
    assert!(later.stamp_refreshed);
    let before: Vec<&str> = first.contents.lines().collect();
    let after: Vec<&str> = later.contents.lines().collect();
    assert_eq!(before.len(), after.len());
    let diffs: Vec<(usize, &str, &str)> = before
        .iter()
        .zip(after.iter())
        .enumerate()
        .filter(|(_, (old, new))| old != new)
        .map(|(index, (old, new))| (index, *old, *new))
        .collect();
    assert_eq!(diffs.len(), 1, "{diffs:?}");
    assert_eq!(diffs[0].1, stamp_line(12, 0));
    assert_eq!(diffs[0].2, stamp_line(18, 0));
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
    let thirteen = stamp_line(13, 0);
    assert!(idle.changed, "{}", idle.contents);
    assert!(idle.stamp_refreshed);
    assert_eq!(
        idle.contents,
        format!(
            "## Events\n\n{thirteen}\n\nPersonal notes for the day.\n\n## Pomodoros\n"
        )
    );
    let same =
        run_at(&idle.contents, Vec::new(), ACCOUNT, now_at(13, 0), false);
    assert!(!same.changed, "{}", same.contents);
    assert_eq!(same.contents, idle.contents);
    assert!(!same.stamp_refreshed);
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

#[test]
fn stamp_only_refresh_leaves_meet_lines_below_events_unmoved() {
    let stamp = stamp_line(12, 0);
    let input = format!(
        concat!(
            "## Events\n\n",
            "{stamp}\n",
            "- [ ] #task **0900-0930** Standup [📅](https://calendar.google.com/event?eid=meet \"Open in Google Calendar\") ^standup\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] () — MEET\n",
            "\t- [[#^standup]]\n",
        ),
        stamp = stamp,
    );
    let meet_line = input
        .lines()
        .position(|line| line.contains("— MEET"))
        .expect("MEET")
        + 1;
    let later = run_at(&input, Vec::new(), ACCOUNT, now_at(18, 0), true);
    assert!(later.changed);
    assert!(later.stamp_refreshed);
    assert!(later.pomodoro_entries.is_empty());
    let later_meet = later
        .contents
        .lines()
        .position(|line| line.contains("— MEET"))
        .expect("MEET")
        + 1;
    assert_eq!(later_meet, meet_line);
    let before: Vec<&str> = input.lines().collect();
    let after: Vec<&str> = later.contents.lines().collect();
    assert_eq!(before.len(), after.len());
    let diffs: Vec<(usize, &str, &str)> = before
        .iter()
        .zip(after.iter())
        .enumerate()
        .filter(|(_, (old, new))| old != new)
        .map(|(index, (old, new))| (index, *old, *new))
        .collect();
    assert_eq!(diffs.len(), 1, "{diffs:?}");
    assert_eq!(diffs[0].2, stamp_line(18, 0));
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
                description: DescriptionState::Absent,
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
                description: DescriptionState::Absent,
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
fn new_task_row_carries_its_bullets_and_tracks_them() {
    let occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let plan = run(empty_ledger(), vec![occ]);
    assert_eq!(plan.rows_added.len(), 1);
    let row = row_line(&plan.contents, &plan.rows_added[0].block_id);
    assert!(row.contains("**0800-0815** Swings!"), "{row}");
    let lines: Vec<&str> = plan.contents.lines().collect();
    let row_index = lines
        .iter()
        .position(|line| line.contains("**0800-0815**"))
        .expect("row");
    assert_eq!(
        lines[row_index + 1],
        "\t- And retainers!",
        "{}",
        plan.contents
    );
    let DescriptionState::Tracked { hash, lines: count } =
        &plan.rows_added[0].description
    else {
        panic!(
            "insert records Tracked: {:?}",
            plan.rows_added[0].description
        );
    };
    assert_eq!(*count, 1);
    assert_eq!(
        *hash,
        crate::native::gcal::description::bullets::bullets_hash([(
            1,
            "- And retainers!"
        )])
    );
}

#[test]
fn empty_bullet_block_tracks_zero_lines_and_stays_put() {
    let mut occ = occurrence("standup", "Standup", true, "0800-0830", 480);
    occ.description = Some(DescriptionBullets {
        bullets: Vec::new(),
    });
    let plan = run(empty_ledger(), vec![occ]);
    assert_eq!(plan.rows_added.len(), 1);
    assert_eq!(
        plan.rows_added[0].description,
        DescriptionState::Tracked {
            hash: crate::native::gcal::description::bullets::bullets_hash(
                std::iter::empty()
            ),
            lines: 0,
        }
    );
    let journal = journal_from_added(&plan);
    let second = run_with_journal(
        &plan.contents,
        vec![{
            let mut occ =
                occurrence("standup", "Standup", true, "0800-0830", 480);
            occ.description = Some(DescriptionBullets {
                bullets: Vec::new(),
            });
            occ
        }],
        &journal,
    );
    assert_eq!(second.contents, plan.contents);
    assert!(!second.changed);
}

#[test]
fn context_rows_record_absent_and_write_no_bullets() {
    let occ = occurrence("klass", "Toddler class", false, "0915-1000", 555);
    let plan = run(empty_ledger(), vec![occ]);
    assert_eq!(plan.rows_added.len(), 1);
    assert_eq!(plan.rows_added[0].row_kind, "context");
    assert_eq!(plan.rows_added[0].description, DescriptionState::Absent);
    assert!(
        plan.contents
            .lines()
            .all(|line| !line.starts_with([' ', '\t'])),
        "no bullets for context rows:\n{}",
        plan.contents
    );
}

#[test]
fn upgrade_lands_bullets_above_existing_notes() {
    let occ = occurrence("klass", "Evening class", false, "2000-2100", 1200);
    let first = run(empty_ledger(), vec![occ.clone()]);
    assert_eq!(first.rows_added[0].row_kind, "context");
    let journal = journal_from_added(&first);
    // Bryan adds a note under the Maybe row before it goes Yes.
    let mut noted_lines: Vec<String> =
        first.contents.lines().map(str::to_string).collect();
    let row_at = noted_lines
        .iter()
        .position(|line| line.contains("Evening class"))
        .expect("context row");
    noted_lines.insert(row_at + 1, "  - my note".to_string());
    let noted = noted_lines.join("\n") + "\n";
    let yes = with_description(
        occurrence("klass", "Evening class", true, "2000-2100", 1200),
        &["- Stretch"],
    );
    let plan = run_with_journal(&noted, vec![yes], &journal);
    assert_eq!(plan.rows_upgraded.len(), 1);
    let lines: Vec<&str> = plan.contents.lines().collect();
    let row_index = lines
        .iter()
        .position(|line| {
            line.contains(&format!("^{}", plan.rows_upgraded[0].block_id))
        })
        .expect("upgraded row");
    assert_eq!(lines[row_index + 1], "  - Stretch", "{}", plan.contents);
    assert_eq!(lines[row_index + 2], "  - my note", "{}", plan.contents);
    let DescriptionState::Tracked { lines: count, .. } =
        &plan.rows_upgraded[0].description
    else {
        panic!("upgrade records Tracked");
    };
    assert_eq!(*count, 1);
}

#[test]
fn update_carries_the_description_record_forward() {
    let occ = with_description(
        occurrence("dentist", "Dentist", true, "1830-1900", 1110),
        &["- Bring insurance card"],
    );
    let first = run(empty_ledger(), vec![occ]);
    let journal = journal_from_added(&first);
    let recorded = first.rows_added[0].description.clone();
    // Google retitles the event: the appended row keeps the record.
    let renamed = with_description(
        occurrence("dentist", "Dentist!!", true, "1830-1900", 1110),
        &["- Bring insurance card"],
    );
    let plan = run_with_journal(&first.contents, vec![renamed], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].description, recorded);
    assert_eq!(plan.rows_updated[0].wrote_description_lines, 0);
}

fn with_empty_description(
    mut occurrence: PlannedOccurrence,
) -> PlannedOccurrence {
    occurrence.description = Some(DescriptionBullets {
        bullets: Vec::new(),
    });
    occurrence
}

fn with_bullets(
    mut occurrence: PlannedOccurrence,
    bullets: &[(u8, &str)],
) -> PlannedOccurrence {
    occurrence.description = Some(DescriptionBullets {
        bullets: bullets
            .iter()
            .map(|(depth, body)| Bullet {
                depth: *depth,
                body: (*body).to_string(),
            })
            .collect(),
    });
    occurrence
}

fn child_lines_under<'a>(contents: &'a str, block_id: &str) -> Vec<&'a str> {
    let needle = format!("^{block_id}");
    let lines: Vec<&str> = contents.lines().collect();
    let row = lines
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("no row {needle} in:\n{contents}"));
    let row_indent = lines[row]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .count();
    let mut children = Vec::new();
    for line in &lines[row + 1..] {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .count();
        if indent <= row_indent {
            break;
        }
        children.push(*line);
    }
    children
}

#[test]
fn google_edit_rewrites_the_block_in_place() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let updated = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let plan = run_with_journal(&first.contents, vec![updated], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].wrote_description_lines, 1);
    let children = child_lines_under(&plan.contents, &block_id);
    assert_eq!(children, vec!["\t- Bring the case"], "{}", plan.contents);
    assert!(
        !plan.contents.contains("And retainers!"),
        "{}",
        plan.contents
    );
    let DescriptionState::Tracked { lines: count, .. } =
        &plan.rows_updated[0].description
    else {
        panic!("rewritten block is Tracked");
    };
    assert_eq!(*count, 1);
}

#[test]
fn google_clearing_the_description_removes_the_block() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let cleared = with_empty_description(occurrence(
        "swings",
        "Swings!",
        true,
        "0800-0815",
        480,
    ));
    let plan = run_with_journal(&first.contents, vec![cleared], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert!(
        child_lines_under(&plan.contents, &block_id).is_empty(),
        "{}",
        plan.contents
    );
    assert_eq!(
        plan.rows_updated[0].description,
        DescriptionState::Tracked {
            hash: crate::native::gcal::description::bullets::bullets_hash(
                std::iter::empty()
            ),
            lines: 0,
        }
    );
}

#[test]
fn bryan_edit_is_kept_silently_then_reported_once() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let edited = first
        .contents
        .replace("\t- And retainers!", "\t- And retainers! (packed)");
    let same = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let silent = run_with_journal(&edited, vec![same], &journal);
    assert!(silent.kept.is_empty(), "{:?}", silent.kept);
    assert!(
        silent.contents.contains("\t- And retainers! (packed)"),
        "{}",
        silent.contents
    );
    assert!(!silent.changed || silent.rows_updated.is_empty());

    let google_changed = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let reported =
        run_with_journal(&edited, vec![google_changed.clone()], &journal);
    assert_eq!(reported.kept.len(), 1, "{:?}", reported.kept);
    assert_eq!(reported.kept[0].flag, "kept_description");
    assert!(
        reported.contents.contains("\t- And retainers! (packed)"),
        "{}",
        reported.contents
    );
    assert!(
        !reported.contents.contains("Bring the case"),
        "{}",
        reported.contents
    );

    let journal = overlay_updated(journal, &reported);
    let again =
        run_with_journal(&reported.contents, vec![google_changed], &journal);
    assert!(
        again
            .kept
            .iter()
            .all(|notice| notice.flag != "kept_description"),
        "{:?}",
        again.kept
    );
}

#[test]
fn deleted_block_is_never_re_added() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let deleted = first.contents.replace("\t- And retainers!\n", "");
    let same = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let plan = run_with_journal(&deleted, vec![same], &journal);
    assert!(
        child_lines_under(&plan.contents, &block_id).is_empty(),
        "{}",
        plan.contents
    );
    assert!(plan.kept.is_empty(), "{:?}", plan.kept);

    let changed = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let later = run_with_journal(&plan.contents, vec![changed], &journal);
    assert!(
        child_lines_under(&later.contents, &block_id).is_empty(),
        "{}",
        later.contents
    );
    assert_eq!(later.kept[0].flag, "kept_description");
}

#[test]
fn nested_note_hands_the_block_to_bryan() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let nested = first.contents.replace(
        "\t- And retainers!",
        "\t- And retainers!\n\t\t- asked about the case",
    );
    let changed = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let plan = run_with_journal(&nested, vec![changed], &journal);
    assert!(
        plan.contents.contains("\t- And retainers!"),
        "{}",
        plan.contents
    );
    assert!(
        plan.contents.contains("\t\t- asked about the case"),
        "{}",
        plan.contents
    );
    assert!(
        !plan.contents.contains("Bring the case"),
        "{}",
        plan.contents
    );
    assert_eq!(plan.kept[0].flag, "kept_description");
}

#[test]
fn sibling_notes_and_work_log_do_not_hide_the_block() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let row = row_line(&first.contents, &block_id);
    let with_siblings = first.contents.replace(
        &format!("{row}\n\t- And retainers!"),
        &format!(
            "{row}\n\t- 🛠️ **WORK LOG**\n\t\t- *12:00* — started\n\t- And retainers!\n\t- my note"
        ),
    );
    let changed = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let plan = run_with_journal(&with_siblings, vec![changed], &journal);
    let children = child_lines_under(&plan.contents, &block_id);
    assert!(
        children.contains(&"\t- Bring the case"),
        "{children:?}\n{}",
        plan.contents
    );
    assert!(children.contains(&"\t- my note"), "{children:?}");
    assert!(
        children.iter().any(|line| line.contains("WORK LOG")),
        "{children:?}"
    );
    assert!(
        !plan.contents.contains("And retainers!"),
        "{}",
        plan.contents
    );
}

#[test]
fn depends_on_above_the_block_does_not_hide_it() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let row = row_line(&first.contents, &block_id);
    let with_dep = first.contents.replace(
        &format!("{row}\n\t- And retainers!"),
        &format!(
            "{row}\n\t- ⛓️ **DEPENDS ON:** [[#^prep]]\n\t- And retainers!"
        ),
    );
    let changed = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let plan = run_with_journal(&with_dep, vec![changed], &journal);
    let children = child_lines_under(&plan.contents, &block_id);
    assert_eq!(
        children,
        vec!["\t- ⛓️ **DEPENDS ON:** [[#^prep]]", "\t- Bring the case",],
        "{}",
        plan.contents
    );
}

#[test]
fn time_change_reslots_the_row_with_its_bullets() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0900-0915", 540),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let moved = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let plan = run_with_journal(&first.contents, vec![moved], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].wrote_description_lines, 0);
    let lines: Vec<&str> = plan.contents.lines().collect();
    let row_at = lines
        .iter()
        .position(|line| line.contains("**0800-0815** Swings!"))
        .expect("moved row");
    assert_eq!(lines[row_at + 1], "\t- And retainers!", "{}", plan.contents);
}

#[test]
fn title_and_description_change_share_one_updated_row() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let both = with_description(
        occurrence("swings", "Swings!!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let plan = run_with_journal(&first.contents, vec![both], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].wrote_description_lines, 1);
    assert!(
        plan.contents.contains("**0800-0815** Swings!!"),
        "{}",
        plan.contents
    );
    assert!(
        plan.contents.contains("\t- Bring the case"),
        "{}",
        plan.contents
    );
}

#[test]
fn pre_feature_row_backfills_once() {
    let seed = occurrence("swings", "Swings!", true, "0800-0815", 480);
    let first = run(empty_ledger(), vec![seed]);
    assert_eq!(first.rows_added[0].description, DescriptionState::Absent);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let fresh = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let plan = run_with_journal(&first.contents, vec![fresh.clone()], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].wrote_description_lines, 1);
    assert_eq!(
        child_lines_under(&plan.contents, &block_id),
        vec!["\t- And retainers!"],
        "{}",
        plan.contents
    );
    let journal = overlay_updated(journal, &plan);
    let again = run_with_journal(&plan.contents, vec![fresh], &journal);
    assert!(!again.changed);
}

#[test]
fn absent_matching_window_is_adopted_without_writing() {
    let seed = occurrence("swings", "Swings!", true, "0800-0815", 480);
    let first = run(empty_ledger(), vec![seed]);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let row = row_line(&first.contents, &block_id);
    let copied = first
        .contents
        .replace(&row, &format!("{row}\n\t- And retainers!"));
    let fresh = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let plan = run_with_journal(&copied, vec![fresh], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].wrote_description_lines, 0);
    assert_eq!(
        copied.matches("And retainers!").count(),
        plan.contents.matches("And retainers!").count()
    );
    let DescriptionState::Tracked { lines: count, .. } =
        &plan.rows_updated[0].description
    else {
        panic!("adopted matching window is Tracked");
    };
    assert_eq!(*count, 1);
}

#[test]
fn absent_empty_description_appends_no_record() {
    let seed = occurrence("swings", "Swings!", true, "0800-0815", 480);
    let first = run(empty_ledger(), vec![seed.clone()]);
    let journal = journal_from_added(&first);
    let again = run_with_journal(&first.contents, vec![seed], &journal);
    assert!(!again.changed);
    assert!(again.rows_updated.is_empty());
}

#[test]
fn crash_adoption_records_matching_bullets_without_writing() {
    let occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![occ.clone()]);
    let adopted = run(&first.contents, vec![occ]);
    assert_eq!(adopted.rows_adopted.len(), 1);
    assert!(adopted.rows_added.is_empty());
    let DescriptionState::Tracked { lines: count, .. } =
        &adopted.rows_adopted[0].description
    else {
        panic!(
            "matching adoption is Tracked: {:?}",
            adopted.rows_adopted[0].description
        );
    };
    assert_eq!(*count, 1);
    assert_eq!(
        first.contents.matches("And retainers!").count(),
        adopted.contents.matches("And retainers!").count(),
        "{}",
        adopted.contents
    );
}

#[test]
fn crash_adoption_without_matching_window_is_hands_off() {
    let input = concat!(
        "## Events\n\n",
        "- [ ] #task **0800-0815** Swings! [📅](https://calendar.google.com/event?eid=swings \"Open in Google Calendar\") ^swings\n",
        "\n## Pomodoros\n",
    );
    let occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let plan = run(input, vec![occ]);
    assert_eq!(plan.rows_adopted.len(), 1);
    assert_eq!(plan.rows_adopted[0].description, DescriptionState::HandsOff);
    assert!(
        !plan.contents.contains("And retainers!"),
        "adoption never writes:\n{}",
        plan.contents
    );
}

#[test]
fn crash_adoption_of_empty_render_is_tracked() {
    let input = concat!(
        "## Events\n\n",
        "- [ ] #task **0800-0815** Swings! [📅](https://calendar.google.com/event?eid=swings \"Open in Google Calendar\") ^swings\n",
        "\n## Pomodoros\n",
    );
    let occ = with_empty_description(occurrence(
        "swings",
        "Swings!",
        true,
        "0800-0815",
        480,
    ));
    let plan = run(input, vec![occ]);
    assert_eq!(plan.rows_adopted.len(), 1);
    assert_eq!(
        plan.rows_adopted[0].description,
        DescriptionState::Tracked {
            hash: crate::native::gcal::description::bullets::bullets_hash(
                std::iter::empty()
            ),
            lines: 0,
        }
    );
}

#[test]
fn hands_off_journal_is_never_rewritten() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let mut journal = journal_from_added(&first);
    let key = (
        occurrence_key("primary", "swings"),
        "2026-10-30".to_string(),
    );
    if let Some(record) =
        journal.get_mut(&key).and_then(|state| state.row.as_mut())
    {
        record.description = DescriptionState::HandsOff;
    }
    let changed = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- Bring the case"],
    );
    let plan = run_with_journal(&first.contents, vec![changed], &journal);
    assert!(
        plan.contents.contains("\t- And retainers!"),
        "{}",
        plan.contents
    );
    assert!(
        !plan.contents.contains("Bring the case"),
        "{}",
        plan.contents
    );
    assert!(plan
        .kept
        .iter()
        .all(|notice| notice.flag != "kept_description"));
}

#[test]
fn off_mode_leaves_existing_bullets_and_carries_the_record() {
    let first_occ = with_description(
        occurrence("swings", "Swings!", true, "0800-0815", 480),
        &["- And retainers!"],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let recorded = first.rows_added[0].description.clone();
    // `None` is off mode: skip the block, carry the record on a title
    // update so a later bullets-mode run does not backfill twice.
    let mut renamed = occurrence("swings", "Swings!!", true, "0800-0815", 480);
    renamed.description = None;
    let plan = run_with_journal(&first.contents, vec![renamed], &journal);
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].description, recorded);
    assert!(
        plan.contents.contains("\t- And retainers!"),
        "{}",
        plan.contents
    );
}

#[test]
fn nested_google_list_rewrites_in_place() {
    let first_occ = with_bullets(
        occurrence("onsite", "Onsite", true, "1400-1500", 840),
        &[(1, "- **Agenda:**"), (2, "- Intros"), (1, "- Bring laptop")],
    );
    let first = run(empty_ledger(), vec![first_occ]);
    let journal = journal_from_added(&first);
    let block_id = first.rows_added[0].block_id.clone();
    let updated = with_bullets(
        occurrence("onsite", "Onsite", true, "1400-1500", 840),
        &[
            (1, "- **Agenda:**"),
            (2, "- Intros"),
            (2, "- System design"),
            (1, "- Bring laptop"),
        ],
    );
    let plan = run_with_journal(&first.contents, vec![updated], &journal);
    assert_eq!(
        child_lines_under(&plan.contents, &block_id),
        vec![
            "\t- **Agenda:**",
            "\t\t- Intros",
            "\t\t- System design",
            "\t- Bring laptop",
        ],
        "{}",
        plan.contents
    );
}

#[test]
fn overflow_url_bullet_is_never_adopted_as_a_row() {
    // Only an indented overflow bullet carries the event URL: without a
    // real row, sync must insert one, never adopt the bullet.
    let url = "https://calendar.google.com/event?eid=late";
    let input = format!(
        "## Events\n\n*stamp*\n\t- *…3 more lines in [Google Calendar]({url})*\n\n## Pomodoros\n"
    );
    let occ = with_description(
        occurrence("late", "Late meeting", true, "1900-1930", 1140),
        &["- *…3 more lines in [Google Calendar](https://example.test/x)*"],
    );
    let plan = run(&input, vec![occ]);
    assert_eq!(plan.rows_added.len(), 1, "{:?}", plan.rows_adopted);
    assert!(plan.rows_adopted.is_empty());
}

#[test]
fn bold_span_bullet_never_moves_the_insert_slot() {
    // A child bullet starting with `**HHMM-HHMM**` must not sort: the
    // 0830 row lands after the whole 0800 block, not mid-block.
    let input = concat!(
        "## Events\n\n",
        "*stamp*\n",
        "- [ ] #task **0800-0830** Standup ^standup\n",
        "\t- **0815-0830** Early recap\n",
        "\t- my note\n",
        "\n## Pomodoros\n",
    );
    let plan = run(
        input,
        vec![occurrence("dentist", "Dentist", true, "0830-0900", 510)],
    );
    assert_eq!(plan.rows_added.len(), 1);
    let lines: Vec<&str> = plan.contents.lines().collect();
    let note_at = lines
        .iter()
        .position(|line| *line == "\t- my note")
        .expect("note");
    let row_at = lines
        .iter()
        .position(|line| line.contains("^dentist"))
        .expect("new row");
    assert!(row_at > note_at, "{}", plan.contents);
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

#[test]
fn new_earlier_event_sorts_before_a_previously_journaled_link() {
    let late = with_phase(
        occurrence("lincoln", "Lincoln Park", true, "1900-1930", 1140),
        LinkPhase::Future,
    );
    let first = run_at(
        empty_ledger(),
        vec![late.clone()],
        ACCOUNT,
        now_at(6, 44),
        true,
    );
    let late_row = first.rows_added.first().expect("late row");
    let (key, mut state) = journal_task(
        &late_row.event_id,
        &late_row.block_id,
        late_row.url.as_deref(),
        true,
    );
    state
        .row
        .as_mut()
        .expect("journal row")
        .spans
        .clone_from(&late_row.spans);
    let journal = BTreeMap::from([(key, state)]);

    let early = with_phase(
        occurrence("swings", "Swings", true, "0800-0830", 480),
        LinkPhase::Future,
    );
    let plan = plan_sync(&SyncPlanInput {
        contents: &first.contents,
        is_today: true,
        occurrences: vec![late, early],
        journal: &journal,
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account: ACCOUNT,
        now: now_at(6, 44),
    });

    let pomodoros = plan.contents.split("## Pomodoros\n").nth(1).unwrap();
    assert!(
        pomodoros.find("[[#^swings]]") < pomodoros.find("[[#^lincoln-park]]"),
        "{pomodoros}"
    );
    assert_eq!(plan.links_added.len(), 1);
    assert_eq!(plan.links_sorted, 2);
}

#[test]
fn sort_only_plan_refreshes_stamp_and_is_idempotent() {
    let occurrences = vec![
        with_phase(
            occurrence("early", "Early", true, "0800-0830", 480),
            LinkPhase::Future,
        ),
        with_phase(
            occurrence("late", "Late", true, "1900-1930", 1140),
            LinkPhase::Future,
        ),
    ];
    let first = run_at(
        empty_ledger(),
        occurrences.clone(),
        ACCOUNT,
        now_at(6, 44),
        true,
    );
    let mut journal = BTreeMap::new();
    for row in &first.rows_added {
        let (key, mut state) = journal_task(
            &row.event_id,
            &row.block_id,
            row.url.as_deref(),
            true,
        );
        state
            .row
            .as_mut()
            .expect("journal row")
            .spans
            .clone_from(&row.spans);
        journal.insert(key, state);
    }
    let early = first
        .rows_added
        .iter()
        .find(|row| row.event_id == "early")
        .expect("early row");
    let late = first
        .rows_added
        .iter()
        .find(|row| row.event_id == "late")
        .expect("late row");
    let early_link = format!("\t- [[#^{}]]", early.block_id);
    let late_link = format!("\t- [[#^{}]]", late.block_id);
    let mut misordered = first.contents.clone();
    let ordered_pair = format!("{early_link}\n{late_link}");
    let misordered_pair = format!("{late_link}\n{early_link}");
    assert!(misordered.contains(&ordered_pair), "{}", misordered);
    misordered = misordered.replacen(&ordered_pair, &misordered_pair, 1);

    let sorted = plan_sync(&SyncPlanInput {
        contents: &misordered,
        is_today: true,
        occurrences: occurrences.clone(),
        journal: &journal,
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account: ACCOUNT,
        now: now_at(6, 44),
    });
    assert!(sorted.changed);
    assert!(sorted.stamp_refreshed);
    assert_eq!(sorted.links_sorted, 2);
    assert!(sorted.rows_added.is_empty());
    assert!(sorted.links_added.is_empty());

    let rerun = plan_sync(&SyncPlanInput {
        contents: &sorted.contents,
        is_today: true,
        occurrences,
        journal: &journal,
        day_string: "2026-10-30".to_string(),
        today: note_day(),
        lifecycle: Vec::new(),
        series_roots: &BTreeMap::new(),
        account: ACCOUNT,
        now: now_at(6, 44),
    });
    assert!(!rerun.changed);
    assert_eq!(rerun.contents, sorted.contents);
    assert_eq!(rerun.links_sorted, 0);
}

#[test]
fn unedited_root_reroutes_to_token_and_adds_meet_type() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    occ.root = Some("love".to_string());
    occ.source = Some("@love".to_string());
    occ.root_kind = RootSourceKind::Token;
    occ.needs_type = true;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: love]"), "{row}");
    assert!(row.contains("[type:: MEET]"), "{row}");
    assert!(
        row.contains("[root:: love] [type:: MEET]"),
        "theme first, then root: {row}"
    );
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].root.as_deref(), Some("love"));
    assert_eq!(plan.rows_updated[0].source.as_deref(), Some("@love"));
}

#[test]
fn unedited_root_reroute_skips_type_when_not_needed() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    occ.root = Some("love".to_string());
    occ.source = Some("@love".to_string());
    occ.root_kind = RootSourceKind::Token;
    occ.needs_type = false;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: love]"), "{row}");
    assert!(!row.contains("[type::]"), "{row}");
}

#[test]
fn existing_type_survives_a_reroute() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    let row = row_line(&contents, &block_id);
    let typed = row.replace("[root:: meet]", "[root:: meet] [type:: CHORE]");
    let contents = contents.replace(&row, &typed);
    occ.root = Some("love".to_string());
    occ.source = Some("@love".to_string());
    occ.root_kind = RootSourceKind::Token;
    occ.needs_type = true;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: love]"), "{row}");
    assert!(row.contains("[type:: CHORE]"), "{row}");
    assert!(!row.contains("[type:: MEET]"), "{row}");
}

#[test]
fn colour_calendar_and_default_reroute_unedited_roots() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    occ.root = Some("job".to_string());
    occ.source = Some("colour tomato".to_string());
    occ.root_kind = RootSourceKind::Colour;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: job]"), "{row}");
    assert_eq!(
        plan.rows_updated[0].source.as_deref(),
        Some("colour tomato")
    );
}

#[test]
fn done_row_still_reroutes() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    let row = row_line(&contents, &block_id);
    let done = row.replacen("- [ ]", "- [x]", 1);
    let contents = contents.replace(&row, &done);
    occ.root = Some("body".to_string());
    occ.source = Some("@body".to_string());
    occ.root_kind = RootSourceKind::Token;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.starts_with("- [x] #task"), "{row}");
    assert!(row.contains("[root:: body]"), "{row}");
}

#[test]
fn edited_root_is_kept_and_reported_once() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    let row = row_line(&contents, &block_id);
    let edited = row.replace("[root:: meet]", "[root:: job]");
    let contents = contents.replace(&row, &edited);
    occ.root = Some("love".to_string());
    occ.source = Some("@love".to_string());
    occ.root_kind = RootSourceKind::Token;
    let plan = run_with_journal(&contents, vec![occ.clone()], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: job]"), "{row}");
    assert!(!row.contains("[root:: love]"), "{row}");
    assert_eq!(plan.rows_updated.len(), 0);
    assert_eq!(plan.kept.len(), 1);
    assert_eq!(plan.kept[0].flag, "kept_root");
    assert!(
        plan.kept[0].detail.contains(
            "kept your [root:: job]; Google now routes to love (@love)"
        ),
        "{}",
        plan.kept[0].detail
    );

    let journal = overlay_updated(journal, &plan);
    let again = run_with_journal(&plan.contents, vec![occ], &journal);
    assert!(again.kept.is_empty(), "{:?}", again.kept);
    assert!(again.rows_updated.is_empty());
}

#[test]
fn series_memory_never_rewrites_an_existing_root() {
    let mut occ = with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    );
    occ.series_id = Some("series-standup".to_string());
    let (contents, journal, mut occ, block_id) = seed_task(occ);
    occ.root = Some("job".to_string());
    occ.source = Some("learned from your 2026-10-29 edit".to_string());
    occ.root_kind = RootSourceKind::SeriesMemory;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: meet]"), "{row}");
    assert!(plan.rows_updated.is_empty());
    assert!(plan.kept.is_empty());
}

#[test]
fn missing_fresh_root_never_removes_an_existing_root() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    occ.root = None;
    occ.source = None;
    occ.root_kind = RootSourceKind::None;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: meet]"), "{row}");
    assert!(plan.rows_updated.is_empty());
}

#[test]
fn kept_title_edit_still_reroutes() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    let row = row_line(&contents, &block_id);
    let edited = row.replace("Standup", "Standup!!");
    let contents = contents.replace(&row, &edited);
    occ.root = Some("love".to_string());
    occ.source = Some("@love".to_string());
    occ.root_kind = RootSourceKind::Token;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("Standup!!"), "{row}");
    assert!(row.contains("[root:: love]"), "{row}");
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].root.as_deref(), Some("love"));
    assert_eq!(
        plan.rows_updated[0].spans,
        journal
            .values()
            .next()
            .and_then(|state| state.row.as_ref())
            .map(|row| row.spans.clone())
            .expect("journaled spans")
    );
    assert_eq!(plan.kept.len(), 1);
    assert_eq!(plan.kept[0].flag, "kept_edit");
}

#[test]
fn edited_root_still_gets_a_time_update() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    let row = row_line(&contents, &block_id);
    let edited = row.replace("[root:: meet]", "[root:: job]");
    let contents = contents.replace(&row, &edited);
    occ.time_label = "1000-1030".to_string();
    occ.start_min = 600;
    occ.end_min = 630;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("**1000-1030**"), "{row}");
    assert!(row.contains("[root:: job]"), "{row}");
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].root.as_deref(), Some("meet"));
    assert!(plan.kept.is_empty());
}

#[test]
fn missing_root_gains_one() {
    let (contents, journal, mut occ, block_id) =
        seed_task(occurrence("standup", "Standup", true, "0900-0930", 540));
    let row = row_line(&contents, &block_id);
    assert!(!row.contains("[root::]"), "{row}");
    occ.root = Some("love".to_string());
    occ.source = Some("@love".to_string());
    occ.root_kind = RootSourceKind::Token;
    occ.needs_type = true;
    let plan = run_with_journal(&contents, vec![occ], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: love] [type:: MEET]"), "{row}");
    assert_eq!(plan.rows_updated.len(), 1);
    assert_eq!(plan.rows_updated[0].root.as_deref(), Some("love"));
}

#[test]
fn root_reroute_is_idempotent() {
    let (contents, journal, mut occ, block_id) = seed_task(with_root(
        occurrence("standup", "Standup", true, "0900-0930", 540),
        "meet",
        "default",
        RootSourceKind::Default,
        false,
    ));
    occ.root = Some("love".to_string());
    occ.source = Some("@love".to_string());
    occ.root_kind = RootSourceKind::Token;
    occ.needs_type = true;
    let plan = run_with_journal(&contents, vec![occ.clone()], &journal);
    let row = row_line(&plan.contents, &block_id);
    assert!(row.contains("[root:: love]"), "{row}");
    let journal = overlay_updated(journal, &plan);
    let again = run_with_journal(&plan.contents, vec![occ], &journal);
    assert!(!again.changed);
    assert!(again.rows_updated.is_empty());
}

//! Pure detector for a replaced daily-note Events section.
//!
//! A day's Events section counts as replaced when it had at least one
//! journaled, non-gone row, no such row is still in the note, and the
//! note has no recognized Events stamp (or no Events section at all).

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};

use super::{
    journal::{OccurrenceState, RowRecord},
    plan, stamp,
};
use crate::native::event_task;

type Journal = BTreeMap<(String, String), OccurrenceState>;

/// How many journaled rows vanished, and when the newest of them was
/// written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplacedSection {
    pub rows: usize,
    pub written_at: DateTime<FixedOffset>,
}

/// Detect a whole-section replacement from the note and journal.
pub(crate) fn detect(
    contents: &str,
    day: &str,
    journal: &Journal,
) -> Option<ReplacedSection> {
    let rows: Vec<&RowRecord> = journal
        .iter()
        .filter(|((_, row_day), state)| {
            row_day.as_str() == day && state.row.is_some() && !state.gone
        })
        .filter_map(|(_, state)| state.row.as_ref())
        .collect();
    if rows.is_empty() {
        return None;
    }

    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let events = event_task::events_section_range(&refs);
    if rows
        .iter()
        .any(|row| row_present(&lines, events.clone(), row))
    {
        return None;
    }
    let has_stamp = events.as_ref().is_some_and(|range| {
        !stamp::recognize(&refs, range.clone()).is_empty()
    });
    if has_stamp {
        return None;
    }

    Some(ReplacedSection {
        rows: rows.len(),
        written_at: rows.iter().map(|row| row.ts).max()?,
    })
}

/// A task matches its block id or `📅` URL; a context row matches its
/// URL. Inside an Events section, a context row with neither locator
/// cannot be seen, so it counts as present: this never holds on what
/// it cannot decide. A missing Events section means every row is gone.
fn row_present(
    lines: &[String],
    events: Option<std::ops::Range<usize>>,
    row: &RowRecord,
) -> bool {
    let task = row.row_kind == "task";
    let url = row.url.as_deref();
    if events.is_some()
        && !task
        && row.block_id.is_empty()
        && url.is_none_or(str::is_empty)
    {
        return true;
    }
    plan::find_row_line(lines, events, &row.block_id, url, task, None).is_some()
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};

    use super::*;
    use crate::native::gcal::journal::{occurrence_key, DescriptionState};

    const DAY: &str = "2026-10-30";
    const TASK_URL: &str = "https://calendar.google.com/event?eid=interview";
    const CONTEXT_URL: &str = "https://calendar.google.com/event?eid=class";
    const STAMP: &str = "*↻ synced 12:00 · [you@example.com](\
https://calendar.google.com/calendar/r/day/2026/10/30\
?authuser=you%40example.com)*";
    const TASK_LINE: &str = "- [x] #task **0800-0830** Product interview \
[📅](https://calendar.google.com/event?eid=interview) ^product-interview";
    const CONTEXT_URL_LINE: &str =
        "- **0915-1000** Toddler class [📅](https://calendar.google.com/event?eid=class)";
    const CONTEXT_PLAIN_LINE: &str = "- **all day** School holiday";

    fn ts(hour: u32, minute: u32) -> DateTime<FixedOffset> {
        FixedOffset::west_opt(4 * 3600)
            .expect("offset")
            .with_ymd_and_hms(2026, 10, 30, hour, minute, 0)
            .single()
            .expect("ts")
    }

    fn row(
        block_id: &str,
        kind: &str,
        url: Option<&str>,
        hour: u32,
        minute: u32,
    ) -> RowRecord {
        RowRecord {
            block_id: block_id.to_string(),
            row_kind: kind.to_string(),
            spans: BTreeMap::new(),
            root: None,
            source: None,
            series_id: None,
            url: url.map(str::to_string),
            description: DescriptionState::Absent,
            note: None,
            ts: ts(hour, minute),
            host: "test".to_string(),
            path: "2026/20261030.md".to_string(),
        }
    }

    fn live(record: RowRecord) -> OccurrenceState {
        OccurrenceState {
            row: Some(record),
            ..OccurrenceState::default()
        }
    }

    fn gone(record: RowRecord) -> OccurrenceState {
        OccurrenceState {
            row: Some(record),
            gone: true,
            ..OccurrenceState::default()
        }
    }

    fn journal(entries: &[(&str, &str, OccurrenceState)]) -> Journal {
        entries
            .iter()
            .map(|(event_id, day, state)| {
                (
                    (occurrence_key("primary", event_id), (*day).to_string()),
                    state.clone(),
                )
            })
            .collect()
    }

    fn note(events_body: &str) -> String {
        format!("# Friday 2026-10-30\n## Events\n\n{events_body}## Pomodoros\n")
    }

    fn note_without_events() -> String {
        "# Friday 2026-10-30\n## Notes\n\nhello\n## Pomodoros\n".to_string()
    }

    #[test]
    fn no_journaled_rows_for_the_day_is_not_replaced() {
        assert_eq!(detect(&note(""), DAY, &BTreeMap::new()), None);
    }

    #[test]
    fn only_gone_rows_are_not_replaced() {
        let journal = journal(&[(
            "e1",
            DAY,
            gone(row("product-interview", "task", Some(TASK_URL), 8, 0)),
        )]);
        assert_eq!(detect(&note_without_events(), DAY, &journal), None);
    }

    #[test]
    fn a_task_still_present_by_block_id_is_not_replaced() {
        let journal = journal(&[(
            "e1",
            DAY,
            live(row("product-interview", "task", Some(TASK_URL), 8, 0)),
        )]);
        let contents = note(&format!("{STAMP}\n{TASK_LINE}\n"));
        assert_eq!(detect(&contents, DAY, &journal), None);
    }

    #[test]
    fn a_task_present_by_url_with_a_changed_block_id_is_not_replaced() {
        let journal = journal(&[(
            "e1",
            DAY,
            live(row("old-id", "task", Some(TASK_URL), 8, 0)),
        )]);
        let contents = note(&format!("{STAMP}\n{TASK_LINE}\n"));
        assert_eq!(detect(&contents, DAY, &journal), None);
    }

    #[test]
    fn an_emptied_section_that_keeps_its_stamp_is_not_replaced() {
        let journal = journal(&[(
            "e1",
            DAY,
            live(row("product-interview", "task", Some(TASK_URL), 8, 0)),
        )]);
        let contents = note(&format!("{STAMP}\n"));
        assert_eq!(detect(&contents, DAY, &journal), None);
    }

    #[test]
    fn every_row_gone_and_the_stamp_gone_is_replaced() {
        let earlier = row("product-interview", "task", Some(TASK_URL), 8, 0);
        let later = row("dentist", "task", None, 12, 30);
        let journal = journal(&[
            ("e1", DAY, live(earlier)),
            ("e2", DAY, live(later.clone())),
        ]);
        let found = detect(&note(""), DAY, &journal).expect("replaced");
        assert_eq!(found.rows, 2);
        assert_eq!(found.written_at, later.ts);
    }

    #[test]
    fn a_missing_events_section_is_replaced() {
        let record = row("product-interview", "task", Some(TASK_URL), 8, 15);
        let journal = journal(&[("e1", DAY, live(record.clone()))]);
        let found =
            detect(&note_without_events(), DAY, &journal).expect("replaced");
        assert_eq!(found.rows, 1);
        assert_eq!(found.written_at, record.ts);
    }

    #[test]
    fn a_missing_events_section_holds_even_for_url_less_context_rows() {
        let journal =
            journal(&[("e1", DAY, live(row("", "context", None, 0, 0)))]);
        assert!(detect(&note_without_events(), DAY, &journal).is_some());
    }

    #[test]
    fn context_rows_still_present_by_url_with_the_stamp_deleted_are_not_replaced(
    ) {
        let journal = journal(&[(
            "e1",
            DAY,
            live(row("", "context", Some(CONTEXT_URL), 9, 15)),
        )]);
        let contents = note(&format!("{CONTEXT_URL_LINE}\n"));
        assert_eq!(detect(&contents, DAY, &journal), None);
    }

    #[test]
    fn url_less_context_rows_with_the_stamp_deleted_are_not_replaced() {
        let journal =
            journal(&[("e1", DAY, live(row("", "context", None, 0, 0)))]);
        let contents = note(&format!("{CONTEXT_PLAIN_LINE}\n"));
        assert_eq!(detect(&contents, DAY, &journal), None);
    }

    #[test]
    fn rows_from_another_day_are_ignored() {
        let journal = journal(&[(
            "e1",
            "2026-10-31",
            live(row("product-interview", "task", Some(TASK_URL), 8, 0)),
        )]);
        assert_eq!(detect(&note_without_events(), DAY, &journal), None);
    }
}

//! Pure preparation for a one-day forced sync reset.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use super::{
    journal::{occurrence_key, OccurrenceState},
    lifecycle::{line_root, split_row_spans},
    plan::{self, PlannedOccurrence},
};
use crate::native::event_task;

type Journal = BTreeMap<(String, String), OccurrenceState>;

/// The journal state before and after resetting one day. `previous` lets the
/// planner restore missing rows under their old block IDs; `journal` contains
/// only present snapshot rows, with span and root baselines rebased to the
/// current note so the ordinary three-way update applies Google's values.
#[derive(Debug, Clone)]
pub(crate) struct DayReset {
    pub day: String,
    pub previous: Journal,
    pub journal: Journal,
    pub had_state: bool,
}

/// Forget one day's links, tombstones, and once-only flags while retaining
/// journaled rows that are still present in the note and snapshot.
pub(crate) fn prepare(
    contents: &str,
    day: &str,
    original: &Journal,
    occurrences: &[PlannedOccurrence],
) -> DayReset {
    let previous: Journal = original
        .iter()
        .filter(|((_, row_day), _)| row_day == day)
        .map(|(key, state)| (key.clone(), state.clone()))
        .collect();
    let had_state = !previous.is_empty();
    let mut journal = original.clone();
    journal.retain(|(_, row_day), _| row_day != day);

    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let events = event_task::events_section_range(&refs);
    let note_day = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok();

    for (key, state) in &previous {
        let Some(old_row) = state.row.as_ref() else {
            continue;
        };
        let Some(occurrence) = occurrences.iter().find(|occurrence| {
            occurrence_key(&occurrence.calendar_id, &occurrence.event_id)
                == key.0
        }) else {
            continue;
        };
        let text_match = (!occurrence.task && occurrence.html_link.is_none())
            .then(|| {
                plan::render_occurrence(
                    occurrence,
                    "",
                    note_day.unwrap_or(NaiveDate::MIN),
                )
            });
        let Some(line_index) = plan::find_row_line(
            &lines,
            events.clone(),
            &old_row.block_id,
            occurrence.html_link.as_deref(),
            occurrence.task,
            text_match.as_deref(),
        ) else {
            continue;
        };

        let line = &lines[line_index];
        let mut row = old_row.clone();
        let current_id = plan::trailing_block_id(line);
        if !current_id.is_empty() {
            row.block_id = current_id;
        }
        if let Some(spans) = split_row_spans(line) {
            row.spans = spans.hashes();
            row.root = line_root(line);
        }
        journal.insert(
            key.clone(),
            OccurrenceState {
                row: Some(row),
                ..OccurrenceState::default()
            },
        );
    }

    DayReset {
        day: day.to_string(),
        previous,
        journal,
        had_state,
    }
}

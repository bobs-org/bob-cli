//! Additive meeting-notes state for the capture agenda.
//!
//! Every started Event Task row in `events.rows[]` gains the exact EVENT
//! token plus its filed state. The schema version is unchanged and human
//! rendering ignores these fields.

use std::{collections::BTreeMap, path::Path};

use chrono::{Duration, NaiveDate, NaiveDateTime};
use serde::Serialize;

use crate::native::event_task::{self, EventKind};

use super::model::{
    build_row, event_ref_for, filed_index_for, journal_identities, vault_index,
    FiledNotesIndex, MeetingRow,
};

/// Filed state for one agenda event row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MeetingNotesState {
    /// `missing`, or `filed` with `note` and `task` present.
    pub state: MeetingNotesStateKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// Whether the meeting already has filed notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MeetingNotesStateKind {
    Missing,
    Filed,
}

/// The filed state for one meeting row, or missing when no notes exist.
pub(crate) fn meeting_notes_for(
    filed: Option<&FiledNotesIndex>,
    row: &MeetingRow,
) -> MeetingNotesState {
    let found = filed.and_then(|filed| {
        row.event_id
            .as_deref()
            .and_then(|event_id| filed.by_event_id.get(event_id))
            .or_else(|| {
                row.block_id.as_ref().and_then(|id| {
                    filed.by_target.get(&(row.path.clone(), id.clone()))
                })
            })
    });
    match found {
        Some(filed) => MeetingNotesState {
            state: MeetingNotesStateKind::Filed,
            note: Some(filed.note.clone()),
            task: Some(filed.task.clone()),
        },
        None => MeetingNotesState {
            state: MeetingNotesStateKind::Missing,
            note: None,
            task: None,
        },
    }
}

/// The agenda fields for one meeting row: its EVENT token and notes state.
///
/// `None` for rows without a `^id`; callers leave those fields unset.
pub(crate) fn agenda_fields(
    filed: Option<&FiledNotesIndex>,
    row: &MeetingRow,
) -> Option<(String, MeetingNotesState)> {
    let event_ref = event_ref_for(&row.path, row.block_id.as_deref()?)?;
    Some((event_ref, meeting_notes_for(filed, row)))
}

/// The EVENT token and notes state for every started Event Task row with
/// a `^id`, keyed by zero-based line index. Empty when no such row
/// exists, so the vault walk and journal fold never run for plain days.
pub(crate) fn agenda_meetings(
    vault: &Path,
    day_relative: &Path,
    contents: &str,
    rows: &[event_task::EventNoteRow],
    date: NaiveDate,
    now: NaiveDateTime,
) -> BTreeMap<usize, (String, MeetingNotesState)> {
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    let started: Vec<&event_task::EventNoteRow> = rows
        .iter()
        .filter(|event| {
            event.kind == EventKind::Task
                && event.block_id.is_some()
                && event.status.is_some_and(|status| status != '-')
                && event.span.is_some_and(|span| {
                    midnight + Duration::minutes(i64::from(span.start_min))
                        <= now
                })
        })
        .collect();
    if started.is_empty() {
        return BTreeMap::new();
    }
    let lines: Vec<&str> = contents.lines().collect();
    let index = vault_index(vault);
    let (by_row, by_url) = journal_identities(vault);
    let filed = filed_index_for(vault);
    let mut out = BTreeMap::new();
    for event in started {
        let raw = lines.get(event.line_index).copied().unwrap_or("");
        let row = build_row(
            vault,
            &index,
            day_relative,
            event,
            raw,
            &by_row,
            &by_url,
        );
        if let Some(fields) = agenda_fields(filed.as_ref(), &row) {
            out.insert(event.line_index, fields);
        }
    }
    out
}

//! EVENT token resolution against the vault.
//!
//! Precedence mirrors the `bob ref meet` contract: an explicit ref or a
//! Google Calendar URL always wins, then title query words. A wrong link
//! is worse than asking, so zero and multi-row outcomes are typed errors
//! carrying candidates and evidence instead of guesses.

use std::{fs, path::Path};

use chrono::NaiveDate;

use crate::native::{
    event_task,
    gcal::{decode_calendar_eid, matches_query},
    vault_links::LinkResolution,
};

use super::{
    model::{
        build_row, collect_window_rows, filed_notes_index, journal_identities,
        vault_index, MeetingRow,
    },
    token::EventToken,
};

/// One ambiguous candidate with its identifying detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmbiguousCandidate {
    pub meeting: MeetingRow,
    /// For example `20261123#^standup · 0930-1000 Standup`.
    pub detail: String,
}

/// Why an EVENT token resolved to no single meeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolveError {
    /// Nothing matched, or the row cannot take notes.
    NoMeeting { message: String, hint: String },
    /// Several rows matched; the caller picks.
    Ambiguous {
        message: String,
        candidates: Vec<AmbiguousCandidate>,
    },
    /// The matched row has no `^id`; notes need one to link to.
    MissingId { message: String, hint: String },
}

impl ResolveError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::NoMeeting { message, .. }
            | Self::Ambiguous { message, .. }
            | Self::MissingId { message, .. } => message,
        }
    }
}

/// One recent meeting with its filed-notes state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentMeeting {
    pub meeting: MeetingRow,
    pub notes: Option<super::model::FiledNotes>,
    /// The exact EVENT token, when the row has a `^id`.
    pub event_ref: Option<String>,
}

/// Resolve one EVENT token to its meeting row.
///
/// `window_days` bounds query scans; `day` restricts to one local day.
/// Rows without a `^id` refuse with a hint to add one.
pub(crate) fn resolve_event(
    vault: &Path,
    token: &EventToken,
    window_days: u32,
    day: Option<NaiveDate>,
) -> Result<MeetingRow, ResolveError> {
    let row = match token {
        EventToken::Explicit { note, block_id } => {
            resolve_explicit(vault, note, block_id)?
        }
        EventToken::Url { url } => resolve_url(vault, url, window_days, day)?,
        EventToken::Query { words } => {
            resolve_query(vault, words, window_days, day)?
        }
    };
    if row.block_id.is_none() {
        return Err(ResolveError::MissingId {
            message: format!(
                "`{}` has no `^id`; notes link to the meeting by id",
                row.title
            ),
            hint: "add a `^id` to the Event Task row, for example `^standup`"
                .to_string(),
        });
    }
    Ok(row)
}

/// Resolve an explicit `note#^id` ref to its Event Task row.
fn resolve_explicit(
    vault: &Path,
    note: &str,
    block_id: &str,
) -> Result<MeetingRow, ResolveError> {
    let index = vault_index(vault);
    let relative = match index.resolve_detailed(None, note) {
        LinkResolution::Found(path) => path,
        LinkResolution::Ambiguous => {
            return Err(ResolveError::NoMeeting {
                message: format!("`{note}` matches more than one note"),
                hint:
                    "use the vault-relative path, for example `2026/20261123`"
                        .to_string(),
            });
        }
        LinkResolution::Missing => {
            return Err(ResolveError::NoMeeting {
                message: format!("no note matches `{note}`"),
                hint: "use a daily note such as `20261123`, or title words to search".to_string(),
            });
        }
    };
    let Ok(contents) = fs::read_to_string(vault.join(&relative)) else {
        return Err(ResolveError::NoMeeting {
            message: format!("cannot read `{}`", relative.display()),
            hint: "check the note exists and is readable".to_string(),
        });
    };
    let Some(date) =
        crate::native::task_status_hooks::canonical_daily_date(&relative)
    else {
        return Err(ResolveError::NoMeeting {
            message: format!("`{}` is not a daily note", relative.display()),
            hint: "event refs point at daily notes such as `20261123`"
                .to_string(),
        });
    };
    // Collect every matching row in the Events section: zero is a miss,
    // more than one is a duplicate id.
    let lines: Vec<&str> = contents.lines().collect();
    let index = vault_index(vault);
    let (by_row, by_url) = journal_identities(vault);
    let mut matches = Vec::new();
    for event in event_task::events_in_note(&contents, date)
        .into_iter()
        .filter(|event| {
            event.kind == event_task::EventKind::Task
                && event.block_id.as_deref() == Some(block_id)
        })
    {
        let raw = lines.get(event.line_index).copied().unwrap_or("");
        matches.push(build_row(
            vault, &index, &relative, &event, raw, &by_row, &by_url,
        ));
    }
    match matches.as_slice() {
        [] => Err(ResolveError::NoMeeting {
            message: format!(
                "no `^{block_id}` Event Task in `{}`",
                relative.display()
            ),
            hint: "list recent meetings to find the right EVENT token"
                .to_string(),
        }),
        [single] => Ok(single.clone()),
        _ => Err(ResolveError::Ambiguous {
            message: format!(
                "`^{block_id}` appears more than once in `{}`",
                relative.display()
            ),
            candidates: matches
                .into_iter()
                .map(|row| AmbiguousCandidate {
                    detail: candidate_detail(&row),
                    meeting: row,
                })
                .collect(),
        }),
    }
}

/// Resolve one Google Calendar URL through the journal to its row,
/// falling back to the row carrying the same 📅 URL.
fn resolve_url(
    vault: &Path,
    url: &str,
    window_days: u32,
    day: Option<NaiveDate>,
) -> Result<MeetingRow, ResolveError> {
    let Some(event_id) = decode_calendar_eid(url) else {
        return Err(ResolveError::NoMeeting {
            message: "could not decode a Google event id from the URL"
                .to_string(),
            hint: "paste the full Calendar URL carrying `eid=`".to_string(),
        });
    };
    if let Ok((state, _, _)) = crate::native::gcal::journal::load_state(vault) {
        // Every journal row for this event id, latest day first.
        let mut hits: Vec<(String, String, String)> = Vec::new();
        for ((key, day), occurrence) in &state {
            let Some((_, key_event)) = key.split_once('\u{1f}') else {
                continue;
            };
            if key_event != event_id {
                continue;
            }
            if let Some(row) = occurrence.row.as_ref() {
                hits.push((
                    day.clone(),
                    row.path.clone(),
                    row.block_id.clone(),
                ));
            }
        }
        hits.sort();
        hits.reverse();
        if let Some(row) =
            hits.into_iter().find_map(|(_day, path, block_id)| {
                journal_row(vault, &path, &block_id)
            })
        {
            return Ok(row);
        }
    }
    if let Some(row) = collect_window_rows(vault, window_days, day)
        .into_iter()
        .find(|row| row.calendar_url.as_deref() == Some(url))
    {
        return Ok(row);
    }
    Err(ResolveError::NoMeeting {
        message: "the Google event is not in the vault journal".to_string(),
        hint: "use an explicit event ref such as `20261123#^standup`"
            .to_string(),
    })
}

/// Read one journal `(path, block id)` hit back into its meeting row.
fn journal_row(vault: &Path, path: &str, block_id: &str) -> Option<MeetingRow> {
    let relative = Path::new(path).to_path_buf();
    let Ok(contents) = fs::read_to_string(vault.join(&relative)) else {
        return None;
    };
    let event = event_task::event_task_by_id(&relative, &contents, block_id)?;
    let lines: Vec<&str> = contents.lines().collect();
    let raw = lines.get(event.line_index).copied().unwrap_or("");
    let index = vault_index(vault);
    let (by_row, by_url) = journal_identities(vault);
    Some(build_row(
        vault,
        &index,
        &relative,
        &event_task::EventNoteRow {
            line_index: event.line_index,
            kind: event_task::EventKind::Task,
            span: Some(event.span),
            buffer: event.buffer,
            untracked: event.untracked,
            off_clock: event.off_clock,
            clock_tag: event.clock_tag,
            clock_source: event.clock_source,
            clock_note: event.clock_note,
            clock_key: event.clock_key,
            status: Some(event.status),
            block_id: event.block_id,
            title: event.title,
        },
        raw,
        &by_row,
        &by_url,
    ))
}

/// Resolve title query words: every word appears in the title, case
/// folded, inside the window or day.
fn resolve_query(
    vault: &Path,
    words: &[String],
    window_days: u32,
    day: Option<NaiveDate>,
) -> Result<MeetingRow, ResolveError> {
    if words.iter().all(|word| word.trim().is_empty()) {
        return Err(ResolveError::NoMeeting {
            message:
                "name the meeting: an event ref, a Calendar URL, or title words"
                    .to_string(),
            hint: "list recent meetings to find the right EVENT token"
                .to_string(),
        });
    }
    let mut candidates: Vec<MeetingRow> =
        collect_window_rows(vault, window_days, day)
            .into_iter()
            .filter(|row| matches_query(&row.title, words))
            .collect();
    // Best textual match first, then the most recent meeting.
    candidates.sort_by(|left, right| {
        rank_query(&left.title, words, left.date).cmp(&rank_query(
            &right.title,
            words,
            right.date,
        ))
    });
    match candidates.as_slice() {
        [] => Err(ResolveError::NoMeeting {
            message: format!("no meetings match {}", words.join(" ")),
            hint: "list recent meetings to find the right EVENT token"
                .to_string(),
        }),
        [single] => Ok(single.clone()),
        _ => Err(ResolveError::Ambiguous {
            message: format!(
                "several meetings match {}; be more specific",
                words.join(" ")
            ),
            candidates: candidates
                .into_iter()
                .map(|row| AmbiguousCandidate {
                    detail: candidate_detail(&row),
                    meeting: row,
                })
                .collect(),
        }),
    }
}

/// Query rank: exact beats prefix beats substring, then newer beats older.
fn rank_query(
    title: &str,
    words: &[String],
    date: NaiveDate,
) -> (u8, std::cmp::Reverse<NaiveDate>) {
    let query = words.join(" ").to_ascii_lowercase();
    let lower = title.to_ascii_lowercase();
    let rank = if lower == query {
        0
    } else if lower.starts_with(&query) {
        1
    } else if lower.contains(&query) {
        2
    } else {
        3
    };
    (rank, std::cmp::Reverse(date))
}

/// One-line picker detail for an ambiguous candidate.
fn candidate_detail(row: &MeetingRow) -> String {
    match super::model::event_ref_for(
        &row.path,
        row.block_id.as_deref().unwrap_or(""),
    ) {
        Some(event_ref) => {
            format!("{event_ref} · {} {}", row.time_range, row.title)
        }
        None => format!("{} {} (no ^id)", row.date, row.title),
    }
}

/// Recent started, non-cancelled Event Task rows, newest first, each with
/// its filed-notes state. Cancelled (`-`) rows never walk.
pub(crate) fn recent_meetings(
    vault: &Path,
    window_days: u32,
    now: chrono::NaiveDateTime,
) -> Vec<RecentMeeting> {
    let filed = filed_notes_index(vault);
    let mut meetings: Vec<RecentMeeting> =
        collect_window_rows(vault, window_days, None)
            .into_iter()
            .filter(|row| row.starts_at <= now && row.status != '-')
            .map(|row| {
                let notes = filed_notes_for_row(&filed, &row);
                let event_ref = row
                    .block_id
                    .as_deref()
                    .and_then(|id| super::model::event_ref_for(&row.path, id));
                RecentMeeting {
                    meeting: row,
                    notes,
                    event_ref,
                }
            })
            .collect();
    meetings.sort_by(|left, right| {
        (
            right.meeting.date,
            right.meeting.starts_at,
            right.meeting.line,
        )
            .cmp(&(
                left.meeting.date,
                left.meeting.starts_at,
                left.meeting.line,
            ))
    });
    meetings
}

/// The filed state for one row: journal event id first, else the `event`
/// link target.
fn filed_notes_for_row(
    filed: &super::model::FiledNotesIndex,
    row: &MeetingRow,
) -> Option<super::model::FiledNotes> {
    if let Some(event_id) = row.event_id.as_deref()
        && let Some(found) = filed.by_event_id.get(event_id)
    {
        return Some(found.clone());
    }
    row.block_id.as_ref().and_then(|id| {
        filed
            .by_target
            .get(&(row.path.clone(), id.clone()))
            .cloned()
    })
}

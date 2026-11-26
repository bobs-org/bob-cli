//! Event-first suggestions for terminal capture snooze times.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use chrono::{Datelike, NaiveDateTime, Timelike};

use super::model::{CompleteError, SnoozeCandidate};
use crate::native::{
    env,
    event_task::{self, EventKind},
    snooze::{
        parse::{parse_section, SnoozeRowKind},
        when::{format_hhmm, parse_snooze_when, ResolvedTime, SnoozeClock},
    },
};

/// Build same-day suggestions in their source order: upcoming event ends,
/// relative times, then times already present in today's Snoozed section.
/// The core parser validates every suggestion and supplies its insertion
/// value, so the completion menu cannot diverge from capture execution.
pub(super) fn candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<Vec<SnoozeCandidate>, CompleteError> {
    let wall = env::current_datetime();
    let zone = env::configured_timezone()
        .or_else(crate::native::snooze::host_timezone)
        .ok_or_else(|| {
            CompleteError::io(
                "cannot identify the host timezone; set timezone in Bob's config",
            )
        })?;
    let clock = SnoozeClock {
        date: wall.date(),
        now_minutes: (wall.hour() * 60 + wall.minute()) as u16,
        zone,
    };

    let contents = fs::read_to_string(today_file(bob_dir, wall))
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(String::new())
            } else {
                Err(error)
            }
        })
        .map_err(|error| {
            CompleteError::io(format!("could not read today's note: {error}"))
        })?;

    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    add_event_ends(&contents, clock, &mut rows, &mut seen);
    add_relative_times(clock, &mut rows, &mut seen);
    add_existing_times(&contents, clock, &mut rows, &mut seen);

    let query = query.to_ascii_lowercase();
    rows.retain(|row| {
        query.is_empty()
            || row.replacement.starts_with(&query)
            || row
                .filter_aliases
                .iter()
                .any(|alias| alias.starts_with(&query))
    });
    Ok(rows)
}

fn add_event_ends(
    contents: &str,
    clock: SnoozeClock,
    rows: &mut Vec<SnoozeCandidate>,
    seen: &mut HashSet<u16>,
) {
    let mut events: Vec<_> = event_task::events_in_note(contents, clock.date)
        .into_iter()
        .filter_map(|event| {
            let span = event.span?;
            if span.crosses_midnight
                || span.end_min <= clock.now_minutes
                || (event.kind == EventKind::Task && !event.is_open_task())
            {
                return None;
            }
            Some(event)
        })
        .collect();
    // The Events parser orders by start; snooze suggestions follow actual
    // meeting end, with source order preserved for ties.
    events.sort_by_key(|event| event.span.map(|span| span.end_min));

    for event in events {
        let Some(span) = event.span else { continue };
        let raw = format_hhmm(span.end_min);
        let Ok(resolved) = parse_snooze_when(&raw, clock) else {
            continue;
        };
        let title = if event.title.trim().is_empty() {
            "event"
        } else {
            event.title.trim()
        };
        push_candidate(
            rows,
            seen,
            resolved,
            format!("{} · after {title}", display_time(span.end_min)),
            Vec::new(),
        );
    }
}

fn add_relative_times(
    clock: SnoozeClock,
    rows: &mut Vec<SnoozeCandidate>,
    seen: &mut HashSet<u16>,
) {
    for alias in ["+30m", "+1h", "+2h"] {
        let Ok(resolved) = parse_snooze_when(alias, clock) else {
            // Relative times that resolve beyond midnight are simply absent.
            continue;
        };
        push_candidate(
            rows,
            seen,
            resolved.clone(),
            format!("{alias} · {}", display_time(resolved.minutes)),
            vec![alias.to_string()],
        );
    }
}

fn add_existing_times(
    contents: &str,
    clock: SnoozeClock,
    rows: &mut Vec<SnoozeCandidate>,
    seen: &mut HashSet<u16>,
) {
    let lines: Vec<_> = contents.lines().collect();
    let Some(section) = parse_section(&lines) else {
        return;
    };
    for row in section.rows {
        if row.kind != SnoozeRowKind::Active {
            continue;
        }
        let Some(raw) = row.hhmm.as_deref() else {
            continue;
        };
        let Ok(resolved) = parse_snooze_when(raw, clock) else {
            continue;
        };
        push_candidate(
            rows,
            seen,
            resolved.clone(),
            format!("{} · used today", display_time(resolved.minutes)),
            Vec::new(),
        );
    }
}

fn push_candidate(
    rows: &mut Vec<SnoozeCandidate>,
    seen: &mut HashSet<u16>,
    resolved: ResolvedTime,
    label: String,
    filter_aliases: Vec<String>,
) {
    if !seen.insert(resolved.minutes) {
        return;
    }
    rows.push(SnoozeCandidate {
        replacement: resolved.hhmm,
        label,
        filter_aliases,
    });
}

fn display_time(minutes: u16) -> String {
    let hhmm = format_hhmm(minutes);
    format!("{}:{}", &hhmm[..2], &hhmm[2..])
}

/// Resolve the daily note from one injected `BOB_NOW`; asking
/// `pomodoro::day_file_for` here would read the clock a second time at a
/// date boundary. `BOB_DAY_FILE` remains the fixture/runtime override.
fn today_file(bob_dir: &Path, now: NaiveDateTime) -> PathBuf {
    if let Some(path) =
        env::var_os("BOB_DAY_FILE").filter(|path| !path.is_empty())
    {
        return PathBuf::from(path);
    }
    bob_dir.join(format!(
        "{:04}/{:04}{:02}{:02}.md",
        now.year(),
        now.year(),
        now.month(),
        now.day()
    ))
}

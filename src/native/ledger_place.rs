//! Shared chronological placement for completed Pomodoro rows.
//!
//! Extracted from `gcal::plan::closed_insert_at` so `bob gcal sync` and
//! `bob capture x=` record share one rule: after the last completed row
//! whose start is <= the new start, otherwise before the first completed
//! row, otherwise before the first open row, otherwise at the section
//! start. Complete blocks stay before open ones.

use super::{
    capture::line_spans,
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    pomodoro::pomodoros_section_range,
};

/// Where to insert a completed block: before `index` (0-based) or after
/// `index` (0-based headline line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerInsert {
    Before(usize),
    After(usize),
}

/// Shared chronological insertion point for a completed row starting at
/// `cluster_start` (minutes since midnight, unwrapped, e.g. 1440 for a
/// midnight end).
pub(crate) fn closed_insert_at(
    contents: &str,
    cluster_start: i32,
) -> LedgerInsert {
    let scan = capture_pomodoros::scan(contents);
    let lines = line_spans(contents);
    let line_text: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let section =
        pomodoros_section_range(&line_text).expect("Pomodoros section exists");
    let completed: Vec<&PomodoroEntry> = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Completed)
        .collect();
    if let Some(last) = completed
        .iter()
        .rev()
        .find(|entry| entry_start_min(entry) <= cluster_start)
    {
        let end = crate::native::capture_task_toggle::child_block_end_line(
            &lines,
            last.line - 1,
        );
        return LedgerInsert::After(end);
    }
    if let Some(first) = completed.first() {
        return LedgerInsert::Before(first.line - 1);
    }
    if let Some(first_open) = scan
        .entries
        .iter()
        .find(|entry| entry.state == PomodoroState::Open)
    {
        return LedgerInsert::Before(first_open.line - 1);
    }
    LedgerInsert::Before(section.start)
}

fn entry_start_min(entry: &PomodoroEntry) -> i32 {
    entry
        .time_range
        .as_deref()
        .and_then(|range| range.split_once('-'))
        .and_then(|(start, _)| parse_hhmm(start).map(i32::from))
        .unwrap_or(0)
}

fn parse_hhmm(value: &str) -> Option<u16> {
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour: u16 = value[..2].parse().ok()?;
    let minute: u16 = value[2..].parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

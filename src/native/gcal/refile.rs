//! Re-file untouched Google-owned MEET links after Google moves an event.

use std::collections::{HashMap, HashSet};

use chrono::{NaiveDate, Timelike};

use crate::native::{
    capture::line_spans,
    capture_pomodoro_start,
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    capture_task_toggle::{child_block_end_line, set_task_line_status},
    event_task::{self, EventKind, EventNoteRow},
    task_complete::tree::{
        add_or_replace_completion_field, remove_completion_fields,
    },
    task_fields,
};

use super::{
    journal::occurrence_key,
    lifecycle,
    plan::{
        self, push_kept_once, LinkPhase, MovedRow, PlannedOccurrence, SyncPlan,
        SyncPlanInput, WantedLink, WorkingNote,
    },
};
use crate::native::pomodoro_homes::ClockLookup;

#[derive(Debug, Clone)]
enum Filing {
    Planned {
        block_id: String,
    },
    BobClosed {
        digest: String,
        members: Vec<String>,
    },
    Running,
    Unlinked,
    Yours,
}

#[derive(Debug, Clone)]
struct ClassifiedMove {
    moved: MovedRow,
    phase: LinkPhase,
    filing: Filing,
}

#[derive(Debug, Clone)]
struct Mention {
    entry_index: usize,
    line_index: usize,
    exact_plain: bool,
    exact_tomato: bool,
    childless: bool,
}

/// Find the old filing for each moved row, remove only Bob-owned filing,
/// and return links for the normal phase-placement pass.
pub(super) fn plan_refile<C: ClockLookup + ?Sized>(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan_out: &mut SyncPlan,
    daily_rel: Option<&std::path::Path>,
    mut clock: Option<&mut C>,
) -> Vec<WantedLink> {
    if plan_out.moved_rows.is_empty() {
        return Vec::new();
    }

    let contents = working.as_string();
    let mut moved_by_id = HashMap::<String, MovedRow>::new();
    for moved in &plan_out.moved_rows {
        moved_by_id
            .entry(moved.block_id.clone())
            .or_insert_with(|| moved.clone());
    }
    let events = event_task::events_in_note(&contents, note_day(input));
    let mut classified = Vec::new();
    for moved in moved_by_id.into_values() {
        let Some(event) = unique_event_by_id(&events, &moved.block_id) else {
            classified.push(ClassifiedMove {
                phase: phase_for_occurrence(input, &moved.block_id)
                    .unwrap_or(LinkPhase::Future),
                filing: Filing::Yours,
                moved,
            });
            continue;
        };
        let phase = phase_for_occurrence(input, &moved.block_id)
            .unwrap_or_else(|| phase_for_event(input, event));
        let filing = classify_filing(
            input,
            &contents,
            &events,
            &moved,
            event,
            &plan_out.moved_rows,
            daily_rel,
            &mut clock,
        );
        classified.push(ClassifiedMove {
            moved,
            phase,
            filing,
        });
    }

    let mut wants = Vec::new();
    let mut removed_filing = false;
    let mut closed_groups = HashMap::<String, Vec<String>>::new();
    for item in &classified {
        if let Filing::BobClosed { digest, members } = &item.filing {
            let group = closed_groups.entry(digest.clone()).or_default();
            for member in members {
                if !group.contains(member) {
                    group.push(member.clone());
                }
            }
        }
    }

    // Remove closed clusters from the bottom upward so their recorded line
    // numbers remain valid while earlier entries are still present.
    let mut groups: Vec<(String, Vec<String>, usize)> = closed_groups
        .into_iter()
        .map(|(digest, members)| {
            let line = capture_pomodoros::scan(&contents)
                .entries
                .into_iter()
                .find(|entry| entry.pomodoro_ref.digest == digest)
                .map_or(0, |entry| entry.line);
            (digest, members, line)
        })
        .collect();
    groups.sort_by_key(|(_, _, line)| std::cmp::Reverse(*line));
    for (digest, members, _) in groups {
        if remove_closed_entry(working, &digest) {
            removed_filing = true;
            let current_events = event_task::events_in_note(
                &working.as_string(),
                note_day(input),
            );
            for member in members {
                let Some(event) = unique_event_by_id(&current_events, &member)
                else {
                    continue;
                };
                let moved = classified
                    .iter()
                    .find(|item| item.moved.block_id == member);
                let phase =
                    moved.map(|item| item.phase).unwrap_or(LinkPhase::Past);
                if phase != LinkPhase::Past {
                    reopen_row(working, event.line_index);
                }
                if let Some(want) = wanted_for_event(
                    input,
                    &working.as_string(),
                    event,
                    phase,
                    true,
                    daily_rel,
                    &mut clock,
                ) {
                    wants.push(want);
                }
            }
        }
    }

    for item in &classified {
        match &item.filing {
            Filing::Planned { block_id } if item.phase == LinkPhase::Past => {
                let current = working.as_string();
                let (without, removed) =
                    lifecycle::remove_meet_link(&current, block_id);
                if removed {
                    working.replace(&without);
                    removed_filing = true;
                }
                if let Some(event) = unique_event_by_id(
                    &event_task::events_in_note(
                        &working.as_string(),
                        note_day(input),
                    ),
                    block_id,
                ) {
                    complete_row(working, event.line_index, &input.day_string);
                    let refreshed = event_task::events_in_note(
                        &working.as_string(),
                        note_day(input),
                    );
                    if let Some(event) =
                        unique_event_by_id(&refreshed, block_id)
                        && let Some(want) = wanted_for_event(
                            input,
                            &working.as_string(),
                            event,
                            LinkPhase::Past,
                            true,
                            daily_rel,
                            &mut clock,
                        )
                    {
                        wants.push(want);
                    }
                }
            }
            Filing::Yours => report_kept(input, plan_out, &item.moved),
            Filing::Planned { .. }
            | Filing::BobClosed { .. }
            | Filing::Running
            | Filing::Unlinked => {}
        }
    }

    if removed_filing {
        let current = working.as_string();
        let (cleaned, _) = lifecycle::remove_childless_untimed_meet(&current);
        working.replace(&cleaned);
    }

    wants
}

fn classify_filing<C: ClockLookup + ?Sized>(
    input: &SyncPlanInput<'_>,
    contents: &str,
    events: &[EventNoteRow],
    moved: &MovedRow,
    event: &EventNoteRow,
    moved_rows: &[MovedRow],
    daily_rel: Option<&std::path::Path>,
    clock: &mut Option<&mut C>,
) -> Filing {
    let Some(mentions) = ledger_mentions(contents, &moved.block_id) else {
        return Filing::Unlinked;
    };
    if mentions.is_empty() {
        return Filing::Unlinked;
    }

    let scan = capture_pomodoros::scan(contents);
    if mentions.iter().any(|mention| {
        scan.entries.get(mention.entry_index).is_some_and(|entry| {
            entry.state == PomodoroState::Open && entry.time_range.is_some()
        })
    }) {
        return Filing::Running;
    }

    let untouched =
        lifecycle::row_untouched(contents, event.line_index, &moved.block_id);
    if untouched
        && mentions.iter().all(|mention| {
            mention.exact_plain
                && mention.childless
                && scan.entries.get(mention.entry_index).is_some_and(|entry| {
                    entry.state == PomodoroState::Open
                        && entry.name.as_deref() == Some("MEET")
                        && entry.placeholder
                        && entry.time_range.is_none()
                })
        })
        && mentions
            .iter()
            .map(|mention| mention.entry_index)
            .collect::<HashSet<_>>()
            .len()
            == 1
    {
        return Filing::Planned {
            block_id: moved.block_id.clone(),
        };
    }

    let closed_members = mentions
        .first()
        .and_then(|mention| scan.entries.get(mention.entry_index))
        .filter(|entry| {
            entry.state == PomodoroState::Completed
                && entry.name.as_deref() == Some("MEET")
                && entry.time_range.is_some()
        })
        .and_then(|entry| {
            bob_closed_members(
                input, contents, events, entry, moved_rows, daily_rel, clock,
            )
        });
    if mentions.len() == 1
        && mentions[0].exact_tomato
        && mentions[0].childless
        && let Some(entry) = scan.entries.get(mentions[0].entry_index)
        && entry.state == PomodoroState::Completed
        && entry.name.as_deref() == Some("MEET")
        && entry.time_range.is_some()
        && let Some(members) = closed_members
        && members.iter().any(|member| member == &moved.block_id)
    {
        return Filing::BobClosed {
            digest: entry.pomodoro_ref.digest.clone(),
            members,
        };
    }

    Filing::Yours
}

fn ledger_mentions(contents: &str, block_id: &str) -> Option<Vec<Mention>> {
    let lines = line_spans(contents);
    let scan = capture_pomodoros::scan(contents);
    if !scan.has_section {
        return None;
    }
    let child_end = |entry: &PomodoroEntry| {
        child_block_end_line(&lines, entry.line.saturating_sub(1))
            .min(lines.len().saturating_sub(1))
    };
    let target = format!("#^{block_id}");
    let mut mentions = Vec::new();
    for (entry_index, entry) in scan.entries.iter().enumerate() {
        let start = entry.line.saturating_sub(1);
        let end = child_end(entry);
        if start >= lines.len() {
            continue;
        }
        for line_index in start.saturating_add(1)..=end {
            let Some(line) = lines.get(line_index).map(|span| span.text) else {
                continue;
            };
            if !line.contains(&target) {
                continue;
            }
            let trimmed = line.trim();
            mentions.push(Mention {
                entry_index,
                line_index,
                exact_plain: trimmed == format!("- [[#^{block_id}]]"),
                exact_tomato: trimmed == format!("- 🍅 ~~[[#^{block_id}]]~~"),
                childless: child_block_end_line(&lines, line_index)
                    == line_index,
            });
        }
    }
    Some(mentions)
}

fn bob_closed_members<C: ClockLookup + ?Sized>(
    input: &SyncPlanInput<'_>,
    contents: &str,
    events: &[EventNoteRow],
    entry: &PomodoroEntry,
    moved_rows: &[MovedRow],
    daily_rel: Option<&std::path::Path>,
    clock: &mut Option<&mut C>,
) -> Option<Vec<String>> {
    let lines = line_spans(contents);
    let header_index = entry.line.checked_sub(1)?;
    let header = lines.get(header_index)?.text;
    let end = child_block_end_line(&lines, header_index)
        .min(lines.len().saturating_sub(1));
    let links = capture_pomodoro_start::session_links(contents, header_index);
    if links.is_empty() || links.len() != entry.child_count {
        return None;
    }
    let mut members = Vec::new();
    let mut expected_lines = HashMap::<usize, String>::new();
    for link in &links {
        let index = link.ledger_line.checked_sub(1)?;
        let id = link.block_id.clone();
        let expected = format!("- 🍅 ~~[[#^{id}]]~~");
        if lines.get(index)?.text.trim() != expected
            || child_block_end_line(&lines, index) != index
        {
            return None;
        }
        let mentions = ledger_mentions(contents, &id)?;
        let mention_entry_line = capture_pomodoros::scan(contents)
            .entries
            .get(mentions.first()?.entry_index)
            .map(|entry| entry.line);
        if mentions.len() != 1
            || mention_entry_line != Some(entry.line)
            || mentions[0].line_index != index
            || !mentions[0].exact_tomato
        {
            return None;
        }
        let event = unique_event_by_id(events, &id)?;
        if !born_done(contents, event, &input.day_string) {
            return None;
        }
        members.push(id.clone());
        expected_lines.insert(index, expected);
    }

    let mut header_off_clock = None;
    if header.contains("[off::") {
        header_off_clock = Some(true);
    } else if header.contains("[t::") {
        header_off_clock = Some(false);
    }
    let header_off_clock = header_off_clock?;

    // Every physical child line must be one of the dedicated member links;
    // this excludes notes and grandchildren even when the scanner would
    // otherwise ignore them.
    let child_indent = links
        .first()
        .and_then(|link| lines.get(link.ledger_line.saturating_sub(1)))
        .map(|line| indentation(line.text))?;
    for index in header_index + 1..=end {
        let line = lines.get(index)?.text;
        if line.trim().is_empty() {
            continue;
        }
        if indentation(line) != child_indent
            || expected_lines
                .get(&index)
                .is_none_or(|expected| line.trim() != expected)
        {
            return None;
        }
    }

    let mut old_spans = Vec::new();
    for id in &members {
        let event = unique_event_by_id(events, id)?;
        let old_line = moved_rows
            .iter()
            .find(|moved| moved.block_id == *id)
            .map(|moved| moved.old_line.as_str())
            .unwrap_or(lines.get(event.line_index)?.text);
        let span = event_task::parse_event_span(old_line)?;
        let buffer = event_task::parse_event_buffer(old_line);
        let fallback = occurrence_for_block_id(input, id)
            .map(|occurrence| occurrence.off_clock)
            .unwrap_or(event.off_clock);
        let off_clock = plan::resolved_off_clock(
            clock,
            daily_rel,
            Some(old_line),
            fallback,
        );
        if off_clock != header_off_clock {
            return None;
        }
        let (start_min, end_min) = event_task::buffered_timeline_range(
            i32::from(span.start_min),
            span.end_on_timeline(),
            buffer,
        );
        old_spans.push(WantedLink {
            calendar_id: String::new(),
            event_id: String::new(),
            block_id: id.clone(),
            phase: LinkPhase::Past,
            start_min,
            end_min,
            title: event.title.clone(),
            off_clock,
            refiled: false,
        });
    }
    let clusters = plan::cluster_past(old_spans);
    if clusters.len() != 1 || clusters[0].len() != members.len() {
        return None;
    }
    let start_min = clusters[0].iter().map(|link| link.start_min).min()?;
    let end_min = clusters[0].iter().map(|link| link.end_min).max()?;
    let (span, _) =
        plan::format_meet_span(start_min, end_min, header_off_clock);
    (header == format!("- [x] {span} — MEET")).then_some(members)
}

fn born_done(contents: &str, event: &EventNoteRow, day: &str) -> bool {
    let lines = line_spans(contents);
    let Some(line) = lines.get(event.line_index).map(|line| line.text) else {
        return false;
    };
    if event.kind != EventKind::Task
        || event.status != Some('x')
        || task_fields::inline_fields(line, "completion").len() != 1
        || task_fields::inline_fields(line, "completion")[0].value != day
    {
        return false;
    }
    let end = child_block_end_line(&lines, event.line_index)
        .min(lines.len().saturating_sub(1));
    if lines[event.line_index..=end]
        .iter()
        .any(|line| line.text.contains('🍅'))
    {
        return false;
    }
    let Some(block_id) = event.block_id.as_deref() else {
        return false;
    };
    ledger_mentions(contents, block_id)
        .is_some_and(|mentions| mentions.len() == 1)
}

fn unique_event_by_id<'a>(
    events: &'a [EventNoteRow],
    block_id: &str,
) -> Option<&'a EventNoteRow> {
    let mut matches = events.iter().filter(|event| {
        event.kind == EventKind::Task
            && event.block_id.as_deref() == Some(block_id)
    });
    let event = matches.next()?;
    matches.next().is_none().then_some(event)
}

fn occurrence_for_block_id<'a>(
    input: &'a SyncPlanInput<'_>,
    block_id: &str,
) -> Option<&'a PlannedOccurrence> {
    input.occurrences.iter().find(|occurrence| {
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        input
            .journal
            .get(&(key, input.day_string.clone()))
            .and_then(|state| state.row.as_ref())
            .is_some_and(|row| row.block_id == block_id)
    })
}

fn phase_for_occurrence(
    input: &SyncPlanInput<'_>,
    block_id: &str,
) -> Option<LinkPhase> {
    occurrence_for_block_id(input, block_id)?.link_phase
}

fn phase_for_event(
    input: &SyncPlanInput<'_>,
    event: &EventNoteRow,
) -> LinkPhase {
    let Some(day) =
        NaiveDate::parse_from_str(&input.day_string, "%Y-%m-%d").ok()
    else {
        return LinkPhase::Future;
    };
    if day < input.now.date() {
        return LinkPhase::Past;
    }
    if day > input.now.date() || !input.is_today {
        return LinkPhase::Future;
    }
    let Some(span) = event.span else {
        return LinkPhase::Future;
    };
    let (start, end) = event_task::buffered_timeline_range(
        i32::from(span.start_min),
        span.end_on_timeline(),
        event.buffer,
    );
    let now = input.now.time().num_seconds_from_midnight() as i32 / 60;
    if end <= now {
        LinkPhase::Past
    } else if start <= now {
        LinkPhase::Current
    } else {
        LinkPhase::Future
    }
}

fn wanted_for_event<C: ClockLookup + ?Sized>(
    input: &SyncPlanInput<'_>,
    contents: &str,
    event: &EventNoteRow,
    phase: LinkPhase,
    refiled: bool,
    daily_rel: Option<&std::path::Path>,
    clock: &mut Option<&mut C>,
) -> Option<WantedLink> {
    let span = event.span?;
    let block_id = event.block_id.clone()?;
    let occurrence = occurrence_for_block_id(input, &block_id);
    let (calendar_id, event_id) = occurrence
        .map(|occurrence| {
            (occurrence.calendar_id.clone(), occurrence.event_id.clone())
        })
        .unwrap_or_default();
    let line = line_spans(contents)
        .get(event.line_index)
        .map(|line| line.text)?;
    let fallback = occurrence
        .map(|occurrence| occurrence.off_clock)
        .unwrap_or(event.off_clock);
    let off_clock =
        plan::resolved_off_clock(clock, daily_rel, Some(line), fallback);
    let (start_min, end_min) = event_task::buffered_timeline_range(
        i32::from(span.start_min),
        span.end_on_timeline(),
        event.buffer,
    );
    Some(WantedLink {
        calendar_id,
        event_id,
        block_id,
        phase,
        start_min,
        end_min,
        title: event.title.clone(),
        off_clock,
        refiled,
    })
}

fn complete_row(working: &mut WorkingNote, line_index: usize, day: &str) {
    let Some(line) = working.lines.get_mut(line_index) else {
        return;
    };
    if let Some(completed) = set_task_line_status(line, 'x') {
        *line = add_or_replace_completion_field(&completed, day);
    }
}

fn reopen_row(working: &mut WorkingNote, line_index: usize) {
    let Some(line) = working.lines.get_mut(line_index) else {
        return;
    };
    if let Some(open) = set_task_line_status(line, '*') {
        *line = remove_completion_fields(&open);
    }
}

fn remove_closed_entry(working: &mut WorkingNote, digest: &str) -> bool {
    let contents = working.as_string();
    let Some(entry) = capture_pomodoros::scan(&contents)
        .entries
        .into_iter()
        .find(|entry| entry.pomodoro_ref.digest == digest)
    else {
        return false;
    };
    let spans = line_spans(&contents);
    let start = entry.line.saturating_sub(1);
    let end =
        child_block_end_line(&spans, start).min(spans.len().saturating_sub(1));
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    if start >= lines.len() {
        return false;
    }
    lines.drain(start..=end);
    let mut updated = lines.join("\n");
    if contents.ends_with('\n') && !lines.is_empty() {
        updated.push('\n');
    }
    working.replace(&updated);
    true
}

fn report_kept(
    input: &SyncPlanInput<'_>,
    plan_out: &mut SyncPlan,
    moved: &MovedRow,
) {
    let occurrence = input.occurrences.iter().find(|occurrence| {
        occurrence.calendar_id == moved.calendar_id
            && occurrence.event_id == moved.event_id
    });
    let title = occurrence
        .map(|occurrence| occurrence.title.clone())
        .unwrap_or_else(|| moved.block_id.clone());
    let new_time = occurrence
        .map(|occurrence| format_time_range(&occurrence.time_label))
        .unwrap_or_else(|| "a new time".to_string());
    push_kept_once(
        input,
        plan_out,
        &moved.calendar_id,
        &moved.event_id,
        title,
        format!("kept your MEET Pomodoro; Google moved it to {new_time}"),
        "kept_meet",
    );
}

fn format_time_range(value: &str) -> String {
    value
        .split_once('-')
        .map(|(start, end)| format!("{start}–{end}"))
        .unwrap_or_else(|| value.to_string())
}

fn note_day(input: &SyncPlanInput<'_>) -> NaiveDate {
    NaiveDate::parse_from_str(&input.day_string, "%Y-%m-%d")
        .unwrap_or(input.today)
}

fn indentation(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

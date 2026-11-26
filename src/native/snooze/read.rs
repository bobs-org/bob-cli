//! Snapshot-backed, read-only Snoozed row resolution shared by CLI and readers.
//! Callers supply one Bob-zone clock, note date, resolver, and cached note lookup.

use super::{
    parse::{self, SnoozeRow, SnoozeRowKind},
    plan,
    state::{
        self, ClassifiedRows, ClassifyInput, ClassifyRow, ResolutionKind,
        TaskIdentity, TaskState,
    },
    when::SnoozeClock,
};
use crate::native::{
    capture::{line_spans, nearest_shallower_list_item_parent},
    capture_pomodoros::{self, PomodoroState},
    event_task, markdown, note_tasks, pomodoro,
    task_dependencies::block_link_spans,
    task_status_hooks::TaskStatusType,
    vault_links::{LinkResolution, NoteIndex},
};
use chrono::NaiveDate;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Debug, Clone)]
pub(crate) struct RowDetail {
    pub(crate) parsed: SnoozeRow,
    pub(crate) identity: Option<TaskIdentity>,
    pub(crate) title: Option<String>,
}

pub(crate) struct ReadView {
    pub(crate) rows: Vec<RowDetail>,
    pub(crate) classified: ClassifiedRows,
    pub(crate) identities: plan::IdentityMap,
}

/// Paths in the index and lookup are vault-relative, including the current note.
/// No locks, mutations, receipts, notifications, or implicit clock reads.
pub(crate) fn read<'a>(
    day: &str,
    day_path: &Path,
    note_date: NaiveDate,
    clock: &SnoozeClock,
    note_index: &NoteIndex,
    settings: &note_tasks::NoteTaskSettings,
    mut lookup: impl FnMut(&Path) -> Option<&'a str>,
) -> ReadView {
    let lines: Vec<&str> = day.lines().collect();
    let section = parse::parse_section(&lines);
    let (mut identities, live_links) =
        ledger_identities(day, day_path, note_index);
    let mut details = Vec::new();
    let mut rows = Vec::new();
    let mut scanned_notes = BTreeMap::new();
    if let Some(section) = section {
        for row in section.rows {
            let mut identity = None;
            let mut title = None;
            let mut resolution = ResolutionKind::Missing;
            let mut fact = None;
            if let (Some(target), Some(id)) =
                (row.target.as_deref(), row.block_id.as_deref())
            {
                match note_index.resolve_detailed(Some(day_path), target) {
                    LinkResolution::Found(path) => {
                        let key = TaskIdentity {
                            route: path.clone(),
                            block_id: id.to_string(),
                        };
                        identities.insert(
                            (target.to_string(), id.to_string()),
                            key.clone(),
                        );
                        identity = Some(key);
                        if let Some(contents) = lookup(&path) {
                            let scan = scanned_notes
                                .entry(path.clone())
                                .or_insert_with(|| {
                                    note_tasks::scan(contents, settings)
                                });
                            match scan.by_block_id(id) {
                                note_tasks::BlockIdLookup::Found(task) => {
                                    resolution = ResolutionKind::Found;
                                    let is_task = task.status_type
                                        != TaskStatusType::NonTask;
                                    if is_task {
                                        title = Some(task.description.clone());
                                    }
                                    fact = Some(TaskState {
                                        status: normalized_status(
                                            task.status_symbol,
                                            task.status_type,
                                            &settings.done_statuses,
                                        ),
                                        is_task,
                                        is_event: is_task
                                            && event_task::event_task_at(
                                                &path,
                                                contents,
                                                task.line_index,
                                            )
                                            .is_some(),
                                    });
                                }
                                note_tasks::BlockIdLookup::NotATask {
                                    ..
                                } => {
                                    resolution = ResolutionKind::Found;
                                    fact = Some(TaskState {
                                        status: ' ',
                                        is_task: false,
                                        is_event: false,
                                    });
                                }
                                note_tasks::BlockIdLookup::Duplicate(_) => {
                                    resolution = ResolutionKind::Ambiguous
                                }
                                note_tasks::BlockIdLookup::Missing => {}
                            }
                        }
                    }
                    LinkResolution::Ambiguous => {
                        resolution = ResolutionKind::Ambiguous
                    }
                    LinkResolution::Missing => {}
                }
            }
            rows.push(ClassifyRow {
                line: row.line,
                minutes: row.minutes,
                is_history: row.kind == SnoozeRowKind::History,
                malformed: row.kind == SnoozeRowKind::Malformed,
                resolution,
                identity: identity.clone(),
                task: fact,
                has_live_link: identity
                    .as_ref()
                    .is_some_and(|id| live_links.contains(id)),
            });
            details.push(RowDetail {
                parsed: row,
                identity,
                title,
            });
        }
    }
    ReadView {
        rows: details,
        identities,
        classified: state::classify_rows(&ClassifyInput {
            today: clock.date,
            note_date,
            now_minutes: clock.now_minutes,
            zone: clock.zone,
            rows,
        }),
    }
}

pub(crate) fn ledger_identities(
    day: &str,
    day_path: &Path,
    note_index: &NoteIndex,
) -> (plan::IdentityMap, BTreeSet<TaskIdentity>) {
    let mut identities = plan::IdentityMap::new();
    let mut live = BTreeSet::new();
    let lines = line_spans(day);
    let line_text = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let Some(section) = pomodoro::pomodoros_section_range(&line_text) else {
        return (identities, live);
    };
    let fenced = markdown::fenced_lines(&line_text, section.clone());
    for entry in capture_pomodoros::scan(day).entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        let entry_index = entry.line.saturating_sub(1);
        let range = capture_pomodoros::pomodoro_block_range(
            &line_text,
            entry_index,
            section.end,
        );
        for index in range.start.saturating_add(1)..range.end {
            if fenced.contains(&index)
                || nearest_shallower_list_item_parent(&lines, index)
                    != Some(entry_index)
            {
                continue;
            }
            let text = line_text[index];
            for span in block_link_spans(text) {
                if span.struck {
                    continue;
                }
                let LinkResolution::Found(path) =
                    note_index.resolve_detailed(Some(day_path), &span.target)
                else {
                    continue;
                };
                let identity = TaskIdentity {
                    route: path,
                    block_id: span.block_id.clone(),
                };
                identities
                    .insert((span.target, span.block_id), identity.clone());
                live.insert(identity);
            }
        }
    }
    (identities, live)
}

pub(crate) fn normalized_status(
    status: char,
    kind: TaskStatusType,
    done: &BTreeSet<char>,
) -> char {
    match kind {
        TaskStatusType::Done => 'x',
        TaskStatusType::Cancelled => '-',
        // Bob's Tasks config uses ON_HOLD for both Next (`*`) and
        // Blocked (`?`); only the actual blocked symbol is blocked.
        TaskStatusType::OnHold => status,
        TaskStatusType::NonTask | TaskStatusType::Empty => status,
        TaskStatusType::Todo | TaskStatusType::InProgress => {
            if done.contains(&status) {
                'x'
            } else {
                status
            }
        }
    }
}

/// Membership ignores time-of-day and zone: Asleep, Due, and Blocked all
/// count. Date arguments are still the caller's explicit Bob-zone dates.
pub(crate) fn planned_from_snapshot<'a>(
    day: &str,
    day_path: &Path,
    note_date: NaiveDate,
    today: NaiveDate,
    index: &NoteIndex,
    settings: &note_tasks::NoteTaskSettings,
    lookup: impl FnMut(&Path) -> Option<&'a str>,
) -> BTreeSet<TaskIdentity> {
    let view = read(
        day,
        day_path,
        note_date,
        &SnoozeClock {
            date: today,
            now_minutes: 0,
            zone: chrono_tz::UTC,
        },
        index,
        settings,
        lookup,
    );
    state::live_targets(&view.classified).planned
}

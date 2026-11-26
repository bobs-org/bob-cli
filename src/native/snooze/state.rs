//! Resolved row classification and the `live_targets` reader API.
//!
//! Contract: `docs/snooze.md` § Rows, time, and identity. Snapshot loading
//! (vault reads) stays separate from this pure classifier: callers resolve
//! each row to exactly one `#task` identity, supply the task fact and the
//! live-ledger-link flag, and [`classify_rows`] derives asleep, due,
//! blocked, history, and ignored states plus `next_wake_at`. Later phases
//! (mutation planners, CLI, reader integrations) own all writes.
//!
//! Identity compares resolved vault-relative path plus block id, never
//! spelling: basename, local, embedded, and aliased links that resolve to
//! the same note and block id are the same task.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use chrono::NaiveDate;
use chrono_tz::Tz;

use super::when::format_hhmm;

/// Canonical task identity: resolved vault-relative note route plus block
/// id. Compare this, never link spelling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TaskIdentity {
    pub(crate) route: PathBuf,
    pub(crate) block_id: String,
}

impl TaskIdentity {
    pub(crate) fn key(&self) -> String {
        format!("{}#^{}", self.route.display(), self.block_id)
    }
}

/// How a row's link resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionKind {
    Found,
    Missing,
    Ambiguous,
}

/// Minimal task fact for classification. `status` is the raw checkbox
/// character (`' '`, `'*'`, `'/'`, `'?'`, `'x'`, `'X'`, `'-'`, …).
/// `is_task` is false when the target line is not a `#task` line at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskState {
    pub(crate) status: char,
    pub(crate) is_task: bool,
    pub(crate) is_event: bool,
}

/// One classifier input row: the parsed row plus its resolved identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifyRow {
    /// One-based source line of the row bullet.
    pub(crate) line: usize,
    /// Target minutes since midnight (`None` for malformed/history tails
    /// that carry no usable time).
    pub(crate) minutes: Option<u16>,
    /// Whether this row is struck history.
    pub(crate) is_history: bool,
    /// Whether the row bullet itself is malformed.
    pub(crate) malformed: bool,
    pub(crate) resolution: ResolutionKind,
    pub(crate) identity: Option<TaskIdentity>,
    pub(crate) task: Option<TaskState>,
    /// True when the task has a live link under an open ledger entry.
    pub(crate) has_live_link: bool,
}

/// Classifier inputs: explicit snapshot, no disk writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifyInput {
    /// Today's date in Bob's zone.
    pub(crate) today: NaiveDate,
    /// Date of the note holding the section.
    pub(crate) note_date: NaiveDate,
    /// Current minutes since midnight in Bob's zone.
    pub(crate) now_minutes: u16,
    pub(crate) zone: Tz,
    pub(crate) rows: Vec<ClassifyRow>,
}

/// Ignored-row reason. The first ten spellings are the shared wake/list
/// contract reasons; `previous_day` is row-model-only (wake reads today's
/// note only, so it never appears in a wake report).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IgnoreReason {
    Done,
    Cancelled,
    AlreadyLinked,
    Missing,
    Ambiguous,
    Ready,
    EventTask,
    Duplicate,
    Malformed,
    NotTask,
    PreviousDay,
}

impl IgnoreReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::AlreadyLinked => "already_linked",
            Self::Missing => "missing",
            Self::Ambiguous => "ambiguous",
            Self::Ready => "ready",
            Self::EventTask => "event_task",
            Self::Duplicate => "duplicate",
            Self::Malformed => "malformed",
            Self::NotTask => "not_task",
            Self::PreviousDay => "previous_day",
        }
    }
}

/// Classified row state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    Asleep,
    Due,
    Blocked,
    History,
    Ignored(IgnoreReason),
}

impl RowState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Asleep => "asleep",
            Self::Due => "due",
            Self::Blocked => "blocked",
            Self::History => "woken",
            Self::Ignored(reason) => reason.as_str(),
        }
    }

    /// Whether the row counts as live (actionable by wake).
    pub(crate) fn is_live(self) -> bool {
        matches!(self, Self::Asleep | Self::Due)
    }
}

/// One classified row: source position, identity, state, and wake time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifiedRow {
    pub(crate) line: usize,
    pub(crate) minutes: Option<u16>,
    pub(crate) hhmm: Option<String>,
    pub(crate) identity: Option<TaskIdentity>,
    pub(crate) state: RowState,
    /// True when an Event row also emits the `snoozed_event_task` lint.
    pub(crate) event_lint: bool,
}

/// Classifier output: evaluated rows in source order plus `next_wake_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifiedRows {
    pub(crate) rows: Vec<ClassifiedRow>,
    /// Earliest live target as RFC 3339, or `None` when no live row
    /// remains. Blocked and ignored rows never feed this timer.
    pub(crate) next_wake_at: Option<String>,
}

/// The `live_targets` reader API for the outer planned-today phase.
/// `planned` holds live-or-blocked identities (the exclusion set);
/// `blocked` distinguishes the parked subset. Ready, history, duplicate,
/// and ignored rows never enter planned-today.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveTargets {
    pub(crate) planned: BTreeSet<TaskIdentity>,
    pub(crate) blocked: BTreeSet<TaskIdentity>,
}

/// Pure classification: no vault reads, no clock reads. `input.zone` and
/// `input.today` render `next_wake_at`; every other input is explicit.
pub(crate) fn classify_rows(input: &ClassifyInput) -> ClassifiedRows {
    let note_is_today = input.note_date == input.today;
    // Base states before duplicate resolution.
    let mut base: Vec<RowState> = Vec::new();
    for row in &input.rows {
        base.push(base_state(row, note_is_today, input.now_minutes));
    }
    // Duplicate resolution: among rows that are otherwise live-or-blocked
    // for the same identity, the earliest target time wins and ties use
    // source order; the rest become `duplicate`.
    let mut best: BTreeMap<TaskIdentity, (u16, usize)> = BTreeMap::new();
    for (position, row) in input.rows.iter().enumerate() {
        let state = base[position];
        if !matches!(
            state,
            RowState::Asleep | RowState::Due | RowState::Blocked
        ) {
            continue;
        }
        let Some(identity) = row.identity.clone() else {
            continue;
        };
        let minutes = row.minutes.unwrap_or(u16::MAX);
        let entry = best.entry(identity).or_insert((minutes, position));
        if (minutes, position) < *entry {
            *entry = (minutes, position);
        }
    }
    let mut winners: BTreeSet<usize> = BTreeSet::new();
    for (_, position) in best.values() {
        winners.insert(*position);
    }
    let mut rows = Vec::new();
    for (position, row) in input.rows.iter().enumerate() {
        let mut state = base[position];
        // The earliest row per identity keeps its state; every other
        // live-or-blocked row for the same identity is a duplicate.
        // `winners` already encodes earliest-time, source-order ties.
        if row.identity.is_some()
            && matches!(
                state,
                RowState::Asleep | RowState::Due | RowState::Blocked
            )
            && !winners.contains(&position)
        {
            state = RowState::Ignored(IgnoreReason::Duplicate);
        }
        rows.push(ClassifiedRow {
            line: row.line,
            minutes: row.minutes,
            hhmm: row.minutes.map(format_hhmm),
            identity: row.identity.clone(),
            state,
            event_lint: is_event_row(row),
        });
    }
    let next_wake_at = next_wake_at(&rows, input);
    ClassifiedRows { rows, next_wake_at }
}

/// Documented `live_targets` helper for the outer planned-today phase.
/// The planned set includes the blocked targets; the separately exposed
/// blocked set distinguishes them. Keep `Today` strictly ledger-derived:
/// callers union this set only at exclusion sites.
pub(crate) fn live_targets(classified: &ClassifiedRows) -> LiveTargets {
    let mut targets = LiveTargets::default();
    for row in &classified.rows {
        let Some(identity) = row.identity.clone() else {
            continue;
        };
        match row.state {
            RowState::Asleep | RowState::Due => {
                targets.planned.insert(identity);
            }
            RowState::Blocked => {
                targets.planned.insert(identity.clone());
                targets.blocked.insert(identity);
            }
            RowState::History | RowState::Ignored(_) => {}
        }
    }
    targets
}

fn base_state(
    row: &ClassifyRow,
    note_is_today: bool,
    now_minutes: u16,
) -> RowState {
    if row.is_history {
        return RowState::History;
    }
    if !note_is_today {
        return RowState::Ignored(IgnoreReason::PreviousDay);
    }
    if row.malformed || row.minutes.is_none() {
        return RowState::Ignored(IgnoreReason::Malformed);
    }
    match row.resolution {
        ResolutionKind::Missing => {
            return RowState::Ignored(IgnoreReason::Missing);
        }
        ResolutionKind::Ambiguous => {
            return RowState::Ignored(IgnoreReason::Ambiguous);
        }
        ResolutionKind::Found => {}
    }
    let Some(task) = row.task else {
        return RowState::Ignored(IgnoreReason::NotTask);
    };
    if !task.is_task {
        return RowState::Ignored(IgnoreReason::NotTask);
    }
    if task.is_event {
        return RowState::Ignored(IgnoreReason::EventTask);
    }
    if is_done_status(task.status) {
        return RowState::Ignored(IgnoreReason::Done);
    }
    if is_cancelled_status(task.status) {
        return RowState::Ignored(IgnoreReason::Cancelled);
    }
    if row.has_live_link {
        return RowState::Ignored(IgnoreReason::AlreadyLinked);
    }
    match task.status {
        '?' => RowState::Blocked,
        ' ' => RowState::Ignored(IgnoreReason::Ready),
        '*' | '/' => {
            let minutes = row.minutes.unwrap_or(u16::MAX);
            if minutes <= now_minutes {
                RowState::Due
            } else {
                RowState::Asleep
            }
        }
        _ => RowState::Ignored(IgnoreReason::NotTask),
    }
}

fn is_event_row(row: &ClassifyRow) -> bool {
    !row.is_history
        && row.resolution == ResolutionKind::Found
        && row.task.is_some_and(|task| task.is_task && task.is_event)
}

fn is_done_status(status: char) -> bool {
    matches!(status, 'x' | 'X')
}

fn is_cancelled_status(status: char) -> bool {
    matches!(status, '-')
}

fn next_wake_at(
    rows: &[ClassifiedRow],
    input: &ClassifyInput,
) -> Option<String> {
    // Earliest live target; asleep rows sort before due rows only by
    // target time, and ties use source order. Blocked and ignored rows
    // never feed the timer.
    let mut best: Option<(u16, usize)> = None;
    for (position, row) in rows.iter().enumerate() {
        if !row.state.is_live() {
            continue;
        }
        let Some(minutes) = row.minutes else {
            continue;
        };
        let candidate = (minutes, position);
        if best.is_none_or(|current| candidate < current) {
            best = Some(candidate);
        }
    }
    let (minutes, _) = best?;
    super::when::resolve_snooze_target(input.today, minutes, input.zone)
        .map(|resolved| resolved.rfc3339)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::Tz;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 25).expect("date")
    }

    fn identity(route: &str, id: &str) -> TaskIdentity {
        TaskIdentity {
            route: PathBuf::from(route),
            block_id: id.to_string(),
        }
    }

    fn row(
        line: usize,
        minutes: u16,
        status: char,
        is_event: bool,
        live_link: bool,
    ) -> ClassifyRow {
        ClassifyRow {
            line,
            minutes: Some(minutes),
            is_history: false,
            malformed: false,
            resolution: ResolutionKind::Found,
            identity: Some(identity("job.md", "x")),
            task: Some(TaskState {
                status,
                is_task: true,
                is_event,
            }),
            has_live_link: live_link,
        }
    }

    fn input(rows: Vec<ClassifyRow>, now: u16) -> ClassifyInput {
        ClassifyInput {
            today: today(),
            note_date: today(),
            now_minutes: now,
            zone: Tz::UTC,
            rows,
        }
    }

    #[test]
    fn asleep_due_blocked_and_ignored_states() {
        let classified = classify_rows(&input(
            vec![
                ClassifyRow {
                    minutes: Some(19 * 60),
                    identity: Some(identity("job.md", "a")),
                    ..row(2, 19 * 60, '*', false, false)
                },
                ClassifyRow {
                    minutes: Some(15 * 60 + 30),
                    identity: Some(identity("job.md", "b")),
                    ..row(3, 15 * 60 + 30, '/', false, false)
                },
                ClassifyRow {
                    minutes: Some(19 * 60),
                    identity: Some(identity("job.md", "c")),
                    ..row(4, 19 * 60, '?', false, false)
                },
                ClassifyRow {
                    minutes: Some(19 * 60),
                    identity: Some(identity("job.md", "d")),
                    ..row(5, 19 * 60, ' ', false, false)
                },
            ],
            16 * 60,
        ));
        assert_eq!(classified.rows[0].state, RowState::Asleep);
        assert_eq!(classified.rows[1].state, RowState::Due);
        assert_eq!(classified.rows[2].state, RowState::Blocked);
        assert_eq!(
            classified.rows[3].state,
            RowState::Ignored(IgnoreReason::Ready)
        );
    }

    #[test]
    fn live_link_wins_and_event_emits_lint() {
        let classified = classify_rows(&input(
            vec![
                ClassifyRow {
                    identity: Some(identity("job.md", "a")),
                    ..row(2, 19 * 60, '*', false, true)
                },
                ClassifyRow {
                    identity: Some(identity("job.md", "e")),
                    ..row(3, 19 * 60, '*', true, false)
                },
            ],
            16 * 60,
        ));
        assert_eq!(
            classified.rows[0].state,
            RowState::Ignored(IgnoreReason::AlreadyLinked)
        );
        assert_eq!(
            classified.rows[1].state,
            RowState::Ignored(IgnoreReason::EventTask)
        );
        assert!(classified.rows[1].event_lint);
    }

    #[test]
    fn earliest_time_wins_ties_use_source_order() {
        let classified = classify_rows(&input(
            vec![
                ClassifyRow {
                    line: 2,
                    minutes: Some(19 * 60),
                    identity: Some(identity("job.md", "same")),
                    ..row(2, 19 * 60, '*', false, false)
                },
                ClassifyRow {
                    line: 3,
                    minutes: Some(15 * 60 + 30),
                    identity: Some(identity("job.md", "same")),
                    ..row(3, 15 * 60 + 30, '*', false, false)
                },
                ClassifyRow {
                    line: 4,
                    minutes: Some(15 * 60 + 30),
                    identity: Some(identity("job.md", "same")),
                    ..row(4, 15 * 60 + 30, '*', false, false)
                },
            ],
            13 * 60,
        ));
        // The 15:30 rows sort first; source order picks line 3.
        assert_eq!(
            classified.rows[0].state,
            RowState::Ignored(IgnoreReason::Duplicate)
        );
        assert_eq!(classified.rows[1].state, RowState::Asleep);
        assert_eq!(
            classified.rows[2].state,
            RowState::Ignored(IgnoreReason::Duplicate)
        );
    }

    #[test]
    fn live_targets_include_blocked_but_exclude_others() {
        let classified = classify_rows(&input(
            vec![
                ClassifyRow {
                    identity: Some(identity("job.md", "a")),
                    ..row(2, 19 * 60, '*', false, false)
                },
                ClassifyRow {
                    identity: Some(identity("job.md", "c")),
                    ..row(3, 19 * 60, '?', false, false)
                },
                ClassifyRow {
                    identity: Some(identity("job.md", "d")),
                    ..row(4, 19 * 60, ' ', false, false)
                },
                ClassifyRow {
                    line: 5,
                    minutes: Some(19 * 60),
                    is_history: true,
                    malformed: false,
                    resolution: ResolutionKind::Found,
                    identity: Some(identity("job.md", "h")),
                    task: Some(TaskState {
                        status: '*',
                        is_task: true,
                        is_event: false,
                    }),
                    has_live_link: false,
                },
            ],
            16 * 60,
        ));
        let targets = live_targets(&classified);
        assert!(targets.planned.contains(&identity("job.md", "a")));
        assert!(targets.planned.contains(&identity("job.md", "c")));
        assert!(targets.blocked.contains(&identity("job.md", "c")));
        assert!(!targets.planned.contains(&identity("job.md", "d")));
        assert!(!targets.planned.contains(&identity("job.md", "h")));
    }

    #[test]
    fn next_wake_at_ignores_blocked_and_previous_day() {
        let mut yesterday =
            input(vec![row(2, 19 * 60, '*', false, false)], 16 * 60);
        yesterday.note_date =
            NaiveDate::from_ymd_opt(2026, 11, 24).expect("yesterday");
        let classified = classify_rows(&yesterday);
        assert_eq!(
            classified.rows[0].state,
            RowState::Ignored(IgnoreReason::PreviousDay)
        );
        assert_eq!(classified.next_wake_at, None);

        let classified = classify_rows(&input(
            vec![
                ClassifyRow {
                    identity: Some(identity("job.md", "b")),
                    ..row(2, 19 * 60, '?', false, false)
                },
                ClassifyRow {
                    identity: Some(identity("job.md", "a")),
                    ..row(3, 20 * 60, '*', false, false)
                },
            ],
            16 * 60,
        ));
        assert!(classified
            .next_wake_at
            .as_deref()
            .is_some_and(|value| value.contains("2026-11-25T20:00:00")));
    }
}

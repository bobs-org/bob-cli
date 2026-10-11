use std::collections::BTreeMap;

use chrono::NaiveDate;

/// Shared vault and journal facts for evaluators. The inputs phase
/// fills this; no evaluator performs I/O.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatsIndex {
    pub(crate) tasks: Vec<IndexedTask>,
    pub(crate) daily_notes: BTreeMap<NaiveDate, DailyNoteInputs>,
    pub(crate) parent_of: BTreeMap<String, String>,
    pub(crate) journal_events: Vec<IndexedJournalEvent>,
    pub(crate) health: IndexHealth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedTask {
    pub(crate) source_path: String,
    pub(crate) archive_path: Option<String>,
    pub(crate) status_type: String,
    pub(crate) created: DatedValue,
    pub(crate) done: DatedValue,
    pub(crate) cancelled: DatedValue,
    pub(crate) scheduled: DatedValue,
    pub(crate) recurring: bool,
    pub(crate) tags: Vec<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) description: String,
    pub(crate) task_kind: TaskKindPart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKindPart {
    Task,
    Prj,
    Ref,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DatedValue {
    pub(crate) raw: Option<String>,
    pub(crate) date: Option<NaiveDate>,
    pub(crate) diagnostic: DateDiagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DateDiagnostic {
    #[default]
    Missing,
    Ok,
    Malformed,
    Future,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DailyNoteInputs {
    pub(crate) path: String,
    pub(crate) has_pomodoros_heading: bool,
    pub(crate) closed_pomodoros: Vec<PomodoroSession>,
    pub(crate) frontmatter_fields: BTreeMap<String, String>,
    pub(crate) inline_fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroSession {
    pub(crate) name: Option<String>,
    pub(crate) recorded_minutes: Option<u32>,
    pub(crate) range_minutes: Option<u32>,
    pub(crate) line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedJournalEvent {
    pub(crate) day: NaiveDate,
    pub(crate) ts: String,
    pub(crate) host: String,
    pub(crate) tier: String,
    pub(crate) outcome: String,
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IndexHealth {
    pub(crate) invalid_dates: u32,
    pub(crate) closed_without_date: u32,
    pub(crate) missing_created: u32,
    pub(crate) journal: JournalHealth,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JournalHealth {
    pub(crate) since: Option<NaiveDate>,
    pub(crate) malformed_lines: u32,
    pub(crate) hosts: Vec<JournalHostHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalHostHealth {
    pub(crate) host: String,
    pub(crate) events: u32,
    pub(crate) last_ts: String,
    pub(crate) last_day: NaiveDate,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::NaiveDate;

    use super::*;

    #[test]
    fn default_index_is_empty() {
        let index = StatsIndex::default();
        assert!(index.tasks.is_empty());
        assert!(index.daily_notes.is_empty());
        assert!(index.parent_of.is_empty());
        assert!(index.journal_events.is_empty());
        assert_eq!(index.health.invalid_dates, 0);
        assert_eq!(index.health.closed_without_date, 0);
        assert_eq!(index.health.missing_created, 0);
        assert!(index.health.journal.hosts.is_empty());
    }

    #[test]
    fn interface_rows_hold_residence_dates_and_journal_facts() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let task = IndexedTask {
            source_path: "sase.md".to_string(),
            archive_path: Some("done/sase_done.md".to_string()),
            status_type: "DONE".to_string(),
            created: DatedValue {
                raw: Some("2026-10-01".to_string()),
                date: Some(day),
                diagnostic: DateDiagnostic::Ok,
            },
            done: DatedValue {
                raw: Some("bad".to_string()),
                date: None,
                diagnostic: DateDiagnostic::Malformed,
            },
            cancelled: DatedValue {
                raw: None,
                date: None,
                diagnostic: DateDiagnostic::Missing,
            },
            scheduled: DatedValue {
                raw: Some("2099-01-01".to_string()),
                date: NaiveDate::from_ymd_opt(2099, 1, 1),
                diagnostic: DateDiagnostic::Future,
            },
            recurring: false,
            tags: vec!["ref".to_string()],
            block_id: Some("ref".to_string()),
            description: "read".to_string(),
            task_kind: TaskKindPart::Ref,
        };
        assert_eq!(task.task_kind, TaskKindPart::Ref);
        assert_eq!(TaskKindPart::Prj, TaskKindPart::Prj);
        assert_eq!(TaskKindPart::Task, TaskKindPart::Task);
        let daily = DailyNoteInputs {
            path: "2026/20261011.md".to_string(),
            has_pomodoros_heading: true,
            closed_pomodoros: vec![PomodoroSession {
                name: Some("GTD".to_string()),
                recorded_minutes: Some(25),
                range_minutes: Some(30),
                line: 12,
            }],
            frontmatter_fields: BTreeMap::from([(
                "energy".to_string(),
                "3".to_string(),
            )]),
            inline_fields: BTreeMap::new(),
        };
        assert_eq!(daily.closed_pomodoros[0].recorded_minutes, Some(25));
        let event = IndexedJournalEvent {
            day,
            ts: "2026-10-11T05:41:07-04:00".to_string(),
            host: "apollo".to_string(),
            tier: "next".to_string(),
            outcome: "keep".to_string(),
            key: "sase.md#x".to_string(),
            path: ".bob/stats/review/2026-10.apollo.jsonl".to_string(),
            line: 1,
        };
        assert_eq!(event.outcome, "keep");
        let host = JournalHostHealth {
            host: "apollo".to_string(),
            events: 41,
            last_ts: event.ts.clone(),
            last_day: day,
        };
        assert_eq!(host.events, 41);
    }
}

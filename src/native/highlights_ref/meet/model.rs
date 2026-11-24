//! Read-only preparation models shared by `ref meet`'s later write routes.

use std::path::PathBuf;

use serde::Serialize;

use crate::native::{
    meetings::{FiledNotes, MeetingNotes, MeetingRow},
    parent_notes::{parent_kind_name, ParentCandidate},
};

use super::super::{recapture::MeetingOverwritePreflight, stamp::TargetPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatchMethod {
    Explicit,
    Query,
    Auto,
    Picked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MatchEvidence {
    pub method: MatchMethod,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TranscriptDisposition {
    Kept,
    Omitted,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectiveMeetingsConfig {
    pub producer: Option<String>,
    pub relation_label: String,
    pub status: String,
    pub window_days: u32,
}

/// The complete, immutable input snapshot needed by first filing, overwrite,
/// and the eventual command/report layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedFiling {
    pub vault_dir: PathBuf,
    pub meeting: MeetingRow,
    pub notes: MeetingNotes,
    pub input_sha256: String,
    pub match_evidence: MatchEvidence,
    pub parent: PreparedParent,
    pub guarded_root_edit: Option<GuardedRootEdit>,
    pub effective_config: EffectiveMeetingsConfig,
    pub status: String,
    pub status_explicit: bool,
    pub title: String,
    pub stem: String,
    pub target: TargetPlan,
    pub note_path: PathBuf,
    pub task_block_id: String,
    pub relation_target: String,
    pub existing: Option<ExistingReferenceSelection>,
    pub include_transcript: bool,
    pub transcript: TranscriptDisposition,
    pub edited: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExistingReferenceSelection {
    pub filed: FiledNotes,
    pub overwrite: MeetingOverwritePreflight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedParent {
    pub route: String,
    pub kind: String,
    pub label: String,
    pub explicit: bool,
}

/// Exact daily-note preimage for the only automatic event-root edit the
/// filing flow may make. The intake transaction rechecks these bytes while
/// holding its existing locks before applying the new root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardedRootEdit {
    pub path: PathBuf,
    pub line: usize,
    pub block_id: String,
    pub event_id: Option<String>,
    pub from: Option<String>,
    pub to: String,
    pub expected_line: String,
    pub expected_file_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetingCandidate {
    pub meeting: MeetingRow,
    pub detail: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ParentChoice {
    pub route: String,
    pub kind: String,
    pub label: String,
    pub aliases: Vec<String>,
    pub status: Option<String>,
}

impl From<ParentCandidate> for ParentChoice {
    fn from(candidate: ParentCandidate) -> Self {
        Self {
            route: candidate.route,
            kind: parent_kind_name(&candidate.kind).to_string(),
            label: candidate.label,
            aliases: candidate.aliases,
            status: candidate.status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrepareError {
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
    pub root: Option<String>,
    pub candidates: Vec<MeetingCandidate>,
    pub parent_candidates: Vec<ParentChoice>,
    pub existing: Option<FiledNotes>,
    /// The CLI contract maps selection-required refusals to exit 2.
    pub exit_code: i32,
}

impl PrepareError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        exit_code: i32,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: None,
            root: None,
            candidates: Vec::new(),
            parent_candidates: Vec::new(),
            existing: None,
            exit_code,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MeetingNotesLink {
    pub note: String,
    pub task: String,
    pub status: String,
}

impl From<FiledNotes> for MeetingNotesLink {
    fn from(filed: FiledNotes) -> Self {
        Self {
            note: filed.note,
            task: filed.task,
            status: filed.status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MeetingRowReport {
    #[serde(rename = "ref")]
    pub event_ref: Option<String>,
    pub path: String,
    pub block_id: Option<String>,
    pub date: String,
    pub start: String,
    pub end: String,
    pub title: String,
    pub root: Option<String>,
    pub root_is_inbox: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    pub notes: Option<MeetingNotesLink>,
}

impl MeetingRowReport {
    pub fn from_row(meeting: &MeetingRow, notes: Option<FiledNotes>) -> Self {
        Self {
            event_ref: meeting.block_id.as_deref().and_then(|block_id| {
                crate::native::meetings::event_ref_for(&meeting.path, block_id)
            }),
            path: meeting.path.to_string_lossy().replace('\\', "/"),
            block_id: meeting.block_id.clone(),
            date: meeting.date.format("%Y-%m-%d").to_string(),
            start: meeting.starts_at.format("%H:%M").to_string(),
            end: meeting.ends_at.format("%H:%M").to_string(),
            title: meeting.title.clone(),
            root: meeting.root.clone(),
            root_is_inbox: meeting.root_is_inbox,
            status: meeting.status.to_string(),
            event_id: meeting.event_id.clone(),
            notes: notes.map(MeetingNotesLink::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NotesSectionOutline {
    pub heading: Option<String>,
    /// Number of normalized items. The report does not duplicate section
    /// contents, which remain in the dossier input.
    pub items: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NextStepOutline {
    pub owner: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NotesOutline {
    pub tldr: String,
    pub next_steps: Vec<NextStepOutline>,
    pub sections: Vec<NotesSectionOutline>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NotesReport {
    pub sha256: String,
    pub shape: crate::native::meetings::NotesShape,
    pub producer: Option<String>,
    pub outline: NotesOutline,
    pub transcript: TranscriptDisposition,
}

impl NotesReport {
    pub fn from_prepared(prepared: &PreparedFiling) -> Self {
        Self {
            sha256: prepared.input_sha256.clone(),
            shape: prepared.notes.shape,
            producer: prepared.effective_config.producer.clone(),
            outline: NotesOutline {
                tldr: prepared.notes.tldr.clone(),
                next_steps: prepared
                    .notes
                    .next_steps
                    .iter()
                    .map(|step| NextStepOutline {
                        owner: step.owner.clone(),
                        action: step.action.clone(),
                    })
                    .collect(),
                sections: prepared
                    .notes
                    .sections
                    .iter()
                    .map(|section| NotesSectionOutline {
                        heading: section.heading.clone(),
                        items: section.items.len(),
                    })
                    .collect(),
            },
            transcript: prepared.transcript,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RerootReport {
    pub from: Option<String>,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TaskPlanReport {
    pub destination: String,
    pub block_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RelationReport {
    pub label: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FilingReport {
    pub parent: String,
    pub parent_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reroot: Option<RerootReport>,
    pub title: String,
    pub stem: String,
    pub note: String,
    pub pdf: String,
    pub task: TaskPlanReport,
    pub relation: RelationReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MatchReport {
    pub method: MatchMethod,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MeetingPrepareReport {
    pub outcome: &'static str,
    pub meeting: MeetingRowReport,
    #[serde(rename = "match")]
    pub match_report: MatchReport,
    pub filing: FilingReport,
    pub notes: NotesReport,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MeetingListReport {
    pub meetings: Vec<MeetingRowReport>,
}

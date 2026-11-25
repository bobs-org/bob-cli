//! Meeting notes and read-only meeting resolution.
//!
//! The notes layer handles pure meeting-note intake, normalization, dossier,
//! and recap rendering. Dossiers use Pandoc `commonmark_x` syntax. Their YAML
//! front matter carries the resolved meeting metadata and normalized model for
//! exact round trips; the visible body uses a fenced div with the
//! `meeting-tldr` class, followed by the next-step and section headings.
//! Transcript text is rendered only when `DossierMeta` opts in. The meeting
//! print profile renders that dossier Markdown to a one-to-two-page PDF.
//!
//! The resolution layer resolves EVENT tokens to Event Task rows for
//! `bob ref meet`, scores automatic matches with evidence, lists recent
//! meetings with their filed-notes state, looks up Google identity, resolves
//! the filing parent, and adds the additive notes state to the capture
//! agenda's event rows. It never writes the vault: every function here reads
//! files and folds journals.
//!
//! Token parsing, matching, parent resolution, and event resolution are
//! shared with the `ref meet` CLI phase; only the agenda surface has a
//! caller yet, so the not-yet-wired modules stay warning-free behind
//! `cfg_attr(not(test), ...)` until their phases land, exactly like the
//! task merge/split helpers in [`super::task_locator`]. The notes layer
//! stays behind the top-level gate on [`super::meetings`] until a production
//! caller wires it.

mod agenda;
mod birth;
mod dossier;
mod intake;
mod matching;
mod model;
mod normalize;
mod parent;
#[cfg_attr(not(test), allow(dead_code))]
mod profile;
mod recap;
mod resolve;
mod token;

#[cfg(test)]
mod tests;

// The agenda surface: the only consumer outside this module so far.
pub(crate) use agenda::{agenda_meetings, MeetingNotesState};

// These crate-facing APIs are staged for the meeting-reference integration phases.
pub(crate) use birth::{
    meeting_links_child, render_birth_recap, render_empty_recap,
};
#[allow(unused_imports)]
pub(crate) use dossier::{
    parse_dossier, render_dossier, DossierDocument, DossierError,
};
pub(crate) use intake::{
    read_notes, NotesError, NotesInput, NotesSource, MAX_NOTES_BYTES,
};
pub(crate) use matching::{
    auto_match, content_words, AutoMatch, ScoredCandidate,
};
pub(crate) use model::{
    collect_window_rows, event_ref_for, filed_index_for, filed_notes_index,
    parse_event_link, vault_index, FiledNotes, FiledNotesIndex, MeetingRow,
};
pub(crate) use normalize::normalize;
pub(crate) use parent::{meeting_parent, MeetingParent};
#[allow(unused_imports)]
pub(crate) use profile::{render_meeting_pdf, MeetingMeta};
pub(crate) use recap::{
    apply_overwrite_recap, recap_preservation_span, recap_state, render_recap,
    replace_recap, strip_recap, RecapOverwrite, RecapState, ReplaceRecapError,
    EDITED_RECAP_WARNING,
};
pub(crate) use resolve::{
    recent_meetings, resolve_event, AmbiguousCandidate, RecentMeeting,
    ResolveError,
};
pub(crate) use token::{parse_event_token, EventToken};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MeetingNotes {
    pub(crate) own_title: Option<String>,
    pub(crate) tldr: String,
    pub(crate) sections: Vec<MeetingSection>,
    pub(crate) next_steps: Vec<MeetingNextStep>,
    pub(crate) transcript: Option<String>,
    pub(crate) shape: NotesShape,
    pub(crate) warnings: Vec<String>,
    /// Preserves the original order of sections and next steps.
    pub(crate) content_order: Vec<ContentRef>,
    pub(crate) next_steps_heading: Option<String>,
    pub(crate) next_steps_level: u8,
}

impl Default for MeetingNotes {
    fn default() -> Self {
        Self {
            own_title: None,
            tldr: String::new(),
            sections: Vec::new(),
            next_steps: Vec::new(),
            transcript: None,
            shape: NotesShape::PlainParagraphs,
            warnings: Vec::new(),
            content_order: Vec::new(),
            next_steps_heading: None,
            next_steps_level: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MeetingSection {
    pub(crate) heading: Option<String>,
    pub(crate) level: u8,
    pub(crate) items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MeetingNextStep {
    pub(crate) owner: Option<String>,
    pub(crate) action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotesShape {
    WisprPlainText,
    Markdown,
    PlainParagraphs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContentRef {
    Section(usize),
    NextSteps,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DossierMeta {
    pub(crate) event_title: String,
    pub(crate) date: String,
    pub(crate) span: String,
    pub(crate) root: String,
    pub(crate) producer: Option<String>,
    pub(crate) filed_date: String,
    pub(crate) include_transcript: bool,
    pub(crate) edited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecapMeta {
    pub(crate) date: String,
    pub(crate) span: String,
    pub(crate) event_link: String,
    pub(crate) producer: Option<String>,
}

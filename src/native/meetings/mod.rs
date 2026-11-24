//! Pure meeting-note intake, normalization, dossier, and recap rendering.
//!
//! Dossiers use Pandoc `commonmark_x` syntax. Their YAML front matter carries the
//! resolved meeting metadata and normalized model for exact round trips; the visible
//! body uses a fenced div with the `meeting-tldr` class, followed by the next-step and
//! section headings. Transcript text is rendered only when `DossierMeta` opts in.

mod dossier;
mod intake;
mod normalize;
mod recap;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub(crate) use dossier::{
    parse_dossier, render_dossier, DossierDocument, DossierError,
};
pub(crate) use intake::{
    read_notes, NotesError, NotesInput, NotesSource, MAX_NOTES_BYTES,
};
pub(crate) use normalize::normalize;
pub(crate) use recap::{
    recap_state, render_recap, replace_recap, RecapState, ReplaceRecapError,
};

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

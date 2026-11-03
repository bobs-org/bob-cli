use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ClipMode {
    Inline,
    Lines,
    Attachments,
    Snippet,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AttachmentKind {
    Image,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AttachmentOutput {
    pub(crate) source: String,
    pub(crate) saved: String,
    pub(crate) kind: AttachmentKind,
    pub(crate) reused: bool,
    /// SHA-256 of the planned bytes. Never serialized: `bob capture`'s
    /// JSON stays byte-identical, and the origin journal reads this field
    /// instead.
    #[serde(skip_serializing)]
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipStatus {
    Read,
    NotRead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipOutput {
    pub(crate) status: ClipStatus,
    pub(crate) count: Option<usize>,
    pub(crate) mode: ClipMode,
    pub(crate) lines: Vec<String>,
    pub(crate) attachments: Vec<AttachmentOutput>,
    pub(crate) snippet: Option<String>,
    // bob-mac-capture decodes clip collections as required fields; omitting
    // empty entries broke every `%c` capture in the app.
    pub(crate) entries: Vec<ClipOutput>,
}

impl ClipOutput {
    pub(crate) fn not_read(count: usize) -> Self {
        Self {
            status: ClipStatus::NotRead,
            count: Some(count),
            mode: ClipMode::Inline,
            lines: Vec::new(),
            attachments: Vec::new(),
            snippet: None,
            entries: Vec::new(),
        }
    }
}

impl ClipPlan {
    pub(crate) fn not_read(count: usize) -> Self {
        Self {
            output: ClipOutput::not_read(count),
            files: Vec::new(),
        }
    }
}

impl Serialize for ClipOutput {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match self.status {
            ClipStatus::NotRead => {
                let mut state = serializer.serialize_struct("ClipOutput", 2)?;
                state.serialize_field("status", &self.status)?;
                state.serialize_field("count", &self.count.unwrap_or(0))?;
                state.end()
            }
            ClipStatus::Read => {
                let mut field_count = 5;
                if self.snippet.is_some() {
                    field_count += 1;
                }
                let mut state =
                    serializer.serialize_struct("ClipOutput", field_count)?;
                state.serialize_field("status", &self.status)?;
                state.serialize_field("mode", &self.mode)?;
                state.serialize_field("lines", &self.lines)?;
                state.serialize_field("attachments", &self.attachments)?;
                if let Some(snippet) = &self.snippet {
                    state.serialize_field("snippet", snippet)?;
                }
                state.serialize_field("entries", &self.entries)?;
                state.end()
            }
        }
    }
}

impl ClipOutput {
    pub(crate) fn file_confirmations(&self) -> Vec<(String, bool)> {
        let outputs = if self.entries.is_empty() {
            std::slice::from_ref(self)
        } else {
            self.entries.as_slice()
        };
        let mut seen = HashSet::new();
        let mut confirmations = Vec::new();
        for output in outputs {
            for attachment in &output.attachments {
                if seen.insert(attachment.saved.clone()) {
                    confirmations
                        .push((attachment.saved.clone(), attachment.reused));
                }
            }
            if let Some(snippet) = &output.snippet
                && seen.insert(snippet.clone())
            {
                confirmations.push((snippet.clone(), false));
            }
        }
        confirmations
    }
}

#[derive(Debug, Clone)]
pub(super) struct PlannedFile {
    pub(super) destination: PathBuf,
    pub(super) contents: Vec<u8>,
    pub(super) reused: bool,
    pub(super) kind: PlannedFileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlannedFileKind {
    Attachment,
    Snippet,
}

#[derive(Debug, Clone)]
pub(crate) struct ClipPlan {
    pub(crate) output: ClipOutput,
    pub(super) files: Vec<PlannedFile>,
}

#[derive(Debug, Default)]
pub(crate) struct ClipReservations {
    pub(super) files: FileReservations,
}

#[derive(Debug, Default)]
pub(super) struct FileReservations {
    pub(super) files: Vec<PlannedFile>,
    by_destination: HashMap<PathBuf, usize>,
}

impl FileReservations {
    pub(super) fn contains(&self, destination: &Path) -> bool {
        self.by_destination.contains_key(destination)
    }

    pub(super) fn get(&self, destination: &Path) -> Option<&PlannedFile> {
        self.by_destination
            .get(destination)
            .map(|index| &self.files[*index])
    }

    pub(super) fn reserve(
        &mut self,
        destination: PathBuf,
        contents: Vec<u8>,
        reused: bool,
        kind: PlannedFileKind,
    ) {
        let index = self.files.len();
        self.by_destination.insert(destination.clone(), index);
        self.files.push(PlannedFile {
            destination,
            contents,
            reused,
            kind,
        });
    }
}

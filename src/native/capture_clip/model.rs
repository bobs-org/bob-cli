use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::Serialize;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ClipOutput {
    pub(crate) header: Option<String>,
    pub(crate) mode: ClipMode,
    pub(crate) lines: Vec<String>,
    pub(crate) attachments: Vec<AttachmentOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) snippet: Option<String>,
    // bob-mac-capture decodes clip collections as required fields; omitting
    // empty entries broke every `%` capture in the app.
    pub(crate) entries: Vec<ClipOutput>,
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

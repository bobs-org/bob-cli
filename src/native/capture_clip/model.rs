use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
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
    /// Provenance belongs to the aggregate history result only. Ordinary
    /// `%c` output intentionally keeps its existing JSON shape.
    pub(crate) history: Option<ClipHistoryOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ClipHistoryOutput {
    pub(crate) provider: String,
    pub(crate) entries: Vec<ClipHistoryEntryOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ClipHistoryEntryOutput {
    pub(crate) age_seconds: Option<u64>,
    pub(crate) app: Option<String>,
}

impl ClipHistoryOutput {
    pub(crate) fn from_rows(
        provider: &str,
        rows: &[super::history::HistoryEntry],
        now: SystemTime,
    ) -> Self {
        Self {
            provider: provider.to_string(),
            entries: rows
                .iter()
                .map(|row| ClipHistoryEntryOutput {
                    age_seconds: row.at.map(|at| {
                        now.duration_since(at)
                            .unwrap_or(Duration::ZERO)
                            .as_secs()
                    }),
                    app: row.app.clone(),
                })
                .collect(),
        }
    }
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
            history: None,
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
                if self.history.is_some() {
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
                if let Some(history) = &self.history {
                    state.serialize_field("history", history)?;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_history_ages_are_clamped_and_unknowns_stay_null() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let rows = vec![
            super::super::history::HistoryEntry::text(
                "newest".to_string(),
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(97)),
                Some("Safari".to_string()),
            ),
            super::super::history::HistoryEntry::text(
                "older".to_string(),
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(101)),
                None,
            ),
            super::super::history::HistoryEntry::text(
                "unknown".to_string(),
                None,
                None,
            ),
        ];

        let history = ClipHistoryOutput::from_rows("alfred", &rows, now);
        assert_eq!(history.provider, "alfred");
        assert_eq!(history.entries[0].age_seconds, Some(3));
        assert_eq!(history.entries[0].app.as_deref(), Some("Safari"));
        assert_eq!(history.entries[1].age_seconds, Some(0));
        assert_eq!(history.entries[1].app, None);
        assert_eq!(history.entries[2].age_seconds, None);
        assert_eq!(history.entries[2].app, None);
        let json = serde_json::to_value(history).expect("serialize provenance");
        assert_eq!(json["entries"][1]["age_seconds"], 0);
        assert_eq!(json["entries"][2]["age_seconds"], serde_json::Value::Null);
        assert_eq!(json["entries"][2]["app"], serde_json::Value::Null);
    }
}

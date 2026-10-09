//! Row model for the read-only reference-library index.
//!
//! [`RefRow`] is the one-row-per-note serde model from the `index` phase of
//! `plan:202610/bob_ref_reference_library.md`. Field order below is the JSON
//! field order. Only `frontmatter_status` is conditional (`present only when
//! `status_sync` is not `ok`); every other `Option` serializes as `null`.
use serde::Serialize;

/// One indexed reference note.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct RefRow {
    pub path: String,
    pub link: String,
    pub title: String,
    pub origin: String,
    pub ref_type: Option<String>,
    pub era: String,
    pub status: Option<String>,
    pub status_sync: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontmatter_status: Option<String>,
    pub legacy_status: Option<String>,
    /// True when the note's single `^ref` tracker is Blocked `[?]`:
    /// an overlay on the reading lane, always present in JSON.
    pub blocked: bool,
    pub reading_state: String,
    pub reading_state_source: String,
    pub parent: Option<String>,
    pub urls: Vec<String>,
    pub identity: RefIdentity,
    pub author: Option<String>,
    pub published: Option<String>,
    pub captured: Option<String>,
    pub added: Option<String>,
    pub added_source: Option<String>,
    pub finished: Option<String>,
    pub finished_source: Option<String>,
    pub source_pdf: Option<String>,
    pub audio: Option<String>,
    pub annotation_count: usize,
    pub comment_count: usize,
    pub snapshot: Option<RefSnapshot>,
    pub research_ref: Option<String>,
    pub superseded_by: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    /// Frontmatter `id`, kept out of JSON; only query resolution reads it.
    #[serde(skip_serializing)]
    pub id: Option<String>,
    /// Frontmatter `source_block`, kept out of JSON; only the zorg-era
    /// coverage count reads it to subtract already-mirrored records.
    #[serde(skip_serializing)]
    pub source_block: Option<String>,
    /// Frontmatter `source_path`, kept out of JSON; only the zorg-era
    /// coverage count reads it to subtract already-mirrored records.
    #[serde(skip_serializing)]
    pub source_path: Option<String>,
    /// Frontmatter `source_id`, kept out of JSON; only the zorg-era
    /// coverage count reads it to tell records sharing one owner block
    /// apart.
    #[serde(skip_serializing)]
    pub source_id: Option<String>,
    /// Frontmatter `source_blocks`, kept out of JSON; only the zorg-era
    /// coverage count reads it so one book note can mirror several
    /// folded chapter blocks.
    #[serde(skip_serializing)]
    pub source_blocks: Vec<String>,
}

/// Stored identity keys for one row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct RefIdentity {
    pub keys: Vec<String>,
    pub arxiv: Option<String>,
    pub doi: Option<String>,
}

/// The last scan's snapshot, or `None` when the note was never synced.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct RefSnapshot {
    pub synced_at: Option<String>,
    pub highlights_count: Option<serde_json::Value>,
}

/// A machine-stable diagnostic attached to a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Diagnostic {
    pub code: String,
    pub detail: String,
}

impl Diagnostic {
    pub(crate) fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            detail: detail.into(),
        }
    }
}

/// Per-build coverage disclosure.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Coverage {
    pub ref_dir: String,
    pub notes: usize,
    pub skipped: usize,
    pub intake: String,
    pub scope: String,
    pub annotations: String,
    /// Set only by `list -g` when the vault Git pass fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_dates: Option<String>,
}

impl Coverage {
    pub(crate) fn scope_text() -> String {
        "Only notes under ref/ are indexed. Zorg-era status:: reading records outside ref/ are not until `bob ref migrate-zorg` moves them into ref/zorg/; run `bob ref doctor` to count any that remain. Absence from this index is never proof that something was not read.".to_string()
    }

    pub(crate) fn annotations_text() -> String {
        "Annotations are the snapshot written by the last Highlights scan, not live PDF state.".to_string()
    }
}

/// Library counts over non-superseded rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct LibraryCounts {
    pub notes: usize,
    pub finished: usize,
    pub started: usize,
    pub queued: usize,
    pub dropped: usize,
    pub unknown: usize,
}

/// Strip one layer of `[[...]]` brackets, keeping any `|alias` target head.
pub(crate) fn strip_wikilink_brackets(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(inner) = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
    {
        inner.split('|').next().unwrap_or("").trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Reduce a `parent` frontmatter value to its bare note name.
pub(crate) fn bare_parent_name(value: &str) -> Option<String> {
    let target = strip_wikilink_brackets(value);
    let target = target.split('#').next().unwrap_or("").trim();
    (!target.is_empty()).then(|| target.to_string())
}

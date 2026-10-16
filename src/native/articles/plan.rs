//! Formatted-article result: hunks, counts, findings, and sections.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArticleContext<'a> {
    pub(crate) path: &'a Path,
    pub(crate) require_marker: bool,
}

impl<'a> ArticleContext<'a> {
    pub(crate) fn new(path: &'a Path) -> Self {
        Self {
            path,
            require_marker: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn bypass_marker(path: &'a Path) -> Self {
        Self {
            path,
            require_marker: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArticleStatus {
    Changed,
    Unchanged,
    Skipped,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Refusal {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LineEdit {
    pub(crate) line: usize,
    pub(crate) delete: usize,
    pub(crate) insert: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub(crate) struct ChangeCounts {
    pub(crate) numbered: usize,
    pub(crate) renumbered: usize,
    pub(crate) unnumbered: usize,
    pub(crate) promoted: usize,
    pub(crate) setext: usize,
    pub(crate) links: usize,
    pub(crate) layout: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplacedToken {
    pub(crate) line: usize,
    pub(crate) token: String,
    pub(crate) number: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ArticlePlan {
    pub(crate) path: PathBuf,
    pub(crate) status: ArticleStatus,
    pub(crate) reason: Option<Refusal>,
    pub(crate) sha256: String,
    #[serde(skip_serializing)]
    pub(crate) formatted: Option<String>,
    pub(crate) edits: Vec<LineEdit>,
    pub(crate) changes: ChangeCounts,
    pub(crate) replaced: Vec<ReplacedToken>,
    pub(crate) notice: String,
    pub(crate) findings: Vec<super::findings::Finding>,
    pub(crate) outline: super::outline::ArticleOutline,
    pub(crate) inbound: Vec<InboundLink>,
}

impl ArticlePlan {
    pub(crate) fn skipped(
        path: &Path,
        sha256: String,
        reason: Refusal,
    ) -> Self {
        let notice = format!("Article skipped · {}", reason.message);
        Self {
            path: path.to_path_buf(),
            status: ArticleStatus::Skipped,
            reason: Some(reason),
            sha256,
            formatted: None,
            edits: Vec::new(),
            changes: ChangeCounts::default(),
            replaced: Vec::new(),
            findings: Vec::new(),
            notice,
            outline: super::outline::empty_outline(path, true),
            inbound: Vec::new(),
        }
    }

    pub(crate) fn error(
        path: &Path,
        sha256: String,
        reason: Refusal,
        findings: Vec<super::findings::Finding>,
        outline: super::outline::ArticleOutline,
    ) -> Self {
        let notice = format!("Article formatting failed · {}", reason.message);
        Self {
            path: path.to_path_buf(),
            status: ArticleStatus::Error,
            reason: Some(reason),
            sha256,
            formatted: None,
            edits: Vec::new(),
            changes: ChangeCounts::default(),
            replaced: Vec::new(),
            findings,
            notice,
            outline,
            inbound: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct InboundLink {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) sha256: String,
    pub(crate) before: String,
    pub(crate) after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ArticlePlanResponse {
    pub(crate) schema_version: u8,
    pub(crate) notes: Vec<ArticlePlan>,
    pub(crate) summary: PlanSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanSummary {
    pub(crate) articles: usize,
    pub(crate) changed: usize,
    pub(crate) unchanged: usize,
    pub(crate) skipped: usize,
    pub(crate) errors: usize,
}

impl ArticlePlanResponse {
    pub(crate) fn new(notes: Vec<ArticlePlan>) -> Self {
        let mut summary = PlanSummary {
            articles: notes.len(),
            changed: 0,
            unchanged: 0,
            skipped: 0,
            errors: 0,
        };
        for note in &notes {
            match note.status {
                ArticleStatus::Changed => summary.changed += 1,
                ArticleStatus::Unchanged => summary.unchanged += 1,
                ArticleStatus::Skipped => summary.skipped += 1,
                ArticleStatus::Error => summary.errors += 1,
            }
        }
        Self {
            schema_version: 1,
            notes,
            summary,
        }
    }
}

pub(crate) fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub(crate) fn line_edits(old: &[&str], new: &[&str]) -> Vec<LineEdit> {
    let mut edits = Vec::new();
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, old, new)
    {
        match op {
            similar::DiffOp::Equal { .. } => {}
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => edits.push(LineEdit {
                line: old_index + 1,
                delete: old_len,
                insert: Vec::new(),
            }),
            similar::DiffOp::Insert {
                old_index,
                new_index,
                new_len,
                ..
            } => edits.push(LineEdit {
                line: old_index + 1,
                delete: 0,
                insert: new[new_index..new_index + new_len]
                    .iter()
                    .map(|line| (*line).to_string())
                    .collect(),
            }),
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => edits.push(LineEdit {
                line: old_index + 1,
                delete: old_len,
                insert: new[new_index..new_index + new_len]
                    .iter()
                    .map(|line| (*line).to_string())
                    .collect(),
            }),
        }
    }
    edits
}

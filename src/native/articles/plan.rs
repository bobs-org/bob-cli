//! Formatted-article result: hunks, counts, findings, and sections.

use std::path::{Path, PathBuf};

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

    pub(crate) fn bypass_marker(path: &'a Path) -> Self {
        Self {
            path,
            require_marker: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArticleStatus {
    Changed,
    Unchanged,
    Skipped,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineEdit {
    pub(crate) line: usize,
    pub(crate) delete: usize,
    pub(crate) insert: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ChangeCounts {
    pub(crate) numbered: usize,
    pub(crate) renumbered: usize,
    pub(crate) unnumbered: usize,
    pub(crate) promoted: usize,
    pub(crate) setext: usize,
    pub(crate) links: usize,
    pub(crate) layout: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplacedToken {
    pub(crate) line: usize,
    pub(crate) token: String,
    pub(crate) number: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArticleSection {
    pub(crate) number: Option<String>,
    pub(crate) written: Option<String>,
    pub(crate) depth: usize,
    pub(crate) level: usize,
    pub(crate) original_level: usize,
    pub(crate) title: String,
    pub(crate) line: usize,
    pub(crate) end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArticlePlan {
    pub(crate) path: PathBuf,
    pub(crate) status: ArticleStatus,
    pub(crate) reason: Option<Refusal>,
    pub(crate) sha256: String,
    pub(crate) formatted: Option<String>,
    pub(crate) edits: Vec<LineEdit>,
    pub(crate) changes: ChangeCounts,
    pub(crate) replaced: Vec<ReplacedToken>,
    pub(crate) findings: Vec<super::findings::Finding>,
    pub(crate) sections: Vec<ArticleSection>,
}

impl ArticlePlan {
    pub(crate) fn skipped(
        path: &Path,
        sha256: String,
        reason: Refusal,
    ) -> Self {
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
            sections: Vec::new(),
        }
    }

    pub(crate) fn error(
        path: &Path,
        sha256: String,
        reason: Refusal,
        findings: Vec<super::findings::Finding>,
        sections: Vec<ArticleSection>,
    ) -> Self {
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
            sections,
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

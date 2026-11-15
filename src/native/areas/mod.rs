//! Pure analysis and planning for nested `## Areas` catalogs.
//!
//! Shared by the follow-on areas-report/areas-sync phases; remove these
//! allows as the report and sync wiring lands.
#![allow(dead_code)]

#[allow(dead_code)]
pub(crate) mod catalog;
#[allow(dead_code)]
pub(crate) mod graph;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use crate::native::notebooks::model::Severity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootState {
    Found,
    Missing,
    Ambiguous,
}

impl RootState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Found => "found",
            Self::Missing => "missing",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AreaRoot {
    pub(crate) state: RootState,
    pub(crate) path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AreaIssue {
    pub(crate) path: PathBuf,
    pub(crate) code: &'static str,
    pub(crate) severity: Severity,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

impl AreaIssue {
    pub(crate) fn error(
        path: impl Into<PathBuf>,
        code: &'static str,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            code,
            severity: Severity::Error,
            line,
            message: message.into(),
        }
    }

    pub(crate) fn warning(
        path: impl Into<PathBuf>,
        code: &'static str,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            code,
            severity: Severity::Warning,
            line,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AreaNode {
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    pub(crate) link: String,
    pub(crate) title: String,
    pub(crate) titled: bool,
    pub(crate) summary: Option<String>,
    /// Resolved path of a valid direct parent, including the root sentinel.
    pub(crate) parent: Option<PathBuf>,
    pub(crate) parent_raw: Option<String>,
    /// One-based tree depth from the root, or `None` when not rooted there.
    pub(crate) depth: Option<usize>,
    /// Top-level area stem, or `None` when not rooted there.
    pub(crate) top: Option<String>,
    pub(crate) children: Vec<PathBuf>,
    pub(crate) issues: Vec<AreaIssue>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AreaGraph {
    pub(crate) root: Option<AreaRoot>,
    pub(crate) areas: std::collections::BTreeMap<PathBuf, AreaNode>,
    pub(crate) root_children: Vec<PathBuf>,
    pub(crate) findings: Vec<AreaIssue>,
}

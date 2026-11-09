//! Notebook report model: entries, sections, findings, and counts.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookIssue {
    pub(crate) code: &'static str,
    pub(crate) severity: Severity,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

impl NotebookIssue {
    pub(crate) fn error(
        code: &'static str,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: Severity::Error,
            line,
            message: message.into(),
        }
    }

    pub(crate) fn warning(
        code: &'static str,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            line,
            message: message.into(),
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "code": self.code,
            "severity": self.severity.as_str(),
            "line": self.line,
            "message": self.message,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookSection {
    pub(crate) title: String,
    pub(crate) notes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookEntry {
    pub(crate) stem: String,
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) summary: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) parent_kind: Option<String>,
    pub(crate) owner: Option<String>,
    pub(crate) root: Option<String>,
    pub(crate) breadcrumb: Vec<String>,
    pub(crate) children: Vec<String>,
    pub(crate) notes: usize,
    pub(crate) sections: Vec<NotebookSection>,
    pub(crate) last_note: Option<String>,
    pub(crate) issues: Vec<NotebookIssue>,
}

impl NotebookEntry {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "stem": self.stem,
            "path": self.path,
            "title": self.title,
            "summary": self.summary,
            "parent": self.parent,
            "parent_kind": self.parent_kind,
            "owner": self.owner,
            "root": self.root,
            "breadcrumb": self.breadcrumb,
            "children": self.children,
            "notes": self.notes,
            "sections": self.sections.iter().map(|section| json!({
                "title": section.title,
                "notes": section.notes,
            })).collect::<Vec<_>>(),
            "last_note": self.last_note,
            "issues": self.issues.iter().map(|issue| issue.to_json()).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookCounts {
    pub(crate) notebooks: usize,
    pub(crate) roots: usize,
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookReport {
    pub(crate) notebooks: Vec<NotebookEntry>,
    pub(crate) counts: NotebookCounts,
}

impl NotebookReport {
    pub(crate) fn ok(&self) -> bool {
        self.counts.errors == 0
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "schema_version": 1,
            "notebooks": self.notebooks.iter().map(|entry| entry.to_json()).collect::<Vec<_>>(),
            "catalogs": [],
            "counts": {
                "notebooks": self.counts.notebooks,
                "roots": self.counts.roots,
                "errors": self.counts.errors,
                "warnings": self.counts.warnings,
            },
        })
    }
}

//! Report-only findings the formatter never auto-fixes.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) code: String,
    pub(crate) severity: Severity,
    pub(crate) line: usize,
    pub(crate) message: String,
    pub(crate) hint: String,
}

impl Finding {
    pub(crate) fn warning(
        code: &str,
        line: usize,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: Severity::Warning,
            line,
            message: message.into(),
            hint: hint.into(),
        }
    }

    pub(crate) fn info(
        code: &str,
        line: usize,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: Severity::Info,
            line,
            message: message.into(),
            hint: hint.into(),
        }
    }
}

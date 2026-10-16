//! Report-only findings the formatter never auto-fixes.

use std::path::Path;

use serde::Serialize;

use super::scan::{ClassifiedLine, Region};
use super::sections::{heading_plain_identity, PlannedHeading};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Finding {
    pub(crate) code: String,
    pub(crate) severity: Severity,
    pub(crate) line: usize,
    pub(crate) message: String,
    pub(crate) hint: String,
}

pub(crate) fn pseudo_footnotes(lines: &[ClassifiedLine<'_>]) -> Vec<Finding> {
    lines
        .iter()
        .filter_map(|line| {
            if matches!(
                line.region,
                Region::Frontmatter
                    | Region::Fence
                    | Region::UnclosedFence
                    | Region::Math
                    | Region::PercentComment
                    | Region::HtmlComment
                    | Region::HtmlBlock
            ) {
                return None;
            }
            let visible = super::marker::mask_inline_spans(line.content);
            let rest = visible.strip_prefix('[')?;
            let (label, body) = rest.split_once("]:")?;
            let body = body.trim_start();
            if label.is_empty()
                || label.starts_with('^')
                || body.contains("://")
                || body.to_ascii_lowercase().contains("mailto:")
                || body.contains('#')
                || body.contains('/')
                || body.starts_with("./")
            {
                return None;
            }
            Some(Finding::warning(
                "pseudo-footnote",
                line.index + 1,
                format!("[{label}] is a link definition without a URL or path"),
                format!("Use [^{label}] footnotes"),
            ))
        })
        .collect()
}

pub(crate) fn title_heading(
    headings: &[PlannedHeading],
    path: &Path,
    title: Option<&str>,
) -> Option<Finding> {
    let first = headings.first()?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let mut candidates = vec![stem];
    if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
        candidates.push(title);
    }
    let repeated = candidates.into_iter().find(|candidate| {
        normalize(&first.title) == normalize(candidate)
            || heading_plain_identity(&first.title)
                == heading_plain_identity(candidate)
    })?;
    Some(Finding::info(
        "title-heading",
        first.line + 1,
        format!("first section repeats the article title {repeated:?}"),
        "Delete the heading",
    ))
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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

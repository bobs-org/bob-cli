//! Per-note contract findings shared by `props show` and list JSON.

use serde::Serialize;

use crate::native::{
    freshness::state::parse_note_refresh,
    note_ready::{parse_ready_cap, CapSource, LINT_NOTE_READY_CAP_INVALID},
    pomodoro_homes::{NoteColor, NoteFacts, NoteOffClock, NoteOffClockTypes},
    projects::parse_frontmatter,
};

/// One setting-related finding on an area or project row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ContractIssue {
    pub(crate) code: String,
    pub(crate) severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

impl ContractIssue {
    fn warning(
        code: &str,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            severity: "warning".to_owned(),
            line,
            message: message.into(),
        }
    }

    fn error(
        code: &str,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            severity: "error".to_owned(),
            line,
            message: message.into(),
        }
    }
}

/// Findings from a note's own setting bytes. Catalog stale/damaged codes
/// stay on the existing `issues`/`findings` arrays.
pub(crate) fn from_contents(contents: &str) -> Vec<ContractIssue> {
    let facts = NoteFacts::parse(contents);
    let mut issues = Vec::new();
    let frontmatter = parse_frontmatter(contents);
    let summary = frontmatter.as_ref().and_then(|front| {
        crate::native::projects::frontmatter_value(front, "summary")
            .map(str::to_string)
    });
    if let Some(summary) = summary.as_deref() {
        if summary.contains(['\n', '\r']) {
            issues.push(ContractIssue::warning(
                "area_summary_multiline",
                None,
                "summary must fit on one line",
            ));
        } else if summary.chars().count() > 140 {
            issues.push(ContractIssue::warning(
                "area_summary_long",
                None,
                "summary exceeds 140 characters",
            ));
        }
    }
    if let Some(NoteColor::Invalid(raw)) = &facts.color {
        issues.push(ContractIssue::error(
            "area_color_invalid",
            None,
            format!("color {raw:?} is not a palette hue or grey"),
        ));
    }
    if facts.theme_conflict {
        issues.push(ContractIssue::warning(
            "type_default_conflict",
            None,
            "task_type and theme both set; task_type wins",
        ));
    }
    if let (Some(offclock), Some(_)) = (&facts.offclock, &facts.offclock_types)
    {
        let raw = match offclock {
            NoteOffClock::True => "true",
            NoteOffClock::False => "false",
            NoteOffClock::Invalid(raw) => raw.as_str(),
        };
        issues.push(ContractIssue::warning(
            "note_offclock_shadowed",
            None,
            format!(
                "offclock: {raw} is ignored because offclock_types decides"
            ),
        ));
    }
    if let Some(NoteOffClock::Invalid(raw)) = &facts.offclock {
        issues.push(ContractIssue::error(
            "note_offclock_invalid",
            None,
            format!("offclock {raw:?} is not true or false"),
        ));
    }
    if let Some(NoteOffClockTypes::Invalid(raw)) = &facts.offclock_types {
        issues.push(ContractIssue::error(
            "note_offclock_invalid",
            None,
            format!("offclock_types {raw:?} is not a type list"),
        ));
    }
    let cap_raw = frontmatter.as_ref().and_then(|front| {
        crate::native::projects::frontmatter_value(front, "ready_cap")
            .map(str::to_string)
    });
    let (_, _, _, invalid) =
        parse_ready_cap(cap_raw.as_deref(), 5, CapSource::Config);
    if invalid {
        issues.push(ContractIssue::error(
            LINT_NOTE_READY_CAP_INVALID,
            None,
            "ready_cap is not 1–999, off, or false",
        ));
    }
    let refresh_raw = frontmatter.as_ref().and_then(|front| {
        crate::native::projects::frontmatter_value(front, "task_refresh")
            .map(str::to_string)
    });
    if parse_note_refresh(refresh_raw.as_deref()).1.is_some() {
        issues.push(ContractIssue::error(
            "task_refresh_invalid",
            None,
            "task_refresh is not 1–365 days",
        ));
    }
    issues
}

/// Graph parent/summary findings that belong on the contract lint chip.
pub(crate) fn from_area_codes(
    code: &str,
    severity: &str,
    line: Option<usize>,
    message: &str,
) -> Option<ContractIssue> {
    const CODES: &[&str] = &[
        "area_parent_missing",
        "area_parent_list",
        "area_parent_unresolved",
        "area_parent_ambiguous",
        "area_parent_self",
        "area_parent_kind",
        "area_parent_unquoted",
        "area_cycle",
        "area_summary_long",
        "area_summary_multiline",
    ];
    CODES.contains(&code).then(|| ContractIssue {
        code: code.to_owned(),
        severity: severity.to_owned(),
        line,
        message: message.to_owned(),
    })
}

pub(crate) fn merge(mut issues: Vec<ContractIssue>) -> Vec<ContractIssue> {
    issues.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.message.cmp(&right.message))
    });
    issues.dedup();
    issues
}

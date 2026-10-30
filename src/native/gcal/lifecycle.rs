//! `gcal-lifecycle` row surgery: three-way span updates, cancels,
//! RSVP transitions, and series-root learning.
//!
//! The planner in `plan.rs` orchestrates; this module owns the pure
//! line-level operations on Events rows. Google owns the time, title,
//! and badge spans; Bryan owns the checkbox, `[root::]`, children, and
//! the row itself. A Google span is replaced in place only while the
//! current text still matches what Bob last wrote (the journal hash).
//! Anything else is left alone and reported once.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::native::{
    capture::line_spans,
    capture_pomodoros::{self, PomodoroState},
    capture_task_toggle::child_block_end_line,
    pomodoro::pomodoros_section_range,
    task_cancel::line::{edit_source_create_log, CancelLogOutcome},
};

use super::journal;

/// One journaled occurrence whose event no longer has an included row:
/// declined, deleted, moved, or otherwise gone from the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleItem {
    pub calendar_id: String,
    pub event_id: String,
    pub verdict: AbsentVerdict,
}

/// Why a journaled row has no included occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AbsentVerdict {
    /// `status: cancelled`, a 404/410 lookup, or another deletion signal.
    Deleted,
    /// The guest declined (or the organizer cancelled their instance).
    Declined,
    /// The event now starts on another day; the label reads
    /// `Fri 2026-10-31`.
    Moved(String),
    /// Anything else: leave the row and report `detail` once.
    LeftAlone(String),
}

impl AbsentVerdict {
    /// The cancel reason recorded in the CANCEL LOG, if this verdict
    /// cancels or removes rows.
    pub(crate) fn cancel_reason(&self) -> Option<String> {
        match self {
            Self::Deleted => Some("Deleted in Google Calendar".to_string()),
            Self::Declined => Some("Declined in Google Calendar".to_string()),
            Self::Moved(label) => {
                Some(format!("Moved to {label} in Google Calendar"))
            }
            Self::LeftAlone(_) => None,
        }
    }
}

/// One lifecycle notice reported once per (occurrence, day).
#[derive(Debug, Clone)]
pub(crate) struct KeptNotice {
    pub calendar_id: String,
    pub event_id: String,
    pub title: String,
    /// Human detail, for example `kept your edit to the title`.
    pub detail: String,
    /// Journal flag that remembers the report.
    pub flag: String,
}

/// One vault edit that teaches a recurring series its root.
#[derive(Debug, Clone)]
pub(crate) struct LearnedSeries {
    pub calendar_id: String,
    pub series: String,
    pub root: String,
    pub title: String,
    pub day: String,
    pub block_id: String,
}

/// One Events row split into its Google-owned spans and Bryan-owned
/// shell. `badges` keeps its leading space (or is empty); `suffix`
/// keeps its leading space (or is empty); `qualifier` is the context
/// `· *maybe*` style tag without decoration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowSpans {
    /// Through the opening `**`, for example `- [*] #task **`.
    pub prefix: String,
    pub time: String,
    pub title: String,
    pub qualifier: Option<String>,
    pub badges: String,
    pub suffix: String,
    pub task: bool,
}

/// Split one Events row into spans. Returns `None` for lines this
/// module must not rewrite.
pub(crate) fn split_row_spans(line: &str) -> Option<RowSpans> {
    let trimmed = line.trim_end();
    let mut rest = trimmed.strip_prefix("- ")?;
    let task = rest.starts_with('[');
    if task {
        rest = rest.strip_prefix('[')?;
        let close = rest.find(']')?;
        if close != 1 {
            return None;
        }
        rest = rest.get(close + 1..)?.trim_start_matches([' ', '\t']);
        if let Some(without_tag) = rest.strip_prefix("#task") {
            rest = without_tag.trim_start_matches([' ', '\t']);
        }
    }
    let prefix_len = trimmed.len() - rest.len();
    let bold = rest.strip_prefix("**")?;
    let close = bold.find("**")?;
    let time = bold.get(..close)?.to_string();
    if time.is_empty() {
        return None;
    }
    let mut middle = bold.get(close + 2..)?;
    if !middle.is_empty() && !middle.starts_with([' ', '\t']) {
        return None;
    }
    middle = middle.trim_start_matches([' ', '\t']);
    let prefix = format!("{}**", &trimmed[..prefix_len]);
    let (mut middle, suffix) = split_suffix(middle, task);
    let mut badges = String::new();
    if let Some(badge_at) = middle.find(" [📅](") {
        badges = middle.get(badge_at..)?.to_string();
        middle = middle.get(..badge_at)?;
    }
    let middle = middle.trim_end();
    let (title, qualifier) = split_qualifier(middle, task);
    Some(RowSpans {
        prefix,
        time,
        title: title.to_string(),
        qualifier,
        badges,
        suffix,
        task,
    })
}

/// Split the trailing `[root::]` / `[type::]` / `^block` shell off a
/// task row. Context rows keep no suffix.
fn split_suffix(middle: &str, task: bool) -> (&str, String) {
    if !task {
        return (middle, String::new());
    }
    for marker in [" [root::", " [type::"] {
        if let Some(at) = middle.find(marker) {
            return (
                middle.get(..at).unwrap_or(middle),
                middle.get(at..).unwrap_or("").to_string(),
            );
        }
    }
    if let Some(caret) = middle.rfind('^') {
        let id = middle.get(caret + 1..).unwrap_or("");
        let before = middle.get(..caret).unwrap_or("");
        if !id.is_empty()
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && before.ends_with([' ', '\t'])
        {
            return (before.trim_end(), middle[caret - 1..].to_string());
        }
    }
    (middle, String::new())
}

/// Split a trailing `· *qualifier*` tag off a context row middle.
fn split_qualifier(middle: &str, task: bool) -> (&str, Option<String>) {
    if task {
        return (middle, None);
    }
    let Some(tag_at) = middle.rfind(" · *") else {
        return (middle, None);
    };
    // `" · *"` is 5 bytes: the middle dot takes two in UTF-8.
    let tag = middle.get(tag_at + " · *".len()..).unwrap_or("");
    if tag.ends_with('*') && !tag[..tag.len().saturating_sub(1)].contains('*') {
        (
            middle.get(..tag_at).unwrap_or(middle).trim_end(),
            Some(tag[..tag.len() - 1].to_string()),
        )
    } else {
        (middle, None)
    }
}

impl RowSpans {
    /// Rebuild the row after span surgery.
    pub(crate) fn rebuild(&self) -> String {
        // `prefix` already ends with the opening `**`.
        let mut out = format!("{}{}**", self.prefix, self.time);
        if !self.title.is_empty()
            || self.qualifier.is_some()
            || !self.badges.is_empty()
            || !self.suffix.is_empty()
        {
            out.push(' ');
            out.push_str(&self.title);
        }
        if let Some(qualifier) = self.qualifier.as_deref() {
            out.push_str(" · *");
            out.push_str(qualifier);
            out.push('*');
        }
        out.push_str(&self.badges);
        out.push_str(&self.suffix);
        out
    }

    /// Short SHA-256 hashes of the three Google-owned spans, keyed
    /// exactly like the journal's span map.
    pub(crate) fn hashes(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("time".to_string(), journal::span_hash(&self.time)),
            ("title".to_string(), journal::span_hash(&self.title)),
            ("badges".to_string(), journal::span_hash(&self.badges)),
        ])
    }
}

/// The `[root:: X]` value on a task row, if it parses.
pub(crate) fn line_root(line: &str) -> Option<String> {
    let start = line.find("[root::")?;
    let rest = line.get(start + "[root::".len()..)?;
    let end = rest.find(']')?;
    Some(rest.get(..end)?.trim().to_string())
}

/// The checkbox status character of a task row.
pub(crate) fn status_char(line: &str) -> Option<char> {
    let rest = line.trim_start().strip_prefix("- [")?;
    let mut chars = rest.chars();
    let status = chars.next()?;
    if chars.next() == Some(']') {
        Some(status)
    } else {
        None
    }
}

/// Whether a journaled row is still untouched: an open or promoted
/// checkbox, no 🍅 history in its child block, and its link never
/// landed under a closed Pomodoro. A worked task is never cancelled;
/// it is reported once instead.
pub(crate) fn row_untouched(
    contents: &str,
    line_index: usize,
    block_id: &str,
) -> bool {
    let lines: Vec<&str> = contents.lines().collect();
    let Some(line) = lines.get(line_index) else {
        return false;
    };
    if !matches!(status_char(line), Some(' ') | Some('*')) {
        return false;
    }
    let spans = line_spans(contents);
    let end = child_block_end_line(&spans, line_index).min(lines.len());
    if lines
        .get(line_index..=end)
        .is_none_or(|block| block.iter().any(|line| line.contains('🍅')))
    {
        return false;
    }
    let link = format!("[[#^{block_id}]]");
    let scan = capture_pomodoros::scan(contents);
    let range = pomodoros_section_range(&lines);
    let mut bounds: Vec<usize> = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Completed)
        .map(|entry| entry.line.saturating_sub(1))
        .collect();
    bounds.sort_unstable();
    let section_end = range.map(|range| range.end).unwrap_or(lines.len());
    for (position, start) in bounds.iter().enumerate() {
        let end = bounds
            .get(position + 1)
            .copied()
            .unwrap_or(section_end)
            .min(lines.len());
        let from = (*start).min(end);
        if lines
            .get(from..end)
            .is_some_and(|scope| scope.iter().any(|line| line.contains(&link)))
        {
            return false;
        }
    }
    true
}

/// Drop one MEET link from the Pomodoros section. A dedicated link
/// bullet loses its line; a shared bullet loses just the token.
/// Returns the updated note and whether anything was removed.
pub(crate) fn remove_meet_link(
    contents: &str,
    block_id: &str,
) -> (String, bool) {
    let token = format!("[[#^{block_id}]]");
    let lines: Vec<&str> = contents.lines().collect();
    let Some(section) = pomodoros_section_range(&lines) else {
        return (contents.to_string(), false);
    };
    let mut removed = false;
    let mut kept: Vec<String> = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if !(section.contains(&index) && line.contains(token.as_str())) {
            kept.push(line.to_string());
            continue;
        }
        removed = true;
        let stripped = line.replacen(token.as_str(), "", 1);
        let body = stripped.trim_start_matches([' ', '\t']);
        if body == "-" || body == "-\n" {
            continue;
        }
        kept.push(stripped.trim_end().to_string());
    }
    let mut out = kept.join("\n");
    if contents.ends_with('\n') {
        out.push('\n');
    }
    (out, removed)
}

/// Remove every untimed open MEET entry left childless, returning the
/// updated note and the removed count. Timed entries and entries that
/// still carry links stay.
pub(crate) fn remove_childless_untimed_meet(contents: &str) -> (String, usize) {
    let scan = capture_pomodoros::scan(contents);
    if !scan.has_section {
        return (contents.to_string(), 0);
    }
    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let mut drop: Vec<usize> = Vec::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        if entry.name.as_deref() != Some("MEET") || entry.time_range.is_some() {
            continue;
        }
        if entry.child_count == 0 {
            drop.push(entry.line.saturating_sub(1));
        }
    }
    if drop.is_empty() {
        return (contents.to_string(), 0);
    }
    let kept: Vec<String> = lines
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !drop.contains(index))
        .map(|(_, line)| line)
        .collect();
    let mut out = kept.join("\n");
    if contents.ends_with('\n') {
        out.push('\n');
    }
    let removed = drop.len();
    (out, removed)
}

/// Cancel one task row in place: mark it cancelled with today's date
/// and create the `❌ **CANCEL LOG**` first child (with this cancel's
/// dated entry) when the task has none. Returns `None` when the line
/// cannot be cancelled.
pub(crate) fn cancel_task_row(
    contents: &str,
    line_index: usize,
    today: NaiveDate,
    reason: &str,
) -> Option<String> {
    let edit = edit_source_create_log(
        contents,
        line_index,
        today,
        &[reason.to_string()],
    );
    if edit.log == CancelLogOutcome::None {
        return None;
    }
    Some(edit.content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(line: &str) -> RowSpans {
        split_row_spans(line).expect("row parses")
    }

    #[test]
    fn splits_task_and_context_rows() {
        let task = spans(
            "- [*] #task **0800-0830** Product interview \
             [📅](https://cal.example/e1 \"Open in Google Calendar\") \
             [root:: job] [type:: MEET] ^product-interview",
        );
        assert!(task.task);
        assert_eq!(task.prefix, "- [*] #task **");
        assert_eq!(task.time, "0800-0830");
        assert_eq!(task.title, "Product interview");
        assert_eq!(task.qualifier, None);
        assert!(task.badges.contains("[📅]("));
        assert!(task.suffix.contains("[root:: job]"));
        assert!(task.suffix.ends_with("^product-interview"));

        let rebuilt = task.rebuild();
        assert!(rebuilt.contains("**0800-0830** Product interview"));
        assert!(rebuilt.ends_with("^product-interview"));

        let context = spans("- **all day** School holiday · *day 2 of 2* [📅](https://cal.example/e2 \"Open in Google Calendar\")");
        assert!(!context.task);
        assert_eq!(context.time, "all day");
        assert_eq!(context.title, "School holiday");
        assert_eq!(context.qualifier.as_deref(), Some("day 2 of 2"));
        assert_eq!(context.suffix, "");
    }

    #[test]
    fn hashes_match_the_journal_spans() {
        let line = "- [ ] #task **1830-1900** Dentist [📅](https://cal.example/e3 \"Open in Google Calendar\") [root:: body] [type:: MEET] ^dentist";
        let parsed = spans(line);
        let hashes = parsed.hashes();
        assert_eq!(hashes["time"], journal::span_hash("1830-1900"));
        assert_eq!(hashes["title"], journal::span_hash("Dentist"));
        assert!(hashes["badges"].len() == 16);
        assert_eq!(line_root(line).as_deref(), Some("body"));
        assert_eq!(status_char(line), Some(' '));
    }

    #[test]
    fn untimed_meet_entries_go_when_childless() {
        let note =
            "# Day\n\n## Pomodoros\n\n- [ ] () — MEET\n  - [[#^dentist]]\n";
        let (without, removed) = remove_meet_link(note, "dentist");
        assert!(removed);
        assert!(!without.contains("[[#^dentist]]"));
        let (cleaned, count) = remove_childless_untimed_meet(&without);
        assert_eq!(count, 1);
        assert!(!cleaned.contains("— MEET"));
    }

    #[test]
    fn timed_meet_entries_survive_cleanup() {
        let note = "# Day\n\n## Pomodoros\n\n- [ ] (0800-0900) — MEET\n";
        let (same, count) = remove_childless_untimed_meet(note);
        assert_eq!(count, 0);
        assert_eq!(same, note);
    }

    #[test]
    fn cancel_creates_the_log_when_missing() {
        let note = "- [-] #task **0800-0830** Old [root:: meet] ^old\n- [ ] #task **0900-1000** New [root:: meet] ^new\n";
        let cancelled = cancel_task_row(
            note,
            1,
            NaiveDate::from_ymd_opt(2026, 10, 30).expect("date"),
            "Declined in Google Calendar",
        )
        .expect("cancel applies");
        assert!(cancelled.contains("- [-] #task **0900-1000** New"));
        assert!(cancelled.contains("❌ **CANCEL LOG**"));
        assert!(
            cancelled.contains("*2026-10-30* — Declined in Google Calendar")
        );
    }
}

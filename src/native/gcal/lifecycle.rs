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
    clock_tags::strip_trailing_clock_tag,
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

/// Bob-owned warning tag sync writes on open, unacked external Event
/// Task rows as the last token before the `^id`. Deleting it by hand is
/// itself an ack; sync never writes it on context or closed rows.
pub(crate) const UNACKED_TAG: &str = "#unacked";

/// Whether `suffix` carries Bob's exact unacked warning token.
pub(crate) fn suffix_has_unacked(suffix: &str) -> bool {
    suffix.split([' ', '\t']).any(|token| token == UNACKED_TAG)
}

/// Whether a task row line carries Bob's unacked warning token. A token
/// Bryan typed by hand counts as present; the planner decides what that
/// means.
pub(crate) fn row_has_unacked(line: &str) -> bool {
    split_row_spans(line).is_some_and(|spans| suffix_has_unacked(&spans.suffix))
}

/// Append Bob's unacked warning token to a task row line as the last
/// token before the `^id`. Returns `None` when the line does not parse
/// or already carries the token.
pub(crate) fn add_unacked_tag(line: &str) -> Option<String> {
    let mut spans = split_row_spans(line)?;
    if !spans.task || suffix_has_unacked(&spans.suffix) {
        return None;
    }
    if spans.suffix.is_empty() {
        spans.suffix = format!(" {UNACKED_TAG}");
    } else if let Some(caret) = trailing_caret_token(&spans.suffix) {
        spans.suffix.insert_str(caret, &format!("{UNACKED_TAG} "));
    } else {
        spans.suffix.push_str(&format!(" {UNACKED_TAG}"));
    }
    Some(spans.rebuild())
}

/// Drop Bob's unacked warning token from a task row line. Returns `None`
/// when the line does not parse or carries no token.
pub(crate) fn strip_unacked_tag(line: &str) -> Option<String> {
    let mut spans = split_row_spans(line)?;
    if !spans.task || !suffix_has_unacked(&spans.suffix) {
        return None;
    }
    let kept: Vec<&str> = spans
        .suffix
        .split([' ', '\t'])
        .filter(|token| !token.is_empty() && *token != UNACKED_TAG)
        .collect();
    spans.suffix = if kept.is_empty() {
        String::new()
    } else {
        format!(" {}", kept.join(" "))
    };
    Some(spans.rebuild())
}

/// Index of the caret of a trailing `^block-id` token, when `suffix`
/// ends with one (so the token can be kept last).
fn trailing_caret_token(suffix: &str) -> Option<usize> {
    let caret = suffix.rfind('^')?;
    let id = suffix.get(caret + 1..)?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || caret == 0
        || !suffix.as_bytes()[caret - 1].is_ascii_whitespace()
    {
        return None;
    }
    Some(caret)
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
    } else if let Some((head, run)) = split_trailing_google_run(middle) {
        // Off-clock aliases and `[buffer::]` live in the Google-owned badge
        // span even without `📅`, so sync can track edits to both in place.
        badges = run;
        middle = head;
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

/// Split the trailing `[root::]` / `[type::]` / `[completion::]` /
/// `#unacked` / `^block` shell off a task row. Context rows keep no
/// suffix. The warning tag must split here (never into the hashed title
/// span), even on rootless rows whose only shell is ` #unacked ^id`.
fn split_suffix(middle: &str, task: bool) -> (&str, String) {
    if !task {
        return (middle, String::new());
    }
    let marker_at = ["[root::", "[type::", "[completion::"]
        .into_iter()
        .filter_map(|marker| {
            middle.match_indices(marker).find_map(|(at, _)| {
                (at > 0 && middle.as_bytes()[at - 1].is_ascii_whitespace())
                    .then_some(at)
            })
        })
        .chain(unacked_marker_at(middle))
        .min();
    if let Some(at) = marker_at {
        let mut start = at;
        while start > 0 && middle.as_bytes()[start - 1].is_ascii_whitespace() {
            start -= 1;
        }
        return (
            middle.get(..start).unwrap_or(middle),
            middle.get(start..).unwrap_or("").to_string(),
        );
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

/// Start of Bob's exact, whitespace-bounded `#unacked` warning token
/// in a task row middle, when one is present. A mid-title `#unacked`
/// always renders escaped (`\#unacked`), so a bare token here is the
/// shell marker, never title text; lookalikes like `#unackedx` never
/// match.
fn unacked_marker_at(middle: &str) -> Option<usize> {
    middle.match_indices(UNACKED_TAG).find_map(|(at, _)| {
        let preceded =
            at > 0 && middle.as_bytes()[at - 1].is_ascii_whitespace();
        let end = at + UNACKED_TAG.len();
        let followed =
            middle.len() == end || middle.as_bytes()[end].is_ascii_whitespace();
        (preceded && followed).then_some(at)
    })
}

/// Split the trailing Google-owned run (an explicit clock tag and/or
/// `[buffer:: …]` field) off `middle` when no `📅` badge is present.
/// Returns the head and the badge span (keeping one leading space). An
/// empty run returns `None`, so unbuffered tracked rows parse exactly
/// as before.
fn split_trailing_google_run(middle: &str) -> Option<(&str, String)> {
    let trimmed = middle.trim_end();
    let mut head = trimmed;
    loop {
        let next = strip_trailing_clock_token(head)
            .or_else(|| strip_trailing_buffer_token(head));
        match next {
            Some(rest) => head = rest.trim_end(),
            None => break,
        }
    }
    if head.len() == trimmed.len() {
        return None;
    }
    let run = trimmed.get(head.len()..).unwrap_or("").trim_start();
    Some((head, format!(" {run}")))
}

/// Strip one trailing standalone clock token using the shared spelling
/// table and vault-line boundaries.
fn strip_trailing_clock_token(text: &str) -> Option<&str> {
    strip_trailing_clock_tag(text)
}

/// Strip one trailing `[buffer:: …]` field off `text`. The field must be
/// whitespace-bounded so titles that merely end with a bracket keep
/// their text.
fn strip_trailing_buffer_token(text: &str) -> Option<&str> {
    if !text.ends_with(']') {
        return None;
    }
    let open = text.rfind('[')?;
    if open > 0 && !text[..open].ends_with([' ', '\t']) {
        return None;
    }
    let field = text.get(open..)?;
    if !field.starts_with("[buffer::") {
        return None;
    }
    Some(text.get(..open)?)
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
    for entry in scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Completed)
    {
        let start = entry.line.saturating_sub(1);
        let end = child_block_end_line(&spans, start)
            .min(lines.len().saturating_sub(1));
        let from = start.min(end);
        if lines
            .get(from..=end)
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
    fn completion_fields_stay_in_the_vault_owned_suffix() {
        let cases = [
            (
                "- [x] #task **0815-0830** Swings! [📅](https://cal.example/swings) #off  [completion:: 2026-11-04] ^swings",
                "- [x] #task **0815-0830** Swings! [📅](https://cal.example/swings) #off ^swings",
            ),
            (
                "- [x] #task **0800-0830** Product interview [📅](https://cal.example/interview) [root:: job] [type:: MEET]  [completion:: 2026-10-30] ^product-interview",
                "- [x] #task **0800-0830** Product interview [📅](https://cal.example/interview) [root:: job] [type:: MEET] ^product-interview",
            ),
        ];
        for (completed, without_completion) in cases {
            let parsed = spans(completed);
            let baseline = spans(without_completion);
            assert_eq!(parsed.hashes(), baseline.hashes(), "{completed}");
            assert_eq!(parsed.rebuild(), completed);
        }
    }

    #[test]
    fn trailing_clock_badges_split_with_or_without_calendar_link() {
        let without_calendar = spans(
            "- [/] #task **1200-1300** Lunch #OFFCLOCK [buffer:: -30m] \
             [root:: love] ^lunch",
        );
        assert_eq!(without_calendar.title, "Lunch");
        assert_eq!(without_calendar.badges, " #OFFCLOCK [buffer:: -30m]");

        let with_calendar = spans(
            "- [/] #task **1200-1300** Lunch \
             [📅](https://calendar.example/e1) #off-clock [buffer:: +15m] \
             [root:: love] ^lunch",
        );
        assert_eq!(with_calendar.title, "Lunch");
        assert_eq!(
            with_calendar.badges,
            " [📅](https://calendar.example/e1) #off-clock [buffer:: +15m]"
        );

        let on = spans(
            "- [*] #task **1200-1300** Planning #on [root:: love] ^planning",
        );
        assert_eq!(on.title, "Planning");
        assert_eq!(on.badges, " #on");

        let prose = spans(
            "- [*] #task **1200-1300** Contact #office [root:: love] ^contact",
        );
        assert_eq!(prose.title, "Contact #office");
        assert!(prose.badges.is_empty());
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
    fn unacked_tag_splits_into_the_suffix_not_the_title() {
        // Rootless and typeless: without the marker the tag would fall
        // into the hashed title span and read as a user edit.
        let row = spans(
            "- [*] #task **0800-0830** Standup [📅](https://cal.example/e1) \
             #unacked ^standup",
        );
        assert_eq!(row.title, "Standup");
        assert!(row.suffix.contains("#unacked"));
        assert_eq!(row.suffix, " #unacked ^standup");
        assert_eq!(row.hashes()["title"], journal::span_hash("Standup"));

        let rooted = spans(
            "- [ ] #task **0800-0830** Standup [📅](https://cal.example/e1) \
             [root:: meet] [type:: MEET] #unacked ^standup",
        );
        assert_eq!(rooted.title, "Standup");
        assert!(rooted.hashes()["badges"].len() == 16);

        // A lookalike stays title text and never counts as present.
        let lookalike = "- [ ] #task **0800-0830** Fix #unackedx thing ^fix";
        assert_eq!(spans(lookalike).title, "Fix #unackedx thing");
        assert!(!row_has_unacked(lookalike));
    }

    #[test]
    fn rendered_unacked_in_a_title_cannot_collide_with_the_marker() {
        // The renderer escapes mid-line hashes, so a literal word in a
        // title never parses as the shell marker.
        let title = crate::native::gcal::render::escape_title(
            "Discuss #unacked policy",
        );
        assert!(title.contains("\\#unacked"), "{title}");
        let line = format!("- [ ] #task **0800-0830** {title} ^discuss");
        let parsed = spans(&line);
        assert_eq!(parsed.title, title);
        assert!(!row_has_unacked(&line));
        // Hand-typed, unescaped: the marker wins and the planner only
        // reads presence off the suffix; the line itself is left alone.
        let typed =
            "- [ ] #task **0800-0830** Discuss #unacked policy ^discuss";
        assert!(row_has_unacked(typed));
        assert_eq!(spans(typed).title, "Discuss");
    }

    #[test]
    fn unacked_tag_add_and_strip_round_trip() {
        let line =
            "- [*] #task **0800-0830** Standup [📅](https://cal.example/e1) \
             [root:: meet] ^standup";
        let tagged = add_unacked_tag(line).expect("add");
        assert_eq!(
            tagged,
            "- [*] #task **0800-0830** Standup [📅](https://cal.example/e1) \
             [root:: meet] #unacked ^standup",
            "{tagged}",
        );
        assert!(row_has_unacked(&tagged));
        assert!(add_unacked_tag(&tagged).is_none(), "never double the tag");
        assert_eq!(strip_unacked_tag(&tagged).as_deref(), Some(line));

        // Rootless rows keep the tag last before the block id.
        let bare = "- [ ] #task **0800-0830** Standup ^standup";
        let tagged_bare = add_unacked_tag(bare).expect("add bare");
        assert_eq!(
            tagged_bare,
            "- [ ] #task **0800-0830** Standup #unacked ^standup",
        );
        assert_eq!(strip_unacked_tag(&tagged_bare).as_deref(), Some(bare));

        // Context rows never carry the tag.
        assert!(add_unacked_tag("- **all day** Holiday ^holiday").is_none());
        assert!(!row_has_unacked("- **all day** Holiday ^holiday"));
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
    fn open_meet_links_after_a_closed_entry_stay_untouched() {
        let note = concat!(
            "## Events\n",
            "- [/] #task **0800-0830** Standup ^standup\n",
            "- [*] #task **1830-1900** Dentist ^dentist\n",
            "\n## Pomodoros\n",
            "- [x] (**0800-0830** [t:: 30m]) — MEET\n",
            "\t- 🍅 [[#^standup]]\n",
            "- [ ] () — MEET\n",
            "\t- [[#^dentist]]\n",
        );
        let standup = note
            .lines()
            .position(|line| line.contains("^standup"))
            .expect("standup");
        let dentist = note
            .lines()
            .position(|line| line.contains("^dentist"))
            .expect("dentist");
        assert!(!row_untouched(note, standup, "standup"));
        assert!(row_untouched(note, dentist, "dentist"));
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
        assert_eq!(
            cancelled,
            concat!(
                "- [-] #task **0800-0830** Old [root:: meet] ^old\n",
                "- [-] #task **0900-1000** New [root:: meet] [cancelled:: 2026-10-30] ^new\n",
                "\t- ❌ **CANCEL LOG**\n",
                "\t\t- *2026-10-30* — Declined in Google Calendar\n",
            )
        );
    }

    #[test]
    fn cancel_log_reuses_tab_child_indent_and_lands_first() {
        let note = concat!(
            "- [ ] #task **0900-1000** New [root:: meet] ^new\n",
            "\t- And retainers!\n",
        );
        let cancelled = cancel_task_row(
            note,
            0,
            NaiveDate::from_ymd_opt(2026, 10, 30).expect("date"),
            "Declined in Google Calendar",
        )
        .expect("cancel applies");
        assert_eq!(
            cancelled,
            concat!(
                "- [-] #task **0900-1000** New [root:: meet] [cancelled:: 2026-10-30] ^new\n",
                "\t- ❌ **CANCEL LOG**\n",
                "\t\t- *2026-10-30* — Declined in Google Calendar\n",
                "\t- And retainers!\n",
            )
        );
    }
}

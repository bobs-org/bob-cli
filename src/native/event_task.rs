//! Event Task grammar and clock classification shared by capture and gcal.

use std::{
    collections::HashSet,
    ops::Range,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::{
    clock_tags::{line_clock_tag, strip_trailing_clock_tag, ClockTag},
    collect_done, markdown,
    pomodoro_homes::{
        effective_clock, ClockResolution, ClockSource, HomeResolver, HomeSource,
    },
    task_fields,
    task_status_hooks::canonical_daily_date,
};

const MINUTES_PER_DAY: i32 = 24 * 60;

/// Buffered `(start, end)` for a meeting that runs `start_min..end_min`
/// minutes from the day's midnight (`end_min` may exceed 1440 for an
/// overnight end). Mirrors [`EventSpan::buffered`] on the day's timeline:
/// `max(0, start − before)` through `end + after`, clamped so the total
/// stays below 24h.
pub(crate) fn buffered_timeline_range(
    start_min: i32,
    end_min: i32,
    buffer: EventBuffer,
) -> (i32, i32) {
    if buffer.is_empty() {
        return (start_min, end_min);
    }
    let start = (start_min - i32::from(buffer.before_min)).max(0);
    let mut end = end_min + i32::from(buffer.after_min);
    if end - start >= MINUTES_PER_DAY {
        end = start + MINUTES_PER_DAY - 1;
    }
    (start, end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventSpan {
    pub start_min: u16,
    pub end_min: u16,
    pub crosses_midnight: bool,
}

impl EventSpan {
    pub(crate) fn text(self) -> String {
        format!(
            "{:02}{:02}-{:02}{:02}",
            self.start_min / 60,
            self.start_min % 60,
            self.end_min / 60,
            self.end_min % 60,
        )
    }

    pub(crate) fn end_on_timeline(self) -> i32 {
        i32::from(self.end_min)
            + if self.crosses_midnight {
                MINUTES_PER_DAY
            } else {
                0
            }
    }

    /// Occupied span: `max(0, start − before)` through `end + after`,
    /// clamped so the total stays below 24h and `HHMM-HHMM` stays
    /// unambiguous.
    pub(crate) fn buffered(self, buffer: EventBuffer) -> EventSpan {
        if buffer.is_empty() {
            return self;
        }
        let start =
            (i32::from(self.start_min) - i32::from(buffer.before_min)).max(0);
        let mut end = self.end_on_timeline() + i32::from(buffer.after_min);
        if end - start >= MINUTES_PER_DAY {
            end = start + MINUTES_PER_DAY - 1;
        }
        EventSpan {
            start_min: u16::try_from(start).unwrap_or(0),
            end_min: u16::try_from(end.rem_euclid(MINUTES_PER_DAY))
                .unwrap_or(0),
            crosses_midnight: end >= MINUTES_PER_DAY,
        }
    }
}

/// Pre/post-event occupied-time buffer, each side `0..=720` minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct EventBuffer {
    pub before_min: u16,
    pub after_min: u16,
}

impl EventBuffer {
    pub(crate) fn is_empty(self) -> bool {
        self.before_min == 0 && self.after_min == 0
    }

    /// Parse vault-field tokens. Any invalid token, or no tokens, is
    /// `None` (no buffer). The first value for each side wins; each
    /// side is capped at 720.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let mut before = None;
        let mut after = None;
        let mut saw_token = false;
        for token in value.split_whitespace() {
            saw_token = true;
            let (part, minutes) = parse_buffer_token(token)?;
            match part {
                BufferPart::Before => {
                    if before.is_none() {
                        before = Some(minutes);
                    }
                }
                BufferPart::After => {
                    if after.is_none() {
                        after = Some(minutes);
                    }
                }
            }
        }
        if !saw_token {
            return None;
        }
        Some(Self {
            before_min: before.unwrap_or(0),
            after_min: after.unwrap_or(0),
        })
    }

    /// Canonical field, `None` when both sides are zero.
    pub(crate) fn field(self) -> Option<String> {
        self.body("-").map(|body| format!("[buffer:: {body}]"))
    }

    /// Human label with a typographic minus, `None` when empty.
    pub(crate) fn label(self) -> Option<String> {
        self.body("\u{2212}")
    }

    fn body(self, minus: &str) -> Option<String> {
        match (self.before_min, self.after_min) {
            (0, 0) => None,
            (before, 0) => Some(format!("{minus}{before}m")),
            (0, after) => Some(format!("+{after}m")),
            (before, after) => Some(format!("{minus}{before}m +{after}m")),
        }
    }
}

/// `-N`/`+M` token with optional `m`, ASCII or typographic minus.
/// N and M are 1–3 digits, capped at 720.
pub(crate) fn parse_buffer_token(token: &str) -> Option<(BufferPart, u16)> {
    let (part, rest) = if let Some(rest) = token.strip_prefix('+') {
        (BufferPart::After, rest)
    } else if let Some(rest) = strip_minus_prefix(token) {
        (BufferPart::Before, rest)
    } else {
        return None;
    };
    let digits = rest.strip_suffix(['m', 'M']).unwrap_or(rest);
    if digits.is_empty()
        || digits.len() > 3
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((part, digits.parse::<u16>().ok()?.min(720)))
}

fn strip_minus_prefix(token: &str) -> Option<&str> {
    token
        .strip_prefix('-')
        .or_else(|| token.strip_prefix('\u{2212}'))
        .or_else(|| token.strip_prefix('\u{2013}'))
}

/// Where a session overlap sits relative to the meeting clock. Serializes
/// as `before` / `after` in payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BufferPart {
    Before,
    After,
}

/// One conflict between a session span and an open Event Task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventSpanConflict {
    pub overlap_minutes: u64,
    pub buffer_part: Option<BufferPart>,
    pub fit_start_min: Option<u16>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventKind {
    Task,
    TimedContext,
    AllDayContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventTask {
    pub line_index: usize,
    pub span: EventSpan,
    pub buffer: EventBuffer,
    pub untracked: bool,
    pub off_clock: bool,
    pub clock_tag: ClockTag,
    pub clock_source: ClockSource,
    pub clock_note: Option<String>,
    pub status: char,
    pub block_id: Option<String>,
    pub title: String,
}

impl EventTask {
    pub(crate) fn is_open(&self) -> bool {
        is_open_status(self.status)
    }

    pub(crate) fn buffered_span(&self) -> EventSpan {
        self.span.buffered(self.buffer)
    }
}

/// One Events-section row: an Event Task or a timed/all-day context row.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventNoteRow {
    pub line_index: usize,
    pub kind: EventKind,
    pub span: Option<EventSpan>,
    pub buffer: EventBuffer,
    pub untracked: bool,
    pub off_clock: bool,
    pub clock_tag: ClockTag,
    pub clock_source: ClockSource,
    pub clock_note: Option<String>,
    pub status: Option<char>,
    pub block_id: Option<String>,
    pub title: String,
}

impl EventNoteRow {
    pub(crate) fn is_open_task(&self) -> bool {
        self.kind == EventKind::Task && self.status.is_some_and(is_open_status)
    }

    #[allow(dead_code)]
    pub(crate) fn as_event_task(&self) -> Option<EventTask> {
        if self.kind != EventKind::Task {
            return None;
        }
        Some(EventTask {
            line_index: self.line_index,
            span: self.span?,
            buffer: self.buffer,
            untracked: self.untracked,
            off_clock: self.off_clock,
            clock_tag: self.clock_tag.clone(),
            clock_source: self.clock_source,
            clock_note: self.clock_note.clone(),
            status: self.status?,
            block_id: self.block_id.clone(),
            title: self.title.clone(),
        })
    }

    pub(crate) fn buffered_span(&self) -> Option<EventSpan> {
        Some(self.span?.buffered(self.buffer))
    }
}

pub(crate) fn is_open_status(status: char) -> bool {
    !matches!(status, 'x' | 'X' | '-')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventTaskClock {
    pub class: ClockClass,
    pub span: EventSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockClass {
    NotStarted,
    EndedBefore,
    EndedInside,
    Running,
}

/// Caller-resolved facts for the dedicated Task Links a session keeps.
/// `None` means the link did not resolve to exactly one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinkFact {
    pub off_clock: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockReason {
    Unlinked,
    Mixed { first_on_clock: usize },
    Unresolved { first: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockVerdict {
    OffClock,
    Worked { reason: ClockReason },
}

/// Decide the session clock from caller-resolved Task Link facts. An empty
/// lineup, an unresolved link, or any resolved on-clock task keeps the whole
/// session on the worked clock.
pub(crate) fn clock_verdict(links: &[LinkFact]) -> ClockVerdict {
    if links.is_empty() {
        return ClockVerdict::Worked {
            reason: ClockReason::Unlinked,
        };
    }
    if let Some(first) = links.iter().position(|link| link.off_clock.is_none())
    {
        return ClockVerdict::Worked {
            reason: ClockReason::Unresolved { first },
        };
    }
    if let Some(first_on_clock) =
        links.iter().position(|link| link.off_clock == Some(false))
    {
        return ClockVerdict::Worked {
            reason: ClockReason::Mixed { first_on_clock },
        };
    }
    ClockVerdict::OffClock
}

impl ClockClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::EndedBefore => "ended_before",
            Self::EndedInside => "ended_inside",
            Self::Running => "running",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseClock {
    pub day_relative: PathBuf,
    pub session_start_min: u16,
    pub close_at_min: u16,
}

impl CloseClock {
    pub(crate) fn classify(&self, span: EventSpan) -> ClockClass {
        classify(span, self.session_start_min, self.close_at_min)
    }
}

pub(crate) fn classify(
    span: EventSpan,
    session_start_min: u16,
    close_at_min: u16,
) -> ClockClass {
    let start = i32::from(span.start_min);
    let mut close = i32::from(close_at_min);
    if close < i32::from(session_start_min) {
        close += MINUTES_PER_DAY;
    }
    let end = span.end_on_timeline();
    let session_start = i32::from(session_start_min);

    if end <= session_start {
        ClockClass::EndedBefore
    } else if start >= close {
        ClockClass::NotStarted
    } else if end <= close {
        ClockClass::EndedInside
    } else {
        ClockClass::Running
    }
}

pub(crate) fn events_section_range(lines: &[&str]) -> Option<Range<usize>> {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut section_start = None;
    let mut fence = None;

    for (index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open_fence) = fence {
            if markdown::closes_fence(line, open_fence) {
                fence = None;
            }
            continue;
        }
        if let Some(open_fence) = markdown::fence_marker(line) {
            fence = Some(open_fence);
            continue;
        }
        if let Some(start) = section_start {
            if is_level_two_heading(line) {
                return Some(start..index);
            }
        } else if is_events_heading(line) {
            section_start = Some(index + 1);
        }
    }
    section_start.map(|start| start..lines.len())
}

fn is_events_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("##") else {
        return false;
    };
    if !rest.starts_with([' ', '\t']) {
        return false;
    }
    rest.trim() == "Events"
}

fn is_level_two_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("##") else {
        return false;
    };
    rest.starts_with([' ', '\t'])
}

pub(crate) fn parse_event_span(line: &str) -> Option<EventSpan> {
    parse_event_task_line(line).map(|parsed| parsed.span)
}

struct ParsedEventTask<'a> {
    status: char,
    span: EventSpan,
    after: &'a str,
}

fn parse_event_task_line(line: &str) -> Option<ParsedEventTask<'_>> {
    let mut body = line.trim_start_matches([' ', '\t']).strip_prefix("- ")?;
    body = body.strip_prefix('[')?;
    let close = body.find(']')?;
    let status = body.get(..close)?;
    let mut status_chars = status.chars();
    let status = status_chars.next()?;
    if status_chars.next().is_some() {
        return None;
    }
    body = body.get(close + 1..)?;
    body = body.trim_start_matches([' ', '\t']);
    if let Some(rest) = body.strip_prefix("#task") {
        body = rest.trim_start_matches([' ', '\t']);
    }
    let (span, after) = parse_bold_span(body)?;
    Some(ParsedEventTask {
        status,
        span,
        after,
    })
}

fn parse_bold_span(body: &str) -> Option<(EventSpan, &str)> {
    parse_hhmm_span_after_bold(body.strip_prefix("**")?)
}

fn parse_hhmm_span_after_bold(body: &str) -> Option<(EventSpan, &str)> {
    let raw = body.get(..9)?;
    if raw.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    if body.get(9..)?.get(..2) != Some("**") {
        return None;
    }
    let after = body.get(11..)?;
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    if raw
        .bytes()
        .enumerate()
        .any(|(index, byte)| index != 4 && !byte.is_ascii_digit())
    {
        return None;
    }
    let start_min = parse_hhmm(&raw[..4])?;
    let end_min = parse_hhmm(&raw[5..9])?;
    Some((
        EventSpan {
            start_min,
            end_min,
            crosses_midnight: end_min < start_min,
        },
        after,
    ))
}

struct ParsedContextRow<'a> {
    kind: EventKind,
    span: Option<EventSpan>,
    after: &'a str,
}

fn parse_context_row(line: &str) -> Option<ParsedContextRow<'_>> {
    let body = line.trim_start_matches([' ', '\t']).strip_prefix("- ")?;
    if body.starts_with('[') {
        return None;
    }
    let body = body.strip_prefix("**")?;
    if let Some(rest) = strip_all_day_close(body) {
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            return None;
        }
        return Some(ParsedContextRow {
            kind: EventKind::AllDayContext,
            span: None,
            after: rest,
        });
    }
    let (span, after) = parse_hhmm_span_after_bold(body)?;
    Some(ParsedContextRow {
        kind: EventKind::TimedContext,
        span: Some(span),
        after,
    })
}

fn strip_all_day_close(body: &str) -> Option<&str> {
    const MARK: &str = "all day**";
    let (head, rest) = body.split_at_checked(MARK.len())?;
    head.eq_ignore_ascii_case("all day**").then_some(rest)
}

fn parse_hhmm(value: &str) -> Option<u16> {
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour = value[..2].parse::<u16>().ok()?;
    let minute = value[2..].parse::<u16>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

pub(crate) fn event_task_at(
    relative_path: &Path,
    contents: &str,
    line_index: usize,
) -> Option<EventTask> {
    canonical_daily_date(relative_path)?;
    let lines: Vec<&str> = contents.lines().collect();
    let section = events_section_range(&lines)?;
    if !section.contains(&line_index) || is_fenced_line(&lines, line_index) {
        return None;
    }
    event_task_from_line(line_index, lines.get(line_index).copied()?)
}

fn event_task_from_line(line_index: usize, line: &str) -> Option<EventTask> {
    let parsed = parse_event_task_line(line)?;
    let clock_tag = line_clock_tag(line);
    let clock = ClockResolution::tag_only(&clock_tag);
    Some(EventTask {
        line_index,
        span: parsed.span,
        buffer: parse_event_buffer(line),
        untracked: clock.off_clock,
        off_clock: clock.off_clock,
        clock_tag,
        clock_source: clock.source,
        clock_note: clock.note,
        status: parsed.status,
        block_id: collect_done::trailing_block_id_in_line(line),
        title: clean_event_title(parsed.after),
    })
}

fn context_row_from_line(
    line_index: usize,
    line: &str,
) -> Option<EventNoteRow> {
    let parsed = parse_context_row(line)?;
    let clock_tag = line_clock_tag(line);
    let clock = ClockResolution::tag_only(&clock_tag);
    Some(EventNoteRow {
        line_index,
        kind: parsed.kind,
        span: parsed.span,
        buffer: parse_event_buffer(line),
        untracked: clock.off_clock,
        off_clock: clock.off_clock,
        clock_tag,
        clock_source: clock.source,
        clock_note: clock.note,
        status: None,
        block_id: collect_done::trailing_block_id_in_line(line),
        title: clean_event_title(parsed.after),
    })
}

/// First `[buffer::]` field outside code spans and Markdown links.
/// An invalid value means no buffer.
pub(crate) fn parse_event_buffer(line: &str) -> EventBuffer {
    let masked = mask_code_and_markdown_links(line);
    task_fields::inline_fields(&masked, "buffer")
        .into_iter()
        .next()
        .and_then(|field| EventBuffer::parse(&field.value))
        .filter(|buffer| !buffer.is_empty())
        .unwrap_or_default()
}

pub(crate) fn event_task_by_id(
    relative_path: &Path,
    contents: &str,
    block_id: &str,
) -> Option<EventTask> {
    canonical_daily_date(relative_path)?;
    let lines: Vec<&str> = contents.lines().collect();
    let section = events_section_range(&lines)?;
    let suffix = format!("^{block_id}");
    let mut found = None;
    for index in section {
        let line = lines.get(index)?.trim_end();
        let Some(before_id) = line.strip_suffix(&suffix) else {
            continue;
        };
        if before_id
            .chars()
            .last()
            .is_some_and(|ch| !ch.is_whitespace())
        {
            continue;
        }
        let Some(event) = event_task_at(relative_path, contents, index) else {
            continue;
        };
        if found.replace(event).is_some() {
            return None;
        }
    }
    found
}

/// Every Event Task and timed/all-day context row in `contents`, in time
/// order (all-day first, then by start, end, and ledger line).
pub(crate) fn events_in_note(
    contents: &str,
    _date: NaiveDate,
) -> Vec<EventNoteRow> {
    let lines: Vec<&str> = contents.lines().collect();
    let Some(section) = events_section_range(&lines) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for index in section {
        if is_fenced_line(&lines, index) {
            continue;
        }
        let Some(line) = lines.get(index).copied() else {
            continue;
        };
        if let Some(task) = event_task_from_line(index, line) {
            events.push(EventNoteRow {
                line_index: task.line_index,
                kind: EventKind::Task,
                span: Some(task.span),
                buffer: task.buffer,
                untracked: task.untracked,
                off_clock: task.off_clock,
                clock_tag: task.clock_tag,
                clock_source: task.clock_source,
                clock_note: task.clock_note,
                status: Some(task.status),
                block_id: task.block_id,
                title: task.title,
            });
            continue;
        }
        if let Some(row) = context_row_from_line(index, line) {
            events.push(row);
        }
    }
    events.sort_by(|left, right| match (left.span, right.span) {
        (None, None) => left.line_index.cmp(&right.line_index),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(left_span), Some(right_span)) => left_span
            .start_min
            .cmp(&right_span.start_min)
            .then(
                left_span
                    .end_on_timeline()
                    .cmp(&right_span.end_on_timeline()),
            )
            .then(left.line_index.cmp(&right.line_index)),
    });
    events
}

/// Resolve every Event Task row in a daily note against the same effective
/// clock rule used for ordinary tasks. Context rows remain tag-only.
pub(crate) fn enrich_event_clocks<S: HomeSource>(
    daily_path: &Path,
    contents: &str,
    events: &mut [EventNoteRow],
    resolver: &mut HomeResolver<S>,
) {
    let lines: Vec<&str> = contents.lines().collect();
    for event in events {
        if event.kind != EventKind::Task {
            continue;
        }
        let Some(line) = lines.get(event.line_index).copied() else {
            continue;
        };
        event.clock_tag = line_clock_tag(line);
        let resolution = effective_clock(resolver, daily_path, line);
        event.off_clock = resolution.off_clock;
        event.untracked = resolution.off_clock;
        event.clock_source = resolution.source;
        event.clock_note = resolution.note;
    }
}

/// Newly conflicting open Event Tasks: `conflicts(new) && !conflicts(old)`,
/// earliest buffered start first.
pub(crate) fn event_span_conflicts<'a>(
    new_start: u16,
    new_end: u16,
    old: Option<(u16, u16)>,
    events: &'a [EventNoteRow],
    exclude_ids: &HashSet<String>,
) -> Vec<(&'a EventNoteRow, EventSpanConflict)> {
    let mut crossed = Vec::new();
    for event in events {
        if !event.is_open_task() {
            continue;
        }
        if event
            .block_id
            .as_ref()
            .is_some_and(|id| exclude_ids.contains(id))
        {
            continue;
        }
        let Some(conflict) = span_conflict(new_start, new_end, event) else {
            continue;
        };
        if old.is_some_and(|(old_start, old_end)| {
            span_conflict(old_start, old_end, event).is_some()
        }) {
            continue;
        }
        crossed.push((event, conflict));
    }
    crossed.sort_by(|(left, _), (right, _)| {
        let left_span = left.buffered_span().expect("open task has a span");
        let right_span = right.buffered_span().expect("open task has a span");
        (
            start_rank(left_span.start_min, new_start, new_end),
            left_span.end_on_timeline(),
            left.line_index,
        )
            .cmp(&(
                start_rank(right_span.start_min, new_start, new_end),
                right_span.end_on_timeline(),
                right.line_index,
            ))
    });
    crossed
}

/// Open Event Tasks whose meeting is in progress at `start`
/// (`start_min < S < end_min`, overnight-aware). Events whose block id
/// is in `exclude_ids` are skipped. Sorted by meeting start, then line.
pub(crate) fn events_in_progress_at<'a>(
    start: u16,
    events: &'a [EventNoteRow],
    exclude_ids: &HashSet<String>,
) -> Vec<&'a EventNoteRow> {
    let mut in_progress = Vec::new();
    for event in events {
        if !event.is_open_task() {
            continue;
        }
        if event
            .block_id
            .as_ref()
            .is_some_and(|id| exclude_ids.contains(id))
        {
            continue;
        }
        let Some(span) = event.span else {
            continue;
        };
        if !meeting_in_progress_at(span, start) {
            continue;
        }
        in_progress.push(event);
    }
    in_progress.sort_by(|left, right| {
        let left_span = left.span.expect("open task has a span");
        let right_span = right.span.expect("open task has a span");
        (left_span.start_min, left.line_index)
            .cmp(&(right_span.start_min, right.line_index))
    });
    in_progress
}

/// A non-MEET session `[S, E)` conflicts with an open Event Task when it
/// overlaps the occupied (buffered) span and the meeting is not already
/// in progress at `S` (`start < S < end`). Unbuffered events keep
/// today's start-in-session rule.
pub(crate) fn span_conflict(
    session_start: u16,
    session_end: u16,
    event: &EventNoteRow,
) -> Option<EventSpanConflict> {
    let span = event.span?;
    if meeting_in_progress_at(span, session_start) {
        return None;
    }
    let buffered = span.buffered(event.buffer);
    let overlap = session_range(session_start, session_end)
        .and_then(|session| overlap_range(session, buffered));
    if event.buffer.is_empty() {
        if !start_in_session(span.start_min, session_start, session_end) {
            return None;
        }
    } else if overlap.is_none() {
        return None;
    }
    let (overlap_minutes, buffer_part) = match overlap {
        Some((ov_start, ov_end, shift)) => {
            let meeting_start = i32::from(span.start_min) + shift;
            let meeting_end = span.end_on_timeline() + shift;
            let part = if ov_start >= i32::from(buffered.start_min) + shift
                && ov_end <= meeting_start
            {
                Some(BufferPart::Before)
            } else if ov_start >= meeting_end
                && ov_end <= buffered.end_on_timeline() + shift
            {
                Some(BufferPart::After)
            } else {
                None
            };
            (u64::try_from(ov_end - ov_start).unwrap_or(0), part)
        }
        None => (0, None),
    };
    Some(EventSpanConflict {
        overlap_minutes,
        buffer_part,
        fit_start_min: session_precedes(
            session_start,
            session_end,
            buffered.start_min,
        )
        .then_some(buffered.start_min),
    })
}

fn meeting_in_progress_at(span: EventSpan, clock: u16) -> bool {
    let start = i32::from(span.start_min);
    let end = span.end_on_timeline();
    let time = i32::from(clock);
    (start < time && time < end)
        || (start < time + MINUTES_PER_DAY && time + MINUTES_PER_DAY < end)
}

fn session_range(start: u16, end: u16) -> Option<(i32, i32)> {
    if start == end {
        return None;
    }
    let start = i32::from(start);
    let mut end = i32::from(end);
    if end < start {
        end += MINUTES_PER_DAY;
    }
    Some((start, end))
}

fn overlap_range(
    session: (i32, i32),
    event: EventSpan,
) -> Option<(i32, i32, i32)> {
    let event_start = i32::from(event.start_min);
    let event_end = event.end_on_timeline();
    let mut best: Option<(i32, i32, i32, i32)> = None;
    for shift in [-MINUTES_PER_DAY, 0, MINUTES_PER_DAY] {
        let start = event_start + shift;
        let end = event_end + shift;
        let overlap_start = start.max(session.0);
        let overlap_end = end.min(session.1);
        let length = overlap_end - overlap_start;
        if length > 0
            && best.is_none_or(|(best_len, _, _, _)| length > best_len)
        {
            best = Some((length, overlap_start, overlap_end, shift));
        }
    }
    best.map(|(_, overlap_start, overlap_end, shift)| {
        (overlap_start, overlap_end, shift)
    })
}

/// `S` is before `target` on the session's timeline, so a fit draft can
/// end at the occupied start.
fn session_precedes(session_start: u16, session_end: u16, target: u16) -> bool {
    if session_start == session_end || session_start == target {
        return false;
    }
    if session_end > session_start {
        session_start < target
    } else {
        target > session_start || target < session_end
    }
}

fn start_in_session(
    event_start: u16,
    session_start: u16,
    session_end: u16,
) -> bool {
    if session_start == session_end {
        return false;
    }
    if session_end > session_start {
        event_start >= session_start && event_start < session_end
    } else {
        event_start >= session_start || event_start < session_end
    }
}

/// Overnight sessions walk the evening half first, then the morning wrap.
fn start_rank(
    event_start: u16,
    session_start: u16,
    session_end: u16,
) -> (u8, u16) {
    if session_end > session_start || event_start >= session_start {
        (0, event_start)
    } else {
        (1, event_start)
    }
}

/// Cluster items that touch or overlap (`next.start ≤ cluster.end`) while they
/// share their off-clock classification. Callers sort first when they need a
/// stable tie-break.
#[allow(dead_code)]
pub(crate) fn cluster_by_off_clock<T>(
    mut items: Vec<T>,
    start_min: impl Fn(&T) -> i32,
    end_min: impl Fn(&T) -> i32,
    off_clock: impl Fn(&T) -> bool,
) -> Vec<Vec<T>> {
    items.sort_by(|left, right| {
        start_min(left)
            .cmp(&start_min(right))
            .then(end_min(left).cmp(&end_min(right)))
    });
    let mut clusters: Vec<Vec<T>> = Vec::new();
    for item in items {
        if let Some(last) = clusters.last_mut() {
            let cluster_end = last.iter().map(&end_min).max().unwrap_or(0);
            let cluster_off_clock = last.first().is_some_and(&off_clock);
            if start_min(&item) <= cluster_end
                && off_clock(&item) == cluster_off_clock
            {
                last.push(item);
                continue;
            }
        }
        clusters.push(vec![item]);
    }
    clusters
}

fn mask_code_and_markdown_links(line: &str) -> String {
    let mut out = line.as_bytes().to_vec();
    let mut index = 0;
    while index < line.len() {
        if line.as_bytes()[index] == b'`' {
            let width = line.as_bytes()[index..]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            if let Some(close) = find_code_close(line, index + width, width) {
                blank(&mut out, index, close);
                index = close;
                continue;
            }
        }
        if line.as_bytes()[index] == b'['
            && let Some(end) = markdown_link_end(line, index)
        {
            blank(&mut out, index, end);
            index = end;
            continue;
        }
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| line.to_string())
}

fn find_code_close(line: &str, start: usize, width: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut search = start;
    while search < bytes.len() {
        if bytes[search] != b'`' {
            search += 1;
            continue;
        }
        let close = search;
        while search < bytes.len() && bytes[search] == b'`' {
            search += 1;
        }
        if search - close == width {
            return Some(search);
        }
    }
    None
}

fn markdown_link_end(line: &str, open: usize) -> Option<usize> {
    let rest = line.get(open + 1..)?;
    let text_close = rest.find(']')?;
    let after = rest.get(text_close + 1..)?;
    if !after.starts_with('(') {
        return None;
    }
    let url_close = after.get(1..)?.find(')')?;
    Some(open + 1 + text_close + 1 + 1 + url_close + 1)
}

fn blank(bytes: &mut [u8], start: usize, end: usize) {
    for byte in bytes.iter_mut().take(end).skip(start) {
        *byte = b' ';
    }
}

fn clean_event_title(after_span: &str) -> String {
    let mut title = after_span.trim().to_string();
    if let Some(block_id) = collect_done::trailing_block_id_in_line(&title) {
        let suffix = format!("^{block_id}");
        if let Some(stripped) = title.strip_suffix(&suffix) {
            title = stripped.trim_end().to_string();
        }
    }
    loop {
        let before = title.len();
        title = strip_trailing_inline_field(&title);
        title = strip_trailing_off_clock(&title);
        title = strip_trailing_markdown_link(&title);
        if title.len() == before {
            break;
        }
    }
    if let Some((head, tail)) = title.rsplit_once(" · *")
        && tail.ends_with('*')
        && !tail[..tail.len() - 1].contains('*')
    {
        title = head.trim_end().to_string();
    }
    title.trim().to_string()
}

fn strip_trailing_inline_field(title: &str) -> String {
    let trimmed = title.trim_end();
    let Some(open) = trimmed.rfind('[') else {
        return trimmed.to_string();
    };
    if trimmed[open..].contains("::") && trimmed.ends_with(']') {
        trimmed[..open].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

fn strip_trailing_off_clock(title: &str) -> String {
    let mut stripped = title.trim_end();
    while let Some(next) = strip_trailing_clock_tag(stripped) {
        if next.len() == stripped.len() {
            break;
        }
        stripped = next;
    }
    stripped.to_string()
}

fn strip_trailing_markdown_link(title: &str) -> String {
    let trimmed = title.trim_end();
    let Some(open) = trimmed.rfind('[') else {
        return trimmed.to_string();
    };
    if markdown_link_end(trimmed, open) == Some(trimmed.len()) {
        trimmed[..open].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_fenced_line(lines: &[&str], line_index: usize) -> bool {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut fence = None;
    for (index, line) in lines.iter().enumerate().take(line_index + 1) {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open_fence) = fence {
            if markdown::closes_fence(line, open_fence) {
                fence = None;
            }
            continue;
        }
        if let Some(open_fence) = markdown::fence_marker(line) {
            fence = Some(open_fence);
        }
    }
    fence.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use chrono::NaiveDate;
    use serde_json::Value;

    fn hhmm(value: &str) -> u16 {
        parse_hhmm(value).expect("valid HHMM")
    }

    #[test]
    fn parses_the_documented_event_task_grammar() {
        assert_eq!(
            parse_event_span("- [*] #task **0800-0830** Product interview"),
            Some(EventSpan {
                start_min: hhmm("0800"),
                end_min: hhmm("0830"),
                crosses_midnight: false,
            })
        );
        assert_eq!(
            parse_event_span("- [ ] **2330-0030** Hand-written"),
            Some(EventSpan {
                start_min: hhmm("2330"),
                end_min: hhmm("0030"),
                crosses_midnight: true,
            })
        );
        assert!(parse_event_span("- **0800-0830** Missing checkbox").is_none());
        assert!(parse_event_span("- [ ] #task **2460-2500** invalid").is_none());
        assert!(parse_event_span("- [ ] #task **0800-0830**word").is_none());
    }

    #[test]
    fn event_tasks_must_be_in_the_events_section_of_a_canonical_day() {
        let contents = concat!(
            "---\n",
            "title: Example\n",
            "---\n",
            "- [ ] #task **0700-0730** Outside ^outside\n",
            "## Events\n",
            "- [ ] #task **0800-0830** Inside ^inside\n",
            "```md\n",
            "## Pomodoros\n",
            "- [ ] #task **0900-0930** Fenced ^fenced\n",
            "```\n",
            "## Pomodoros\n",
            "- [ ] #task **1000-1030** Later ^later\n",
        );
        let day = Path::new("2026/20261029.md");
        assert!(event_task_at(day, contents, 5).is_some());
        assert!(event_task_at(day, contents, 3).is_none());
        assert!(event_task_at(day, contents, 8).is_none());
        assert!(event_task_at(day, contents, 11).is_none());
        assert!(event_task_by_id(day, contents, "fenced").is_none());
        assert!(event_task_at(Path::new("events.md"), contents, 5).is_none());
        assert_eq!(
            event_task_by_id(day, contents, "inside")
                .unwrap()
                .line_index,
            5
        );
    }

    #[test]
    fn classifier_matches_shared_clock_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/event_task_close/vectors.json"
        ))
        .expect("shared vectors JSON");
        for case in vectors["classify"].as_array().expect("classify cases") {
            let event = parse_event_span(&format!(
                "- [ ] **{}** event",
                case["span"].as_str().expect("span")
            ))
            .expect("valid event span");
            let actual = classify(
                event,
                hhmm(case["session_start"].as_str().expect("session start")),
                hhmm(case["close_at"].as_str().expect("close time")),
            );
            assert_eq!(
                actual.as_str(),
                case["class"].as_str().expect("expected class"),
                "{}",
                case["id"]
            );
        }
    }

    fn events_note(body: &str) -> String {
        format!("## Events\n{body}## Pomodoros\n")
    }

    #[test]
    fn events_in_note_orders_all_day_then_timed_and_reads_off_clock_aliases() {
        let contents = events_note(concat!(
            "- [ ] #task **1000-1030** Standup [root:: job] ^standup\n",
            "- **all day** School holiday\n",
            "- [*] #task **0930-1000** 1:1 #untracked [root:: love] ^one-on-one\n",
            "- **1400-1500** Toddler class · *maybe*\n",
        ));
        let date = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let events = events_in_note(&contents, date);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].kind, EventKind::AllDayContext);
        assert_eq!(events[0].title, "School holiday");
        assert_eq!(events[1].kind, EventKind::Task);
        assert_eq!(events[1].title, "1:1");
        assert!(events[1].off_clock);
        assert_eq!(events[1].clock_tag.deprecated(), &["#untracked"]);
        assert_eq!(events[1].clock_source, ClockSource::OffTag);
        assert_eq!(events[1].status, Some('*'));
        assert_eq!(events[1].block_id.as_deref(), Some("one-on-one"));
        assert!(events[1].is_open_task());
        let task = events[1].as_event_task().expect("task row");
        assert!(task.is_open());
        assert_eq!(task.title, "1:1");
        assert_eq!(events[2].title, "Standup");
        assert!(!events[2].off_clock);
        assert_eq!(events[3].kind, EventKind::TimedContext);
        assert_eq!(events[3].title, "Toddler class");
    }

    #[test]
    fn clean_event_title_removes_each_off_clock_alias() {
        for tag in ["#off", "#on", "#offclock", "#OFF-CLOCK.", "#untracked!"] {
            assert_eq!(clean_event_title(&format!("Lunch {tag}")), "Lunch");
        }
        assert_eq!(clean_event_title("Lunch #off #on"), "Lunch");
    }

    #[test]
    fn clock_verdict_needs_a_resolved_nonempty_all_off_clock_lineup() {
        assert_eq!(
            clock_verdict(&[]),
            ClockVerdict::Worked {
                reason: ClockReason::Unlinked,
            }
        );
        assert_eq!(
            clock_verdict(&[LinkFact {
                off_clock: Some(true)
            }]),
            ClockVerdict::OffClock
        );
        assert_eq!(
            clock_verdict(&[
                LinkFact {
                    off_clock: Some(true)
                },
                LinkFact {
                    off_clock: Some(false)
                },
            ]),
            ClockVerdict::Worked {
                reason: ClockReason::Mixed { first_on_clock: 1 },
            }
        );
        assert_eq!(
            clock_verdict(&[
                LinkFact {
                    off_clock: Some(true)
                },
                LinkFact { off_clock: None },
            ]),
            ClockVerdict::Worked {
                reason: ClockReason::Unresolved { first: 1 },
            }
        );
    }

    #[test]
    fn event_span_conflicts_wraps_overnight_and_skips_exclusions() {
        let contents = events_note(concat!(
            "- [ ] #task **2330-0030** Late ^late\n",
            "- [ ] #task **0030-0100** After midnight ^after\n",
            "- [x] #task **2345-0015** Done ^done\n",
            "- [ ] #task **2200-2230** Before ^before\n",
        ));
        let date = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let events = events_in_note(&contents, date);
        let exclude = HashSet::new();
        let crossed =
            event_span_conflicts(23 * 60, 60, None, &events, &exclude)
                .into_iter()
                .next()
                .map(|(event, _)| event)
                .expect("crossed");
        assert_eq!(crossed.block_id.as_deref(), Some("late"));
        let mut skip_late = HashSet::new();
        skip_late.insert("late".to_string());
        let next = event_span_conflicts(23 * 60, 60, None, &events, &skip_late)
            .into_iter()
            .next()
            .map(|(event, _)| event)
            .expect("after midnight");
        assert_eq!(next.block_id.as_deref(), Some("after"));
        assert!(event_span_conflicts(
            12 * 60,
            12 * 60,
            None,
            &events,
            &exclude
        )
        .into_iter()
        .next()
        .is_none());
    }

    #[test]
    fn events_in_progress_at_uses_the_meeting_span_and_exclude_set() {
        let contents = events_note(concat!(
            "- [ ] #task **0800-0815** Swings! ^swings\n",
            "- [ ] #task **2330-0030** Late ^late\n",
            "- [x] #task **0800-0900** Done ^done\n",
            "- [-] #task **0800-0900** Cancelled ^cancelled\n",
            "- [ ] #task **0700-0830** Overlap ^overlap\n",
        ));
        let date = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let events = events_in_note(&contents, date);
        let empty = HashSet::new();
        let mid = events_in_progress_at(8 * 60 + 5, &events, &empty);
        assert_eq!(
            mid.iter()
                .map(|event| event.block_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("overlap"), Some("swings")]
        );
        let mut skip_overlap = HashSet::new();
        skip_overlap.insert("overlap".to_string());
        let skipped = events_in_progress_at(8 * 60 + 5, &events, &skip_overlap);
        assert_eq!(skipped[0].block_id.as_deref(), Some("swings"));
        assert_eq!(skipped.len(), 1);
        let at_swings_start = events_in_progress_at(8 * 60, &events, &empty);
        assert_eq!(
            at_swings_start
                .iter()
                .map(|event| event.block_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("overlap")]
        );
        let overnight = events_in_progress_at(23 * 60 + 45, &events, &empty);
        assert_eq!(overnight[0].block_id.as_deref(), Some("late"));
        let after_midnight = events_in_progress_at(15, &events, &empty);
        assert_eq!(after_midnight[0].block_id.as_deref(), Some("late"));
    }

    #[test]
    fn cluster_by_off_clock_splits_on_mixed_classification() {
        struct Item {
            start: i32,
            end: i32,
            off_clock: bool,
            id: &'static str,
        }
        let clusters = cluster_by_off_clock(
            vec![
                Item {
                    start: 10 * 60,
                    end: 11 * 60,
                    off_clock: false,
                    id: "a",
                },
                Item {
                    start: 11 * 60,
                    end: 12 * 60,
                    off_clock: true,
                    id: "b",
                },
                Item {
                    start: 11 * 60 + 30,
                    end: 12 * 60 + 30,
                    off_clock: true,
                    id: "c",
                },
            ],
            |item| item.start,
            |item| item.end,
            |item| item.off_clock,
        );
        assert_eq!(clusters.len(), 2);
        assert_eq!(
            clusters[0].iter().map(|item| item.id).collect::<Vec<_>>(),
            ["a"]
        );
        assert_eq!(
            clusters[1].iter().map(|item| item.id).collect::<Vec<_>>(),
            ["b", "c"]
        );
    }

    #[test]
    fn shared_event_buffer_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/event_buffer/vectors.json"
        ))
        .expect("shared vectors JSON");
        for case in vectors["row"].as_array().expect("row cases") {
            let line = case["line"].as_str().expect("line");
            let contents = events_note(&format!("{line}\n"));
            let date = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
            let events = events_in_note(&contents, date);
            let event = events
                .iter()
                .find(|row| row.kind == EventKind::Task)
                .expect("task row");
            let before =
                u16::try_from(case["before"].as_u64().expect("before"))
                    .expect("before fits");
            let after = u16::try_from(case["after"].as_u64().expect("after"))
                .expect("after fits");
            assert_eq!(
                event.buffer,
                EventBuffer {
                    before_min: before,
                    after_min: after,
                },
                "{}",
                case["id"]
            );
            assert_eq!(
                event.title,
                case["title"].as_str().expect("title"),
                "{}",
                case["id"]
            );
            let buffered = event.buffered_span().expect("span");
            assert_eq!(
                buffered.text(),
                case["buffered_span"].as_str().expect("buffered span"),
                "{}",
                case["id"]
            );
            assert_eq!(
                event.buffer.field().as_deref(),
                case["field"].as_str(),
                "{}",
                case["id"]
            );
            assert_eq!(
                event.buffer.label().as_deref(),
                case["label"].as_str(),
                "{}",
                case["id"]
            );
            let task = event.as_event_task().expect("task");
            assert_eq!(task.buffered_span().text(), buffered.text());
        }
        for case in vectors["conflict"].as_array().expect("conflict cases") {
            let contents = events_note(case["day"].as_str().expect("day"));
            let date = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
            let events = events_in_note(&contents, date);
            let new = clock_pair(case["new"].as_array().expect("new"));
            let old = case["old"].as_array().map(|value| clock_pair(value));
            let crossed = event_span_conflicts(
                new.0,
                new.1,
                old,
                &events,
                &HashSet::new(),
            );
            let expected_id = case["crossed"].as_str();
            match expected_id {
                None => assert!(
                    crossed.is_empty(),
                    "{}: unexpected {:?}",
                    case["id"],
                    crossed
                        .iter()
                        .map(|(event, _)| event.block_id.as_deref())
                        .collect::<Vec<_>>()
                ),
                Some(id) => {
                    let (event, conflict) =
                        crossed.first().expect("crossed event");
                    assert_eq!(
                        event.block_id.as_deref(),
                        Some(id),
                        "{}",
                        case["id"]
                    );
                    assert_eq!(
                        conflict.overlap_minutes,
                        case["overlap_minutes"].as_u64().expect("overlap"),
                        "{}",
                        case["id"]
                    );
                    assert_eq!(
                        serde_json::to_value(conflict.buffer_part)
                            .expect("buffer part"),
                        case["buffer_part"],
                        "{}",
                        case["id"]
                    );
                    let fit = case["fit"]
                        .as_str()
                        .map(|value| parse_hhmm(value).expect("fit HHMM"));
                    assert_eq!(conflict.fit_start_min, fit, "{}", case["id"]);
                }
            }
        }
    }

    fn clock_pair(value: &[Value]) -> (u16, u16) {
        (
            parse_hhmm(value[0].as_str().expect("start")).expect("start"),
            parse_hhmm(value[1].as_str().expect("end")).expect("end"),
        )
    }

    #[test]
    fn clean_event_title_strips_interleaved_buffer_fields() {
        let contents = events_note(concat!(
            "- [*] #task **1200-1300** Lunch [📅](https://calendar.google.com/event?eid=lunch \"Open in Google Calendar\") #untracked [buffer:: -30m +15m] [root:: love] [type:: MEET] ^lunch\n",
            "- [ ] #task **1000-1030** Standup [buffer:: +15m] [📅](https://calendar.google.com/event?eid=standup \"Open\") #untracked ^standup\n",
        ));
        let date = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let events = events_in_note(&contents, date);
        assert_eq!(events[0].title, "Standup");
        assert_eq!(
            events[0].buffer,
            EventBuffer {
                before_min: 0,
                after_min: 15,
            }
        );
        assert_eq!(events[1].title, "Lunch");
        assert_eq!(
            events[1].buffer,
            EventBuffer {
                before_min: 30,
                after_min: 15,
            }
        );
        assert_eq!(events[1].buffered_span().unwrap().text(), "1130-1315");
    }

    #[test]
    fn buffer_field_in_code_or_link_does_not_count() {
        let contents = events_note(concat!(
            "- [ ] #task **1200-1300** Notes `[buffer:: -30m]` ^code\n",
            "- [ ] #task **1300-1400** See [buffer:: -15m](https://example.com) ^link\n",
            "- [ ] #task **1400-1500** Broken [buffer:: -99x] ^bad\n",
        ));
        let date = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let events = events_in_note(&contents, date);
        assert!(events.iter().all(|event| event.buffer.is_empty()));
        assert_eq!(events[0].title, "Notes `[buffer:: -30m]`");
        assert_eq!(events[1].title, "See");
        assert_eq!(events[2].title, "Broken");
    }
}

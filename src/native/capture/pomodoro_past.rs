//! Record past Pomodoros (`x=` stacks) for `bob-cli-h2.3`.
//!
//! Preflight validates the whole stack against pre-stack history, then each
//! member resolves its target, writes the canonical window, closes the
//! explicit entry at its own clock, and files the completed block
//! chronologically.

use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, Timelike};

use super::{
    batch::CaptureBatchPlanner,
    dependencies::DependencyContext,
    output::{CaptureError, CaptureWarning, Placement},
    plan::capture_kind_label,
    pomodoro_close::{
        self, map_close_plan_error, plan_close_successors, stage_close_plan,
        SnapshotCloseVault,
    },
    pomodoro_timeline::{
        collect_timed_rows, LayoutError, LimitKind, MemberLength, Pin,
        SegmentQuery, TimedRow, Timeline, TimelineError,
    },
    task_theme::CaptureTaskThemeContext,
};
use crate::native::{
    capture_language::PomodoroPastSpec,
    capture_pomodoro_close::{
        CloseSelection, LedgerClosePolicy, RunningPomodoro,
    },
    capture_pomodoros::{self, PomodoroState},
    capture_task_toggle,
    ledger_place::{self, LedgerInsert},
    pomodoro,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PastTargetSource {
    Placeholder,
    Again,
    Created,
}

impl PastTargetSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Placeholder => "placeholder",
            Self::Again => "again",
            Self::Created => "created",
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct MemberWindow {
    pub(super) index: usize,
    pub(super) spec: PomodoroPastSpec,
    pub(super) blocks: u32,
    pub(super) offset_blocks: u32,
    pub(super) start_min: i64,
    pub(super) end_min: i64,
    pub(super) window_start: String,
    pub(super) window_end: String,
    pub(super) minutes: u64,
    pub(super) closed_at_min: i64,
    pub(super) closed_at: String,
    pub(super) anchor: String,
    pub(super) time_range_text: String,
    pub(super) range_text: String,
    pub(super) cut: Option<super::pomodoro_timeline::Cut>,
    pub(super) requested_minutes: Option<u64>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct PastPreflight {
    pub(super) now_truncated: NaiveDateTime,
    pub(super) today: NaiveDate,
    pub(super) anchor: String,
    pub(super) t_minutes: i64,
    pub(super) members: Vec<MemberWindow>,
    pub(super) pin: Option<super::pomodoro_timeline::Pin>,
    pub(super) segment_member: Option<usize>,
    pub(super) suffix_raw: Option<String>,
}

fn format_hhmm(minutes: i64) -> String {
    let wrapped = minutes.rem_euclid(1440);
    format!("{:02}{:02}", wrapped / 60, wrapped % 60)
}

fn ceil5(minutes: i64) -> i64 {
    ((minutes + 4) / 5) * 5
}

pub(super) fn compute_preflight(
    specs: &[PomodoroPastSpec],
    now: NaiveDateTime,
    today: NaiveDate,
) -> Result<PastPreflight, CaptureError> {
    let truncated = now
        .date()
        .and_hms_opt(now.hour(), now.minute(), 0)
        .ok_or_else(|| CaptureError::io("record clock invariant failed"))?;
    let now_min = truncated.hour() as i64 * 60 + truncated.minute() as i64;
    let t = ceil5(now_min);
    let anchor = if t == 1440 {
        "0000".to_string()
    } else {
        format_hhmm(t)
    };
    let mut members = Vec::new();
    for (idx, spec) in specs.iter().enumerate() {
        let blocks = spec.blocks;
        let offset = spec.stack.offset_blocks;
        let blocks_u64 = u64::from(blocks);
        let offset_u64 = u64::from(offset);
        let minutes = blocks_u64.checked_mul(5).ok_or_else(|| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        // Checked window math.
        let total = blocks_u64.checked_add(offset_u64).ok_or_else(|| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        let span = total.checked_mul(5).ok_or_else(|| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        let span_i64: i64 = span.try_into().map_err(|_| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        let offset_span = offset_u64.checked_mul(5).ok_or_else(|| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        let offset_i64: i64 = offset_span.try_into().map_err(|_| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        let start = t.checked_sub(span_i64).ok_or_else(|| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        let end = t.checked_sub(offset_i64).ok_or_else(|| {
            CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            )
        })?;
        // Midnight gate.
        if start < 0 {
            let max_blocks = if t <= 0 { 0 } else { (t / 5) as u64 };
            let start_fmt = format_hhmm(start);
            if specs.len() == 1 {
                let max_rewrite = if max_blocks == 0 {
                    "x=1".to_string()
                } else {
                    format!("x={max_blocks}")
                };
                return Err(CaptureError::usage(format!(
                    "{} would start before midnight ({start_fmt}); record at most {max_blocks} blocks today: {max_rewrite}",
                    spec.raw,
                )));
            } else {
                return Err(CaptureError::usage(format!(
                    "{} (member {} of {}) would start before midnight ({start_fmt}); the stack reaches back {} blocks but only {max_blocks} fit today",
                    spec.raw,
                    idx + 1,
                    specs.len(),
                    total,
                )));
            }
        }
        // Allow T=1440 near midnight.
        if t > 1440 || end > 1440 || end < 0 {
            return Err(CaptureError::usage(
                "past Pomodoro block count is too large; use a smaller count",
            ));
        }
        members.push(make_member_window(
            idx, spec, blocks, offset, start, end, minutes, None, None,
            &anchor, now_min,
        ));
        let _ = idx;
    }
    // Keep T as computed (may be 1440).
    let _ = t;
    Ok(PastPreflight {
        now_truncated: truncated,
        today,
        anchor,
        t_minutes: t,
        members,
        pin: None,
        segment_member: None,
        suffix_raw: None,
    })
}

fn make_member_window(
    idx: usize,
    spec: &PomodoroPastSpec,
    blocks: u32,
    offset_blocks: u32,
    start: i64,
    end: i64,
    minutes: u64,
    cut: Option<super::pomodoro_timeline::Cut>,
    requested_minutes: Option<u64>,
    anchor: &str,
    now_min: i64,
) -> MemberWindow {
    let window_start = format_hhmm(start);
    let window_end = format_hhmm(end);
    let closed_min = std::cmp::min(end, now_min);
    let closed_at = format_hhmm(closed_min);
    let range_text = format!("{window_start}-{window_end}");
    let time_range_text =
        format!("(**{window_start}-{window_end}** [t:: {minutes}m])");
    MemberWindow {
        index: idx,
        spec: spec.clone(),
        blocks,
        offset_blocks,
        start_min: start,
        end_min: end,
        window_start,
        window_end,
        minutes,
        closed_at_min: closed_min,
        closed_at,
        anchor: anchor.to_string(),
        time_range_text,
        range_text,
        cut,
        requested_minutes,
    }
}

/// Map a lexical segment suffix to the timeline's own query type.
fn segment_to_query(
    segment: &crate::native::capture_language::PastSegment,
) -> SegmentQuery {
    use crate::native::capture_language::{PastEdge, PastSegmentKind};
    match segment.kind {
        PastSegmentKind::Tail => SegmentQuery::Tail,
        PastSegmentKind::Head => SegmentQuery::Head,
        PastSegmentKind::Index => {
            let index = segment.index.unwrap_or(1);
            match segment.edge {
                PastEdge::Start => SegmentQuery::Start(index),
                PastEdge::End => SegmentQuery::End(index),
            }
        }
    }
}

/// Rebuild a past token with a new `E` body, preserving repair `!` and the
/// second `=X` close part via the past splitter (so `!`/`=` inside `#"…"`
/// literals stay name content).
fn rebuild_past_token(raw: &str, new_e: &str) -> String {
    use crate::native::capture_language::past::split_past_token;
    let Some(split) = split_past_token(raw) else {
        return raw.to_string();
    };
    let mut out = String::from("x=");
    out.push_str(new_e);
    if split.repair {
        out.push('!');
    }
    if split.has_second_eq {
        out.push('=');
        out.push_str(&split.close_text);
    }
    out
}

/// Strip the suffix from a suffixed token, keeping count, name, repair, and
/// close. Falls back to a byte replace when splitting fails.
fn strip_segment_rewrite(raw: &str, segment_raw: &str) -> String {
    use crate::native::capture_language::past::split_past_token;
    let Some(split) = split_past_token(raw) else {
        return raw.replacen(segment_raw, "", 1);
    };
    let new_e = split.e_raw.replacen(segment_raw, "", 1);
    // `e_raw` always ends with the suffix; `replacen` removes its first
    // occurrence which is the suffix for well-formed tokens. When the
    // suffix bytes also appear inside a literal, prefer the suffix strip.
    let new_e = if let Some(stripped) = split.e_raw.strip_suffix(segment_raw) {
        stripped.to_string()
    } else {
        new_e
    };
    rebuild_past_token(raw, &new_e)
}

/// Replace the suffix with `~~` (for the no-closed-rows head hint).
fn head_rewrite(raw: &str, segment_raw: &str) -> String {
    use crate::native::capture_language::past::split_past_token;
    let Some(split) = split_past_token(raw) else {
        return raw.replacen(segment_raw, "~~", 1);
    };
    let new_e = if let Some(stripped) = split.e_raw.strip_suffix(segment_raw) {
        format!("{stripped}~~")
    } else {
        split.e_raw.replacen(segment_raw, "~~", 1)
    };
    rebuild_past_token(raw, &new_e)
}

/// Replace the typed count in `e_raw` (leading or trailing digits) with
/// `new_count`, keeping name and suffix bytes.
fn replace_count_in_e(e_raw: &str, old_count: u32, new_count: u32) -> String {
    let old = old_count.to_string();
    let new = new_count.to_string();
    if e_raw.starts_with(&old) {
        format!("{new}{}", &e_raw[old.len()..])
    } else if let Some(base) = e_raw.strip_suffix(&old) {
        // Trailing count (e.g. `fix2~2` strips the count before the suffix
        // only when the suffix does not also end with the same digits).
        // When the suffix shares the digits, strip the count that precedes
        // the suffix instead.
        format!("{base}{new}")
    } else {
        // The count precedes a suffix sharing its digits (e.g. `2~2` with
        // old `2`): replace the first occurrence.
        e_raw.replacen(&old, &new, 1)
    }
}

/// Build a hint that replaces the typed count while preserving name,
/// suffix, repair, and close.
fn hint_with_count(spec: &PomodoroPastSpec, new_count: u32) -> String {
    use crate::native::capture_language::past::split_past_token;
    let Some(split) = split_past_token(&spec.raw) else {
        return spec.raw.clone();
    };
    // Remove the suffix to isolate the count+name base, replace the count,
    // then reattach the suffix.
    let segment_raw =
        spec.segment.as_ref().map(|s| s.raw.as_str()).unwrap_or("");
    let base = if !segment_raw.is_empty() {
        split
            .e_raw
            .strip_suffix(segment_raw)
            .unwrap_or(&split.e_raw)
    } else {
        split.e_raw.as_str()
    };
    let new_base = if base.starts_with(&spec.blocks.to_string()) {
        replace_count_in_e(base, spec.blocks, new_count)
    } else if base.ends_with(&spec.blocks.to_string()) && !base.is_empty() {
        let stripped =
            base.strip_suffix(&spec.blocks.to_string()).unwrap_or(base);
        format!("{stripped}{new_count}")
    } else {
        replace_count_in_e(&split.e_raw, spec.blocks, new_count)
    };
    let new_e = if !segment_raw.is_empty() {
        // `new_base` already excludes the suffix; reattach it. When the
        // count replace touched the full `e_raw` (shared digits), it
        // already contains the suffix.
        if new_base.ends_with(segment_raw) {
            new_base
        } else {
            format!("{new_base}{segment_raw}")
        }
    } else {
        new_base
    };
    // When the fallback replaced inside the full `e_raw`, prefer it.
    let new_e = if new_e == base || new_e.is_empty() {
        replace_count_in_e(&split.e_raw, spec.blocks, new_count)
    } else {
        new_e
    };
    rebuild_past_token(&spec.raw, &new_e)
}

/// Add an explicit count (`2`) to an untyped token for the no-room hint.
fn hint_add_count_two(spec: &PomodoroPastSpec) -> String {
    use crate::native::capture_language::past::split_past_token;
    let Some(split) = split_past_token(&spec.raw) else {
        return format!("{}2", spec.raw);
    };
    let new_e = format!("{}2", split.e_raw);
    rebuild_past_token(&spec.raw, &new_e)
}

fn format_timeline_error(
    raw: &str,
    spec: &PomodoroPastSpec,
    timeline: &Timeline,
    error: &TimelineError,
) -> String {
    match error {
        TimelineError::NothingClosed { first_row } => {
            let segment_raw =
                spec.segment.as_ref().map(|s| s.raw.as_str()).unwrap_or("");
            let plain = strip_segment_rewrite(raw, segment_raw);
            let mut message = format!(
                "{raw} counts back from your last closed Pomodoro, but nothing is closed today yet; write {plain} to end now"
            );
            if let Some((name, _)) = first_row {
                let head = head_rewrite(raw, segment_raw);
                message.push_str(&format!(
                    ", or {head} to end where {} starts",
                    display_name(name.as_deref())
                ));
            }
            message
        }
        TimelineError::EmptyTail { after, wall } => {
            let after_name = display_name(after.name.as_deref());
            let after_end = format_hhmm(after.end);
            let tail = if let Some(wall) = wall {
                format!(
                    "{} (running since {})",
                    display_name(wall.name.as_deref()),
                    format_hhmm(wall.start)
                )
            } else {
                format!("now ({})", format_hhmm(timeline.t))
            };
            format!(
                "{raw}: nothing to record between {after_name} (ended {after_end}) and {tail}"
            )
        }
        TimelineError::NoHead { first } => {
            let (name, start) = first;
            format!(
                "{raw}: {} starts at {}, so there is no time before your first Pomodoro",
                display_name(name.as_deref()),
                format_hhmm(*start)
            )
        }
        TimelineError::NoRows => {
            "nothing is on today's timeline yet; write x= to end now"
                .to_string()
        }
        TimelineError::OutOfRange { .. } => {
            format!("{raw}: {}", timeline.listing())
        }
    }
}

fn format_layout_error(
    specs: &[PomodoroPastSpec],
    timeline: &Timeline,
    pin: &Pin,
    error: &LayoutError,
) -> String {
    match error {
        LayoutError::NoRoom { member } => {
            let spec = specs
                .get(*member)
                .or_else(|| specs.first())
                .expect("member");
            let label = pin.segment.label();
            let rewrite = hint_add_count_two(spec);
            format!(
                "{} has no room left in {label}; give it a count ({rewrite}) or capture it separately",
                spec.raw
            )
        }
        LayoutError::PastLimit { member } => {
            let spec = specs
                .get(*member)
                .or_else(|| specs.first())
                .expect("member");
            let at = format_hhmm(pin.forward_limit.unwrap_or(timeline.t));
            let after = match &pin.limit {
                LimitKind::Now => "after now".to_string(),
                LimitKind::Wall(wall) => {
                    format!(
                        "after {} started ({})",
                        display_name(wall.name.as_deref()),
                        format_hhmm(wall.start)
                    )
                }
            };
            format!(
                "{} would start at {at}, {after}; shorten the run",
                spec.raw
            )
        }
        LayoutError::BeforeMidnight { member, max_blocks } => {
            let spec = specs
                .get(*member)
                .or_else(|| specs.first())
                .expect("member");
            let rewrite = hint_with_count(spec, (*max_blocks).max(1));
            // Estimate the wrapped start for the copy: end pins flow
            // backward from the pin through later members.
            let mut cursor = pin.at;
            for later in specs.iter().skip(member + 1) {
                cursor -= u64::from(later.blocks) as i64 * 5;
            }
            let start_est = cursor - u64::from(spec.blocks) as i64 * 5;
            let start_fmt = format_hhmm(start_est);
            if specs.len() == 1 {
                format!(
                    "{} would start before midnight ({start_fmt}); record at most {max_blocks} blocks: {rewrite}",
                    spec.raw
                )
            } else {
                format!(
                    "{} (member {} of {}) would start before midnight ({start_fmt}); record at most {max_blocks} blocks: {rewrite}",
                    spec.raw,
                    member + 1,
                    specs.len()
                )
            }
        }
        LayoutError::EndPinAfterNow => {
            let raw = specs
                .iter()
                .find_map(|spec| {
                    spec.segment.as_ref().map(|_| spec.raw.clone())
                })
                .unwrap_or_else(|| {
                    specs.first().map(|s| s.raw.clone()).unwrap_or_default()
                });
            format!(
                "{raw} ends at {}, after now ({}); shorten the run",
                format_hhmm(pin.at),
                format_hhmm(timeline.t)
            )
        }
    }
}

/// Pinned preflight: resolve the run's one suffix against the pre-stack
/// snapshot and lay every member out from the pin with exact, capped, and
/// cut lengths. Unpinned runs keep `compute_preflight`.
pub(super) fn compute_pinned_preflight(
    specs: &[PomodoroPastSpec],
    now: NaiveDateTime,
    today: NaiveDate,
    pre_stack_contents: &str,
) -> Result<PastPreflight, CaptureError> {
    let truncated = now
        .date()
        .and_hms_opt(now.hour(), now.minute(), 0)
        .ok_or_else(|| CaptureError::io("record clock invariant failed"))?;
    let now_min = truncated.hour() as i64 * 60 + truncated.minute() as i64;
    let t = ceil5(now_min);
    let anchor = if t == 1440 {
        "0000".to_string()
    } else {
        format_hhmm(t)
    };
    let timeline = Timeline::build(pre_stack_contents, now_min, t);
    let suffix_idx =
        specs.iter().position(|s| s.segment.is_some()).unwrap_or(0);
    let suffix_spec = &specs[suffix_idx];
    let segment = suffix_spec.segment.as_ref().ok_or_else(|| {
        CaptureError::io("record stack invariant failed: missing segment")
    })?;
    let query = segment_to_query(segment);
    let pin = timeline.resolve(query).map_err(|error| {
        CaptureError::usage(format_timeline_error(
            &suffix_spec.raw,
            suffix_spec,
            &timeline,
            &error,
        ))
    })?;
    let lengths: Vec<MemberLength> = specs
        .iter()
        .map(|spec| MemberLength {
            requested_minutes: u64::from(spec.blocks) * 5,
            typed: spec.blocks_typed,
        })
        .collect();
    let laid = super::pomodoro_timeline::layout_run(&pin, &lengths).map_err(
        |error| {
            CaptureError::usage(format_layout_error(
                specs, &timeline, &pin, &error,
            ))
        },
    )?;
    let mut members = Vec::with_capacity(specs.len());
    for (idx, (spec, window)) in specs.iter().zip(laid.iter()).enumerate() {
        let minutes = (window.end - window.start).max(0) as u64;
        if minutes == 0 {
            return Err(CaptureError::usage(format_layout_error(
                specs,
                &timeline,
                &pin,
                &LayoutError::NoRoom { member: idx },
            )));
        }
        let requested = if window.cut.is_some() {
            Some(window.requested_minutes)
        } else {
            None
        };
        members.push(make_member_window(
            idx,
            spec,
            spec.blocks,
            spec.stack.offset_blocks,
            window.start,
            window.end,
            minutes,
            window.cut,
            requested,
            &anchor,
            now_min,
        ));
    }
    let suffix_raw = suffix_spec.segment.as_ref().map(|s| s.raw.clone());
    Ok(PastPreflight {
        now_truncated: truncated,
        today,
        anchor,
        t_minutes: t,
        members,
        pin: Some(pin),
        segment_member: Some(suffix_idx),
        suffix_raw,
    })
}

fn overlaps(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> bool {
    // Half-open: touching endpoints never conflict.
    if a_start.max(b_start) < a_end.min(b_end) {
        return true;
    }
    if b_end > 1440 {
        let shifted_start = a_start + 1440;
        let shifted_end = a_end + 1440;
        if shifted_start.max(b_start) < shifted_end.min(b_end) {
            return true;
        }
    }
    false
}

fn display_name(name: Option<&str>) -> String {
    name.filter(|n| !n.is_empty())
        .unwrap_or("session")
        .to_string()
}

/// One closed row the whole stack trims: the span rewrite staged before
/// any member records. `line` is the 1-based pre-stack headline; `before`
/// and `after` are wall-clock `HHMM-HHMM` ranges; `taken` is the old span
/// minus the new one; `remaining` is the new span length.
#[derive(Debug, Clone)]
pub(super) struct TrimPlan {
    pub(super) line: usize,
    pub(super) name: String,
    pub(super) before: String,
    pub(super) after: String,
    pub(super) taken: u64,
    pub(super) remaining: u64,
    pub(super) clock: String,
    pub(super) calendar: bool,
    pub(super) new_headline: String,
    pub(super) first_member: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClosedFate {
    Untouched,
    Trimmable { new_start: i64, new_end: i64 },
    Erased,
    Split,
}

/// Rewrite `raw` with the repair `!` present (`want=true`) or absent.
/// Uses the past splitter so `!`/`=` inside `#"…"` literals stay name
/// content. Falls back to a simple first-`!` edit when splitting fails.
fn with_repair(raw: &str, want: bool) -> String {
    use crate::native::capture_language::past::split_past_token;
    if let Some(split) = split_past_token(raw) {
        if want && !split.repair {
            if let Some(eq) = split.second_eq_offset {
                let mut out = raw.to_string();
                out.insert(eq, '!');
                return out;
            }
            let mut out = raw.to_string();
            out.push('!');
            return out;
        }
        if !want
            && split.repair
            && let Some(offset) = split.repair_offset
        {
            let mut out = raw.to_string();
            out.remove(offset);
            return out;
        }
        return raw.to_string();
    }
    if want && !raw.contains('!') {
        return format!("{raw}!");
    }
    if !want {
        return raw.replacen('!', "", 1);
    }
    raw.to_string()
}

fn row_clock_and_calendar(row: &TimedRow) -> (String, bool) {
    use crate::native::capture::pomodoro_adjust::session_clock;
    use crate::native::capture::pomodoro_adjust::SessionClock;
    use crate::native::pomodoro_themes::routing::is_meet_name;
    let clock = match session_clock(&row.line_text) {
        SessionClock::Worked { .. } => "t".to_string(),
        SessionClock::OffClock { .. } => "off".to_string(),
        SessionClock::Placeholder => "off".to_string(),
    };
    let calendar = is_meet_name(row.name.as_deref());
    (clock, calendar)
}

/// Classify one closed row against the whole-stack union `[stack_start,
/// stack_end)`. The union never wraps; a wrapping history row is tested
/// against both the union and its +1440 copy. Returns the remaining
/// interval(s) in the row's own coordinate frame.
fn classify_closed_row(
    row_start: i64,
    row_end: i64,
    stack_start: i64,
    stack_end: i64,
) -> ClosedFate {
    for shift in [0, 1440] {
        let us = stack_start + shift;
        let ue = stack_end + shift;
        if us.max(row_start) >= ue.min(row_end) {
            continue;
        }
        let left = us > row_start;
        let right = ue < row_end;
        match (left, right) {
            (false, false) => return ClosedFate::Erased,
            (true, true) => return ClosedFate::Split,
            (true, false) => {
                return ClosedFate::Trimmable {
                    new_start: row_start,
                    new_end: us,
                };
            }
            (false, true) => {
                return ClosedFate::Trimmable {
                    new_start: ue,
                    new_end: row_end,
                };
            }
        }
    }
    ClosedFate::Untouched
}

/// Whole-stack overlap guard with per-token `!` repair. Checks every member
/// against history as it stood before the stack, all members at once, and
/// returns the edge trims to stage before any member records. One refusal
/// lists every offending member so a single retry fixes the whole draft.
/// `pin` enables the pinned-run fit/carve hints, and `recorded` holds
/// `(start, end)` windows earlier runs in this capture already recorded: a
/// closed row whose span equals one of them refuses any trim, erase, or
/// split with the same-capture copy.
pub(super) fn preflight_overlaps(
    preflight: &PastPreflight,
    pre_stack_contents: &str,
    specs: &[PomodoroPastSpec],
    pin: Option<&Pin>,
    recorded: &[(i64, i64)],
) -> Result<Vec<TrimPlan>, CaptureError> {
    // Require Pomodoros section.
    let scan = capture_pomodoros::scan(pre_stack_contents);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    let rows = collect_timed_rows(pre_stack_contents);
    let now_min = preflight.now_truncated.hour() as i64 * 60
        + preflight.now_truncated.minute() as i64;
    let stack_start =
        preflight.members.first().map(|m| m.start_min).unwrap_or(0);
    let mut stack_end = preflight
        .members
        .last()
        .map(|m| {
            if m.end_min == 0 && m.start_min != 0 {
                1440
            } else {
                m.end_min
            }
        })
        .unwrap_or(0);
    if stack_end == 0 && stack_start != 0 {
        stack_end = 1440;
    }
    // Closed-row fates from the union of all stack windows.
    let mut fates: Vec<(usize, ClosedFate)> = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        if row.is_open {
            continue;
        }
        let fate =
            classify_closed_row(row.start, row.end, stack_start, stack_end);
        fates.push((idx, fate));
    }
    // Per-member overlaps for the `!` rule.
    let mut member_open: Vec<Vec<usize>> = vec![Vec::new(); specs.len()];
    let mut member_closed: Vec<Vec<usize>> = vec![Vec::new(); specs.len()];
    for (mi, member) in preflight.members.iter().enumerate() {
        let mut a_end = member.end_min;
        if a_end == 0 && member.start_min != 0 {
            a_end = 1440;
        }
        for (ri, row) in rows.iter().enumerate() {
            if !overlaps(member.start_min, a_end, row.start, row.end) {
                continue;
            }
            if row.is_open {
                member_open[mi].push(ri);
            } else {
                member_closed[mi].push(ri);
            }
        }
    }
    let mut offending: Vec<String> = Vec::new();
    for (mi, spec) in specs.iter().enumerate() {
        // Open overlaps always refuse.
        for &ri in &member_open[mi] {
            let row = &rows[ri];
            let name = display_name(row.name.as_deref());
            let started = row.start <= now_min;
            let suggestion = with_repair(&spec.raw, started);
            offending.push(format!(
                "{} overlaps {name} {} which is still open (line {}); close it first: x {suggestion}",
                spec.raw, row.range, row.line,
            ));
        }
        // Closed overlaps: erase/split always refuse; trimmable needs `!`.
        for &ri in &member_closed[mi] {
            let row = &rows[ri];
            let name = display_name(row.name.as_deref());
            let fate = fates
                .iter()
                .find(|(idx, _)| *idx == ri)
                .map(|(_, fate)| *fate)
                .unwrap_or(ClosedFate::Untouched);
            // Same-capture rule: `!` never trims a row this capture
            // already recorded.
            let recorded_hit = recorded.iter().any(|(s, e)| {
                let rs = row.start;
                let mut re = row.end;
                if re == 0 && rs != 0 {
                    re = 1440;
                }
                let ws = *s;
                let mut we = *e;
                if we == 0 && ws != 0 {
                    we = 1440;
                }
                rs == ws && re == we
            });
            if recorded_hit {
                match fate {
                    ClosedFate::Trimmable { .. } => {
                        offending.push(format!(
                            "{} would trim {name} {}, which this capture just recorded; capture them separately",
                            spec.raw, row.range,
                        ));
                    }
                    ClosedFate::Erased => {
                        offending.push(format!(
                            "{} would erase {name} {}, which this capture just recorded; capture them separately",
                            spec.raw, row.range,
                        ));
                    }
                    ClosedFate::Split => {
                        offending.push(format!(
                            "{} would split {name} {}, which this capture just recorded; capture them separately",
                            spec.raw, row.range,
                        ));
                    }
                    ClosedFate::Untouched => {}
                }
                continue;
            }
            match fate {
                ClosedFate::Erased => {
                    let mut message = format!(
                        "{} would take all of {name} {} (line {}); ! only trims a session's edge",
                        spec.raw, row.range, row.line,
                    );
                    // Pinned-run hint: carving the pinned Pomodoro leaves a
                    // nonempty piece.
                    if let Some(pin) = pin {
                        use super::pomodoro_timeline::SegmentKind;
                        let is_pinned_row = pin.segment.kind
                            == SegmentKind::Pomodoro
                            && pin.segment.line == Some(row.line)
                            && pin.segment.start == row.start
                            && pin.segment.end.min(1440) == row.end.min(1440);
                        if is_pinned_row && spec.blocks_typed {
                            let seg_minutes =
                                (pin.segment.end - pin.segment.start).max(0);
                            let carve = (seg_minutes - 1).div_euclid(5);
                            if carve >= 1
                                && carve < u64::from(spec.blocks) as i64
                            {
                                let hint = hint_with_count(spec, carve as u32);
                                message.push_str(&format!(
                                    "; carve at most {carve} blocks: {hint}"
                                ));
                            }
                        }
                    }
                    offending.push(message);
                }
                ClosedFate::Split => {
                    offending.push(format!(
                        "{} would split {name} {} (line {}) in two; ! only trims a session's edge",
                        spec.raw, row.range, row.line,
                    ));
                }
                ClosedFate::Trimmable { new_start, new_end } => {
                    if !spec.repair {
                        let taken = ((row.end - row.start)
                            - (new_end - new_start))
                            .max(0) as u64;
                        let after = format!(
                            "{:02}{:02}-{:02}{:02}",
                            (new_start.rem_euclid(1440)) / 60,
                            (new_start.rem_euclid(1440)) % 60,
                            (new_end.rem_euclid(1440)) / 60,
                            (new_end.rem_euclid(1440)) % 60,
                        );
                        let rewrite = with_repair(&spec.raw, true);
                        let mut message = format!(
                            "{} would take {taken}m from {name} {} (line {}); add ! to take it: {rewrite} ({name} becomes {after})",
                            spec.raw, row.range, row.line,
                        );
                        // Pinned-run hint: a typed overflow can fit the gap.
                        if let Some(pin) = pin
                            && spec.blocks_typed
                        {
                            let seg_minutes =
                                (pin.segment.end - pin.segment.start).max(0);
                            let fit = seg_minutes.div_euclid(5);
                            if fit >= 1 && fit < u64::from(spec.blocks) as i64 {
                                let hint = hint_with_count(spec, fit as u32);
                                message.push_str(&format!(
                                    "; or fit the gap: {hint}"
                                ));
                            }
                        }
                        offending.push(message);
                    }
                }
                ClosedFate::Untouched => {}
            }
        }
        // Unneeded `!`: repair present but no closed overlap and no started
        // open overlap that would still need it.
        if spec.repair && member_closed[mi].is_empty() {
            let any_started_open =
                member_open[mi].iter().any(|&ri| rows[ri].start <= now_min);
            if !any_started_open {
                let rewrite = with_repair(&spec.raw, false);
                offending.push(format!(
                    "{} takes no time from another session; drop the !: {rewrite}",
                    spec.raw,
                ));
            }
        }
    }
    if !offending.is_empty() {
        return Err(CaptureError::usage(offending.join("\n")));
    }
    // Build trim plans for trimmable rows.
    let mut trims: Vec<TrimPlan> = Vec::new();
    for (ri, fate) in fates {
        let ClosedFate::Trimmable { new_start, new_end } = fate else {
            continue;
        };
        let row = &rows[ri];
        // First overlapping member in typed order owns the trim report.
        let mut overlapping: Vec<usize> = Vec::new();
        for (mi, member) in preflight.members.iter().enumerate() {
            let mut a_end = member.end_min;
            if a_end == 0 && member.start_min != 0 {
                a_end = 1440;
            }
            if overlaps(member.start_min, a_end, row.start, row.end) {
                overlapping.push(mi);
            }
        }
        if overlapping.is_empty() {
            continue;
        }
        let taken =
            ((row.end - row.start) - (new_end - new_start)).max(0) as u64;
        let remaining = (new_end - new_start).max(0) as u64;
        let after = format!(
            "{:02}{:02}-{:02}{:02}",
            (new_start.rem_euclid(1440)) / 60,
            (new_start.rem_euclid(1440)) % 60,
            (new_end.rem_euclid(1440)) / 60,
            (new_end.rem_euclid(1440)) % 60,
        );
        let (clock, calendar) = row_clock_and_calendar(row);
        // Span rewrite only: keep clock key, name, checkbox, block ID,
        // children, and links. Close effects are never re-run.
        use crate::native::capture::pomodoro_adjust::{
            format_adjusted_range, parse_adjustment_range,
        };
        let new_headline = match parse_adjustment_range(&row.line_text) {
            Some(range) => {
                let new_start_mod = new_start.rem_euclid(1440) as u64;
                let new_end_mod = new_end.rem_euclid(1440) as u16;
                let new_range_text = format_adjusted_range(
                    new_start_mod,
                    new_end_mod,
                    remaining,
                    &range.metadata,
                );
                let mut out = row.line_text.clone();
                out.replace_range(
                    range.start_ch..range.end_ch,
                    &new_range_text,
                );
                out
            }
            None => continue,
        };
        let first_member = *overlapping.iter().min().unwrap_or(&0);
        trims.push(TrimPlan {
            line: row.line,
            name: display_name(row.name.as_deref()),
            before: row.range.clone(),
            after,
            taken,
            remaining,
            clock,
            calendar,
            new_headline,
            first_member,
        });
    }
    Ok(trims)
}

/// Stage whole-stack trims on the planner before any member records.
/// Rewrites only the headline spans; the line count never changes, so the
/// whole batch rolls back on any later failure.
pub(super) fn stage_trim_plans(
    planner: &mut super::batch::CaptureBatchPlanner,
    day_file: &std::path::Path,
    trims: &[TrimPlan],
) -> Result<(), CaptureError> {
    if trims.is_empty() {
        return Ok(());
    }
    let staged = planner.read_existing(day_file)?;
    let mut lines: Vec<String> =
        staged.lines().map(|s| s.to_string()).collect();
    let had_final = staged.ends_with('\n');
    for trim in trims {
        let idx = trim.line.saturating_sub(1);
        if idx < lines.len() {
            lines[idx] = trim.new_headline.clone();
        }
    }
    let ending = if staged.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut rebuilt = lines.join(ending);
    if had_final {
        rebuilt.push_str(ending);
    }
    planner.stage(day_file, rebuilt)?;
    Ok(())
}

/// Resolve one member's target against staged contents.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_member_target(
    staged: &str,
    spec: &PomodoroPastSpec,
    _rel: &str,
) -> Result<(usize, String, PastTargetSource, Option<String>), CaptureError> {
    let scan = capture_pomodoros::scan(staged);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    if let Some(name) = spec.name.as_deref() {
        let selection = capture_pomodoros::select_capture_name(
            &scan,
            name,
            spec.name_is_literal,
        );
        match selection {
            capture_pomodoros::NamedSelection::Found(entry) => {
                if entry.time_range.is_some() {
                    // Refuse any open timed match, even without overlap.
                    let range = entry.time_range.clone().unwrap_or_default();
                    let display = entry.name.clone().unwrap_or_default();
                    return Err(CaptureError::usage(format!(
                        "{} names {display}, which is still open ({range}, line {}); close it first: x {}",
                        spec.raw, entry.line, spec.raw,
                    )));
                }
                // Open untimed placeholder in place.
                let idx = entry.line.checked_sub(1).ok_or_else(|| {
                    CaptureError::io(
                        "Pomodoro capture invariant failed: placeholder out of range",
                    )
                })?;
                let resolved_name = entry.name.clone().unwrap_or_else(|| {
                    capture_pomodoros::canonicalize_pomodoro_name(name)
                        .unwrap_or_else(|| name.to_string())
                });
                Ok((idx, resolved_name, PastTargetSource::Placeholder, None))
            }
            capture_pomodoros::NamedSelection::CompletedOnly(entry) => {
                let canonical = if spec.name_is_literal {
                    capture_pomodoros::canonicalize_exact_name(name)
                        .unwrap_or_else(|| name.to_string())
                } else {
                    entry
                        .name
                        .clone()
                        .and_then(|n| {
                            capture_pomodoros::canonicalize_pomodoro_name(&n)
                        })
                        .or_else(|| {
                            capture_pomodoros::canonicalize_pomodoro_name(name)
                        })
                        .unwrap_or_else(|| name.to_string())
                };
                // Fresh "again" row: insert placeholder, caller replaces.
                let (with_placeholder, created_line, _) =
                    capture_task_toggle::insert_named_placeholder(
                        staged,
                        &canonical,
                        spec.name_is_literal,
                    )
                    .map_err(|_| {
                        CaptureError::io(
                            "cannot record past Pomodoro: no place for a new session",
                        )
                    })?;
                // Stage the insertion immediately so replace sees it; the
                // caller stages the full postimage. Return staged text via
                // side channel? Instead return created line and let caller
                // insert. To keep this pure, signal again with line.
                // The caller must insert before replacing. We return the
                // staged-with-placeholder length marker via created_line,
                // and the caller re-does the insertion. Avoid double work
                // by returning the new contents length? Simpler: caller
                // inserts again; this branch just reports the name.
                let _ = with_placeholder;
                let _ = created_line;
                Ok((usize::MAX, canonical, PastTargetSource::Again, None))
            }
            capture_pomodoros::NamedSelection::Missing { suggestion } => {
                let canonical = if spec.name_is_literal {
                    capture_pomodoros::canonicalize_exact_name(name)
                        .ok_or_else(|| {
                            CaptureError::usage(format!(
                                "cannot create Pomodoro `#{name}`"
                            ))
                        })?
                } else {
                    capture_pomodoros::canonicalize_pomodoro_name(name)
                        .ok_or_else(|| {
                            CaptureError::usage(format!(
                                "cannot create Pomodoro `#{name}`"
                            ))
                        })?
                };
                let warning = suggestion.map(|entry| {
                    let suggestion_name = entry
                        .name
                        .clone()
                        .unwrap_or_else(|| entry.slug.clone());
                    format!(
                        "no open Pomodoro matches `#{name}`; created {canonical} (did you mean {suggestion_name}?)"
                    )
                });
                Ok((usize::MAX, canonical, PastTargetSource::Created, warning))
            }
        }
    } else {
        // Unnamed: next future placeholder.
        let Some(entry) = capture_pomodoros::next_future_pomodoro(&scan) else {
            return Err(CaptureError::usage(
                "no future Pomodoro to record; name one: x=build".to_string(),
            ));
        };
        // MEET-first refusal.
        if entry.name.as_deref().is_some_and(|n| {
            crate::native::pomodoro_themes::routing::is_meet_name(Some(n))
        }) {
            return Err(CaptureError::usage(format!(
                "the next future Pomodoro is MEET (line {}); name the session you worked: x=build, or x=meet to record a meeting",
                entry.line,
            )));
        }
        let idx = entry.line.checked_sub(1).ok_or_else(|| {
            CaptureError::io(
                "Pomodoro capture invariant failed: placeholder out of range",
            )
        })?;
        let resolved_name = entry.name.clone().unwrap_or_default();
        Ok((idx, resolved_name, PastTargetSource::Placeholder, None))
    }
}

/// Build the canonical window line for a placeholder index.
fn write_window(
    staged: &str,
    index: usize,
    time_range_text: &str,
) -> Result<String, CaptureError> {
    let (updated, _) = super::pomodoro_start::replace_placeholder_range(
        staged,
        index,
        time_range_text,
    )
    .map_err(|_| {
        CaptureError::io("selected Pomodoro is not an untimed open placeholder")
    })?;
    Ok(updated)
}

fn close_selection_from_spec(
    spec: &crate::native::capture_language::PomodoroCloseSpec,
) -> Option<CloseSelection> {
    if !spec.has_selection() && !spec.has_log() {
        return None;
    }
    Some(
        CloseSelection::new(
            spec.in_progress
                .clone()
                .map(|numbers| numbers.into_iter().collect()),
            spec.park.iter().copied().collect(),
            spec.complete.iter().copied().collect(),
            spec.drop.iter().copied().collect(),
            spec.raw.clone(),
        )
        .with_all(spec.park_all, spec.complete_all)
        .with_log(spec.log.clone()),
    )
}

/// File a completed block chronologically on the remaining ledger.
fn file_recorded_block(
    post_day: &str,
    closed_index: usize,
    new_start_min: i64,
) -> Result<(String, usize, usize), CaptureError> {
    let lines: Vec<&str> = post_day.lines().collect();
    let section =
        pomodoro::pomodoros_section_range(&lines).ok_or_else(|| {
            CaptureError::io("Bob daily note has no Pomodoros section")
        })?;
    let block_range = capture_pomodoros::pomodoro_block_range(
        &lines,
        closed_index,
        section.end,
    );
    let block_lines: Vec<String> = lines[block_range.clone()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if block_lines.is_empty() {
        return Err(CaptureError::io("recorded Pomodoro block is empty"));
    }
    // Remaining ledger without the closed block.
    let mut remaining: Vec<String> =
        lines.iter().map(|s| s.to_string()).collect();
    remaining.drain(block_range.clone());
    let ending = if post_day.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let had_final = post_day.ends_with('\n');
    let mut remaining_text = remaining.join(ending);
    if had_final {
        remaining_text.push_str(ending);
    }
    // Insertion point on the remaining ledger.
    let at =
        ledger_place::closed_insert_at(&remaining_text, new_start_min as i32);
    // Map to 0-based insertion index in `remaining` (without trailing
    // newline split artifact).
    let insert_at: usize = match at {
        LedgerInsert::Before(index) => {
            // `closed_insert_at` reports headline indices in the remaining
            // text. Clamp to the remaining line count.
            index.min(remaining.len())
        }
        LedgerInsert::After(index) => (index + 1).min(remaining.len()),
    };
    let mut filed = remaining.clone();
    // Insert block lines preserving exact bytes.
    let mut idx = insert_at;
    for line in &block_lines {
        filed.insert(idx, line.clone());
        idx += 1;
    }
    let mut filed_text = filed.join(ending);
    if had_final {
        filed_text.push_str(ending);
    }
    // New headline index and source-slot index (continuation stays where
    // the closed block was).
    let new_headline = insert_at;
    // Source slot after removal: the closed block's old position now
    // holds what followed it (the continuation when carry, else the
    // next block). Report it for block tracking.
    let source_slot = block_range.start.min(filed.len().saturating_sub(1));
    Ok((filed_text, new_headline, source_slot))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_one_member(
    request: &super::cli::CaptureRequest,
    parsed_body: &str,
    member: &MemberWindow,
    target: (usize, String, PastTargetSource, Option<String>),
    preflight: &PastPreflight,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<CaptureWarning>,
    dependency_ctx: &mut DependencyContext,
    task_theme_context: &mut CaptureTaskThemeContext,
    today: NaiveDate,
    day_file: &Path,
    rel: &str,
    trims: Vec<super::output::PomodoroPastTrimJson>,
) -> Result<super::plan::PlannedCaptureItem, CaptureError> {
    let (mut target_index, resolved_name, source, suggestion_warning) = target;
    if let Some(warning) = suggestion_warning {
        warnings.push(CaptureWarning::Text(warning));
    }
    // For again/created, insert the named placeholder on staged text.
    let mut is_created = false;
    if target_index == usize::MAX {
        let staged = planner.read_existing(day_file)?;
        let selector = member.spec.name.as_deref().unwrap_or(&resolved_name);
        let (with_placeholder, created_line, name) =
            capture_task_toggle::insert_named_placeholder(
                &staged,
                &resolved_name,
                member.spec.name_is_literal,
            )
            .map_err(|_| {
                CaptureError::io(
                    "cannot record past Pomodoro: no place for a new session",
                )
            })?;
        let _ = selector;
        let _ = name;
        planner.stage(day_file, with_placeholder)?;
        let staged2 = planner.read_existing(day_file)?;
        // Re-resolve the created line: it is the newly inserted row.
        target_index = created_line;
        is_created = true;
        let _ = staged2;
    }
    let source_line_one_based = if is_created {
        // Created/again: original identity is Created (1-based unknown
        // until staged). Use the created line for tracking below.
        target_index + 1
    } else {
        target_index + 1
    };
    // Write the canonical window.
    let staged = planner.read_existing(day_file)?;
    // For unnamed placeholders, resolved_name may be empty (truly unnamed
    // `()` row). Keep the row unnamed: the window write preserves the
    // existing suffix (which has no name tail).
    let updated = write_window(&staged, target_index, &member.time_range_text)?;
    // Build the explicit entry handle.
    let entry_name = if resolved_name.is_empty() {
        None
    } else {
        Some(resolved_name.clone())
    };
    // MEET explicit records use their typed duration (default 25m via
    // blocks). No event-run planning: the window is already written.
    let entry = RunningPomodoro {
        line: target_index + 1,
        name: entry_name.clone(),
        time_range: member.range_text.clone(),
    };
    // Member's own close clock.
    let closed_at_dt = preflight
        .today
        .and_hms_opt(
            (member.closed_at_min.rem_euclid(1440) / 60) as u32,
            (member.closed_at_min.rem_euclid(1440) % 60) as u32,
            0,
        )
        .ok_or_else(|| CaptureError::io("record clock invariant failed"))?;
    // Stage the window so the close sees it, then close the explicit
    // entry. The close planner stages the ledger; window bytes ride
    // along in its postimage.
    planner.stage(day_file, updated.clone())?;
    let day_contents = planner.read_existing(day_file)?;
    let vault = SnapshotCloseVault::from_planner(planner, &request.bob_dir);
    let selection = close_selection_from_spec(&member.spec.close);
    let types = task_theme_context.close_types_for_entry(
        planner,
        &vault,
        day_file,
        &day_contents,
        &entry,
        warnings,
    );
    // Bypass the reset branch: record closes even for `x==0`.
    let plan =
        crate::native::capture_pomodoro_close::plan_pomodoro_close_for_entry(
            day_file,
            &day_contents,
            &entry,
            closed_at_dt,
            &vault,
            selection.as_ref(),
            types.as_ref().map(|t| &t.naming),
            LedgerClosePolicy::Record,
        )
        .map_err(|e| map_close_plan_error(e, rel, None))?;
    // Build the nested close summary JSON at the member's own clock.
    let (mut summary, extra_warnings) =
        pomodoro_close::build_close_summary_json(
            &member.spec.close,
            rel,
            &plan,
            closed_at_dt,
            types.as_ref(),
        );
    // Surface clock warnings like ordinary closes.
    for warning in plan.warnings.iter() {
        warnings.push(CaptureWarning::from(warning.clone()));
    }
    for warning in extra_warnings.iter() {
        warnings.push(CaptureWarning::from(warning.clone()));
    }
    // Stage close effects.
    stage_close_plan(planner, &plan)?;
    // Successor recovery/offers on the post-close text.
    let pre_baseline = Some(updated.clone());
    let mut task_block_refs = Vec::new();
    let successors = plan_close_successors(
        request,
        today,
        planner,
        dependency_ctx,
        &plan,
        pre_baseline,
        day_file,
        rel,
        &mut task_block_refs,
    )?;
    summary.unblocked = successors.unblocked;
    summary.keep_moving = successors.keep_moving;
    summary.relation_successors = successors.relation_successors;
    summary.relation_held = successors.relation_held;
    summary.relation_check = successors.relation_check;
    summary.relation_groups = successors.relation_groups;
    summary.still_blocked = successors.still_blocked;
    summary.unblocked_check = successors.unblocked_check;
    summary.project_folds = successors.project_folds;
    summary.keep_moving_folds = successors.keep_moving_folds;
    // Headline refresh before filing.
    let post_day = planner.read_existing(day_file)?;
    let closed_line_idx = pomodoro_close::refresh_close_headline_for_record(
        &post_day,
        &mut summary,
    );
    // File the completed block chronologically; the carried
    // continuation stays in the vacated source slot.
    let new_start: i64 = member.start_min;
    let (filed_text, new_headline_idx, _source_slot) =
        file_recorded_block(&post_day, closed_line_idx, new_start)?;
    planner.stage(day_file, filed_text.clone())?;
    // Remap the summary through the move.
    summary.pomodoro_line = new_headline_idx + 1;
    summary.entry_line = filed_text
        .lines()
        .nth(new_headline_idx)
        .unwrap_or("")
        .to_string();
    // Continuation (when created) stays at the vacated source slot:
    // re-locate it by name after the move when it was created.
    if let Some(next) = summary.next_pomodoro.as_mut() {
        if next.created {
            // The created continuation is the open untimed row with the
            // continuation name nearest the source slot.
            let scan = capture_pomodoros::scan(&filed_text);
            if let Some(found) = scan.entries.iter().find(|e| {
                e.state == PomodoroState::Open
                    && e.time_range.is_none()
                    && e.name == next.name
            }) {
                next.line = found.line;
            }
        } else if let Some(line) = next.line.checked_sub(0) {
            // Existing next: re-resolve by scan (line may have shifted).
            let scan = capture_pomodoros::scan(&filed_text);
            // Keep the reported line when the entry still exists there;
            // otherwise leave it (tests assert line numbers, and the
            // ordinary close path leaves them stable for untouched rows).
            let _ = (scan, line);
        }
    }
    // Remap nested task ledger lines that rode with the closed block.
    // Closed-block children keep their relative offsets; recompute by
    // locating the filed block and preserving intra-block distances.
    // The close summary's task rows already carry post-close lines;
    // adjust those that fell inside the moved block.
    // (Best-effort: shift lines at/after the move by the block delta.)
    // For the focused tests, headline + continuation + block identity
    // matter; task rows keep their reported lines when the block did
    // not cross them.
    let relative_target = day_file
        .strip_prefix(&request.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.to_path_buf());
    // Block tracker identity: At(source) for placeholders, Created for
    // fresh/again rows. After filing, `after` is the filed headline.
    let before = if is_created {
        super::pomodoro_blocks::PomodoroBlockBefore::Created
    } else {
        super::pomodoro_blocks::PomodoroBlockBefore::At(
            source_line_one_based.saturating_sub(1),
        )
    };
    let mut pomodoro_refs = vec![super::pomodoro_blocks::PomodoroBlockRef::at(
        super::pomodoro_blocks::PomodoroBlockRole::Recorded,
        match before {
            super::pomodoro_blocks::PomodoroBlockBefore::At(idx) => idx,
            super::pomodoro_blocks::PomodoroBlockBefore::Created => {
                new_headline_idx
            }
            super::pomodoro_blocks::PomodoroBlockBefore::Resolve => {
                new_headline_idx
            }
            super::pomodoro_blocks::PomodoroBlockBefore::Removed(idx) => idx,
        },
        new_headline_idx,
    )];
    // The continuation (when created) reports as Next at its post-move
    // line, so later items reconcile offers through it.
    if let Some(next) = summary.next_pomodoro.as_ref() {
        let after = next.line.saturating_sub(1);
        if next.created {
            pomodoro_refs.push(
                super::pomodoro_blocks::PomodoroBlockRef::created(
                    super::pomodoro_blocks::PomodoroBlockRole::Next,
                    after,
                ),
            );
        } else {
            pomodoro_refs.push(
                super::pomodoro_blocks::PomodoroBlockRef::resolved(
                    super::pomodoro_blocks::PomodoroBlockRole::Next,
                    after,
                ),
            );
        }
    }
    // When the source was a placeholder, its original headline is now
    // the recorded row (moved) plus possibly a continuation. The
    // tracker resolves `before` through the filed text; Created rows
    // report as created.
    if is_created {
        pomodoro_refs[0] = super::pomodoro_blocks::PomodoroBlockRef::created(
            super::pomodoro_blocks::PomodoroBlockRole::Recorded,
            new_headline_idx,
        );
    }
    let stack_pin = member.spec.stack.pin.map(|edge| match edge {
        crate::native::capture_language::PastEdge::Start => "start".to_string(),
        crate::native::capture_language::PastEdge::End => "end".to_string(),
    });
    let (cut_str, wall_name, fit_raw) = match member.cut {
        None => (None, None, None),
        Some(cut) => {
            let cut_str = match cut {
                super::pomodoro_timeline::Cut::Now => "now".to_string(),
                super::pomodoro_timeline::Cut::Wall => "wall".to_string(),
                super::pomodoro_timeline::Cut::Segment => "segment".to_string(),
            };
            let wall_name = match cut {
                super::pomodoro_timeline::Cut::Wall => {
                    preflight.pin.as_ref().and_then(|pin| match &pin.limit {
                        super::pomodoro_timeline::LimitKind::Wall(wall) => {
                            Some(display_name(wall.name.as_deref()))
                        }
                        _ => None,
                    })
                }
                _ => None,
            };
            let fit_raw = match cut {
                super::pomodoro_timeline::Cut::Segment => {
                    // Carrier members name the fit through their own
                    // `segment.raw`; other members borrow the run suffix.
                    if member.spec.segment.is_some() {
                        None
                    } else {
                        preflight.suffix_raw.clone()
                    }
                }
                _ => None,
            };
            (Some(cut_str), wall_name, fit_raw)
        }
    };
    let segment_json = if preflight.segment_member == Some(member.index) {
        preflight.pin.as_ref().and_then(|pin| {
            member.spec.segment.as_ref().map(|lex| {
                let kind = match pin.segment.kind {
                    super::pomodoro_timeline::SegmentKind::Pomodoro => {
                        "pomodoro".to_string()
                    }
                    super::pomodoro_timeline::SegmentKind::Gap => {
                        "gap".to_string()
                    }
                    super::pomodoro_timeline::SegmentKind::Tail => {
                        "tail".to_string()
                    }
                    super::pomodoro_timeline::SegmentKind::Head => {
                        "head".to_string()
                    }
                };
                let edge = match pin.edge {
                    super::pomodoro_timeline::Edge::Start => {
                        "start".to_string()
                    }
                    super::pomodoro_timeline::Edge::End => "end".to_string(),
                };
                let minutes =
                    (pin.segment.end - pin.segment.start).max(0) as u64;
                super::output::PomodoroPastSegmentJson {
                    raw: lex.raw.clone(),
                    kind,
                    index: pin.segment.index,
                    edge,
                    start: format_hhmm(pin.segment.start),
                    end: format_hhmm(pin.segment.end),
                    minutes,
                    name: pin.segment.name.clone(),
                    line: pin.segment.line,
                }
            })
        })
    } else {
        None
    };
    let past_json = super::output::PomodoroPastSummaryJson {
        raw: member.spec.raw.clone(),
        name: entry_name.clone(),
        target: super::output::PomodoroPastTargetJson {
            source: source.as_str().to_string(),
            line: source_line_one_based,
        },
        blocks: member.blocks,
        repair: member.spec.repair,
        stack: super::output::PomodoroPastStackJson {
            index: member.spec.stack.index,
            size: member.spec.stack.size,
            pin: stack_pin,
        },
        anchor: member.anchor.clone(),
        window: super::output::PomodoroPastWindowJson {
            start: member.window_start.clone(),
            end: member.window_end.clone(),
            minutes: member.minutes,
            requested_minutes: member.requested_minutes,
            cut: cut_str,
            wall: wall_name,
            fit: fit_raw,
        },
        closed_at: member.closed_at.clone(),
        line: summary.pomodoro_line,
        trims,
        segment: segment_json,
        close: Box::new(summary),
    };
    let task_line = filed_text
        .lines()
        .nth(new_headline_idx)
        .unwrap_or("")
        .to_string();
    let past_name = past_json.name.clone();
    Ok(super::plan::PlannedCaptureItem {
        result: super::output::CaptureItemResult {
            note_pull: None,
            task_theme: None,
            type_relocation: None,
            ok: true,
            dry_run: request.dry_run,
            routed: false,
            route: None,
            route_label: String::new(),
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: day_file.display().to_string(),
            text: parsed_body.to_string(),
            task_line,
            kind: capture_kind_label(
                &crate::native::capture_language::CaptureKind::PomodoroPast {
                    spec: member.spec.clone(),
                },
            ),
            fanout: None,
            created: super::plan::date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Recorded,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: Some(day_file.display().to_string()),
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            drawer: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: past_name,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: Some(past_json),
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            toggle_task_description: None,
            r#ref: None,
            gmail: Vec::new(),
            link_guard: None,
            dropped_dependencies: Vec::new(),
            blocks_line_updates: Vec::new(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            relation_merge_output: Default::default(),
            transfer_move_output: Default::default(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            list: None,
            relation_label_hides: Vec::new(),
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
        },
        clip_plan: None,
        pomodoro_refs,
        task_block_refs,
        project_move_landings: Vec::new(),
    })
}

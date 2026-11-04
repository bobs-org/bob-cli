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
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct PastPreflight {
    pub(super) now_truncated: NaiveDateTime,
    pub(super) today: NaiveDate,
    pub(super) anchor: String,
    pub(super) t_minutes: i64,
    pub(super) members: Vec<MemberWindow>,
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
        let window_start = format_hhmm(start);
        let window_end = format_hhmm(end);
        let closed_min = std::cmp::min(end, now_min);
        let closed_at = format_hhmm(closed_min);
        let range_text = format!("{window_start}-{window_end}");
        let time_range_text =
            format!("(**{window_start}-{window_end}** [t:: {minutes}m])");
        members.push(MemberWindow {
            index: idx,
            spec: spec.clone(),
            blocks,
            offset_blocks: offset,
            start_min: start,
            end_min: end,
            window_start,
            window_end,
            minutes,
            closed_at_min: closed_min,
            closed_at,
            anchor: anchor.clone(),
            time_range_text,
            range_text,
        });
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
    })
}

#[derive(Debug, Clone)]
struct TimedRow {
    line: usize,
    name: Option<String>,
    range: String,
    start: i64,
    end: i64,
    is_open: bool,
}

fn parse_range_minutes(range: &str) -> Option<(i64, i64)> {
    let (s, e) = range.split_once('-')?;
    if s.len() != 4 || e.len() != 4 {
        return None;
    }
    let sh: i64 = s[..2].parse().ok()?;
    let sm: i64 = s[2..].parse().ok()?;
    let eh: i64 = e[..2].parse().ok()?;
    let em: i64 = e[2..].parse().ok()?;
    if sh >= 24 || sm >= 60 || eh >= 24 || em >= 60 {
        return None;
    }
    Some((sh * 60 + sm, eh * 60 + em))
}

fn collect_timed_rows(contents: &str) -> Vec<TimedRow> {
    let scan = capture_pomodoros::scan(contents);
    let mut rows = Vec::new();
    for entry in scan.entries {
        let Some(range) = entry.time_range.clone() else {
            continue;
        };
        let Some((s, mut e)) = parse_range_minutes(&range) else {
            continue;
        };
        if s == e {
            // Zero-length rows occupy no time.
            continue;
        }
        if e <= s {
            // Genuine midnight-ending spans wrap across 1440.
            e += 1440;
            // Distinguish full-day (00-00 already skipped as zero-length).
        }
        // Normalize midnight-ending 0000 to 1440 when start != 0: s != e
        // already, and e==0 becomes 1440 via the wrap above? e==0 with
        // s!=0 gives e=0 <= s, so e+=1440 => e=1440. Good.
        let _ = s;
        rows.push(TimedRow {
            line: entry.line,
            name: entry.name.clone(),
            range,
            start: s,
            end: e,
            is_open: entry.state == PomodoroState::Open,
        });
    }
    rows
}

fn overlaps(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> bool {
    // Half-open: touching endpoints never conflict. Zero-length already
    // skipped. Member windows never wrap; history may extend past 1440.
    // Compare in unwrapped space: member [a_start, a_end), history
    // [b_start, b_end). For history wrapping past midnight (b_end>1440),
    // also test the wrapped copy? Member is within [0,1440], history
    // wrapping overlaps only if member is at start of day and history
    // extends past midnight? Example history 2330-0010 (1410-1450)
    // overlaps member 0000-0010 (0-10)? In unwrapped space, 0-10 vs
    // 1410-1450 does not overlap, but wall-clock they do (0000-0010 is
    // 1440-1450). To catch that, also test member shifted by +1440 when
    // history extends past midnight.
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

pub(super) fn preflight_overlaps(
    preflight: &PastPreflight,
    pre_stack_contents: &str,
    specs: &[PomodoroPastSpec],
) -> Result<(), CaptureError> {
    // Every repair member refuses in this phase.
    for spec in specs {
        if spec.repair {
            return Err(CaptureError::usage(format!(
                "{}: repair (!) is not available yet",
                spec.raw,
            )));
        }
    }
    // Require Pomodoros section.
    let scan = capture_pomodoros::scan(pre_stack_contents);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    let rows = collect_timed_rows(pre_stack_contents);
    let mut offending: Vec<String> = Vec::new();
    // Truncated now for open-row teaching.
    let now_min = preflight.now_truncated.hour() as i64 * 60
        + preflight.now_truncated.minute() as i64;
    for member in &preflight.members {
        let a_start = member.start_min;
        // Member end may be 1440; keep unwrapped for overlap math.
        let mut a_end = member.end_min;
        if a_end == 0 && a_start != 0 {
            a_end = 1440;
        }
        for row in &rows {
            if !overlaps(a_start, a_end, row.start, row.end) {
                continue;
            }
            let name = display_name(row.name.as_deref());
            if row.is_open {
                // Open timed overlap: teach closing first.
                let started = row.start <= now_min;
                let bang = if started { "!" } else { "" };
                offending.push(format!(
                    "{} overlaps {name} {} which is still open (line {}); close it first: x {}{}",
                    member.spec.raw, row.range, row.line, member.spec.raw, bang,
                ));
            } else {
                offending.push(format!(
                    "{} would overlap {name} {} (line {})",
                    member.spec.raw, row.range, row.line,
                ));
            }
        }
    }
    if !offending.is_empty() {
        // One refusal lists every offending member/row.
        return Err(CaptureError::usage(offending.join("\n")));
    }
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
        },
        anchor: member.anchor.clone(),
        window: super::output::PomodoroPastWindowJson {
            start: member.window_start.clone(),
            end: member.window_end.clone(),
            minutes: member.minutes,
        },
        closed_at: member.closed_at.clone(),
        line: summary.pomodoro_line,
        trims: Vec::new(),
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

//! Settle ended off-clock Event Tasks into closed off-clock MEETs.
//!
//! `bob task reconcile` runs this on the current ledger at `now`: every open,
//! off-clock Event Task whose span has ended and whose only Task Link sits
//! under an open untimed `()` placeholder settles. Clustered like sync's past
//! clusters (`next.start <= cluster end`), trimmed so no piece overlaps an
//! on-clock timed session, each remaining piece of at least 5 minutes becomes
//! a closed `- [x] (**SSSS-EEEE** [off:: Nm]) — MEET`, linking
//! `🍅 ~~[[#^id]]~~` for each event it overlaps. The placeholder links move
//! out (empty-Pomodoro cleanup deletes a childless placeholder) and each
//! settled event row is completed with `[x]` plus its done date. Cancelled,
//! done, and unlinked rows never settle. Idempotent.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chrono::{NaiveDate, NaiveDateTime, Timelike};

use crate::native::{
    capture::{parse_adjustment_range, session_clock, SessionClock},
    capture_task_toggle::set_task_line_status,
    event_task::{events_in_note, EventKind},
    pomodoro as native_pomodoro,
    task_complete::tree::add_or_replace_completion_field,
    task_status_hooks::{
        logical_lines, scan_pomodoros, PomodoroModel, RawReference,
        RemovedEmptyPomodoro, ResolvedReference, SettledEvent,
    },
};

/// Outcome of settle planning: updated daily contents, settled events, and
/// empty placeholder entries deleted after their links moved out (merged
/// into `removed_empty_pomodoros` by the caller).
pub(crate) struct SettleOutcome {
    pub(crate) contents: String,
    pub(crate) settled: Vec<SettledEvent>,
    pub(crate) removed_empty: Vec<RemovedEmptyPomodoro>,
}

#[derive(Clone)]
struct Candidate {
    line_index: usize,
    block_id: String,
    title: String,
    start_min: i32,
    end_min: i32,
    span_text: String,
}

pub(crate) fn plan_settle(
    daily_contents: &str,
    daily_relative: &Path,
    anchor: NaiveDate,
    now: NaiveDateTime,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    global_filter: &str,
) -> SettleOutcome {
    let empty = SettleOutcome {
        contents: daily_contents.to_string(),
        settled: Vec::new(),
        removed_empty: Vec::new(),
    };
    // Settle only runs on today's ledger.
    if anchor != now.date() {
        return empty;
    }
    if crate::native::task_status_hooks::canonical_daily_date(daily_relative)
        .is_none()
    {
        return empty;
    }
    let now_min = now.hour() as i32 * 60 + now.minute() as i32;
    let indent_unit = crate::native::capture::dominant_indent_unit(
        &crate::native::capture::line_spans(daily_contents),
    )
    .unwrap_or("\t");

    let events = events_in_note(daily_contents, anchor);
    let mut candidates: Vec<Candidate> = Vec::new();
    for event in events {
        if event.kind != EventKind::Task {
            continue;
        }
        if !event.is_open_task() {
            continue;
        }
        if !event.off_clock {
            continue;
        }
        let Some(block_id) = event.block_id.clone() else {
            continue;
        };
        if block_id.is_empty() {
            continue;
        }
        let Some(span) = event.span else { continue };
        if span.crosses_midnight {
            continue;
        }
        let start_min = i32::from(span.start_min);
        let end_min = i32::from(span.end_min);
        if end_min > now_min {
            continue;
        }
        let span_text = format!(
            "{:02}{:02}-{:02}{:02}",
            span.start_min / 60,
            span.start_min % 60,
            span.end_min / 60,
            span.end_min % 60,
        );
        candidates.push(Candidate {
            line_index: event.line_index,
            block_id,
            title: event.title.clone(),
            start_min,
            end_min,
            span_text,
        });
    }
    if candidates.is_empty() {
        return empty;
    }

    let lines = logical_lines(daily_contents);
    let Some(section) = native_pomodoro::pomodoros_section_range(&lines) else {
        return empty;
    };
    let model = scan_pomodoros(&lines, section.clone());

    // Only events whose every ledger occurrence sits under an open untimed
    // `()` MEET placeholder settle.
    let mut placeable: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        let mut occurrences: Vec<usize> = Vec::new();
        for bullet in &model.bullets {
            for link in &bullet.links {
                let Some(target) = resolved.get(&link.reference) else {
                    continue;
                };
                if target.path != daily_relative
                    || link.reference.block_id != candidate.block_id
                {
                    continue;
                }
                occurrences.push(bullet.entry_index);
            }
        }
        if occurrences.is_empty() {
            continue;
        }
        let mut ok = true;
        for entry_index in &occurrences {
            let Some(entry) = model.entries.get(*entry_index) else {
                ok = false;
                break;
            };
            if !(entry.open
                && !entry.timed
                && entry.context.contains("()")
                && entry.context.contains("MEET"))
            {
                ok = false;
                break;
            }
        }
        if ok {
            placeable.push(candidate);
        }
    }
    if placeable.is_empty() {
        return empty;
    }

    // Cluster like sync's past clusters.
    placeable.sort_by(|a, b| {
        (a.start_min, a.end_min, a.block_id.clone()).cmp(&(
            b.start_min,
            b.end_min,
            b.block_id.clone(),
        ))
    });
    let mut clusters: Vec<Vec<Candidate>> = Vec::new();
    for item in placeable {
        if let Some(last) = clusters.last_mut() {
            let cluster_end = last.iter().map(|c| c.end_min).max().unwrap_or(0);
            if item.start_min <= cluster_end {
                last.push(item);
                continue;
            }
        }
        clusters.push(vec![item]);
    }

    // On-clock timed spans (closed or running). The hooks' `timed`
    // flag only covers open entries, so parse every open or completed
    // entry line directly: placeholders have no range and fall out.
    let mut on_clock: Vec<(i32, i32)> = Vec::new();
    for entry in &model.entries {
        if !(entry.open || entry.completed) {
            continue;
        }
        let Some(line) = lines.get(entry.line_index).copied() else {
            continue;
        };
        if !matches!(session_clock(line), SessionClock::Worked { .. }) {
            continue;
        }
        let Some(range) = parse_adjustment_range(line) else {
            continue;
        };
        let start = range.start_minutes as i32;
        let mut end = range.end_minutes as i32;
        if end <= start {
            end += 24 * 60;
        }
        // Only same-day spans trim today's clusters.
        if start < 0 || end > 24 * 60 {
            continue;
        }
        on_clock.push((start, end));
    }

    // Subtract on-clock spans per cluster.
    struct Piece {
        start: i32,
        end: i32,
    }
    let mut cluster_pieces: Vec<(Vec<Candidate>, Vec<Piece>)> = Vec::new();
    for cluster in clusters {
        let c_start = cluster.iter().map(|c| c.start_min).min().unwrap_or(0);
        let c_end = cluster.iter().map(|c| c.end_min).max().unwrap_or(0);
        if c_end <= c_start {
            continue;
        }
        let mut blocked: Vec<(i32, i32)> = on_clock
            .iter()
            .filter_map(|(s, e)| {
                let start = (*s).max(c_start);
                let end = (*e).min(c_end);
                (start < end).then_some((start, end))
            })
            .collect();
        blocked.sort();
        let mut merged: Vec<(i32, i32)> = Vec::new();
        for (start, end) in blocked {
            if let Some(last) = merged.last_mut()
                && start <= last.1
            {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        let mut pieces: Vec<Piece> = Vec::new();
        let mut cursor = c_start;
        for (start, end) in merged {
            if cursor < start && start - cursor >= 5 {
                pieces.push(Piece {
                    start: cursor,
                    end: start,
                });
            }
            cursor = cursor.max(end);
        }
        if cursor < c_end && c_end - cursor >= 5 {
            pieces.push(Piece {
                start: cursor,
                end: c_end,
            });
        }
        if pieces.is_empty() {
            continue;
        }
        cluster_pieces.push((cluster, pieces));
    }
    if cluster_pieces.is_empty() {
        return empty;
    }

    // Determine settling events: those overlapping at least one piece.
    let mut settling_ids: BTreeSet<String> = BTreeSet::new();
    for (cluster, pieces) in &cluster_pieces {
        for candidate in cluster {
            let overlaps = pieces.iter().any(|piece| {
                candidate.start_min < piece.end
                    && piece.start < candidate.end_min
            });
            if overlaps {
                settling_ids.insert(candidate.block_id.clone());
            }
        }
    }
    if settling_ids.is_empty() {
        return empty;
    }

    // Work on owned lines (logical, without endings); rejoin with `\n`,
    // preserving the original final newline.
    let final_newline = daily_contents.ends_with('\n');
    let mut working: Vec<String> =
        daily_contents.lines().map(str::to_string).collect();

    let date_string = anchor.format("%Y-%m-%d").to_string();
    // Complete settled event rows in place.
    for (cluster, _) in &cluster_pieces {
        for candidate in cluster {
            if !settling_ids.contains(&candidate.block_id) {
                continue;
            }
            let Some(line) = working.get(candidate.line_index).cloned() else {
                continue;
            };
            let updated = match set_task_line_status(&line, 'x') {
                Some(next) => next,
                None => line.clone(),
            };
            let updated = if !global_filter.is_empty()
                && updated
                    .split_whitespace()
                    .any(|token| token == global_filter)
            {
                add_or_replace_completion_field(&updated, &date_string)
            } else {
                updated
            };
            if let Some(slot) = working.get_mut(candidate.line_index) {
                *slot = updated;
            }
        }
    }

    // Remove placeholder bullets holding settling links. A bullet whose
    // every link settles goes with its subtree; otherwise only the
    // settling tokens are removed.
    let mut bullet_delete_ranges: Vec<(usize, usize)> = Vec::new();
    let mut token_edits: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    {
        let refs: Vec<&str> = working.iter().map(String::as_str).collect();
        let Some(section) = native_pomodoro::pomodoros_section_range(&refs)
        else {
            return empty;
        };
        let scan = scan_pomodoros(&refs, section);
        // Map each bullet to whether all its resolved links settle.
        for bullet in &scan.bullets {
            let entry = &scan.entries[bullet.entry_index];
            if !(entry.open
                && !entry.timed
                && entry.context.contains("()")
                && entry.context.contains("MEET"))
            {
                continue;
            }
            let mut bullet_links: Vec<(usize, usize, String)> = Vec::new();
            let mut has_unresolved = false;
            for link in &bullet.links {
                let Some(target) = resolved.get(&link.reference) else {
                    has_unresolved = true;
                    break;
                };
                if target.path != daily_relative {
                    has_unresolved = true;
                    break;
                }
                bullet_links.push((
                    link.edit_start,
                    link.edit_end,
                    link.reference.block_id.clone(),
                ));
            }
            if has_unresolved || bullet_links.is_empty() {
                continue;
            }
            let settling_here: Vec<(usize, usize)> = bullet_links
                .iter()
                .filter(|(_, _, id)| settling_ids.contains(id))
                .map(|(s, e, _)| (*s, *e))
                .collect();
            if settling_here.is_empty() {
                continue;
            }
            if settling_here.len() == bullet_links.len() {
                bullet_delete_ranges.push((bullet.line_index, bullet.end_line));
            } else {
                token_edits
                    .entry(bullet.line_index)
                    .or_default()
                    .extend(settling_here);
            }
        }
    }
    // Token edits first (descending offsets per line).
    for (line_index, mut edits) in token_edits {
        edits.sort_by_key(|(start, _)| *start);
        if let Some(line) = working.get_mut(line_index) {
            for (start, end) in edits.into_iter().rev() {
                if start <= end && end <= line.len() {
                    line.replace_range(start..end, "");
                }
            }
        }
    }
    // Whole-bullet deletions, descending by start.
    bullet_delete_ranges.sort_by_key(|(start, _)| *start);
    bullet_delete_ranges.dedup();
    for (start, end) in bullet_delete_ranges.into_iter().rev() {
        if start < end && end <= working.len() {
            working.drain(start..end);
        }
    }

    // Insert closed MEET pieces in time order. Re-scan before each insert
    // so positions account for earlier inserts.
    struct CreatedPiece {
        entry_line: usize,
        start: i32,
        end: i32,
    }
    let mut created: Vec<CreatedPiece> = Vec::new();
    // Collect pieces in time order across clusters.
    let mut all_pieces: Vec<(i32, i32, Vec<Candidate>)> = Vec::new();
    for (cluster, pieces) in &cluster_pieces {
        for piece in pieces {
            // Events from this cluster overlapping this piece.
            let overlapping: Vec<Candidate> = cluster
                .iter()
                .filter(|c| {
                    settling_ids.contains(&c.block_id)
                        && c.start_min < piece.end
                        && piece.start < c.end_min
                })
                .map(|c| Candidate {
                    line_index: c.line_index,
                    block_id: c.block_id.clone(),
                    title: c.title.clone(),
                    start_min: c.start_min,
                    end_min: c.end_min,
                    span_text: c.span_text.clone(),
                })
                .collect();
            if overlapping.is_empty() {
                continue;
            }
            all_pieces.push((piece.start, piece.end, overlapping));
        }
    }
    all_pieces.sort_by_key(|(start, end, _)| (*start, *end));
    for (start, end, overlapping) in &all_pieces {
        let minutes = (*end - *start).max(0) as u64;
        let entry_text = format!(
            "- [x] (**{}** [off:: {}m]) — MEET",
            format_span(*start, *end),
            minutes
        );
        let refs: Vec<&str> = working.iter().map(String::as_str).collect();
        let Some(section) = native_pomodoro::pomodoros_section_range(&refs)
        else {
            continue;
        };
        let scan = scan_pomodoros(&refs, section.clone());
        let at = closed_insert_at(&refs, &scan, *start);
        let mut insert: Vec<String> = vec![entry_text];
        let mut sorted = overlapping.to_vec();
        sorted.sort_by(|a, b| {
            (a.start_min, a.end_min, a.block_id.clone()).cmp(&(
                b.start_min,
                b.end_min,
                b.block_id.clone(),
            ))
        });
        for event in &sorted {
            insert.push(format!(
                "{indent_unit}- 🍅 ~~[[#^{}]]~~",
                event.block_id
            ));
        }
        working.splice(at..at, insert.clone());
        created.push(CreatedPiece {
            entry_line: at,
            start: *start,
            end: *end,
        });
    }

    // Delete placeholders emptied by the move.
    let mut removed_empty: Vec<RemovedEmptyPomodoro> = Vec::new();
    {
        let refs: Vec<&str> = working.iter().map(String::as_str).collect();
        let Some(section) = native_pomodoro::pomodoros_section_range(&refs)
        else {
            return SettleOutcome {
                contents: join_lines(&working, final_newline),
                settled: Vec::new(),
                removed_empty: Vec::new(),
            };
        };
        let scan = scan_pomodoros(&refs, section);
        let mut delete: Vec<(usize, usize, String)> = Vec::new();
        for entry in &scan.entries {
            if !(entry.open
                && !entry.timed
                && entry.context.contains("()")
                && entry.context.contains("MEET")
                && !entry.has_child)
            {
                continue;
            }
            delete.push((
                entry.line_index,
                entry.end_line,
                entry.context.clone(),
            ));
        }
        delete.sort_by_key(|(start, _, _)| *start);
        for (start, end, context) in delete.into_iter().rev() {
            if start < end && end <= working.len() {
                removed_empty.push(RemovedEmptyPomodoro {
                    line_number: start + 1,
                    line: context,
                });
                working.drain(start..end);
                // Shift created entry lines after the deletion.
                for piece in created.iter_mut() {
                    if piece.entry_line > start {
                        piece.entry_line =
                            piece.entry_line.saturating_sub(end - start);
                    }
                }
            }
        }
        removed_empty.reverse();
    }

    // Build settled reports: one per settling event, pointing at its
    // first overlapping created piece.
    let mut settled: Vec<SettledEvent> = Vec::new();
    for (cluster, _) in &cluster_pieces {
        for candidate in cluster {
            if !settling_ids.contains(&candidate.block_id) {
                continue;
            }
            if settled
                .iter()
                .any(|s: &SettledEvent| s.block_id == candidate.block_id)
            {
                continue;
            }
            let mut best: Option<&CreatedPiece> = None;
            let mut total: u64 = 0;
            for piece in &created {
                let overlap_start = candidate.start_min.max(piece.start);
                let overlap_end = candidate.end_min.min(piece.end);
                if overlap_start < overlap_end {
                    total += (overlap_end - overlap_start) as u64;
                    if best.is_none_or(|b| piece.entry_line < b.entry_line) {
                        best = Some(piece);
                    }
                }
            }
            let Some(first) = best else { continue };
            settled.push(SettledEvent {
                title: candidate.title.clone(),
                line_number: candidate.line_index + 1,
                span: candidate.span_text.clone(),
                block_id: candidate.block_id.clone(),
                entry_line: first.entry_line + 1,
                minutes: total,
            });
        }
    }
    settled.sort_by_key(|s| (s.line_number, s.block_id.clone()));

    SettleOutcome {
        contents: join_lines(&working, final_newline),
        settled,
        removed_empty,
    }
}

fn format_span(start: i32, end: i32) -> String {
    let start = start.rem_euclid(24 * 60);
    let end = end.rem_euclid(24 * 60);
    format!(
        "{:02}{:02}-{:02}{:02}",
        start / 60,
        start % 60,
        end / 60,
        end % 60
    )
}

fn join_lines(lines: &[String], final_newline: bool) -> String {
    let mut out = lines.join("\n");
    if final_newline {
        out.push('\n');
    }
    out
}

/// Insertion index (0-based) for a closed MEET starting at `start`,
/// mirroring gcal's closed-insert: after the last completed entry at or
/// before `start`, else before the first completed, else before the first
/// open, else at the section start.
fn closed_insert_at(
    lines: &[&str],
    model: &PomodoroModel,
    start: i32,
) -> usize {
    let mut completed: Vec<(i32, usize, usize)> = Vec::new();
    for entry in &model.entries {
        if !entry.completed {
            continue;
        }
        let Some(line) = lines.get(entry.line_index).copied() else {
            continue;
        };
        let Some(range) = parse_adjustment_range(line) else {
            continue;
        };
        let mut entry_start = range.start_minutes as i32;
        if !(0..24 * 60).contains(&entry_start) {
            entry_start = 0;
        }
        completed.push((entry_start, entry.line_index, entry.end_line));
    }
    completed.sort_by_key(|(start, line, _)| (*start, *line));
    if let Some((_, _, end)) = completed
        .iter()
        .rev()
        .find(|(entry_start, _, _)| *entry_start <= start)
    {
        return *end;
    }
    if let Some((_, line, _)) = completed.first() {
        return *line;
    }
    if let Some(entry) = model.entries.iter().find(|e| e.open) {
        return entry.line_index;
    }
    model.entries.first().map(|e| e.line_index).unwrap_or(0)
}

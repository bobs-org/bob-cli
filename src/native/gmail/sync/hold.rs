//! Pure Gmail reply hold/lift line operations with the running-session guard.
//!
//! `line_rules` owns the H1–H17 vectors from the watch plan: an outgoing
//! send holds its open tasks with `#awaiting_reply` plus a nudge
//! `[scheduled::]`, and an incoming move lifts that hold and resurfaces
//! eligible Ready rows through the existing freshness writer. File I/O
//! stays out of the pure transforms; status-validation and freshness-scope
//! inputs travel explicitly, and the guard helper only reads today's
//! ledger and resolves task identities.

// bob-cli-ii.4.1.2+ consumes the watch planning APIs; allow dead code until
// the transaction and polling phases land their callers.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::native::{
    capture_pomodoro_close::{
        find_running_pomodoro, number_task_links, TaskLinkMarker,
    },
    env as bob_env,
    freshness::placement::{mark_unblocked, UnblockScope},
    pomodoro, task_fields,
    task_status_hooks::{
        canonical_daily_date, read_tasks_settings, validate_blocked_status,
    },
    vault_links::VaultLinkResolver,
};

/// Exact hold tag. Near-misses such as `#awaiting_reply_old` never count.
pub(crate) const AWAITING_TAG: &str = "#awaiting_reply";
/// Machine-owned project-block tag that the hold tag follows.
pub(crate) const PRJ_TAG: &str = "#prj_is_blocked";
/// Watching-task tag that must be present for a hold to apply.
pub(crate) const TASK_TAG: &str = "#task";

/// Warning when a hold is refused on a recurring row.
pub(crate) const REFUSAL_RECURRING: &str = "recurring_watch";
/// Warning when a hold is refused on duplicate `scheduled` fields.
pub(crate) const REFUSAL_DUPLICATE_SCHEDULED: &str = "duplicate_scheduled";

/// Open boxes a hold may touch. Closed rows (`x`, `X`, `-`) never change.
const HOLDABLE_STATUSES: [char; 4] = [' ', '*', '/', '?'];

/// Nudge date for a send: the send's local calendar date plus
/// `reply_wait_days` calendar days.
pub(crate) fn nudge_date(
    send_date: NaiveDate,
    reply_wait_days: u32,
) -> NaiveDate {
    send_date + chrono::Days::new(u64::from(reply_wait_days))
}

/// Local calendar date of a Gmail send in Bob's day zone.
pub(crate) fn send_local_date(
    send_at: DateTime<Utc>,
    timezone: Tz,
) -> NaiveDate {
    send_at.with_timezone(&timezone).date_naive()
}

/// Convenience: local send date plus the wait, in one step.
pub(crate) fn nudge_date_for_send(
    send_at: DateTime<Utc>,
    timezone: Tz,
    reply_wait_days: u32,
) -> NaiveDate {
    nudge_date(send_local_date(send_at, timezone), reply_wait_days)
}

/// Inputs for [`apply_hold`]. `nudge_date` is already the send-local date
/// plus `reply_wait_days`; `blocked_ok` carries the Tasks Blocked-status
/// validation so the pure transform does no file I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoldRequest {
    pub(crate) nudge_date: NaiveDate,
    pub(crate) today: NaiveDate,
    pub(crate) blocked_ok: bool,
    pub(crate) blocked_reason: Option<String>,
}

/// Result of [`apply_hold`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoldOutcome {
    pub(crate) line: String,
    pub(crate) changed: bool,
    /// Tag and date are correct after this call (including already-held).
    pub(crate) held: bool,
    /// Refusal code (`recurring_watch`, `duplicate_scheduled`) when the
    /// line was left unchanged with a warning.
    pub(crate) refused: Option<String>,
    /// Whether the checkbox was flipped to `[?]` by this call.
    pub(crate) checkbox_held: bool,
    /// Why a future-date checkbox was left alone despite `blocked_ok`
    /// being false.
    pub(crate) checkbox_skipped: Option<String>,
}

/// Inputs for [`apply_lift`]. `scope` is the caller's freshness scope for
/// the row; the hold date is removed before the writer runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiftRequest<'a> {
    pub(crate) today: NaiveDate,
    pub(crate) scope: &'a UnblockScope,
}

/// Result of [`apply_lift`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiftOutcome {
    pub(crate) line: String,
    pub(crate) changed: bool,
    /// An exact awaiting token was present and removed.
    pub(crate) lifted: bool,
    /// `mark_unblocked` wrote `[unblocked:: today]`.
    pub(crate) unblocked_written: bool,
}

/// Apply an outgoing-send hold to one task line.
///
/// 1. Insert the exact `#awaiting_reply` after `#task` (or after
///    `#prj_is_blocked` when present) unless already there.
/// 2. Replace the sole `[scheduled::]` or insert the nudge date.
/// 3. When the nudge is after today and the box is `[ ]`, `[*]`, or
///    `[/]`, flip it to `[?]` once the Blocked status validates.
///
/// Refusals (a `repeat` field or more than one `scheduled` field) leave
/// the line unchanged with a warning. Closed rows, non-task lines, and
/// lines without `#task` are unchanged without a warning. Freshness
/// fields are never touched.
pub(crate) fn apply_hold(line: &str, req: &HoldRequest) -> HoldOutcome {
    let (body, cr) = split_cr(line);
    let original = body.to_string();
    let unchanged = |held: bool| HoldOutcome {
        line: line.to_string(),
        changed: false,
        held,
        refused: None,
        checkbox_held: false,
        checkbox_skipped: None,
    };

    let Some((status_at, status)) = checkbox_at(body) else {
        return unchanged(false);
    };
    if !HOLDABLE_STATUSES.contains(&status) {
        return unchanged(false);
    }
    if !has_exact_tag(body, TASK_TAG) {
        return unchanged(false);
    }
    if task_fields::has_any_field(body, &["repeat"]) || body.contains('🔁') {
        return HoldOutcome {
            line: line.to_string(),
            changed: false,
            held: false,
            refused: Some(REFUSAL_RECURRING.to_string()),
            checkbox_held: false,
            checkbox_skipped: None,
        };
    }
    let scheduled = task_fields::inline_fields(body, "scheduled");
    if scheduled.len() > 1 {
        return HoldOutcome {
            line: line.to_string(),
            changed: false,
            held: false,
            refused: Some(REFUSAL_DUPLICATE_SCHEDULED.to_string()),
            checkbox_held: false,
            checkbox_skipped: None,
        };
    }

    let already_held = has_exact_tag(body, AWAITING_TAG);
    let mut working = original.clone();

    if !already_held {
        let Some(anchor_end) = hold_anchor_end(&working) else {
            return unchanged(false);
        };
        working = format!(
            "{} {AWAITING_TAG}{}",
            &working[..anchor_end],
            &working[anchor_end..]
        );
    }

    let date_text = task_fields::format_calendar_date(req.nudge_date);
    // Re-resolve after the tag insert: the earlier spans index the
    // pre-insert bytes and Unicode text shifts every later offset.
    let scheduled_now = task_fields::inline_fields(&working, "scheduled");
    if let Some(field) = scheduled_now.first() {
        let current = field.value.trim();
        // Replace even a malformed value with the canonical nudge; byte
        // order is preserved because the replacement happens in place.
        let canonical = format!("[scheduled:: {date_text}]");
        let span_text = working[field.start..field.end].to_string();
        if span_text != canonical || current != date_text {
            let mut rebuilt = String::with_capacity(working.len() + 16);
            rebuilt.push_str(&working[..field.start]);
            rebuilt.push_str(&canonical);
            rebuilt.push_str(&working[field.end..]);
            working = rebuilt;
        }
    } else {
        working = insert_scheduled(&working, &date_text);
    }

    let mut checkbox_held = false;
    let mut checkbox_skipped = None;
    if req.nudge_date > req.today
        && matches!(status, ' ' | '*' | '/')
        && let Some((at, _)) = checkbox_at(&working)
    {
        // `status` is the pre-edit box; the working line still carries it
        // because tag/date edits never touch the checkbox.
        debug_assert_eq!(at, status_at);
        if req.blocked_ok {
            working = set_checkbox_at(&working, at, '?');
            checkbox_held = true;
        } else {
            checkbox_skipped =
                Some(req.blocked_reason.clone().unwrap_or_else(|| {
                    "Tasks Blocked status is not a single compatible '?' \
                     status; leaving the checkbox unchanged"
                        .to_string()
                }));
        }
    }

    if working == original {
        let mut outcome = unchanged(already_held);
        // An already-correct hold still counts as held even when nothing
        // changed, so polling does not reassert Bryan's edits.
        if already_held {
            let scheduled_ok =
                task_fields::inline_fields(&working, "scheduled")
                    .first()
                    .is_some_and(|field| field.value.trim() == date_text);
            if scheduled_ok {
                outcome.held = true;
            }
        }
        return outcome;
    }
    HoldOutcome {
        line: if cr { format!("{working}\r") } else { working },
        changed: true,
        held: true,
        refused: None,
        checkbox_held,
        checkbox_skipped,
    }
}

/// Apply an incoming-move (or bounce) lift to one task line.
///
/// 1. Remove the exact awaiting token; only then remove `[scheduled::]`.
/// 2. Recover `[?]` to `[ ]` only with no `dependsOn` field, no exact
///    `#prj_is_blocked`, and no remaining future schedule.
/// 3. When the row is Ready, run the existing `mark_unblocked` writer
///    with the caller's scope. Its WRITE skips stay: NEW rows, out-of-
///    scope rows, scheduled ticklers, and lane landings write nothing.
///
/// Unheld Next/Pending rows are unchanged. Closed rows never change.
pub(crate) fn apply_lift(line: &str, req: &LiftRequest<'_>) -> LiftOutcome {
    let (body, cr) = split_cr(line);
    let original = body.to_string();
    let unchanged = |lifted: bool| LiftOutcome {
        line: line.to_string(),
        changed: false,
        lifted,
        unblocked_written: false,
    };

    let Some((_, status)) = checkbox_at(body) else {
        return unchanged(false);
    };
    if matches!(status, 'x' | 'X' | '-') {
        return unchanged(false);
    }

    let held = has_exact_tag(body, AWAITING_TAG);
    let mut working = original.clone();
    if held {
        working = remove_exact_token_all(&working, AWAITING_TAG);
        working = remove_all_scheduled(&working);
    }

    // Recover `[?]` only when no other blocker remains. Reconcile owns
    // whatever is left and writes its own `[unblocked::]` on recovery.
    if status == '?' && held {
        let has_deps =
            !task_fields::inline_fields(&working, "dependsOn").is_empty();
        let has_prj = has_exact_tag(&working, PRJ_TAG);
        let has_future = has_future_scheduled(&working, req.today);
        if !has_deps
            && !has_prj
            && !has_future
            && let Some((at, _)) = checkbox_at(&working)
        {
            working = set_checkbox_at(&working, at, ' ');
        }
    }

    let Some((_, after_status)) = checkbox_at(&working) else {
        let changed = working != original;
        return LiftOutcome {
            line: if changed && cr {
                format!("{working}\r")
            } else if changed {
                working
            } else {
                line.to_string()
            },
            changed,
            lifted: held,
            unblocked_written: false,
        };
    };

    // Lane rows never resurface here; their walk covers them.
    if matches!(after_status, '*' | '/' | '?') {
        let changed = working != original;
        return LiftOutcome {
            line: if changed && cr {
                format!("{working}\r")
            } else if changed {
                working
            } else {
                line.to_string()
            },
            changed,
            lifted: held,
            unblocked_written: false,
        };
    }

    // Ready rows (just flipped or already Ready) go through the shared
    // writer, which enforces NEW, scope, tickler, and lane skips. The
    // hold date is already gone, so a nudge-due row still qualifies.
    if let Some(marked) =
        mark_unblocked(&working, req.today, after_status, req.scope)
    {
        let out = if cr { format!("{marked}\r") } else { marked };
        return LiftOutcome {
            changed: out != line,
            line: out,
            lifted: held,
            unblocked_written: true,
        };
    }

    let changed = working != original;
    LiftOutcome {
        line: if changed && cr {
            format!("{working}\r")
        } else if changed {
            working
        } else {
            line.to_string()
        },
        changed,
        lifted: held,
        unblocked_written: false,
    }
}

/// Freshness scope for one lift row, mirroring the project-close helper:
/// recurring rows, hidden rows, canonical daily notes, and Today-linked
/// rows never take `[unblocked::]`.
pub(crate) fn unblock_scope_for(
    relative_path: &Path,
    line: &str,
    is_today: bool,
) -> UnblockScope {
    UnblockScope {
        recurring: task_fields::has_any_field(line, &["repeat"])
            || line.contains('🔁'),
        hidden: has_hide_token(line),
        is_daily_note: is_canonical_daily_note(relative_path),
        is_today,
    }
}

/// Validate the Tasks Blocked status for holds: the checkbox may flip to
/// `[?]` only when exactly one compatible definition exists. Returns
/// `(true, None)` on success and `(false, Some(reason))` otherwise.
pub(crate) fn blocked_status_for_hold(vault: &Path) -> (bool, Option<String>) {
    let settings = read_tasks_settings(vault);
    match validate_blocked_status(&settings) {
        Ok(()) => (true, None),
        Err(error) => (false, Some(error.message().to_string())),
    }
}

/// Live Task Links under today's open timed Pomodoro, keyed by resolved
/// task identity.
///
/// Only plain (worked) links count: planned `()` placeholders carry no
/// link, closed entries never run, struck and dropped links are already
/// excluded from the numbering, and mentions outside the running entry's
/// sub-bullets never enter its range. Identities match by resolved vault
/// path plus block id, never by link label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunningLookup {
    /// No open timed session today: no hold defers.
    NoSession,
    /// Resolved `(slash path, block id)` identities under the session.
    Session(BTreeSet<(String, String)>),
    /// The ledger is ambiguous (several running sessions) or unreadable:
    /// callers must report and defer affected holds rather than guess.
    Ambiguous(String),
}

/// Read today's day file and collect the running session's live links.
/// Never writes; never consumes ledger links (reconcile does that later).
pub(crate) fn load_running_tasks(bob_dir: &Path) -> RunningLookup {
    let day_path = pomodoro::day_file_for(bob_dir);
    let contents = match fs::read_to_string(&day_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RunningLookup::NoSession;
        }
        Err(error) => {
            return RunningLookup::Ambiguous(format!(
                "cannot read today's ledger {}: {error}",
                day_path.display()
            ));
        }
    };
    let running = match find_running_pomodoro(&contents) {
        Ok(running) => running,
        Err(
            crate::native::capture_pomodoro_close::FindRunningError::Multiple(
                many,
            ),
        ) => {
            return RunningLookup::Ambiguous(format!(
                "today's ledger has {} running timed sessions; \
                 deferring holds rather than guessing",
                many.len()
            ));
        }
        Err(_) => return RunningLookup::NoSession,
    };
    let links = number_task_links(&contents, &running);
    let resolver = VaultLinkResolver::new(bob_dir);
    let mut keys = BTreeSet::new();
    for link in links {
        if link.marker != TaskLinkMarker::Plain {
            continue;
        }
        if link.block_id.is_empty() || link.path_part.is_empty() {
            continue;
        }
        let resolved = resolver.resolve(&day_path, &link.path_part);
        let Some(path) = resolved.path() else {
            continue;
        };
        // The resolver returns vault-relative paths for found notes;
        // absolutize-then-strip so absolute hits also key correctly.
        let relative = path
            .strip_prefix(bob_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf());
        keys.insert((slashes(&relative), link.block_id.clone()));
    }
    RunningLookup::Session(keys)
}

/// Whether one watching task is live under the running session. Tasks
/// without a block id can never be a link target, so they never defer.
pub(crate) fn is_running_task(
    lookup: &RunningLookup,
    task_path: &str,
    block_id: Option<&str>,
) -> bool {
    match lookup {
        RunningLookup::Session(keys) => {
            let Some(block_id) = block_id else {
                return false;
            };
            if block_id.is_empty() {
                return false;
            }
            keys.contains(&(task_path.to_string(), block_id.to_string()))
        }
        RunningLookup::NoSession => false,
        // Ambiguous ledgers defer every hold; callers check the variant
        // directly, but a bare membership probe must not claim idle.
        RunningLookup::Ambiguous(_) => true,
    }
}

fn split_cr(line: &str) -> (&str, bool) {
    match line.strip_suffix('\r') {
        Some(body) => (body, true),
        None => (line, false),
    }
}

fn has_exact_tag(line: &str, tag: &str) -> bool {
    line.split_whitespace().any(|token| token == tag)
}

fn has_hide_token(line: &str) -> bool {
    line.split_whitespace().any(|token| token == "#hide")
}

fn is_canonical_daily_note(relative_path: &Path) -> bool {
    canonical_daily_date(relative_path).is_some()
}

/// Byte end of the hold-tag anchor: after `#prj_is_blocked` when that
/// exact token is present, else after `#task`.
fn hold_anchor_end(line: &str) -> Option<usize> {
    if let Some(end) = exact_token_end(line, PRJ_TAG) {
        return Some(end);
    }
    exact_token_end(line, TASK_TAG)
}

fn exact_token_end(line: &str, tag: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(relative) = line[search..].find(tag) {
        let start = search + relative;
        let end = start + tag.len();
        let before_ok = start == 0
            || line[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        let after_ok = end == line.len()
            || line[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace());
        if before_ok && after_ok {
            return Some(end);
        }
        search = start + 1;
    }
    None
}

fn insert_scheduled(line: &str, date_text: &str) -> String {
    let insertion = insertion_before_block_id(line);
    let before = line[..insertion].trim_end();
    let after = &line[insertion..];
    format!("{before} [scheduled:: {date_text}]{after}")
}

fn insertion_before_block_id(line: &str) -> usize {
    let trimmed_len = line.trim_end().len();
    let trimmed = &line[..trimmed_len];
    if let Some(caret) = trimmed.rfind('^') {
        let id_part = &trimmed[caret + 1..];
        if !id_part.is_empty()
            && id_part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && (caret == 0
                || trimmed[..caret]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace))
        {
            return trimmed[..caret].trim_end().len();
        }
    }
    trimmed_len
}

fn remove_exact_token_all(line: &str, tag: &str) -> String {
    let mut spans = Vec::new();
    let mut search = 0;
    while let Some(relative) = line[search..].find(tag) {
        let start = search + relative;
        let end = start + tag.len();
        let before_ok = start == 0
            || line[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        let after_ok = end == line.len()
            || line[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace());
        if before_ok && after_ok {
            spans.push((start, end));
        }
        search = start + 1;
    }
    if spans.is_empty() {
        return line.to_string();
    }
    remove_spans_collapsing_whitespace(line, &spans)
}

fn remove_all_scheduled(line: &str) -> String {
    let spans: Vec<(usize, usize)> =
        task_fields::inline_fields(line, "scheduled")
            .iter()
            .map(|field| (field.start, field.end))
            .collect();
    if spans.is_empty() {
        return line.to_string();
    }
    remove_spans_collapsing_whitespace(line, &spans)
}

fn remove_spans_collapsing_whitespace(
    line: &str,
    spans: &[(usize, usize)],
) -> String {
    let mut sorted = spans.to_vec();
    sorted.sort();
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in sorted {
        let before = &line[cursor..start];
        output.push_str(before.trim_end_matches([' ', '\t']));
        let mut next = end;
        while line[next..]
            .chars()
            .next()
            .is_some_and(|c| c == ' ' || c == '\t')
        {
            next += 1;
        }
        if !output.is_empty() && next < line.len() {
            output.push(' ');
        }
        cursor = next;
    }
    output.push_str(&line[cursor..]);
    if line[cursor..].chars().all(|c| c == ' ' || c == '\t') {
        while output.ends_with([' ', '\t']) {
            output.pop();
        }
    }
    output
}

fn has_future_scheduled(line: &str, today: NaiveDate) -> bool {
    task_fields::inline_fields(line, "scheduled")
        .iter()
        .any(|field| {
            task_fields::parse_strict_calendar_date(field.value.trim())
                .is_some_and(|date| date > today)
        })
}

fn slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Byte offset of the checkbox status character plus the status itself.
fn checkbox_at(line: &str) -> Option<(usize, char)> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }
    // Drop blockquote prefixes the way the vault scanner does: up to
    // three spaces, `>`, one optional space, repeated.
    loop {
        let mut tmp = index;
        let mut spaces = 0;
        while bytes.get(tmp) == Some(&b' ') && spaces < 4 {
            tmp += 1;
            spaces += 1;
        }
        if bytes.get(tmp) != Some(&b'>') {
            break;
        }
        tmp += 1;
        if bytes.get(tmp) == Some(&b' ') {
            tmp += 1;
        }
        index = tmp;
    }
    index = skip_list_marker(line, index)?;
    while bytes.get(index).is_some_and(|b| b.is_ascii_whitespace()) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let status = line[index + 1..].chars().next()?;
    let status_len = status.len_utf8();
    if !line[index + 1 + status_len..].starts_with(']') {
        return None;
    }
    let after = &line[index + 1 + status_len + 1..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    Some((index + 1, status))
}

fn skip_list_marker(line: &str, index: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let first = *bytes.get(index)?;
    if matches!(first, b'-' | b'*' | b'+') {
        return Some(index + 1);
    }
    if first.is_ascii_digit() {
        let mut end = index;
        while bytes.get(end).is_some_and(|b| b.is_ascii_digit()) {
            end += 1;
        }
        if matches!(bytes.get(end), Some(b'.') | Some(b')')) {
            return Some(end + 1);
        }
    }
    None
}

fn set_checkbox_at(line: &str, status_at: usize, to: char) -> String {
    let mut output = line.to_string();
    output.replace_range(status_at..status_at + 1, &to.to_string());
    output
}

#[allow(dead_code)]
pub(crate) fn configured_nudge_for_send(
    send_at: DateTime<Utc>,
    reply_wait_days: u32,
) -> NaiveDate {
    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    nudge_date_for_send(send_at, timezone, reply_wait_days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 7).expect("date")
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("date")
    }

    fn hold_req(nudge: NaiveDate) -> HoldRequest {
        HoldRequest {
            nudge_date: nudge,
            today: day(),
            blocked_ok: true,
            blocked_reason: None,
        }
    }

    fn hold_req_blocked_bad(nudge: NaiveDate, reason: &str) -> HoldRequest {
        HoldRequest {
            nudge_date: nudge,
            today: day(),
            blocked_ok: false,
            blocked_reason: Some(reason.to_string()),
        }
    }

    fn scope() -> UnblockScope {
        UnblockScope {
            recurring: false,
            hidden: false,
            is_daily_note: false,
            is_today: false,
        }
    }

    fn lift(line: &str, today: NaiveDate, scope: &UnblockScope) -> LiftOutcome {
        apply_lift(line, &LiftRequest { today, scope })
    }

    // H1: a new outgoing send holds a Ready row with a future nudge.
    #[test]
    fn h1_ready_hold_sets_tag_date_and_blocked_box() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [created:: 2026-11-07]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(out.changed, "hold must write");
        assert!(out.held);
        assert!(out.checkbox_held);
        assert!(out.refused.is_none());
        assert!(
            out.line.contains("#task #awaiting_reply"),
            "tag follows #task: {}",
            out.line
        );
        assert!(
            out.line.contains("[scheduled:: 2026-11-10]"),
            "nudge date: {}",
            out.line
        );
        assert!(out.line.starts_with("- [?]"), "box flips: {}", out.line);
        assert!(
            !out.line.contains("[unblocked::"),
            "hold never writes unblocked: {}",
            out.line
        );
    }

    // H2: Next holds the same way and freshness stays put.
    #[test]
    fn h2_next_hold_keeps_fresh_without_unblocked() {
        let before = "- [*] #task [[m|✉️ L · Ada]] [fresh:: 2026-11-05] \
                      [created:: 2026-11-07]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(out.changed);
        assert!(out.line.starts_with("- [?]"), "{}", out.line);
        assert!(out.line.contains("[fresh:: 2026-11-05]"), "{}", out.line);
        assert!(!out.line.contains("[unblocked::"), "{}", out.line);
    }

    // H4: a new nudge replaces Bryan's manual date.
    #[test]
    fn h4_new_nudge_replaces_manual_date() {
        let before = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [created:: 2026-11-07] [scheduled:: 2026-11-20]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 15)));
        assert!(out.changed);
        assert!(
            out.line.contains("[scheduled:: 2026-11-15]"),
            "{}",
            out.line
        );
        assert!(!out.line.contains("2026-11-20"), "{}", out.line);
        assert!(out.line.starts_with("- [?]"), "{}", out.line);
    }

    // H3/H6 idea: an already-correct hold is byte-identical (no churn), so
    // manual edits between messages stand until the next message.
    #[test]
    fn h3_correct_hold_is_byte_identical() {
        let line = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                    [created:: 2026-11-07] [scheduled:: 2026-11-10]";
        let out = apply_hold(line, &hold_req(date(2026, 11, 10)));
        assert!(!out.changed, "must not churn: {}", out.line);
        assert_eq!(out.line, line);
        assert!(out.held);
    }

    // H5: an old send is already due, so the box stays open.
    #[test]
    fn h5_old_send_holds_without_blocking() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [created:: 2026-11-01]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 4)));
        assert!(out.changed);
        assert!(
            out.line.contains("[scheduled:: 2026-11-04]"),
            "{}",
            out.line
        );
        assert!(out.line.starts_with("- [ ]"), "due keeps box: {}", out.line);
        assert!(!out.checkbox_held);
    }

    // H7: a reply lifts a held row and resurfaces it.
    #[test]
    fn h7_reply_lifts_and_marks_unblocked() {
        let before = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [fresh:: 2026-11-05] [created:: 2026-11-07] \
                      [scheduled:: 2026-11-10]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.changed);
        assert!(out.lifted);
        assert!(out.unblocked_written, "{}", out.line);
        assert!(!out.line.contains("#awaiting_reply"), "{}", out.line);
        assert!(!out.line.contains("[scheduled::"), "{}", out.line);
        assert!(out.line.starts_with("- [ ] #task"), "{}", out.line);
        assert!(out.line.contains("[fresh:: 2026-11-05]"), "{}", out.line);
        assert!(
            out.line.contains("[unblocked:: 2026-11-08]"),
            "{}",
            out.line
        );
    }

    // H8: an overdue nudge still resurfaces because the hold date is
    // removed before the writer runs.
    #[test]
    fn h8_overdue_nudge_lifts_with_unblocked() {
        let before = "- [ ] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [fresh:: 2026-11-05] [created:: 2026-11-07] \
                      [scheduled:: 2026-11-10]";
        let out = lift(before, date(2026, 11, 11), &scope());
        assert!(out.changed);
        assert!(out.lifted);
        assert!(out.unblocked_written, "{}", out.line);
        assert!(
            out.line.contains("[unblocked:: 2026-11-11]"),
            "{}",
            out.line
        );
        assert!(!out.line.contains("[scheduled::"), "{}", out.line);
    }

    // H9: other blockers keep `[?]` and take no `[unblocked::]`.
    #[test]
    fn h9_depends_on_keeps_blocked_without_unblocked() {
        let before = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [fresh:: 2026-11-05] [created:: 2026-11-07] \
                      [scheduled:: 2026-11-10] [dependsOn:: abc123]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.changed, "tag/date still lift");
        assert!(out.lifted);
        assert!(!out.unblocked_written, "{}", out.line);
        assert!(out.line.starts_with("- [?]"), "{}", out.line);
        assert!(!out.line.contains("#awaiting_reply"), "{}", out.line);
        assert!(!out.line.contains("[scheduled::"), "{}", out.line);
    }

    #[test]
    fn h9_project_block_keeps_blocked_without_unblocked() {
        let before = "- [?] #task #prj_is_blocked #awaiting_reply \
                      [[m|✉️ L · Ada]] [fresh:: 2026-11-05] \
                      [scheduled:: 2026-11-10]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.changed);
        assert!(!out.unblocked_written, "{}", out.line);
        assert!(out.line.starts_with("- [?]"), "{}", out.line);
        assert!(out.line.contains("#prj_is_blocked"), "{}", out.line);
    }

    // H10: NEW rows (no `[fresh::]`) recover the box but take no field.
    #[test]
    fn h10_new_row_recovers_box_without_unblocked() {
        let before = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [created:: 2026-11-07] [scheduled:: 2026-11-10]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.changed);
        assert!(out.lifted);
        assert!(!out.unblocked_written, "{}", out.line);
        assert!(out.line.starts_with("- [ ] #task"), "{}", out.line);
    }

    // H11: an unheld Next row stays exactly as it is.
    #[test]
    fn h11_unheld_next_stays() {
        let before = "- [*] #task [[m|✉️ L · Ada]] [fresh:: 2026-11-05]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(!out.changed, "{}", out.line);
        assert_eq!(out.line, before);
        assert!(!out.lifted);
    }

    // H12: an unheld Ready row still resurfaces.
    #[test]
    fn h12_unheld_ready_resurfaces() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [fresh:: 2026-11-05]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.changed);
        assert!(!out.lifted);
        assert!(out.unblocked_written, "{}", out.line);
        assert!(
            out.line.contains("[unblocked:: 2026-11-08]"),
            "{}",
            out.line
        );
    }

    // H13: recurring rows refuse with a diagnostic.
    #[test]
    fn h13_recurring_hold_is_refused() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [repeat:: every week]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(!out.changed);
        assert_eq!(out.line, before);
        assert_eq!(out.refused.as_deref(), Some(REFUSAL_RECURRING));
    }

    #[test]
    fn h13_duplicate_scheduled_hold_is_refused() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [scheduled:: 2026-11-10] \
                      [scheduled:: 2026-11-12]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(!out.changed);
        assert_eq!(out.refused.as_deref(), Some(REFUSAL_DUPLICATE_SCHEDULED));
    }

    // H15: closed rows are untouched by either operation.
    #[test]
    fn h15_closed_rows_are_untouched() {
        let before =
            "- [x] #task #awaiting_reply [[m|✉️ L · Ada]] [scheduled:: 2026-11-10]";
        let held = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(!held.changed);
        assert_eq!(held.line, before);
        let lifted = lift(before, date(2026, 11, 8), &scope());
        assert!(!lifted.changed);
        assert_eq!(lifted.line, before);
    }

    // H16: a bounce lifts exactly like a reply.
    #[test]
    fn h16_bounce_lifts_like_a_reply() {
        let before = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [fresh:: 2026-11-05] [scheduled:: 2026-11-10]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.lifted);
        assert!(out.unblocked_written, "{}", out.line);
        assert!(out.line.starts_with("- [ ] #task"), "{}", out.line);
    }

    // H17: the hold tag follows the project-blocked token.
    #[test]
    fn h17_awaiting_follows_project_blocked() {
        let before = "- [ ] #task #prj_is_blocked [[m|✉️ L · Ada]] \
                      [created:: 2026-11-07]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(out.changed);
        assert!(
            out.line.contains("#task #prj_is_blocked #awaiting_reply"),
            "{}",
            out.line
        );
    }

    #[test]
    fn near_miss_tags_are_not_the_token() {
        let before =
            "- [?] #task #awaiting_reply_old [[m|✉️ L · Ada]] [fresh:: 2026-11-05]";
        let out = lift(before, date(2026, 11, 8), &scope());
        // No exact token, so nothing lifts; the Ready row still resurfaces
        // as an unheld row, but the near-miss tag and its spelling stand.
        assert!(!out.lifted);
        assert!(out.line.contains("#awaiting_reply_old"), "{}", out.line);
    }

    #[test]
    fn hold_does_not_duplicate_an_existing_tag() {
        let before = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [scheduled:: 2026-11-10]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(!out.changed);
        assert_eq!(
            out.line.matches("#awaiting_reply").count(),
            1,
            "{}",
            out.line
        );
    }

    #[test]
    fn hold_preserves_suffix_and_block_id() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [created:: 2026-11-07] \
                      [priority:: high] ^abc123";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(out.changed);
        assert!(out.line.ends_with("^abc123"), "{}", out.line);
        assert!(out.line.contains("[priority:: high]"), "{}", out.line);
        assert!(
            out.line.contains("[scheduled:: 2026-11-10]"),
            "{}",
            out.line
        );
        // The scheduled field lands before the block id, not after it.
        let sched = out.line.find("[scheduled::").expect("scheduled");
        let block = out.line.rfind("^abc123").expect("block");
        assert!(sched < block, "{}", out.line);
    }

    #[test]
    fn lift_preserves_block_id() {
        let before = "- [?] #task #awaiting_reply [[m|✉️ L · Ada]] \
                      [fresh:: 2026-11-05] [scheduled:: 2026-11-10] ^abc123";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.changed);
        assert!(out.line.ends_with("^abc123"), "{}", out.line);
    }

    #[test]
    fn hold_without_blocked_status_keeps_checkbox_with_reason() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [created:: 2026-11-07]";
        let out = apply_hold(
            before,
            &hold_req_blocked_bad(date(2026, 11, 10), "no Blocked status"),
        );
        assert!(out.changed, "tag/date still apply");
        assert!(out.line.contains("#awaiting_reply"), "{}", out.line);
        assert!(
            out.line.contains("[scheduled:: 2026-11-10]"),
            "{}",
            out.line
        );
        assert!(out.line.starts_with("- [ ]"), "{}", out.line);
        assert!(!out.checkbox_held);
        assert_eq!(out.checkbox_skipped.as_deref(), Some("no Blocked status"));
    }

    #[test]
    fn crlf_lines_keep_their_carriage_return() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [created:: 2026-11-07]\r";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(out.changed);
        assert!(out.line.ends_with('\r'), "{:?}", out.line);
        let lifted = lift(
            "- [?] #task #awaiting_reply [[m|x]] [fresh:: 2026-11-05] \
             [scheduled:: 2026-11-10]\r",
            date(2026, 11, 8),
            &scope(),
        );
        assert!(lifted.line.ends_with('\r'), "{:?}", lifted.line);
    }

    #[test]
    fn same_day_fresh_and_unblocked_tie_still_writes() {
        // fresh == today: the reply arrives the same day the stamp lands.
        let before = "- [ ] #task [[m|✉️ L · Ada]] [fresh:: 2026-11-08]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(out.unblocked_written, "{}", out.line);
        assert!(
            out.line.contains("[unblocked:: 2026-11-08]"),
            "{}",
            out.line
        );
    }

    #[test]
    fn malformed_scheduled_is_replaced_on_hold() {
        let before = "- [ ] #task [[m|✉️ L · Ada]] [scheduled:: someday]";
        let out = apply_hold(before, &hold_req(date(2026, 11, 10)));
        assert!(out.changed);
        assert!(
            out.line.contains("[scheduled:: 2026-11-10]"),
            "{}",
            out.line
        );
        assert!(!out.line.contains("someday"), "{}", out.line);
    }

    #[test]
    fn unheld_manual_scheduled_survives_lift_without_hold() {
        // H3 analogue: with no awaiting token the hold date is not owned,
        // so an unheld row keeps its own schedule through a no-op lift on
        // a lane row.
        let before = "- [*] #task [[m|✉️ L · Ada]] [fresh:: 2026-11-05] \
                      [scheduled:: 2026-11-20]";
        let out = lift(before, date(2026, 11, 8), &scope());
        assert!(!out.changed);
        assert!(
            out.line.contains("[scheduled:: 2026-11-20]"),
            "{}",
            out.line
        );
    }

    #[test]
    fn nudge_date_adds_calendar_days() {
        assert_eq!(nudge_date(date(2026, 11, 7), 3), date(2026, 11, 10));
        assert_eq!(nudge_date(date(2026, 12, 30), 3), date(2027, 1, 2));
    }

    #[test]
    fn send_local_date_respects_the_day_zone_across_dst() {
        // 2026-11-07 is after the US fall-back (Nov 1): New York is UTC-5.
        // 03:30 UTC Nov 8 is still Nov 7 in New York, but Nov 8 in UTC.
        let tz: Tz = "America/New_York".parse().expect("tz");
        let send_at = Utc
            .with_ymd_and_hms(2026, 11, 8, 3, 30, 0)
            .single()
            .expect("send");
        assert_eq!(send_local_date(send_at, tz), date(2026, 11, 7));
        assert_eq!(send_local_date(send_at, chrono_tz::UTC), date(2026, 11, 8));
        assert_eq!(nudge_date_for_send(send_at, tz, 3), date(2026, 11, 10));
    }

    #[test]
    fn running_lookup_matches_by_identity_not_label() {
        let lookup = RunningLookup::Session(BTreeSet::from([(
            "job.md".to_string(),
            "abc123".to_string(),
        )]));
        assert!(is_running_task(&lookup, "job.md", Some("abc123")));
        assert!(!is_running_task(&lookup, "job.md", Some("other")));
        assert!(!is_running_task(&lookup, "other.md", Some("abc123")));
        assert!(!is_running_task(&lookup, "job.md", None));
        assert!(!is_running_task(
            &RunningLookup::NoSession,
            "job.md",
            Some("abc123")
        ));
        // An ambiguous ledger defers every hold rather than guessing.
        assert!(is_running_task(
            &RunningLookup::Ambiguous("two sessions".to_string()),
            "job.md",
            Some("abc123")
        ));
    }

    #[test]
    fn running_lookup_reads_only_plain_links() {
        use crate::native::env as bob_env;
        use std::fs;

        let vault = tempfile::tempdir().expect("vault");
        let bob_dir = vault.path();
        // Day file for BOB_NOW 2026-11-07: vault/2026/20261107.md.
        let day_path = bob_dir.join("2026").join("20261107.md");
        fs::create_dir_all(day_path.parent().expect("parent")).expect("mkdir");
        fs::write(bob_dir.join("job.md"), "- [ ] #task Work ^abc123\n")
            .expect("task note");
        fs::write(
            &day_path,
            "# 2026-11-07\n\n## Pomodoros\n\n\
             - [ ] (**0800-0900** [t:: 60m]) — FOCUS\n\
             \t- [[job#^abc123|Work]]\n\
             \t- ~~[[job#^abc123|Struck]]~~\n",
        )
        .expect("day file");
        let guard = bob_env::TestEnvGuard::set(&[(
            "BOB_DAY_FILE",
            Some(day_path.as_os_str()),
        )]);
        let lookup = load_running_tasks(bob_dir);
        drop(guard);
        match lookup {
            RunningLookup::Session(keys) => {
                assert!(
                    keys.contains(&(
                        "job.md".to_string(),
                        "abc123".to_string()
                    )),
                    "{keys:?}"
                );
                // The struck duplicate resolves to the same identity but
                // contributes no second entry.
                assert_eq!(keys.len(), 1, "{keys:?}");
            }
            other => panic!("expected session, got {other:?}"),
        }
    }

    #[test]
    fn unblock_scope_detects_recurring_hidden_and_daily() {
        let scope =
            unblock_scope_for(Path::new("job.md"), "- [ ] #task", false);
        assert!(!scope.recurring);
        let scope = unblock_scope_for(
            Path::new("job.md"),
            "- [ ] #task [repeat:: every week]",
            false,
        );
        assert!(scope.recurring);
        let scope =
            unblock_scope_for(Path::new("job.md"), "- [ ] #task #hide", false);
        assert!(scope.hidden);
        let scope = unblock_scope_for(
            Path::new("2026/20261107.md"),
            "- [ ] #task",
            false,
        );
        assert!(scope.is_daily_note);
    }
}

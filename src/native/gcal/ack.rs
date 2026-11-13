//! Read-only queue and one-shot management for external Event Task receipts.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use chrono::{DateTime, Duration, FixedOffset, Local, NaiveDate};
use serde_json::{json, Value};
use unicode_width::UnicodeWidthChar;

use super::{
    account,
    classify::{self, DayWindow, Inclusion},
    config::GcalConfig,
    gog::Gog,
    journal::{self, AckScope, AckSeen, AckStatus, AckVia, NewAck},
    model::{EventDateTime, GcalEvent, MyAnswer},
    note::{self, NoteCandidate},
    snapshot::{FetchContext, SnapshotEvent},
    ui, AckArgs, GcalError, NoteArgs,
};
use crate::native::{
    config::GcalAckMode,
    env as bob_env,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
};

const DEFAULT_DAYS: u16 = 7;
const MAX_DAYS: u16 = 366;

#[derive(Debug, Clone, Copy)]
struct Window {
    first: NaiveDate,
    last: NaiveDate,
    days: u16,
}

struct AckRow {
    candidate: NoteCandidate,
    status: AckStatus,
    stale: bool,
}

#[derive(Debug, Clone)]
struct StaleNotice {
    calendar_id: String,
    event_id: String,
    title: String,
    start: Option<String>,
    end: Option<String>,
}

struct Queue {
    rows: Vec<AckRow>,
    waiting: Vec<AckRow>,
    stale: Vec<StaleNotice>,
    warnings: Vec<String>,
}

pub(crate) fn run(args: &AckArgs) -> i32 {
    match execute(args) {
        Ok(code) => code,
        Err(error) => ui::report_error("ack", &error, args.error_format()),
    }
}

fn execute(args: &AckArgs) -> Result<i32, GcalError> {
    validate_args(args)?;
    let config = GcalConfig::resolve()?;
    let target = selected_id(args).is_some() || !args.query.is_empty();
    if target {
        run_selected(args, &config)?;
        Ok(0)
    } else {
        run_queue(args, &config)
    }
}

fn validate_args(args: &AckArgs) -> Result<(), GcalError> {
    let url_id = note::extract_url_event_id(&args.query);
    let has_target =
        args.event_id.is_some() || url_id.is_some() || !args.query.is_empty();
    if args.event_id.is_some() && !args.query.is_empty() {
        return Err(usage(
            "`-i` cannot be combined with a title query",
            "use `-i <event-id> [-C <calendar>]` or a title query",
        ));
    }
    if args.clear && !has_target {
        return Err(usage(
            "`-c` needs an event selector",
            "use `-i <event-id>` or a title query with `-c`",
        ));
    }
    if args.list && (has_target || args.clear || args.scope.is_some()) {
        return Err(usage(
            "`-l` cannot be combined with a selector, `-c`, or `-s`",
            "drop `-l` to act on one event, or remove the selector to list",
        ));
    }
    if args.calendar.is_some() && args.event_id.is_none() && url_id.is_none() {
        return Err(usage(
            "`-C` needs an explicit event id",
            "use `-i <event-id> -C <calendar>`",
        ));
    }
    if args.scope.is_some() && !has_target {
        return Err(usage(
            "`-s` needs an event selector",
            "select an event with `-i` or QUERY",
        ));
    }
    if has_target && (args.all || args.next_n_days.is_some()) {
        return Err(usage(
            "a one-event selector cannot be combined with `-a` or `-n`",
            "drop the queue option or remove the selector",
        ));
    }
    if args.list && args.dry_run && has_target {
        return Err(usage(
            "`-l` is a queue listing",
            "drop the selector or `-l`",
        ));
    }
    if let Some(raw) = args.next_n_days.as_deref() {
        let value = raw.parse::<u16>().map_err(|_| {
            usage(
                "invalid day count",
                "use `-n N` where N is from 1 through 366",
            )
        })?;
        if !(1..=MAX_DAYS).contains(&value) {
            return Err(usage(
                format!("day count {value} is outside 1..=366"),
                "choose a value from 1 through 366",
            ));
        }
    }
    let explicit_walk = !has_target
        && (args.all || args.next_n_days.is_some() || args.day.is_some());
    if explicit_walk
        && !args.list
        && !args.dry_run
        && !args.format.is_json()
        && !note::is_tty()
    {
        return Err(usage(
            "an explicit ack walk needs a TTY",
            "use `-l`, `-d`, or `-f json` to inspect it, or select one event with `-i`",
        ));
    }
    Ok(())
}

fn usage(message: impl Into<String>, hint: &str) -> GcalError {
    GcalError::setup("bad_arguments", message.into()).with_hint(hint)
}

fn selected_id(args: &AckArgs) -> Option<String> {
    args.event_id
        .clone()
        .or_else(|| note::extract_url_event_id(&args.query))
}

fn resolve_window(args: &AckArgs) -> Result<Window, GcalError> {
    let timezone = bob_env::configured_timezone();
    let today = bob_env::current_datetime().date();
    let first = match args.day.as_deref() {
        Some(day) => classify::resolve_day(day, today)?,
        None => today,
    };
    let days = args
        .next_n_days
        .as_deref()
        .map(str::parse::<u16>)
        .transpose()
        .map_err(|_| {
            usage("invalid day count", "use a value from 1 through 366")
        })?
        .unwrap_or(DEFAULT_DAYS);
    let last = first
        .checked_add_signed(Duration::days(i64::from(days - 1)))
        .ok_or_else(|| {
            GcalError::setup(
                "bad_day",
                "ack window is out of range".to_string(),
            )
        })?;
    // Validate both endpoints before any transport call.
    let _ = DayWindow::new(first, timezone)?;
    let _ = DayWindow::new(last, timezone)?;
    Ok(Window { first, last, days })
}

fn run_queue(args: &AckArgs, config: &GcalConfig) -> Result<i32, GcalError> {
    let window = resolve_window(args)?;
    let vault = args.bob_dir();
    let evidence = load_evidence(&vault)?;
    let (queue, account) = fetch_queue(args, config, &vault, window, evidence)?;
    if args.format.is_json() {
        print_queue_json(args, window, &queue, false)?;
        return Ok(0);
    }
    if !args.list && !args.dry_run && note::is_tty() && !queue.rows.is_empty() {
        return Ok(run_session(config, &vault, queue, &account, args.all));
    }
    print_queue_human(args, window, &queue)?;
    Ok(0)
}

struct Evidence {
    notes: journal::NoteBook,
    acks: journal::AckBook,
    state: BTreeMap<(String, String), journal::OccurrenceState>,
    warnings: Vec<String>,
}

fn load_evidence(vault: &Path) -> Result<Evidence, GcalError> {
    let (notes, mut warnings) =
        journal::load_notes(vault).map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("read private-note evidence: {error}"),
            )
        })?;
    let (acks, ack_warnings) = journal::load_acks(vault).map_err(|error| {
        GcalError::runtime("journal", format!("read ack receipts: {error}"))
    })?;
    warnings.extend(ack_warnings);
    let (state, state_warnings, _) =
        journal::load_state(vault).map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("read gcal row journal: {error}"),
            )
        })?;
    warnings.extend(state_warnings);
    Ok(Evidence {
        notes,
        acks,
        state,
        warnings,
    })
}

fn fetch_queue(
    args: &AckArgs,
    config: &GcalConfig,
    vault: &Path,
    window: Window,
    mut evidence: Evidence,
) -> Result<(Queue, String), GcalError> {
    let context = FetchContext::new(config)?;
    let account = context.account().to_string();
    let timezone = bob_env::configured_timezone();
    let now = bob_env::current_datetime();
    let mut days = Vec::with_capacity(usize::from(window.days));
    for offset in 0..window.days {
        days.push(window.first + Duration::days(i64::from(offset)));
    }
    // Fetch all pages over the complete contiguous window before any write.
    let mut raw_days = context.fetch_days(&days)?;
    let mut resolver = HomeResolver::new(DiskHomeSource::new(vault));
    let mut rows = Vec::new();
    let mut waiting = Vec::new();
    let mut stale = BTreeMap::<(String, String), StaleNotice>::new();
    let mut observed = BTreeSet::<(String, String)>::new();
    let ack_on = config.ack_mode() == GcalAckMode::External;

    for day in days.iter().copied() {
        if day != window.first {
            resolver.refresh();
        }
        let raw = raw_days.remove(&day).unwrap_or_default();
        let snapshot = context.assemble_day(
            vault.to_path_buf(),
            day,
            raw,
            &mut resolver,
        )?;
        let daily = read_daily_note_if_needed(
            vault,
            &snapshot.daily_rel,
            &evidence.state,
            day,
        )?;
        for item in &snapshot.events {
            let event = &item.event;
            observed.insert((
                event.key.calendar_id.clone(),
                event.key.event_id.clone(),
            ));
            let Some(receipt) = evidence.acks.event(
                &account,
                &event.key.calendar_id,
                &event.key.event_id,
            ) else {
                continue;
            };
            if journal::ack_is_stale(receipt, &event.ack_seen()) {
                stale.insert(
                    (event.key.calendar_id.clone(), event.key.event_id.clone()),
                    stale_notice(event),
                );
            }
        }
        for item in snapshot.events {
            let event = &item.event;
            let start = match &event.start {
                EventDateTime::Timed(start) => *start,
                EventDateTime::AllDay(_) => continue,
            };
            let start_day = note::base_day_for(&start, timezone);
            if start_day != day
                || start_day < window.first
                || start_day > window.last
            {
                continue;
            }
            if !ends_after_now(event, now, timezone) {
                continue;
            }
            if !event.is_external() || !note::is_eligible(event) {
                continue;
            }
            let status = evidence.acks.status_for(
                &evidence.notes,
                &account,
                &event.key.calendar_id,
                &event.key.event_id,
                event.series_id.as_deref(),
            );
            if status.state != journal::AckState::Unacked {
                continue;
            }
            if !ack_on {
                continue;
            }
            let included_by_all = args.all
                && (event.my_answer == MyAnswer::NoReply
                    || (event.my_answer == MyAnswer::Maybe
                        && item.inclusion != Inclusion::Task));
            if removed_by_user(&evidence.state, daily.as_deref(), &item, day) {
                continue;
            }
            if item.inclusion != Inclusion::Task && !included_by_all {
                if event.my_answer == MyAnswer::NoReply {
                    waiting.push(row_from_snapshot(item, status, day, false));
                }
                continue;
            }
            let row = row_from_snapshot(item, status, day, false);
            if row.candidate.event.my_answer == MyAnswer::NoReply {
                waiting.push(clone_row(&row));
            }
            rows.push(row);
        }
    }

    // A live event receipt may have moved out of the fetched window. Its
    // old start identifies which receipts need a narrow current lookup.
    for receipt in evidence.acks.iter().filter(|receipt| {
        receipt.account == account
            && receipt.scope == AckScope::Event
            && receipt.acked
            && !observed
                .contains(&(receipt.calendar_id.clone(), receipt.key.clone()))
            && seen_day_in_window(&receipt.seen, timezone, window)
    }) {
        match context.lookup_event(&receipt.calendar_id, &receipt.key) {
            Ok(raw) => {
                let event = GcalEvent::from_google(&receipt.calendar_id, raw)?;
                if journal::ack_is_stale(receipt, &event.ack_seen()) {
                    stale.insert(
                        (
                            event.key.calendar_id.clone(),
                            event.key.event_id.clone(),
                        ),
                        stale_notice(&event),
                    );
                }
            }
            Err(error) if note::is_not_found_error(&error) => {}
            Err(error) => return Err(error),
        }
    }

    rows.sort_by(compare_rows);
    waiting.sort_by(compare_rows);
    let stale = stale.into_values().collect();
    evidence.warnings.sort();
    evidence.warnings.dedup();
    Ok((
        Queue {
            rows,
            waiting,
            stale,
            warnings: evidence.warnings,
        },
        account,
    ))
}

fn read_daily_note_if_needed(
    vault: &Path,
    relative: &Path,
    state: &BTreeMap<(String, String), journal::OccurrenceState>,
    day: NaiveDate,
) -> Result<Option<String>, GcalError> {
    if !state.iter().any(|((_, row_day), item)| {
        row_day == &day.to_string() && item.row.is_some() && !item.gone
    }) {
        return Ok(None);
    }
    match std::fs::read_to_string(vault.join(relative)) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Some(String::new()))
        }
        Err(error) => Err(GcalError::runtime(
            "vault",
            format!(
                "read {} to check removed Event Tasks: {error}",
                relative.display()
            ),
        )),
    }
}

fn removed_by_user(
    state: &BTreeMap<(String, String), journal::OccurrenceState>,
    contents: Option<&str>,
    item: &SnapshotEvent,
    day: NaiveDate,
) -> bool {
    let key = journal::occurrence_key(
        &item.event.key.calendar_id,
        &item.event.key.event_id,
    );
    let Some(state) = state.get(&(key, day.to_string())) else {
        return false;
    };
    if state.gone {
        // `line_removed` is lifecycle's persisted form of the same
        // removed-by-you state detected below before sync writes a tombstone.
        // Other gone reasons describe Google-side lifecycle changes.
        return matches!(
            state.gone_reason.as_deref(),
            None | Some("line_removed")
        );
    }
    let Some(row) = state.row.as_ref() else {
        return false;
    };
    let lines: Vec<&str> = contents.unwrap_or_default().lines().collect();
    let suffix = format!("^{}", row.block_id);
    let block_present = lines.iter().any(|line| {
        line.trim_end().strip_suffix(&suffix).is_some_and(|before| {
            before.chars().last().is_some_and(char::is_whitespace)
        })
    });
    let url = row.url.as_deref().or(item.event.html_link.as_deref());
    let url_present = url.is_some_and(|url| {
        !url.is_empty()
            && lines.iter().any(|line| {
                line.contains(url)
                    && super::lifecycle::split_row_spans(line).is_some()
            })
    });
    !block_present && !url_present
}

fn row_from_snapshot(
    item: SnapshotEvent,
    status: AckStatus,
    day: NaiveDate,
    stale: bool,
) -> AckRow {
    AckRow {
        candidate: NoteCandidate {
            event: item.event,
            google_event: item.google_event,
            calendar_name: item.calendar_name,
            day,
            time: item.time,
            sort_at: item.sort_at,
            inclusion: item.inclusion,
            root: item.root,
            clock: item.clock,
            root_only_clock: item.root_only_clock,
        },
        status,
        stale,
    }
}

fn clone_row(row: &AckRow) -> AckRow {
    AckRow {
        candidate: note::clone_candidate(&row.candidate),
        status: row.status.clone(),
        stale: row.stale,
    }
}

fn compare_rows(left: &AckRow, right: &AckRow) -> std::cmp::Ordering {
    left.candidate
        .sort_at
        .cmp(&right.candidate.sort_at)
        .then_with(|| {
            left.candidate
                .event
                .key
                .calendar_id
                .cmp(&right.candidate.event.key.calendar_id)
        })
        .then_with(|| {
            left.candidate
                .event
                .key
                .event_id
                .cmp(&right.candidate.event.key.event_id)
        })
}

fn seen_day_in_window(
    seen: &AckSeen,
    timezone: Option<chrono_tz::Tz>,
    window: Window,
) -> bool {
    let Some(start) = seen.start.as_deref().and_then(parse_instant) else {
        return false;
    };
    let day = note::base_day_for(&start, timezone);
    day >= window.first && day <= window.last
}

fn parse_instant(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

fn ends_after_now(
    event: &GcalEvent,
    now: chrono::NaiveDateTime,
    timezone: Option<chrono_tz::Tz>,
) -> bool {
    let EventDateTime::Timed(end) = &event.end else {
        return false;
    };
    let local_end = match timezone {
        Some(timezone) => end.with_timezone(&timezone).naive_local(),
        None => end.with_timezone(&Local).naive_local(),
    };
    local_end > now
}

fn stale_notice(event: &GcalEvent) -> StaleNotice {
    let seen = event.ack_seen();
    StaleNotice {
        calendar_id: event.key.calendar_id.clone(),
        event_id: event.key.event_id.clone(),
        title: event.title.clone(),
        start: seen.start,
        end: seen.end,
    }
}

fn print_queue_human(
    args: &AckArgs,
    window: Window,
    queue: &Queue,
) -> Result<(), GcalError> {
    let noun = if queue.rows.len() == 1 {
        "event"
    } else {
        "events"
    };
    let mut output = format!(
        "🔔 {} unacked external {noun} · {} through {}\n",
        queue.rows.len(),
        window.first,
        window.last,
    );
    if queue.rows.is_empty() {
        output.push_str("✓ no unacked external Event Tasks in this window\n");
    }
    for row in &queue.rows {
        let candidate = &row.candidate;
        let root = candidate
            .root
            .as_ref()
            .and_then(|root| root.value.as_deref())
            .unwrap_or("no root");
        let root_source = candidate
            .root
            .as_ref()
            .and_then(|root| root.kind.json_label())
            .unwrap_or("none");
        let buffer = candidate.event.buffer.label().unwrap_or_default();
        let mut metadata = vec![
            format!("from {}", candidate.organizer_name()),
            format!("lands as {}", candidate.lands()),
            format!("root {root} ({root_source})"),
        ];
        if !buffer.is_empty() {
            metadata.push(format!("buffer {buffer}"));
        }
        if candidate.clock.off_clock {
            metadata.push("off-clock".to_string());
        }
        output.push_str(&format!(
            "  {} {} · {}\n    {}\n",
            candidate.day,
            candidate.time,
            candidate.event.title,
            metadata.join(" · "),
        ));
        if row.stale {
            output.push_str("    ⚠ ack stale · moved since ack?\n");
        }
    }
    if !queue.waiting.is_empty() {
        output.push_str(&format!(
            "· {} awaiting your reply in Google Calendar{}\n",
            queue.waiting.len(),
            if args.all {
                " (included with -a)"
            } else {
                " · include with -a"
            },
        ));
    }
    if !queue.stale.is_empty() {
        output.push_str("\nStale acknowledgements · moved since review?\n");
        for item in &queue.stale {
            output.push_str(&format!(
                "  ⚠ {} · {} · {} · ack stale · moved since ack?\n",
                item.title,
                item.start.as_deref().unwrap_or("unknown time"),
                item.calendar_id,
            ));
        }
    }
    if args.query.is_empty()
        && args.event_id.is_none()
        && !args.list
        && !args.dry_run
        && !args.format.is_json()
    {
        output.push_str("\nThis listing is read-only · run on a TTY for the one-key review.\n");
    } else if !queue.rows.is_empty()
        && !args.list
        && (args.dry_run || args.format.is_json())
    {
        output.push_str("\nPreview only · no receipt or note was written.\n");
    }
    for warning in &queue.warnings {
        ui::warn(warning);
    }
    if args.quiet {
        for item in &queue.stale {
            ui::warn(&format!(
                "{} · {} · {} · ack stale · moved since ack?",
                item.title,
                item.start.as_deref().unwrap_or("unknown time"),
                item.calendar_id,
            ));
        }
        return write_stdout_text("");
    }
    write_stdout_text(&output)
}

fn print_queue_json(
    args: &AckArgs,
    window: Window,
    queue: &Queue,
    complete: bool,
) -> Result<(), GcalError> {
    let value = json!({
        "schema_version": 1,
        "ok": true,
        "action": "list",
        "preview": args.dry_run || args.format.is_json(),
        "window": window_json(window),
        "queue": queue.rows.iter().map(row_json).collect::<Vec<_>>(),
        "waiting": {
            "count": queue.waiting.len(),
            "events": queue.waiting.iter().map(row_json).collect::<Vec<_>>(),
        },
        "stale": queue.stale.iter().map(stale_json).collect::<Vec<_>>(),
        "counts": {
            "queue": queue.rows.len(),
            "waiting": queue.waiting.len(),
            "stale": queue.stale.len(),
        },
        "complete": complete
            || (queue.rows.is_empty() && queue.waiting.is_empty()),
        "warnings": &queue.warnings,
    });
    write_json(&value)
}

fn window_json(window: Window) -> Value {
    json!({ "from": window.first.to_string(), "to": window.last.to_string(), "days": window.days })
}

fn row_json(row: &AckRow) -> Value {
    let candidate = &row.candidate;
    let event = &candidate.event;
    let buffer = if candidate.inclusion == Inclusion::Task {
        event.buffer
    } else {
        Default::default()
    };
    let root = candidate.root.as_ref();
    json!({
        "calendar_id": event.key.calendar_id,
        "event_id": event.key.event_id,
        "title": event.title,
        "day": candidate.day.to_string(),
        "time": candidate.time,
        "start": event.ack_seen().start,
        "end": event.ack_seen().end,
        "organizer": candidate.organizer_name(),
        "external": event.is_external(),
        "lands": candidate.lands(),
        "root": root.and_then(|root| root.value.as_deref()),
        "root_source": root.and_then(|root| root.kind.json_label()),
        "buffer": if buffer.is_empty() { Value::Null } else { json!({ "before_minutes": buffer.before_min, "after_minutes": buffer.after_min }) },
        "clock": {
            "off_clock": candidate.clock.off_clock,
            "source": candidate.clock.source,
            "note": candidate.clock.note,
        },
        "ack": row.status.json(),
        "ack_stale": row.stale,
    })
}

fn stale_json(item: &StaleNotice) -> Value {
    json!({
        "calendar_id": item.calendar_id,
        "event_id": item.event_id,
        "title": item.title,
        "start": item.start,
        "end": item.end,
        "ack_stale": true,
    })
}

fn write_json(value: &Value) -> Result<(), GcalError> {
    let text = serde_json::to_string(value).map_err(|error| {
        GcalError::runtime("output", format!("serialize ack JSON: {error}"))
    })?;
    write_stdout_text(&format!("{text}\n"))
}

fn write_stdout_text(text: &str) -> Result<(), GcalError> {
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(GcalError::runtime(
            "output",
            format!("write ack output: {error}"),
        )),
    }
}

fn run_selected(args: &AckArgs, config: &GcalConfig) -> Result<(), GcalError> {
    let vault = args.bob_dir();
    let before = load_evidence(&vault)?;
    let gog = Gog::resolve(
        crate::native::google::GCAL,
        config.gog(),
        config.timeout(),
    )?;
    let _ = gog.require_version()?;
    let account = account::resolve_command(&gog, config.account())?;
    if args.clear
        && let (Some(event_id), Some(calendar_id), Some(scope)) = (
            args.event_id.as_deref(),
            args.calendar.as_deref(),
            args.scope.as_deref().and_then(parse_ack_scope),
        )
    {
        let receipt = match scope {
            AckScope::Event => {
                before.acks.event(&account, calendar_id, event_id)
            }
            AckScope::Series => {
                before.acks.series(&account, calendar_id, event_id)
            }
        }
        .cloned();
        if let Some(receipt) = receipt {
            return clear_receipt_by_key(
                args, config, &vault, &account, &before, receipt,
            );
        }
    }
    let calendars = gog.calendars(&account)?;
    let mut resolver = HomeResolver::new(DiskHomeSource::new(&vault));
    let mut warnings = before.warnings.clone();
    let candidate = select_candidate(
        args,
        config,
        &vault,
        &mut resolver,
        &gog,
        &account,
        &calendars,
        &mut warnings,
    )?;
    let preview = args.dry_run || args.format.is_json();
    let scope = choose_scope(args, &candidate, preview)?;
    let current = before.acks.status_for(
        &before.notes,
        &account,
        &candidate.event.key.calendar_id,
        &candidate.event.key.event_id,
        candidate.event.series_id.as_deref(),
    );
    let receipt = before.acks.event(
        &account,
        &candidate.event.key.calendar_id,
        &candidate.event.key.event_id,
    );
    let stale = receipt.is_some_and(|receipt| {
        journal::ack_is_stale(receipt, &candidate.event.ack_seen())
    });
    let action = if args.clear { "clear" } else { "ack" };
    if preview {
        return print_selected_preview(
            args, &vault, &candidate, scope, action, current, stale, &warnings,
        );
    }

    let host = journal::host_label();
    let record = NewAck {
        account: account.clone(),
        calendar_id: candidate.event.key.calendar_id.clone(),
        event_id: (scope == AckScope::Event)
            .then(|| candidate.event.key.event_id.clone()),
        series: (scope == AckScope::Series)
            .then(|| candidate.event.series_id.clone().unwrap_or_default()),
        scope,
        acked: !args.clear,
        via: AckVia::Ack,
        seen: candidate.event.ack_seen(),
    };
    journal::append_ack(&vault, &host, &record).map_err(|error| {
        GcalError::runtime(
            "journal",
            format!("append gcal ack receipt: {error}"),
        )
    })?;
    let (acks, ack_warnings) = journal::load_acks(&vault).map_err(|error| {
        GcalError::runtime(
            "journal",
            format!("verify appended ack receipt: {error}"),
        )
    })?;
    let (notes, note_warnings) =
        journal::load_notes(&vault).map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("verify private-note evidence: {error}"),
            )
        })?;
    warnings.extend(ack_warnings);
    warnings.extend(note_warnings);
    let after = acks.status_for(
        &notes,
        &account,
        &candidate.event.key.calendar_id,
        &candidate.event.key.event_id,
        candidate.event.series_id.as_deref(),
    );
    let after_receipt = acks.event(
        &account,
        &candidate.event.key.calendar_id,
        &candidate.event.key.event_id,
    );
    let after_stale = after_receipt.is_some_and(|receipt| {
        journal::ack_is_stale(receipt, &candidate.event.ack_seen())
    });
    print_selected_result(
        args,
        config,
        &candidate,
        scope,
        action,
        after,
        after_stale,
        &warnings,
    )
}

fn parse_ack_scope(value: &str) -> Option<AckScope> {
    match value {
        "event" => Some(AckScope::Event),
        "series" => Some(AckScope::Series),
        _ => None,
    }
}

/// Clear a receipt whose exact account/calendar/scope key is already in the
/// journal. Orphan remediation must not invent an event or series identity
/// from a failed Google lookup.
fn clear_receipt_by_key(
    args: &AckArgs,
    config: &GcalConfig,
    vault: &Path,
    account: &str,
    before: &Evidence,
    receipt: journal::FoldedAck,
) -> Result<(), GcalError> {
    let scope = receipt.scope;
    let event_id = (scope == AckScope::Event).then_some(receipt.key.as_str());
    let series_id = (scope == AckScope::Series).then_some(receipt.key.as_str());
    let current = before.acks.status_for(
        &before.notes,
        account,
        &receipt.calendar_id,
        event_id.unwrap_or(""),
        series_id,
    );
    let preview = args.dry_run || args.format.is_json();
    if preview {
        return print_receipt_clear_preview(args, &receipt, current);
    }

    let record = NewAck {
        account: account.to_string(),
        calendar_id: receipt.calendar_id.clone(),
        event_id: event_id.map(str::to_string),
        series: series_id.map(str::to_string),
        scope,
        acked: false,
        via: AckVia::Ack,
        seen: receipt.seen.clone(),
    };
    journal::append_ack(vault, &journal::host_label(), &record).map_err(
        |error| {
            GcalError::runtime(
                "journal",
                format!("append gcal ack receipt: {error}"),
            )
        },
    )?;
    let (acks, ack_warnings) = journal::load_acks(vault).map_err(|error| {
        GcalError::runtime(
            "journal",
            format!("verify appended ack receipt: {error}"),
        )
    })?;
    let (notes, note_warnings) =
        journal::load_notes(vault).map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("verify private-note evidence: {error}"),
            )
        })?;
    let after = acks.status_for(
        &notes,
        account,
        &receipt.calendar_id,
        event_id.unwrap_or(""),
        series_id,
    );
    let mut warnings = before.warnings.clone();
    warnings.extend(ack_warnings);
    warnings.extend(note_warnings);
    print_receipt_clear_result(args, config, &receipt, after, &warnings)
}

fn print_receipt_clear_preview(
    args: &AckArgs,
    receipt: &journal::FoldedAck,
    current: AckStatus,
) -> Result<(), GcalError> {
    if args.format.is_json() {
        let key = match receipt.scope {
            AckScope::Event => json!({"event_id": receipt.key}),
            AckScope::Series => json!({"series": receipt.key}),
        };
        return write_json(&json!({
            "schema_version": 1,
            "ok": true,
            "action": "preview",
            "proposed_action": {"action": "clear", "scope": receipt.scope.as_str()},
            "selected": {
                "calendar_id": receipt.calendar_id,
                "key": key,
                "scope_evidence": current.state.as_str(),
                "ack_stale": false,
                "current_event_available": false,
            },
            "queue": [],
            "waiting": {"count": 0, "events": []},
            "stale": [],
            "counts": {"queue": 0, "waiting": 0, "stale": 0},
            "complete": false,
            "warnings": [],
        }));
    }
    let key = &receipt.key;
    let mut output = format!(
        "Preview · would clear the existing {} receipt for {key} ({})\n  same-scope evidence: {}\n",
        receipt.scope.as_str(),
        receipt.calendar_id,
        current.state.as_str(),
    );
    output.push_str(
        "The current Google event is unavailable · no receipt or note was written.\n",
    );
    write_stdout_text(&output)
}

fn print_receipt_clear_result(
    args: &AckArgs,
    config: &GcalConfig,
    receipt: &journal::FoldedAck,
    status: AckStatus,
    warnings: &[String],
) -> Result<(), GcalError> {
    if args.quiet {
        for warning in warnings {
            ui::warn(warning);
        }
        return Ok(());
    }
    let mut output = format!(
        "✓ Cleared the {} receipt for {} ({})\n",
        receipt.scope.as_str(),
        receipt.key,
        receipt.calendar_id,
    );
    if status.state == journal::AckState::Noted {
        output.push_str(
            "  same-scope note history remains · review remains noted\n",
        );
    } else if status.state == journal::AckState::Acked {
        output.push_str("  another same-scope receipt remains live\n");
    } else {
        output.push_str("  no same-scope receipt or note history remains\n");
    }
    output.push_str(
        "  the current Google event was unavailable; other-scope evidence was left unchanged\n",
    );
    let writer = config.writer().unwrap_or("this host");
    let host = journal::host_label();
    if writer == host {
        output.push_str("  run `bob gcal sync` to apply the row change\n");
    } else {
        output.push_str(&format!(
            "  the row change will land on the next sync on {writer}\n"
        ));
    }
    for warning in warnings {
        ui::warn(warning);
    }
    write_stdout_text(&output)
}

fn select_candidate(
    args: &AckArgs,
    config: &GcalConfig,
    vault: &Path,
    resolver: &mut HomeResolver<DiskHomeSource>,
    gog: &Gog,
    account: &str,
    calendars: &[super::gog::GogCalendar],
    warnings: &mut Vec<String>,
) -> Result<NoteCandidate, GcalError> {
    let id = selected_id(args);
    let mut note_args = note_args(args);
    if let Some(id) = id {
        let (candidate, _) = note::lookup_exact(
            config,
            vault,
            resolver,
            gog,
            account,
            calendars,
            &id,
            args.calendar.as_deref(),
        )?;
        if let Some(candidate) = candidate {
            return Ok(candidate);
        }
        if args.event_id.is_some()
            || note::extract_url_event_id(&args.query).is_some()
        {
            return Err(GcalError::setup(
                "no_match",
                format!("no event with id {id}"),
            )
            .with_hint("run `bob gcal list -D <day>` to find an event id"));
        }
        note_args.query = args.query.clone();
    }
    let day_filter = args
        .day
        .as_deref()
        .map(|day| {
            let now = bob_env::current_datetime();
            let today = now.date();
            classify::resolve_day(day, today)
        })
        .transpose()?;
    let (from, to, _, _) = note::window_range(day_filter)?;
    let (candidates, fetch_warnings) = note::fetch_candidates(
        config, vault, resolver, gog, account, calendars, &from, &to,
        day_filter,
    )?;
    warnings.extend(fetch_warnings);
    select_query(&note_args, candidates, day_filter)
}

fn select_query(
    args: &NoteArgs,
    candidates: Vec<NoteCandidate>,
    day_filter: Option<NaiveDate>,
) -> Result<NoteCandidate, GcalError> {
    let mut matching: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            note::matches_query(&candidate.event.title, &args.query)
        })
        .map(|(index, _)| index)
        .collect();
    matching.sort_by(|left, right| {
        note::rank_candidate(
            &candidates[*left].event.title,
            &args.query,
            &candidates[*left].sort_at,
        )
        .cmp(&note::rank_candidate(
            &candidates[*right].event.title,
            &args.query,
            &candidates[*right].sort_at,
        ))
    });
    if matching.is_empty() {
        return Err(GcalError::setup(
            "no_match",
            format!("no events match {}", args.query.join(" ")),
        )
        .with_hint("try `-D DAY` or select an event with `-i <event-id>`"));
    }
    let groups = note::group_candidates(&candidates);
    let mut ranked_groups = Vec::new();
    for index in matching {
        if let Some(group) =
            groups.iter().find(|group| group.indices.contains(&index))
            && !ranked_groups
                .iter()
                .any(|existing: &&note::SeriesGroup| existing.key == group.key)
        {
            ranked_groups.push(group);
        }
    }
    if ranked_groups.len() > 1 {
        if args.format.is_json() || !note::is_tty() || args.dry_run {
            let candidates_text = ranked_groups
                .iter()
                .take(10)
                .map(|group| {
                    let candidate = &candidates[group.soonest];
                    format!(
                        "{} {} · {} (-i {} -C {})",
                        candidate.day,
                        candidate.time,
                        candidate.event.title,
                        candidate.event.key.event_id,
                        candidate.event.key.calendar_id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  ");
            return Err(GcalError::setup(
                "ambiguous",
                format!("several events match; be more specific or use `-i <event-id>`\n  {candidates_text}"),
            ).with_hint("rerun with a narrower query or `-i <event-id>`"));
        }
        let chosen = note::pick_group_owned(&candidates, &ranked_groups)?;
        return note::select_from_group(
            &candidates,
            ranked_groups[chosen],
            day_filter,
        );
    }
    note::select_from_group(&candidates, ranked_groups[0], day_filter)
}

fn note_args(args: &AckArgs) -> NoteArgs {
    NoteArgs {
        adopt: false,
        all: false,
        bob_dir: args.bob_dir.clone(),
        calendar: args.calendar.clone(),
        clear: false,
        day: args.day.clone(),
        dry_run: args.dry_run,
        edit: false,
        event_id: args.event_id.clone(),
        format: args.format,
        list: false,
        scope: args.scope.clone(),
        query: args.query.clone(),
        text: Vec::new(),
    }
}

fn choose_scope(
    args: &AckArgs,
    candidate: &NoteCandidate,
    preview: bool,
) -> Result<AckScope, GcalError> {
    if !candidate.recurring() {
        if args.scope.as_deref() == Some("series") {
            return Err(GcalError::setup(
                "bad_scope",
                "`-s series` needs a recurring event".to_string(),
            )
            .with_hint("drop `-s series` for a one-off event"));
        }
        return Ok(AckScope::Event);
    }
    if let Some(scope) = args.scope.as_deref() {
        return match scope {
            "event" => Ok(AckScope::Event),
            "series" => Ok(AckScope::Series),
            _ => Err(GcalError::setup(
                "bad_scope",
                format!("invalid scope {scope:?}; use event or series"),
            )),
        };
    }
    if preview || !note::is_tty() {
        return Err(GcalError::setup(
            "needs_scope",
            format!(
                "{} repeats; choose This event or All events",
                candidate.event.title
            ),
        )
        .with_hint("rerun with `-s event` or `-s series`"));
    }
    note::write::choose_write_scope(&note_args(args), candidate).map(|scope| {
        match scope {
            journal::NoteScope::Event => AckScope::Event,
            journal::NoteScope::Series => AckScope::Series,
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn print_selected_preview(
    args: &AckArgs,
    vault: &Path,
    candidate: &NoteCandidate,
    scope: AckScope,
    action: &str,
    current: AckStatus,
    stale: bool,
    warnings: &[String],
) -> Result<(), GcalError> {
    let preview = note::write::preview(candidate, vault, false)?;
    if args.format.is_json() {
        let buffer = if candidate.inclusion == Inclusion::Task {
            candidate.event.buffer
        } else {
            Default::default()
        };
        let seen = candidate.event.ack_seen();
        let mut row = json!({
            "calendar_id": candidate.event.key.calendar_id,
            "event_id": candidate.event.key.event_id,
            "title": candidate.event.title,
            "day": candidate.day.to_string(),
            "time": candidate.time,
            "start": seen.start,
            "end": seen.end,
            "organizer": candidate.organizer_name(),
            "external": candidate.event.is_external(),
            "lands": candidate.lands(),
            "root": candidate.root.as_ref().and_then(|root| root.value.as_deref()),
            "root_source": candidate.root.as_ref().and_then(|root| root.kind.json_label()),
            "buffer": if buffer.is_empty() { Value::Null } else { json!({ "before_minutes": buffer.before_min, "after_minutes": buffer.after_min }) },
            "clock": {
                "off_clock": candidate.clock.off_clock,
                "source": candidate.clock.source,
                "note": candidate.clock.note,
            },
            "ack": current.json(),
            "ack_stale": stale,
        });
        if let Some(object) = row.as_object_mut() {
            object.insert(
                "preview_row".to_string(),
                preview.row.map(Value::String).unwrap_or(Value::Null),
            );
        }
        let value = json!({
            "schema_version": 1,
            "ok": true,
            "action": "preview",
            "proposed_action": { "action": action, "scope": scope.as_str() },
            "selected": row,
            "window": target_window_json(candidate.day),
            "queue": [],
            "waiting": { "count": 0, "events": [] },
            "stale": stale.then(|| stale_json(&stale_notice(&candidate.event))).into_iter().collect::<Vec<_>>(),
            "counts": { "queue": 0, "waiting": 0, "stale": usize::from(stale) },
            "complete": false,
            "warnings": warnings,
        });
        return write_json(&value);
    }
    let label = if action == "clear" {
        "clear"
    } else {
        "acknowledge"
    };
    let mut output = format!(
        "Preview · would {label} {} ({})\n",
        candidate.event.title,
        scope.as_str()
    );
    output.push_str(&format!(
        "  {} {} · from {} · lands as {}\n",
        candidate.day,
        candidate.time,
        candidate.organizer_name(),
        candidate.lands()
    ));
    output.push_str(&format!(
        "  current ack state: {}\n",
        current.state.as_str()
    ));
    if let Some(row) = preview.row {
        output.push_str(&format!("  lands as\n  {row}\n"));
    }
    if stale {
        output.push_str("  ⚠ ack stale · moved since ack?\n");
    }
    output.push_str("No receipt, note, sync, or daily-note write was made.\n");
    for warning in warnings {
        ui::warn(warning);
    }
    write_stdout_text(&output)
}

fn target_window_json(day: NaiveDate) -> Value {
    json!({ "from": day.to_string(), "to": day.to_string(), "days": 1 })
}

fn print_selected_result(
    args: &AckArgs,
    config: &GcalConfig,
    candidate: &NoteCandidate,
    scope: AckScope,
    action: &str,
    status: AckStatus,
    stale: bool,
    warnings: &[String],
) -> Result<(), GcalError> {
    if args.format.is_json() {
        unreachable!("JSON actions always preview")
    }
    if args.quiet {
        if stale {
            ui::warn(&format!(
                "{} · ack stale · moved since ack?",
                candidate.event.title
            ));
        }
        for warning in warnings {
            ui::warn(warning);
        }
        return Ok(());
    }
    let writer = config.writer().unwrap_or("this host");
    let host = journal::host_label();
    let writer_message = if writer == host {
        "run `bob gcal sync` to apply the row change".to_string()
    } else {
        format!("the row change will land on the next sync on {writer}")
    };
    let result = if action == "clear" {
        if status.state == journal::AckState::Unacked {
            format!(
                "✓ Cleared the {} receipt · review is unacked again",
                scope.as_str()
            )
        } else {
            format!(
                "✓ Cleared the {} receipt · review remains {}",
                scope.as_str(),
                status.state.as_str()
            )
        }
    } else {
        format!(
            "✓ Acked {} · {} scope",
            candidate.event.title,
            scope.as_str()
        )
    };
    let mut output = format!("{result}\n  {writer_message}\n");
    if stale {
        output.push_str("  ⚠ ack stale · moved since ack?\n");
    }
    for warning in warnings {
        ui::warn(warning);
    }
    write_stdout_text(&output)
}

// ---------------------------------------------------------------------------
// Terminal review session: one key per card over a bare TTY.
// ---------------------------------------------------------------------------

/// Ctrl-C during the session never bypasses terminal restoration: the
/// handler only records the interrupt, and the session observes it at the
/// next key, line, or editor boundary, where it reports saved answers and
/// exits 130. No allocation or printing happens in the handler itself.
#[cfg(unix)]
static SESSION_INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn session_sigint_handler(_: libc::c_int) {
    SESSION_INTERRUPTED.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
fn session_interrupted() -> bool {
    SESSION_INTERRUPTED.load(Ordering::SeqCst)
}

/// Restores the previous SIGINT disposition when the session ends.
struct InterruptGuard {
    #[cfg(unix)]
    previous: libc::sigaction,
}

impl InterruptGuard {
    fn install() -> Self {
        #[cfg(unix)]
        {
            SESSION_INTERRUPTED.store(false, Ordering::SeqCst);
            // SAFETY: installs a flag-only handler with no SA_RESTART, so a
            // blocking key or line read fails with EINTR instead of hanging
            // past Ctrl-C; the previous action is restored on drop.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction =
                session_sigint_handler as *const () as libc::sighandler_t;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            action.sa_flags = 0;
            let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigaction(libc::SIGINT, &action, &mut previous);
            }
            Self { previous }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: restores the action saved at install time.
            unsafe {
                libc::sigaction(
                    libc::SIGINT,
                    &self.previous,
                    std::ptr::null_mut(),
                );
            }
        }
    }
}

/// Scoped libc termios guard: raw mode for single-key reads, cooked mode
/// around line prompts and the editor, and exact restoration on success,
/// error, quit, Ctrl-C, and panic unwind via `Drop`.
struct RawGuard {
    #[cfg(unix)]
    fd: i32,
    #[cfg(unix)]
    original: Option<libc::termios>,
    #[cfg(unix)]
    raw: Option<libc::termios>,
    #[cfg(unix)]
    cooked: bool,
}

impl RawGuard {
    fn enter() -> Self {
        Self::enter_fd(0)
    }

    #[cfg(unix)]
    fn enter_fd(fd: i32) -> Self {
        // `BOB_GCAL_ASSUME_TTY` permits scripted pipes, but termios calls
        // never run on nonterminal descriptors.
        if unsafe { libc::isatty(fd) } != 1 {
            return Self {
                fd,
                original: None,
                raw: None,
                cooked: true,
            };
        }
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `original` is fully written on success before any read.
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Self {
                fd,
                original: None,
                raw: None,
                cooked: true,
            };
        }
        // SAFETY: tcgetattr succeeded, so the value is initialized.
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        raw.c_iflag &= !(libc::BRKINT
            | libc::ICRNL
            | libc::INPCK
            | libc::ISTRIP
            | libc::IXON);
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        raw.c_lflag &= !(libc::ECHO
            | libc::ECHONL
            | libc::ICANON
            | libc::ISIG
            | libc::IEXTEN);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: `raw` derives from the live terminal settings above.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Self {
                fd,
                original: None,
                raw: None,
                cooked: true,
            };
        }
        Self {
            fd,
            original: Some(original),
            raw: Some(raw),
            cooked: false,
        }
    }

    #[cfg(not(unix))]
    fn enter_fd(_fd: i32) -> Self {
        Self {}
    }

    /// Raw mode is active only on a real terminal whose settings applied.
    #[cfg(unix)]
    fn active(&self) -> bool {
        self.original.is_some()
    }

    /// Restore cooked mode around note lines, scope prompts, and the editor.
    fn suspend(&mut self) {
        #[cfg(unix)]
        {
            if self.active() && !self.cooked {
                // SAFETY: restores the exact settings saved at entry.
                unsafe {
                    libc::tcsetattr(
                        self.fd,
                        libc::TCSANOW,
                        self.original.as_ref().expect("saved termios"),
                    );
                }
                self.cooked = true;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self;
        }
    }

    /// Resume raw mode after a cooked section.
    fn resume(&mut self) {
        #[cfg(unix)]
        {
            if self.active() && self.cooked {
                // SAFETY: reapplies the raw settings derived at entry.
                unsafe {
                    libc::tcsetattr(
                        self.fd,
                        libc::TCSANOW,
                        self.raw.as_ref().expect("raw termios"),
                    );
                }
                self.cooked = false;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self;
        }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            if let Some(original) = self.original.as_ref() {
                // SAFETY: restores the exact settings saved at entry; a
                // Drop must not panic, so the result is ignored.
                unsafe {
                    libc::tcsetattr(self.fd, libc::TCSANOW, original);
                }
            }
        }
    }
}

/// One unbuffered byte stream for keys and lines, so buffered stdin
/// read-ahead can never swallow a later action: every key and every cooked
/// line (scope answers, one-line notes, editor confirmations) is read here
/// byte by byte from the same descriptor.
struct SessionInput {
    fd: i32,
    pending: VecDeque<u8>,
    eof: bool,
}

#[derive(Debug)]
enum KeyRead {
    Byte(u8),
    Eof,
    Interrupted,
}

#[derive(Debug)]
enum LineRead {
    Text(String),
    Eof,
    Interrupted,
}

impl SessionInput {
    fn new(fd: i32) -> Self {
        Self {
            fd,
            pending: VecDeque::new(),
            eof: false,
        }
    }

    fn read_byte(&mut self) -> KeyRead {
        if let Some(byte) = self.pending.pop_front() {
            return KeyRead::Byte(byte);
        }
        if self.eof {
            return KeyRead::Eof;
        }
        #[cfg(unix)]
        loop {
            if session_interrupted() {
                return KeyRead::Interrupted;
            }
            let mut byte = 0u8;
            // SAFETY: single-byte unbuffered read from the session fd.
            let read = unsafe {
                libc::read(
                    self.fd,
                    (&mut byte as *mut u8).cast::<libc::c_void>(),
                    1,
                )
            };
            if read == 1 {
                return KeyRead::Byte(byte);
            }
            if read == 0 {
                self.eof = true;
                return KeyRead::Eof;
            }
            let code = std::io::Error::last_os_error().raw_os_error();
            if code == Some(libc::EINTR) {
                continue;
            }
            self.eof = true;
            return KeyRead::Eof;
        }
        #[cfg(not(unix))]
        {
            use std::io::Read as _;
            let mut byte = [0u8; 1];
            match std::io::stdin().lock().read_exact(&mut byte) {
                Ok(()) => KeyRead::Byte(byte[0]),
                Err(_) => {
                    self.eof = true;
                    KeyRead::Eof
                }
            }
        }
    }

    fn unread(&mut self, byte: u8) {
        self.pending.push_front(byte);
    }

    /// Cooked line read: the caller suspends the raw guard first, so the
    /// terminal echoes and buffers the line. Strips one trailing carriage
    /// return for `\r\n` writers.
    fn read_line(&mut self) -> LineRead {
        let mut bytes = Vec::new();
        loop {
            match self.read_byte() {
                KeyRead::Interrupted => return LineRead::Interrupted,
                KeyRead::Eof => {
                    if bytes.is_empty() {
                        return LineRead::Eof;
                    }
                    break;
                }
                KeyRead::Byte(b'\n') => break,
                KeyRead::Byte(byte) => bytes.push(byte),
            }
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        LineRead::Text(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Whether a key is already waiting, without blocking longer than
    /// `timeout_ms`.
    #[cfg(unix)]
    fn has_data(&self, timeout_ms: i32) -> bool {
        if !self.pending.is_empty() {
            return true;
        }
        let mut waiting = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: polls the single session fd with no pointer lifetime.
        let ready = unsafe { libc::poll(&mut waiting, 1, timeout_ms) };
        ready > 0 && (waiting.revents & (libc::POLLIN | libc::POLLHUP)) != 0
    }

    #[cfg(not(unix))]
    fn has_data(&self, _timeout_ms: i32) -> bool {
        !self.pending.is_empty()
    }

    /// Drop a held key's in-flight repeats at a card boundary: only bytes
    /// identical to the processed key that are already available are
    /// discarded, bounded and with a short grace for repeats still arriving.
    /// Separately timed intentional presses arrive after the grace and are
    /// kept, and the first different byte is always pushed back.
    fn drain_repeats(&mut self, key: u8) {
        for _ in 0..64 {
            if !self.pending.is_empty() {
                if self.pending[0] == key {
                    self.pending.pop_front();
                    continue;
                }
                break;
            }
            if !self.has_data(40) {
                break;
            }
            match self.read_byte() {
                KeyRead::Byte(next) if next == key => {}
                KeyRead::Byte(next) => {
                    self.unread(next);
                    break;
                }
                KeyRead::Eof | KeyRead::Interrupted => break,
            }
        }
    }
}

const SESSION_WIDTH: usize = 80;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CardState {
    Pending,
    Skipped,
    Done,
}

struct SessionCard {
    candidate: NoteCandidate,
    state: CardState,
}

#[derive(Default)]
struct SessionTotals {
    acked: usize,
    noted: usize,
    skipped: usize,
    covered: usize,
}

/// What one key did: the card advanced, the card stays up, or the whole
/// session ends now with this exit code (quit, Ctrl-C, EOF, failure).
enum CardOutcome {
    Stay,
    Abort(i32),
}

fn session_writer_hint(config: &GcalConfig) -> String {
    let writer = config.writer().unwrap_or("this host");
    let host = journal::host_label();
    if writer == host {
        "run `bob gcal sync` to apply the row change".to_string()
    } else {
        format!("the row change will land on the next sync on {writer}")
    }
}

fn session_answer_chip(answer: MyAnswer) -> &'static str {
    match answer {
        MyAnswer::Yes => "✓ Yes",
        MyAnswer::Maybe => "~ Maybe",
        MyAnswer::No => "✕ declined",
        MyAnswer::NoReply => "? awaiting reply",
    }
}

fn session_role(candidate: &NoteCandidate) -> &'static str {
    if candidate.event.organizer_is_self {
        "organizer"
    } else {
        "guest"
    }
}

/// The list command's fragile-directives condition: an unanswered task the
/// organizer's description already carries Bob lines for, so `A` adoption
/// is worth offering. Mirrors `list::fragile_directives` without moving it.
fn session_google_carries_bob_lines(event: &GcalEvent) -> bool {
    let directives = &event.description.directives;
    directives.route.is_some()
        || directives.clock_tag.kind()
            != crate::native::clock_tags::ClockKind::None
        || directives.before_set
        || directives.after_set
        || !event.description.buffer_reasons.is_empty()
}

fn session_fragile(candidate: &NoteCandidate) -> bool {
    candidate.inclusion == Inclusion::Task
        && !candidate.event.organizer_or_no_guests
        && candidate.event.note.is_none()
        && session_google_carries_bob_lines(&candidate.event)
}

/// Wrap human-readable card text by terminal columns, retaining every
/// character and hard-wrapping long roots, URLs, and titles as needed.
fn session_wrap(
    text: &str,
    first_prefix: &str,
    continuation: &str,
) -> Vec<String> {
    let mut output = Vec::new();
    let mut line = String::new();
    let mut prefix = first_prefix;
    let mut capacity = SESSION_WIDTH
        .saturating_sub(session_display_width(prefix))
        .max(1);
    let mut used = 0;
    for word in text.split_whitespace() {
        let word_width = session_display_width(word);
        if used > 0 && used + 1 + word_width <= capacity {
            line.push(' ');
            line.push_str(word);
            used += 1 + word_width;
            continue;
        }
        if used > 0 {
            output.push(format!("{prefix}{line}"));
            line.clear();
            used = 0;
            prefix = continuation;
            capacity = SESSION_WIDTH
                .saturating_sub(session_display_width(prefix))
                .max(1);
        }
        if word_width <= capacity {
            line.push_str(word);
            used = word_width;
            continue;
        }
        for character in word.chars() {
            let character_width = character.width().unwrap_or(0);
            if used > 0 && used + character_width > capacity {
                output.push(format!("{prefix}{line}"));
                line.clear();
                used = 0;
                prefix = continuation;
                capacity = SESSION_WIDTH
                    .saturating_sub(session_display_width(prefix))
                    .max(1);
            }
            line.push(character);
            used += character_width;
        }
    }
    if !line.is_empty() || output.is_empty() {
        output.push(format!("{prefix}{line}"));
    }
    output
}

fn session_display_width(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0))
        .sum()
}

fn session_progress(current: usize, total: usize) -> String {
    let label = format!("─ {current} of {total} ─");
    let width = session_display_width(&label);
    if width >= SESSION_WIDTH {
        return label;
    }
    format!("{label}{}", "─".repeat(SESSION_WIDTH - width))
}

fn render_card(
    styler: &crate::native::style::Styler,
    candidate: &NoteCandidate,
    position: usize,
    total: usize,
    vault: &Path,
) -> Result<(), GcalError> {
    let preview = note::write::preview(candidate, vault, false)?;
    eprintln!("{}", session_progress(position, total));
    for line in session_wrap(&candidate.event.title, "", "  ") {
        eprintln!("{}", styler.bold(&line));
    }
    let mut meta = format!(
        "{} {} · from {} · {}",
        candidate.day.format("%a %b %d"),
        candidate.time,
        candidate.organizer_name(),
        session_answer_chip(candidate.event.my_answer),
    );
    if candidate.lands() != "task" {
        meta.push_str(&format!(" · lands as {}", candidate.lands()));
    }
    for line in session_wrap(&meta, "", "  ") {
        eprintln!("{line}");
    }
    eprintln!();
    if let Some(rail) = preview.rail {
        for line in rail.lines() {
            eprintln!("{line}");
        }
    }
    let root = candidate
        .root
        .as_ref()
        .and_then(|root| root.value.as_deref())
        .unwrap_or("no root");
    let root_source = candidate
        .root
        .as_ref()
        .and_then(|root| root.kind.json_label())
        .unwrap_or("none");
    let buffer = candidate.event.buffer.label().unwrap_or_default();
    let root_fact = format!("root {root} ({root_source})");
    for line in session_wrap(&root_fact, "  ", "  ") {
        let rendered = candidate
            .root
            .as_ref()
            .and_then(|root| root.hue)
            .map(|hue| styler.xterm256(hue.xterm(), &line))
            .unwrap_or(line);
        eprintln!("{rendered}");
    }
    let mut facts = vec![
        if buffer.is_empty() {
            "no buffer".to_string()
        } else {
            format!("buffer {buffer}")
        },
        if candidate.clock.off_clock {
            "off-clock".to_string()
        } else {
            "on-clock".to_string()
        },
    ];
    if !preview.bullets.is_empty() {
        facts.push(format!(
            "≡ {} {}",
            preview.bullets.len(),
            if preview.bullets.len() == 1 {
                "bullet"
            } else {
                "bullets"
            },
        ));
    }
    for line in session_wrap(&facts.join(" · "), "  ", "  ") {
        eprintln!("{line}");
    }
    if session_fragile(candidate) {
        for line in session_wrap(
            "⚑ Google description carries Bob lines · press A to adopt",
            "  ",
            "  ",
        ) {
            eprintln!("{line}");
        }
    }
    eprintln!();
    eprintln!("{}", styler.dim("lands as"));
    match preview.row {
        Some(row) => {
            for line in session_wrap(&row, "  ", "  ") {
                eprintln!("{}", styler.dim(&line));
            }
        }
        None => {
            for line in
                session_wrap("(no Event Task row would be written)", "  ", "  ")
            {
                eprintln!("{line}");
            }
        }
    }
    eprintln!();
    Ok(())
}

fn render_keys(fragile: bool) {
    if fragile {
        eprintln!("a ack   n note   e editor   s skip   o open   q quit   ? more   A adopt");
    } else {
        eprintln!(
            "a ack   n note   e editor   s skip   o open   q quit   ? more"
        );
    }
}

fn render_help(fragile: bool) {
    eprintln!("a  ack this event (All events is the default for a repeat)");
    eprintln!("n  note with one line of text (empty returns; This event is the default)");
    eprintln!("e  edit the note in $VISUAL/$EDITOR");
    eprintln!("s  skip this run; the event returns next time");
    eprintln!("o  open the Calendar link in a browser (does not answer)");
    eprintln!("q  quit · Esc and Ctrl-C quit too; answers so far are saved");
    if fragile {
        eprintln!("A  adopt the Google description lines into a note (this card only)");
    }
    eprintln!("?  show these keys (Return does nothing on a card)");
}

enum ScopePick {
    Event,
    Series,
    Cancel,
    Quit(i32),
}

enum Confirm {
    Yes,
    No,
    Quit(i32),
}

struct Session<'a> {
    config: &'a GcalConfig,
    vault: std::path::PathBuf,
    account: String,
    resolver: HomeResolver<DiskHomeSource>,
    guard: RawGuard,
    input: SessionInput,
    cards: Vec<SessionCard>,
    waiting: Vec<AckRow>,
    include_waiting: bool,
    totals: SessionTotals,
    warned: HashSet<String>,
}

impl<'a> Session<'a> {
    fn warn_once(&mut self, warning: &str) {
        if self.warned.insert(warning.to_string()) {
            ui::warn(warning);
        }
    }

    fn remaining(&self) -> usize {
        self.cards
            .iter()
            .filter(|card| card.state != CardState::Done)
            .count()
    }

    fn print_waiting_footer(&self) {
        eprintln!(
            "· {} awaiting your reply in Google Calendar{}",
            self.waiting.len(),
            if self.include_waiting {
                " (included with -a)"
            } else {
                " · include with -a"
            },
        );
    }

    fn position(&self) -> usize {
        self.cards
            .iter()
            .filter(|card| card.state != CardState::Pending)
            .count()
            + 1
    }

    fn print_summary(&self) {
        let totals = &self.totals;
        eprintln!(
            "✓ {} acked · {} noted · {} skipped · {} series-covered",
            totals.acked, totals.noted, totals.skipped, totals.covered,
        );
        let remaining = self.remaining();
        if remaining > 0 {
            if self.totals.skipped > 0 {
                eprintln!(
                    "→ {remaining} remaining · rerun `bob gcal ack` · {} skipped return next run",
                    self.totals.skipped
                );
            } else {
                eprintln!("→ {remaining} remaining · rerun `bob gcal ack`");
            }
        } else {
            eprintln!("✓ queue complete for this window");
        }
        self.print_waiting_footer();
    }

    /// A checked failure: the failed card stays unanswered, earlier answers
    /// stand, the error prints, and the session exits 1.
    fn fail(&self, error: GcalError) -> CardOutcome {
        ui::report_error("ack", &error, "human");
        self.print_summary();
        CardOutcome::Abort(1)
    }

    /// Cooked line read with the raw guard suspended, so the terminal
    /// echoes and buffers the line and the editor sees a normal terminal.
    fn cooked_line(&mut self, prompt: &str) -> LineRead {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        self.guard.suspend();
        let line = self.input.read_line();
        self.guard.resume();
        #[cfg(unix)]
        if session_interrupted() {
            return LineRead::Interrupted;
        }
        line
    }

    fn confirm(&mut self, prompt: &str) -> Confirm {
        match self.cooked_line(prompt) {
            LineRead::Interrupted => Confirm::Quit(130),
            LineRead::Eof => Confirm::No,
            LineRead::Text(line) => {
                let answer = line.trim();
                if answer.is_empty()
                    || answer.eq_ignore_ascii_case("y")
                    || answer.eq_ignore_ascii_case("yes")
                {
                    Confirm::Yes
                } else {
                    Confirm::No
                }
            }
        }
    }

    /// Scope selection on the session's own line reader. `default_all`
    /// selects the ack default; note, editor, and adopt keep This event.
    /// An invalid choice stays on the card instead of consuming later keys.
    fn prompt_scope(
        &mut self,
        candidate: &NoteCandidate,
        default_all: bool,
    ) -> ScopePick {
        eprintln!(
            "{} repeats · {} {} · {} · {}",
            candidate.event.title,
            candidate.day.format("%A"),
            candidate.time,
            session_role(candidate),
            session_answer_chip(candidate.event.my_answer),
        );
        eprintln!(
            "  1  This event   {}{}",
            candidate.day.format("%a %b %d"),
            if default_all { "" } else { " (default)" },
        );
        eprintln!(
            "  2  All events   every {}{}",
            candidate.event.title,
            if default_all { " (default)" } else { "" },
        );
        let prompt = if default_all {
            "Which? [2] › "
        } else {
            "Which? [1] › "
        };
        match self.cooked_line(prompt) {
            LineRead::Interrupted => ScopePick::Quit(130),
            LineRead::Eof => ScopePick::Quit(0),
            LineRead::Text(line) => {
                let choice = line.trim();
                if choice.is_empty() {
                    if default_all {
                        ScopePick::Series
                    } else {
                        ScopePick::Event
                    }
                } else if choice == "1" {
                    ScopePick::Event
                } else if choice == "2" {
                    ScopePick::Series
                } else if choice.eq_ignore_ascii_case("q")
                    || choice.eq_ignore_ascii_case("quit")
                {
                    ScopePick::Cancel
                } else {
                    eprintln!(
                        "invalid scope {choice:?}; enter 1 or 2 (q cancels)"
                    );
                    ScopePick::Cancel
                }
            }
        }
    }

    /// Scope for the note branches. `Err(code)` ends the session with that
    /// exit code after the caller reports saved answers; it never exits
    /// inline, so the terminal and signal guards always restore.
    fn note_scope(
        &mut self,
        candidate: &NoteCandidate,
    ) -> Result<Option<journal::NoteScope>, i32> {
        if !candidate.recurring() {
            return Ok(Some(journal::NoteScope::Event));
        }
        match self.prompt_scope(candidate, false) {
            ScopePick::Event => Ok(Some(journal::NoteScope::Event)),
            ScopePick::Series => Ok(Some(journal::NoteScope::Series)),
            ScopePick::Cancel => Ok(None),
            ScopePick::Quit(code) => Err(code),
        }
    }

    fn related_days(&self, index: usize) -> Vec<chrono::NaiveDate> {
        let current = &self.cards[index].candidate;
        let Some(series) = current.event.series_id.as_deref() else {
            return Vec::new();
        };
        let mut days: Vec<chrono::NaiveDate> = self
            .cards
            .iter()
            .filter(|card| {
                card.candidate.event.key.calendar_id
                    == current.event.key.calendar_id
                    && card.candidate.event.series_id.as_deref() == Some(series)
            })
            .map(|card| card.candidate.day)
            .collect();
        days.sort_unstable();
        days.dedup();
        days
    }

    /// Reload folded evidence and drop queue rows another answer already
    /// covered (for example a series ack covering this run's other
    /// instances). Covered rows count as series-covered, never as answers.
    fn refold(&mut self) -> Result<(), GcalError> {
        let (notes, note_warnings) =
            journal::load_notes(&self.vault).map_err(|error| {
                GcalError::runtime(
                    "journal",
                    format!("read private-note evidence: {error}"),
                )
            })?;
        let (acks, ack_warnings) =
            journal::load_acks(&self.vault).map_err(|error| {
                GcalError::runtime(
                    "journal",
                    format!("read ack receipts: {error}"),
                )
            })?;
        for warning in note_warnings.into_iter().chain(ack_warnings) {
            self.warn_once(&warning);
        }
        let mut covered = 0;
        for card in &mut self.cards {
            if card.state == CardState::Done {
                continue;
            }
            let event = &card.candidate.event;
            let status = acks.status_for(
                &notes,
                &self.account,
                &event.key.calendar_id,
                &event.key.event_id,
                event.series_id.as_deref(),
            );
            if status.state != journal::AckState::Unacked {
                if card.state == CardState::Skipped {
                    self.totals.skipped = self.totals.skipped.saturating_sub(1);
                }
                card.state = CardState::Done;
                covered += 1;
            }
        }
        self.totals.covered += covered;
        let waiting = std::mem::take(&mut self.waiting);
        for mut row in waiting {
            let event = &row.candidate.event;
            let status = acks.status_for(
                &notes,
                &self.account,
                &event.key.calendar_id,
                &event.key.event_id,
                event.series_id.as_deref(),
            );
            if status.state == journal::AckState::Unacked {
                row.status = status;
                self.waiting.push(row);
            }
        }
        Ok(())
    }

    fn persist_ack(
        &mut self,
        index: usize,
        scope: AckScope,
    ) -> Result<(), GcalError> {
        self.refold()?;
        if self.cards[index].state != CardState::Pending {
            eprintln!("this event was already answered during the session");
            return Ok(());
        }
        let candidate = note::clone_candidate(&self.cards[index].candidate);
        let event = &candidate.event;
        let record = NewAck {
            account: self.account.clone(),
            calendar_id: event.key.calendar_id.clone(),
            event_id: (scope == AckScope::Event)
                .then(|| event.key.event_id.clone()),
            series: (scope == AckScope::Series)
                .then(|| event.series_id.clone().unwrap_or_default()),
            scope,
            acked: true,
            via: AckVia::Ack,
            seen: event.ack_seen(),
        };
        journal::append_ack(&self.vault, &journal::host_label(), &record)
            .map_err(|error| {
                GcalError::runtime(
                    "journal",
                    format!("append gcal ack receipt: {error}"),
                )
            })?;
        // Mark the answered card before refolding so series-covered
        // siblings count separately from this manual ack.
        self.cards[index].state = CardState::Done;
        self.totals.acked += 1;
        self.refold()?;
        eprintln!("✓ Acked {} · {} scope", event.title, scope.as_str());
        eprintln!("  {}", session_writer_hint(self.config));
        Ok(())
    }

    fn persist_note(
        &mut self,
        index: usize,
        scope: journal::NoteScope,
        text: &str,
    ) -> Result<bool, GcalError> {
        let candidate = note::clone_candidate(&self.cards[index].candidate);
        let related = self.related_days(index);
        let saved = note::write::save_session_note(
            &candidate,
            &self.vault,
            self.config,
            &self.account,
            scope,
            text,
            &mut self.resolver,
            &related,
        )?;
        for warning in &saved.warnings {
            self.warn_once(warning);
        }
        if saved.unchanged {
            eprintln!("unchanged · nothing was saved");
            return Ok(false);
        }
        self.cards[index].state = CardState::Done;
        self.totals.noted += 1;
        self.refold()?;
        if saved.newly_acked {
            eprintln!(
                "✓ Noted {} · {} scope · acked",
                candidate.event.title,
                scope.as_str(),
            );
        } else {
            eprintln!(
                "✓ Noted {} · {} scope",
                candidate.event.title,
                scope.as_str(),
            );
        }
        eprintln!("  {}", session_writer_hint(self.config));
        Ok(true)
    }

    fn do_ack(&mut self, index: usize) -> CardOutcome {
        let candidate = note::clone_candidate(&self.cards[index].candidate);
        let scope = if !candidate.recurring() {
            AckScope::Event
        } else {
            match self.prompt_scope(&candidate, true) {
                ScopePick::Event => AckScope::Event,
                ScopePick::Series => AckScope::Series,
                ScopePick::Cancel => return CardOutcome::Stay,
                ScopePick::Quit(code) => {
                    self.print_summary();
                    return CardOutcome::Abort(code);
                }
            }
        };
        match self.persist_ack(index, scope) {
            Ok(()) => {
                self.input.drain_repeats(b'a');
                CardOutcome::Stay
            }
            Err(error) => self.fail(error),
        }
    }

    /// Run `$VISUAL`/`$EDITOR` with `seed` above the scissors, mirroring the
    /// note command's edit loop: validation errors keep the draft and offer
    /// another round, while cancel or invalid text never answers the card.
    fn run_editor(
        &mut self,
        candidate: &NoteCandidate,
        mut seed: String,
    ) -> Result<Option<String>, i32> {
        loop {
            self.guard.suspend();
            let edited = note::edit_text(&seed, candidate);
            self.guard.resume();
            #[cfg(unix)]
            if session_interrupted() {
                return Err(130);
            }
            let trimmed = match edited {
                Ok(text) => text.trim().to_string(),
                Err(error) => {
                    eprintln!("could not open the editor: {}", error.message());
                    return Ok(None);
                }
            };
            if trimmed.is_empty() {
                eprintln!("unchanged · nothing was saved");
                return Ok(None);
            }
            let errors = note::write::validation_errors(
                &self.vault,
                candidate.day,
                &trimmed,
            );
            if errors.is_empty() {
                return Ok(Some(trimmed));
            }
            for error in &errors {
                eprintln!("{error}");
            }
            match self.confirm("Edit again? [Y/n] ") {
                Confirm::Yes => {
                    seed = trimmed;
                }
                Confirm::No => return Ok(None),
                Confirm::Quit(code) => return Err(code),
            }
        }
    }

    fn save_after_editor(
        &mut self,
        index: usize,
        scope: journal::NoteScope,
        seed: String,
    ) -> CardOutcome {
        let candidate = note::clone_candidate(&self.cards[index].candidate);
        match self.run_editor(&candidate, seed) {
            Ok(Some(text)) => match self.persist_note(index, scope, &text) {
                Ok(_) => CardOutcome::Stay,
                Err(error) => self.fail(error),
            },
            Ok(None) => CardOutcome::Stay,
            Err(code) => {
                self.print_summary();
                CardOutcome::Abort(code)
            }
        }
    }

    fn do_note_line(&mut self, index: usize) -> CardOutcome {
        let draft = match self.cooked_line("Note › ") {
            LineRead::Interrupted => {
                self.print_summary();
                return CardOutcome::Abort(130);
            }
            LineRead::Eof => {
                self.print_summary();
                return CardOutcome::Abort(0);
            }
            LineRead::Text(line) => line.trim().to_string(),
        };
        if draft.is_empty() {
            eprintln!("unchanged · nothing was saved");
            return CardOutcome::Stay;
        }
        let candidate = note::clone_candidate(&self.cards[index].candidate);
        let scope = match self.note_scope(&candidate) {
            Ok(Some(scope)) => scope,
            Ok(None) => return CardOutcome::Stay,
            Err(code) => {
                self.print_summary();
                return CardOutcome::Abort(code);
            }
        };
        match self.persist_note(index, scope, &draft) {
            Ok(_) => CardOutcome::Stay,
            Err(error) if error.kind() == "invalid_note" => {
                eprintln!("{}", error.message());
                match self.confirm("Open the editor with this draft? [Y/n] ") {
                    Confirm::Yes => self.save_after_editor(index, scope, draft),
                    Confirm::No => CardOutcome::Stay,
                    Confirm::Quit(code) => {
                        self.print_summary();
                        CardOutcome::Abort(code)
                    }
                }
            }
            Err(error) => self.fail(error),
        }
    }

    fn do_editor(&mut self, index: usize) -> CardOutcome {
        let candidate = note::clone_candidate(&self.cards[index].candidate);
        let scope = match self.note_scope(&candidate) {
            Ok(Some(scope)) => scope,
            Ok(None) => return CardOutcome::Stay,
            Err(code) => {
                self.print_summary();
                return CardOutcome::Abort(code);
            }
        };
        let (book, warnings) = match journal::load_notes(&self.vault) {
            Ok(loaded) => loaded,
            Err(error) => {
                return self.fail(GcalError::runtime(
                    "journal",
                    format!("read private-note evidence: {error}"),
                ));
            }
        };
        for warning in &warnings {
            self.warn_once(warning);
        }
        let seed =
            note::write::note_text_for(&candidate, &book, &self.account, scope);
        self.save_after_editor(index, scope, seed)
    }

    fn do_adopt(&mut self, index: usize) -> CardOutcome {
        let candidate = note::clone_candidate(&self.cards[index].candidate);
        if !session_fragile(&candidate) {
            eprintln!("This card has no Google description lines to adopt.");
            return CardOutcome::Stay;
        }
        let adopted = note::adopted_google_text(&candidate.google_event);
        if adopted.trim().is_empty() {
            eprintln!(
                "this event's Google description has no directive or reason lines to adopt"
            );
            return CardOutcome::Stay;
        }
        let scope = match self.note_scope(&candidate) {
            Ok(Some(scope)) => scope,
            Ok(None) => return CardOutcome::Stay,
            Err(code) => {
                self.print_summary();
                return CardOutcome::Abort(code);
            }
        };
        let (book, warnings) = match journal::load_notes(&self.vault) {
            Ok(loaded) => loaded,
            Err(error) => {
                return self.fail(GcalError::runtime(
                    "journal",
                    format!("read private-note evidence: {error}"),
                ));
            }
        };
        for warning in &warnings {
            self.warn_once(warning);
        }
        if !note::write::note_text_for(&candidate, &book, &self.account, scope)
            .trim()
            .is_empty()
        {
            eprintln!(
                "this event already has a private note · edit it with `e`"
            );
            return CardOutcome::Stay;
        }
        let errors = note::write::validation_errors(
            &self.vault,
            candidate.day,
            adopted.trim(),
        );
        if !errors.is_empty() {
            for error in &errors {
                eprintln!("{error}");
            }
            match self.confirm("Open the editor with this draft? [Y/n] ") {
                Confirm::Yes => {
                    return self.save_after_editor(
                        index,
                        scope,
                        adopted.trim().to_string(),
                    );
                }
                Confirm::No => return CardOutcome::Stay,
                Confirm::Quit(code) => {
                    self.print_summary();
                    return CardOutcome::Abort(code);
                }
            }
        }
        match self.persist_note(index, scope, adopted.trim()) {
            Ok(_) => CardOutcome::Stay,
            Err(error) => self.fail(error),
        }
    }

    fn do_open(&mut self, index: usize) -> CardOutcome {
        let candidate = &self.cards[index].candidate;
        let Some(url) = candidate.event.html_link.clone() else {
            eprintln!("this event has no Calendar link to open");
            return CardOutcome::Stay;
        };
        match super::signin::spawn_opener(&url, crate::native::google::GCAL) {
            Ok(_) => eprintln!("↗ Opened {url} in a browser"),
            Err(_) => {
                eprintln!("⚠ couldn't open a browser; use the link above");
                eprintln!("  {url}");
            }
        }
        self.input.drain_repeats(b'o');
        CardOutcome::Stay
    }

    fn do_skip(&mut self, index: usize) -> CardOutcome {
        let title = self.cards[index].candidate.event.title.clone();
        self.cards[index].state = CardState::Skipped;
        self.totals.skipped += 1;
        eprintln!("→ Skipped {title} · returns next run");
        self.input.drain_repeats(b's');
        CardOutcome::Stay
    }
}

fn run_session(
    config: &GcalConfig,
    vault: &Path,
    queue: Queue,
    account: &str,
    include_waiting: bool,
) -> i32 {
    let _interrupts = InterruptGuard::install();
    let styler = crate::native::style::Styler::detect_stderr();
    let mut session = Session {
        config,
        vault: vault.to_path_buf(),
        account: account.to_string(),
        resolver: HomeResolver::new(DiskHomeSource::new(vault)),
        guard: RawGuard::enter(),
        input: SessionInput::new(0),
        cards: queue
            .rows
            .into_iter()
            .map(|row| SessionCard {
                candidate: row.candidate,
                state: CardState::Pending,
            })
            .collect(),
        waiting: queue.waiting,
        include_waiting,
        totals: SessionTotals::default(),
        warned: HashSet::new(),
    };
    for warning in &queue.warnings {
        session.warn_once(warning);
    }
    if !queue.stale.is_empty() {
        eprintln!("Stale acknowledgements · moved since review?");
        for item in &queue.stale {
            eprintln!(
                "  ⚠ {} · {} · {} · ack stale · moved since ack?",
                item.title,
                item.start.as_deref().unwrap_or("unknown time"),
                item.calendar_id,
            );
        }
    }
    let total = session.cards.len();
    loop {
        #[cfg(unix)]
        if session_interrupted() {
            session.print_summary();
            return 130;
        }
        let Some(index) = session
            .cards
            .iter()
            .position(|card| card.state == CardState::Pending)
        else {
            session.print_summary();
            return 0;
        };
        let candidate = note::clone_candidate(&session.cards[index].candidate);
        let fragile = session_fragile(&candidate);
        if let Err(error) = render_card(
            &styler,
            &candidate,
            session.position(),
            total,
            &session.vault,
        ) {
            session.print_summary();
            return ui::report_error("ack", &error, "human");
        }
        render_keys(fragile);
        session.print_waiting_footer();
        match session.input.read_byte() {
            KeyRead::Interrupted => {
                session.print_summary();
                return 130;
            }
            KeyRead::Eof => {
                session.print_summary();
                return 0;
            }
            KeyRead::Byte(b'a') => match session.do_ack(index) {
                CardOutcome::Stay => {}
                CardOutcome::Abort(code) => return code,
            },
            KeyRead::Byte(b'n') => match session.do_note_line(index) {
                CardOutcome::Stay => {}
                CardOutcome::Abort(code) => return code,
            },
            KeyRead::Byte(b'e') => match session.do_editor(index) {
                CardOutcome::Stay => {}
                CardOutcome::Abort(code) => return code,
            },
            KeyRead::Byte(b's') => match session.do_skip(index) {
                CardOutcome::Stay => {}
                CardOutcome::Abort(code) => return code,
            },
            KeyRead::Byte(b'o') => match session.do_open(index) {
                CardOutcome::Stay => {}
                CardOutcome::Abort(code) => return code,
            },
            KeyRead::Byte(b'A') => match session.do_adopt(index) {
                CardOutcome::Stay => {}
                CardOutcome::Abort(code) => return code,
            },
            KeyRead::Byte(b'q') | KeyRead::Byte(0x1b) => {
                session.print_summary();
                return 0;
            }
            KeyRead::Byte(0x03) => {
                session.print_summary();
                return 130;
            }
            KeyRead::Byte(b'?') => {
                render_help(fragile);
            }
            KeyRead::Byte(b'\r') | KeyRead::Byte(b'\n') => {}
            KeyRead::Byte(_) => {
                eprintln!("press ? for the session keys");
            }
        }
        #[cfg(unix)]
        if session_interrupted() {
            session.print_summary();
            return 130;
        }
    }
}

#[cfg(all(test, unix))]
mod session_tests {
    use super::*;

    fn open_pty_pair() -> (i32, i32) {
        // SAFETY: the master fd is closed by the caller; the slave path is
        // a fresh buffer filled by ptsname_r.
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master >= 0, "posix_openpt");
            assert_eq!(libc::grantpt(master), 0, "grantpt");
            assert_eq!(libc::unlockpt(master), 0, "unlockpt");
            let mut name = vec![0 as libc::c_char; 128];
            assert_eq!(
                libc::ptsname_r(master, name.as_mut_ptr(), name.len()),
                0,
                "ptsname_r"
            );
            let path = std::ffi::CStr::from_ptr(name.as_ptr())
                .to_string_lossy()
                .into_owned();
            let path = std::ffi::CString::new(path).expect("slave path");
            let slave =
                libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(slave >= 0, "open slave");
            (master, slave)
        }
    }

    fn close_fd(fd: i32) {
        // SAFETY: closes an owned test fd exactly once.
        unsafe {
            libc::close(fd);
        }
    }

    fn termios_of(fd: i32) -> libc::termios {
        // SAFETY: tcgetattr fully writes `raw` on success, which is asserted.
        unsafe {
            let mut raw: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(fd, &mut raw), 0, "tcgetattr");
            raw
        }
    }

    fn same_settings(left: &libc::termios, right: &libc::termios) -> bool {
        left.c_iflag == right.c_iflag
            && left.c_oflag == right.c_oflag
            && left.c_cflag == right.c_cflag
            && left.c_lflag == right.c_lflag
            && left.c_cc == right.c_cc
    }

    #[test]
    fn card_wrap_preserves_long_words_and_uses_unicode_display_columns() {
        let text = format!(
            "root /projects/{} URL https://example.com/{} wide 界界界界界界 end",
            "root-name-".repeat(10),
            "long-path-segment-".repeat(8),
        );
        let lines = session_wrap(&text, "  ", "  ");
        assert!(lines.len() > 2, "long text should wrap: {lines:?}");
        let wrapped: String = lines
            .iter()
            .map(|line| line.strip_prefix("  ").unwrap_or(line))
            .flat_map(str::chars)
            .filter(|character| !character.is_whitespace())
            .collect();
        let expected: String = text
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        assert_eq!(wrapped, expected);
        assert!(lines.iter().all(|line| session_display_width(line) <= 80));
    }

    #[test]
    fn guard_restores_exact_settings_on_drop() {
        let (master, slave) = open_pty_pair();
        let before = termios_of(slave);
        {
            let mut guard = RawGuard::enter_fd(slave);
            assert!(guard.active());
            let raw = termios_of(slave);
            assert!(!same_settings(&before, &raw), "raw mode differs");
            assert_eq!(raw.c_lflag & libc::ICANON, 0);
            guard.suspend();
            assert!(
                same_settings(&before, &termios_of(slave)),
                "suspend restores cooked mode"
            );
            guard.resume();
            assert_eq!(termios_of(slave).c_lflag & libc::ICANON, 0);
        }
        assert!(
            same_settings(&before, &termios_of(slave)),
            "drop restores the exact settings"
        );
        assert!(
            same_settings(&before, &termios_of(master)),
            "master and slave share the line settings"
        );
        close_fd(master);
        close_fd(slave);
    }

    #[test]
    fn guard_restores_after_panic_unwind() {
        let (master, slave) = open_pty_pair();
        let before = termios_of(slave);
        let result = std::panic::catch_unwind(|| {
            let _guard = RawGuard::enter_fd(slave);
            assert!(_guard.active());
            panic!("simulated session failure");
        });
        assert!(result.is_err());
        assert!(
            same_settings(&before, &termios_of(slave)),
            "unwind still restores the terminal"
        );
        close_fd(master);
        close_fd(slave);
    }

    #[test]
    fn non_terminal_fd_stays_inactive() {
        let (read, write) = {
            let mut fds = [0; 2];
            // SAFETY: pipe fills both fds on success, which is asserted.
            unsafe {
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0, "pipe");
            }
            (fds[0], fds[1])
        };
        {
            let guard = RawGuard::enter_fd(read);
            assert!(!guard.active());
        }
        close_fd(read);
        close_fd(write);
    }

    #[test]
    fn drain_drops_a_burst_but_keeps_timed_presses() {
        let mut fds = [0; 2];
        // SAFETY: pipe fills both fds on success, which is asserted.
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0, "pipe");
        }
        let (read, write) = (fds[0], fds[1]);
        // SAFETY: single writer in this test; the reader drains below.
        unsafe {
            let burst = [b'a', b'a', b'a', b'b'];
            assert_eq!(
                libc::write(
                    write,
                    burst.as_ptr().cast::<libc::c_void>(),
                    burst.len()
                ),
                burst.len() as isize,
                "write burst"
            );
        }
        let mut input = SessionInput::new(read);
        input.drain_repeats(b'a');
        match input.read_byte() {
            KeyRead::Byte(next) => assert_eq!(next, b'b', "kept byte"),
            other => panic!("expected the kept byte, got {other:?}"),
        }
        // A press that arrives after the drain grace is a new intention.
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            // SAFETY: the only writer to this test pipe end.
            unsafe {
                libc::write(write, [b'a'].as_ptr().cast::<libc::c_void>(), 1);
                libc::close(write);
            }
        });
        input.drain_repeats(b'a');
        match input.read_byte() {
            KeyRead::Byte(next) => assert_eq!(next, b'a', "timed press kept"),
            other => panic!("expected the timed press, got {other:?}"),
        }
        match input.read_byte() {
            KeyRead::Eof => {}
            other => panic!("expected EOF, got {other:?}"),
        }
        writer.join().expect("writer thread");
        close_fd(read);
    }
}

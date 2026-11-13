//! Read-only queue and one-shot management for external Event Task receipts.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
};

use chrono::{DateTime, Duration, FixedOffset, Local, NaiveDate};
use serde_json::{json, Value};

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
        Ok(()) => 0,
        Err(error) => ui::report_error("ack", &error, args.error_format()),
    }
}

fn execute(args: &AckArgs) -> Result<(), GcalError> {
    validate_args(args)?;
    let config = GcalConfig::resolve()?;
    let target = selected_id(args).is_some() || !args.query.is_empty();
    if target {
        run_selected(args, &config)
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
    let explicit_walk =
        args.all || args.next_n_days.is_some() || args.day.is_some();
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

fn run_queue(args: &AckArgs, config: &GcalConfig) -> Result<(), GcalError> {
    let window = resolve_window(args)?;
    let vault = args.bob_dir();
    let evidence = load_evidence(&vault)?;
    let queue = fetch_queue(args, config, &vault, window, evidence)?;
    if args.format.is_json() {
        print_queue_json(args, window, &queue, false)
    } else {
        print_queue_human(args, window, &queue)
    }
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
) -> Result<Queue, GcalError> {
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
            if item.inclusion != Inclusion::Task && !included_by_all {
                if event.my_answer == MyAnswer::NoReply {
                    waiting.push(row_from_snapshot(item, status, day, false));
                }
                continue;
            }
            if item.inclusion == Inclusion::Task
                && removed_by_user(
                    &evidence.state,
                    daily.as_deref(),
                    &item,
                    day,
                )
            {
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
    Ok(Queue {
        rows,
        waiting,
        stale,
        warnings: evidence.warnings,
    })
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
    let Some(row) = state
        .get(&(key, day.to_string()))
        .filter(|state| state.row.is_some() && !state.gone)
        .and_then(|state| state.row.as_ref())
    else {
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
        output.push_str("\nOne-key review is not enabled in this command phase; this listing is read-only.\n");
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

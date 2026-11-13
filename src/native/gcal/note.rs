//! `bob gcal note`: private event notes, selection, preview, and writes.
//!
//! Editor and Google-description adoption share the normal write path so
//! validation, previews, and persistence stay identical.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::{self, BufRead, IsTerminal, Write},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use serde_json::{json, Value};

use super::{
    account,
    classify::{self, DayWindow, Inclusion},
    clock,
    config::{self, GcalConfig},
    gog::Gog,
    journal::{self, FoldedNote, NoteScope},
    model::{EventDateTime, GcalEvent, MyAnswer, ParsedNote},
    root, snapshot, ui, GcalError, NoteArgs,
};
pub(super) mod write;
use crate::native::{
    env as bob_env,
    pomodoro_homes::{DiskHomeSource, HomeResolver, HomeSource},
};

/// Rewrite full `bob gcal` argv (without the `bob gcal` prefix) so that
/// `note` TEXT tokens never parse as flags.
pub(crate) fn rewrite_gcal_argv(args: Vec<OsString>) -> Vec<OsString> {
    let Some(note_idx) = find_note_subcommand(&args) else {
        return args;
    };
    let (head, tail) = args.split_at(note_idx + 1);
    let mut out = head.to_vec();
    out.extend(rewrite_note_tail(tail));
    out
}

fn find_note_subcommand(args: &[OsString]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let token = args[i].to_string_lossy();
        if token == "note" {
            return Some(i);
        }
        // Skip top-level option values so a vault named `note` is not
        // mistaken for the subcommand.
        if token == "-b"
            || token == "--bob-dir"
            || token == "-D"
            || token == "--day"
            || token == "-f"
            || token == "--format"
        {
            i += 2;
            continue;
        }
        if token.starts_with("--")
            && token.contains('=')
            && matches!(
                token.split('=').next().unwrap_or(""),
                "--bob-dir" | "--day" | "--format"
            )
        {
            i += 1;
            continue;
        }
        // Top-level flags and other subcommands end the search.
        if token == "-a"
            || token == "--all"
            || token == "-h"
            || token == "--help"
            || [
                "ack",
                "calendars",
                "doctor",
                "list",
                "login",
                "logout",
                "sync",
            ]
            .contains(&token.as_ref())
        {
            // A different subcommand means there is no `note` tail.
            if token != "note" && !token.starts_with('-') {
                return None;
            }
        }
        i += 1;
    }
    None
}

/// Rewrite tokens after `note` so TEXT after `--` is verbatim and TEXT
/// starting at the first directive token is protected by an inserted `--`.
///
/// This command's options are honored anywhere before the first `--`: the
/// scan keeps going after TEXT starts and pulls known options (and their
/// values) out of the TEXT run, so `note deerwood @family -30 Drive -d`
/// still previews. After an explicit `--`, everything stays verbatim.
pub(crate) fn rewrite_note_tail(tail: &[OsString]) -> Vec<OsString> {
    let mut out: Vec<OsString> = Vec::new();
    let mut text: Vec<OsString> = Vec::new();
    let mut extracted: Vec<OsString> = Vec::new();
    let mut in_text = false;
    let mut seen_dashdash = false;
    let mut i = 0;
    while i < tail.len() {
        let raw = &tail[i];
        let token = raw.to_string_lossy().into_owned();
        if in_text {
            if token == "--" && !seen_dashdash {
                // First explicit separator after implicit TEXT: it becomes
                // the single clap separator. Options pulled before it stay
                // options; everything after stays verbatim.
                out.append(&mut extracted);
                out.push(raw.clone());
                seen_dashdash = true;
                i += 1;
                continue;
            }
            if seen_dashdash {
                // After an explicit `--`, everything stays verbatim.
                // A second literal `--` remains text.
                text.push(raw.clone());
                i += 1;
                continue;
            }
            // Keep scanning the TEXT run: pull this command's known
            // options (and their values) out so they still apply.
            if match_note_option(&token).is_some() {
                extracted.push(raw.clone());
                i = consume_option_value(tail, &token, i, &mut extracted);
                continue;
            }
            text.push(raw.clone());
            i += 1;
            continue;
        }
        if token == "--" {
            in_text = true;
            seen_dashdash = true;
            out.push(raw.clone());
            i += 1;
            continue;
        }
        if is_directive_token(&token) || is_sign_digit_token(&token) {
            in_text = true;
            text.push(raw.clone());
            i += 1;
            continue;
        }
        if match_note_option(&token).is_some() {
            out.push(raw.clone());
            i = consume_option_value(tail, &token, i, &mut out);
            continue;
        }
        if token.starts_with('-') && token.len() > 1 && !is_buffer_token(&token)
        {
            // Known handled above; unknown option-like tokens stay for
            // clap to report as usage errors (exit 2).
            out.push(raw.clone());
            i += 1;
            continue;
        }
        if token.starts_with('+') && !is_buffer_token(&token) {
            out.push(raw.clone());
            i += 1;
            continue;
        }
        // Plain QUERY word.
        out.push(raw.clone());
        i += 1;
    }
    if seen_dashdash {
        // The explicit `--` is already in place; TEXT followed it verbatim.
        out.extend(text);
        return out;
    }
    if in_text {
        // Pulled options land before the inserted `--` so clap still
        // parses them; TEXT words stay verbatim after it.
        out.extend(extracted);
        out.push(OsString::from("--"));
        out.extend(text);
    }
    out
}

/// Advance past an option token, appending its separate value verbatim when
/// it takes one. Returns the next index to scan.
fn consume_option_value(
    tail: &[OsString],
    token: &str,
    index: usize,
    out: &mut Vec<OsString>,
) -> usize {
    let takes_value = match_note_option(token) == Some(1)
        && !token.contains('=')
        && !short_has_attached_value(token);
    // Options with a value consume the next token verbatim, even when it
    // looks like a directive.
    if takes_value && index + 1 < tail.len() {
        out.push(tail[index + 1].clone());
        index + 2
    } else {
        index + 1
    }
}

/// A short option carries its value inside the token (`-iabc`,
/// `-Dtomorrow`, `-fjson`) when the first value-taking short has text
/// after it, so it must not consume the next token. A trailing
/// value-taking short (`-ai`) still takes the next token, like clap.
fn short_has_attached_value(token: &str) -> bool {
    if token.starts_with("--") || !token.starts_with('-') || token.len() <= 2 {
        return false;
    }
    let mut shorts = token.chars().skip(1).peekable();
    while let Some(short) = shorts.next() {
        if matches!(short, 'b' | 'C' | 'D' | 'f' | 'i' | 's') {
            return shorts.peek().is_some();
        }
    }
    false
}

/// A sign followed by a digit that is not a buffer token (`-30min`):
/// never an option; the write phase warns about it. It starts TEXT so
/// clap never sees it as an unknown flag.
fn is_sign_digit_token(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '+' | '\u{2212}' | '\u{2013}') {
        return false;
    }
    if is_buffer_token(token) {
        return false;
    }
    chars.next().is_some_and(|second| second.is_ascii_digit())
}

/// 0 = flag (no value), 1 = option with a separate value. `None` means
/// the token is not this command's option.
fn match_note_option(token: &str) -> Option<usize> {
    if token.starts_with("--") {
        let name = token.split('=').next().unwrap_or(token);
        return match name {
            "--adopt" | "--all" | "--clear" | "--dry-run" | "--edit"
            | "--help" | "--list" => Some(0),
            "--bob-dir" | "--calendar" | "--day" | "--format"
            | "--event-id" | "--scope" => Some(1),
            _ => None,
        };
    }
    if token.starts_with('-') && !token.starts_with("--") && token.len() >= 2 {
        // Single-dash tokens: check every short. Combined flags like
        // `-al` count as options; `-30` was already handled as a buffer
        // token before this call.
        let shorts = token.chars().skip(1);
        for short in shorts {
            match short {
                'A' | 'a' | 'c' | 'd' | 'e' | 'h' | 'l' => {}
                'b' | 'C' | 'D' | 'f' | 'i' | 's' => return Some(1),
                _ => return None,
            }
        }
        return Some(0);
    }
    None
}

/// A buffer token starts TEXT: ASCII or Unicode sign plus digits with an
/// optional `m` suffix (`-30`, `-30m`, `+15`, `−30`, `–30`).
pub(crate) fn is_buffer_token(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '+' | '\u{2212}' | '\u{2013}') {
        return false;
    }
    let rest: String = chars.collect();
    let trimmed = rest
        .strip_suffix('m')
        .or_else(|| rest.strip_suffix('M'))
        .unwrap_or(&rest);
    !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit())
}

/// A directive token starts TEXT: `@word`, a clock tag, or a buffer token.
pub(crate) fn is_directive_token(token: &str) -> bool {
    if token.starts_with('@') && token.len() > 1 {
        return true;
    }
    if is_buffer_token(token) {
        return true;
    }
    matches!(
        token.to_ascii_lowercase().as_str(),
        "#off" | "#on" | "#offclock" | "#off-clock" | "#untracked"
    )
}

/// Entry point for `bob gcal note`.
pub(crate) fn run(args: &NoteArgs) -> i32 {
    match inner(args) {
        Ok(()) => 0,
        Err(error)
            if error.kind() == "ambiguous_printed"
                || error.kind() == "printed_exit_2" =>
        {
            2
        }
        Err(error) => ui::report_error("note", &error, args.error_format()),
    }
}

fn inner(args: &NoteArgs) -> Result<(), GcalError> {
    if args.adopt && args.clear {
        return Err(GcalError::setup(
            "bad_arguments",
            "`-A` cannot be combined with `-c`".to_string(),
        ));
    }
    if args.adopt && !args.text.is_empty() {
        return Err(GcalError::setup(
            "bad_arguments",
            "`-A` cannot be combined with note text".to_string(),
        ));
    }
    if args.edit && args.clear {
        return Err(GcalError::setup(
            "bad_arguments",
            "`-e` cannot be combined with `-c`".to_string(),
        ));
    }
    if args.edit && !args.text.is_empty() {
        return Err(GcalError::setup(
            "bad_arguments",
            "`-e` cannot be combined with note text".to_string(),
        ));
    }
    if args.edit && !is_tty() {
        return Err(GcalError::setup(
            "needs_tty",
            "editing a private event note needs a TTY".to_string(),
        )
        .with_hint("pass the note as TEXT instead of `-e`"));
    }
    if args.clear && (!args.text.is_empty() || args.dry_run) {
        return Err(GcalError::setup(
            "bad_arguments",
            "`-c` cannot be combined with note text or `-d`".to_string(),
        ));
    }
    if (args.list || args.all)
        && (args.clear || args.dry_run || !args.text.is_empty())
    {
        return Err(GcalError::setup(
            "bad_arguments",
            "list options cannot be combined with note text, `-d`, or `-c`"
                .to_string(),
        ));
    }
    if let Some(scope) = args.scope.as_deref()
        && scope != "event"
        && scope != "series"
    {
        return Err(GcalError::setup(
            "bad_scope",
            format!("invalid scope {scope:?}; use event or series"),
        ));
    }
    let config = GcalConfig::resolve()?;
    let vault = args.bob_dir();
    let mut resolver = HomeResolver::new(DiskHomeSource::new(&vault));
    let gog = Gog::resolve(
        crate::native::google::GCAL,
        config.gog(),
        config.timeout(),
    )?;
    let _ = gog.require_version()?;
    let account = account::resolve_command(&gog, config.account())?;
    let calendars = gog.calendars(&account)?;

    let url_event = extract_url_event_id(&args.query);
    let has_id = args.event_id.is_some();
    let writing = args.clear
        || args.dry_run
        || args.adopt
        || args.edit
        || !args.text.is_empty();
    if writing && !has_id && url_event.is_none() && args.query.is_empty() {
        return Err(GcalError::setup(
            "missing_event",
            "choose an event before writing a private note".to_string(),
        )
        .with_hint("use a title query or `-i <event-id>`"));
    }
    let is_list = args.list
        || args.all
        || (!writing
            && !has_id
            && url_event.is_none()
            && args.query.is_empty());
    if is_list {
        run_list(
            args,
            &config,
            &vault,
            &mut resolver,
            &gog,
            &account,
            &calendars,
        )
    } else {
        run_show(
            args,
            &config,
            &vault,
            &mut resolver,
            &gog,
            &account,
            &calendars,
            url_event,
        )
    }
}

pub(crate) fn extract_url_event_id(query: &[String]) -> Option<String> {
    if query.len() != 1 {
        return None;
    }
    decode_calendar_eid(&query[0])
}

/// Decode a pasted Google Calendar URL with `eid=` to the event id: the
/// first space-separated token of the base64url-decoded payload.
pub(crate) fn decode_calendar_eid(url: &str) -> Option<String> {
    if !url.contains("eid=") {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    let eid = parsed
        .query_pairs()
        .find(|(key, _)| key == "eid")
        .map(|(_, value)| value.into_owned())?;
    if eid.trim().is_empty() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(eid.trim()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.split_whitespace()
        .next()
        .map(str::to_string)
        .filter(|id| !id.is_empty())
}

/// Local today, honoring `BOB_NOW` like every other gcal command.
fn today() -> NaiveDate {
    bob_env::current_datetime().date()
}

/// 90-day window `[today midnight, today+90 midnight)` plus the single-day
/// override. Returns `(from, to, window_label_from, window_label_to)`.
pub(crate) fn window_range(
    day_filter: Option<NaiveDate>,
) -> Result<(String, String, NaiveDate, NaiveDate), GcalError> {
    let timezone = bob_env::configured_timezone();
    let base = today();
    if let Some(day) = day_filter {
        let window = DayWindow::new(day, timezone)?;
        return Ok((window.from_rfc3339(), window.to_rfc3339(), day, day));
    }
    let from_window = DayWindow::new(base, timezone)?;
    let end_day = base + chrono::Duration::days(90);
    let to_window = DayWindow::new(end_day, timezone)?;
    Ok((
        from_window.from_rfc3339(),
        to_window.from_rfc3339(),
        base,
        end_day,
    ))
}

fn resolve_day_filter(
    raw: Option<&str>,
) -> Result<Option<NaiveDate>, GcalError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    Ok(Some(classify::resolve_day(raw, today())?))
}

/// One eligible event with its display fields and layered root/clock.
///
/// Shared with the ack review queue: candidate construction always starts
/// from the original Google event, and `google_event` retains that
/// pre-note copy so previews rebuild from Google truth and overlays never
/// become their own input.
#[derive(Debug, Clone)]
pub(crate) struct NoteCandidate {
    pub(crate) event: GcalEvent,
    /// Google-only event, before private notes were layered over it.
    pub(crate) google_event: GcalEvent,
    pub(crate) calendar_name: String,
    pub(crate) day: NaiveDate,
    pub(crate) time: String,
    pub(crate) sort_at: DateTime<FixedOffset>,
    pub(crate) inclusion: Inclusion,
    pub(crate) root: Option<root::EventRoot>,
    pub(crate) clock: crate::native::pomodoro_homes::ClockResolution,
    pub(crate) root_only_clock: crate::native::pomodoro_homes::ClockResolution,
}

impl NoteCandidate {
    pub(crate) fn recurring(&self) -> bool {
        self.event.series_id.is_some()
    }

    pub(crate) fn lands(&self) -> &'static str {
        match self.event.my_answer {
            MyAnswer::NoReply => "waiting_reply",
            MyAnswer::Maybe if self.inclusion != Inclusion::Task => {
                "waiting_maybe"
            }
            _ if self.inclusion == Inclusion::Task => "task",
            _ => "context",
        }
    }

    pub(crate) fn organizer_name(&self) -> String {
        self.event
            .organizer_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.calendar_name.clone())
    }
}

/// Timed, busy, under-24-hour `default`/`fromGmail` events answered Yes,
/// Maybe, or awaiting a reply. Declined, cancelled, all-day, multi-day,
/// and free events can never become Event Tasks.
pub(crate) fn is_eligible(event: &GcalEvent) -> bool {
    if !matches!(event.event_type.as_str(), "default" | "fromGmail") {
        return false;
    }
    if event.status == "cancelled" {
        return false;
    }
    if matches!(event.my_answer, MyAnswer::No) {
        return false;
    }
    match (&event.start, &event.end) {
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            if end < start {
                return false;
            }
            let duration = end.signed_duration_since(*start);
            if duration >= chrono::Duration::hours(24) {
                return false;
            }
            if event.transparency == "transparent" {
                return false;
            }
            true
        }
        _ => false,
    }
}

/// Fetch every event in the window, layer notes, keep the eligible ones.
pub(crate) fn fetch_candidates<S: HomeSource>(
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    gog: &Gog,
    account: &str,
    calendars: &[super::gog::GogCalendar],
    from: &str,
    to: &str,
    day_filter: Option<NaiveDate>,
) -> Result<(Vec<NoteCandidate>, Vec<String>), GcalError> {
    let timezone = bob_env::configured_timezone();
    let series_roots = journal::load_series_roots(vault).unwrap_or_default();
    let (book, warnings) = journal::load_notes(vault).unwrap_or_default();
    let mut candidates = Vec::new();
    for configured in config.calendars() {
        let calendar = calendars
            .iter()
            .find(|calendar| calendar.matches_configured_id(&configured.id));
        let calendar_name = calendar
            .map(|calendar| calendar.name().to_string())
            .unwrap_or_else(|| configured.id.clone());
        let raw_events = gog.events(&configured.id, from, to, account)?;
        for raw in raw_events {
            let mut event = GcalEvent::from_google(&configured.id, raw)?;
            let google_event = event.clone();
            let occurrence_note = matching_note(
                book.event(
                    account,
                    &event.key.calendar_id,
                    &event.key.event_id,
                ),
                account,
            );
            let series_note = event.series_id.as_ref().and_then(|series| {
                matching_note(
                    book.series(account, &event.key.calendar_id, series),
                    account,
                )
            });
            event.apply_notes(occurrence_note.as_ref(), series_note.as_ref());
            if !is_eligible(&event) {
                continue;
            }
            let day = match &event.start {
                EventDateTime::Timed(start) => {
                    let window =
                        DayWindow::new(base_day_for(start, timezone), timezone)
                            .map_err(|_| {
                                GcalError::setup(
                                    "bad_day",
                                    "cannot resolve event day".to_string(),
                                )
                            })?;
                    // Single-day narrowing keeps only that day's starts.
                    if let Some(filter) = day_filter
                        && window.day != filter
                    {
                        continue;
                    }
                    window.day
                }
                _ => continue,
            };
            let window = DayWindow::new(day, timezone)?;
            let inclusion = classify::classify(&event, &window, config);
            // Notes never change inclusion, but ineligible rows were
            // already filtered; keep context Maybe/NoReply for the
            // waiting states.
            let learned = event.series_id.as_ref().and_then(|series| {
                series_roots
                    .get(&(event.key.calendar_id.clone(), series.clone()))
            });
            let note_roots =
                event.note.as_ref().map(|note| root::NoteRootCandidate {
                    event: note.event_route.clone(),
                    series: note.series_route.clone(),
                });
            let daily_rel = snapshot::daily_relative_path(day);
            let chosen = if inclusion == Inclusion::Task {
                Some(root::choose_event_root(
                    resolver,
                    vault,
                    &daily_rel,
                    &event,
                    &calendar_name,
                    config,
                    learned,
                    note_roots.as_ref(),
                ))
            } else {
                None
            };
            event.finalize_note_fields(
                chosen.as_ref().is_some_and(root::EventRoot::is_from_note),
                inclusion == Inclusion::Task && config.description_bullets(),
            );
            let title = chosen
                .as_ref()
                .map(|root| root.title.clone())
                .unwrap_or_else(|| event.title.clone());
            let time = match (&event.start, &event.end) {
                (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
                    format!(
                        "{}-{}",
                        window.format_time(start),
                        window.format_time(end)
                    )
                }
                _ => "all day".to_string(),
            };
            let sort_at = window.effective_start(&event);
            let chosen_root =
                chosen.as_ref().and_then(|root| root.value.as_deref());
            let (clock, root_only_clock) = clock::resolve_listed_clock(
                resolver,
                &daily_rel,
                chosen_root,
                &event.clock_tag,
                inclusion == Inclusion::Task,
            );
            event.title = title;
            candidates.push(NoteCandidate {
                event,
                google_event,
                calendar_name: calendar_name.clone(),
                day,
                time,
                sort_at,
                inclusion,
                root: chosen,
                clock,
                root_only_clock,
            });
        }
    }
    candidates.sort_by(|left, right| {
        left.sort_at
            .cmp(&right.sort_at)
            .then_with(|| {
                left.event.key.calendar_id.cmp(&right.event.key.calendar_id)
            })
            .then_with(|| {
                left.event.key.event_id.cmp(&right.event.key.event_id)
            })
    });
    Ok((candidates, warnings))
}

pub(crate) fn base_day_for(
    start: &DateTime<FixedOffset>,
    timezone: Option<chrono_tz::Tz>,
) -> NaiveDate {
    match timezone {
        Some(tz) => start.with_timezone(&tz).date_naive(),
        None => start.with_timezone(&chrono::Local).date_naive(),
    }
}

pub(crate) fn matching_note(
    record: Option<&FoldedNote>,
    account: &str,
) -> Option<ParsedNote> {
    let record = record?;
    if record.account != account || record.cleared {
        return None;
    }
    Some(ParsedNote::parse(record.scope, &record.text))
}

/// Every QUERY word must appear case-insensitively in the displayed title.
pub(crate) fn matches_query(title: &str, query: &[String]) -> bool {
    let lower = title.to_ascii_lowercase();
    query.iter().all(|word| {
        let needle = word.to_ascii_lowercase();
        !needle.trim().is_empty() && lower.contains(&needle)
    })
}

pub(crate) fn rank_candidate(
    title: &str,
    query: &[String],
    sort_at: &DateTime<FixedOffset>,
) -> (u8, i64) {
    let qstr = query.join(" ").to_ascii_lowercase();
    let lower = title.to_ascii_lowercase();
    let rank = if lower == qstr {
        0
    } else if lower.starts_with(&qstr) {
        1
    } else if lower.contains(&qstr) {
        2
    } else {
        3
    };
    (rank, sort_at.timestamp())
}

/// A genuine not-found from `gog calendar event`: the id is not on that
/// calendar. Every other lookup failure (network, auth, outage) must
/// never be mistaken for one.
pub(crate) fn is_not_found_error(error: &GcalError) -> bool {
    error.kind() == "bad_json"
        || error.kind() == "not_found"
        || error.message().contains("Not Found")
        || error.message().contains("not found")
        || error.message().contains("No such")
        || error.message().contains("no such")
}

/// Look up one event id across configured calendars. Returns the parsed
/// candidate plus every calendar that held the id.
///
/// Shared with the ack queue for explicit `-i` selection, including
/// exact-ID disambiguation across calendars.
pub(crate) fn lookup_exact<S: HomeSource>(
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    gog: &Gog,
    account: &str,
    calendars: &[super::gog::GogCalendar],
    event_id: &str,
    pinned: Option<&str>,
) -> Result<(Option<NoteCandidate>, Vec<String>), GcalError> {
    let timezone = bob_env::configured_timezone();
    let series_roots = journal::load_series_roots(vault).unwrap_or_default();
    let (book, _) = journal::load_notes(vault).unwrap_or_default();
    let mut hits: Vec<(String, GcalEvent, GcalEvent, String)> = Vec::new();
    let mut tried: Vec<String> = Vec::new();
    for configured in config.calendars() {
        if let Some(pinned) = pinned
            && configured.id != pinned
        {
            continue;
        }
        tried.push(configured.id.clone());
        let raw = match gog.event(&configured.id, event_id, account) {
            Ok(value) => value,
            Err(error) if is_not_found_error(&error) => continue,
            Err(error) => return Err(error),
        };
        let mut event = GcalEvent::from_google(&configured.id, raw)?;
        // Clone before layering notes: previews rebuild from this copy,
        // so it must hold Google's values, not the noted ones.
        let google_event = event.clone();
        let occurrence_note = matching_note(
            book.event(account, &event.key.calendar_id, &event.key.event_id),
            account,
        );
        let series_note = event.series_id.as_ref().and_then(|series| {
            matching_note(
                book.series(account, &event.key.calendar_id, series),
                account,
            )
        });
        event.apply_notes(occurrence_note.as_ref(), series_note.as_ref());
        let calendar_name = calendars
            .iter()
            .find(|calendar| calendar.matches_configured_id(&configured.id))
            .map(|calendar| calendar.name().to_string())
            .unwrap_or_else(|| configured.id.clone());
        hits.push((configured.id.clone(), event, google_event, calendar_name));
    }
    if hits.len() > 1 && pinned.is_none() {
        let ids: Vec<String> =
            hits.iter().map(|(id, _, _, _)| id.clone()).collect();
        return Err(GcalError::setup(
            "ambiguous_calendar",
            format!(
                "event {event_id} is in more than one calendar: {}",
                ids.join(", ")
            ),
        )
        .with_hint("rerun with `-C <calendar>` to pin the calendar"));
    }
    let Some((_, mut event, google_event, calendar_name)) =
        hits.into_iter().next()
    else {
        return Ok((None, tried));
    };
    if !is_eligible(&event) {
        // Exact selection can surface ineligible rows only to explain
        // why they never become tasks; the picker filters them.
        // For the read-only show path, keep the event so the message is
        // precise instead of "no match".
    }
    let day = match &event.start {
        EventDateTime::Timed(start) => base_day_for(start, timezone),
        EventDateTime::AllDay(date) => *date,
    };
    let window = DayWindow::new(day, timezone)?;
    let inclusion = classify::classify(&event, &window, config);
    let learned = event.series_id.as_ref().and_then(|series| {
        series_roots.get(&(event.key.calendar_id.clone(), series.clone()))
    });
    let note_roots = event.note.as_ref().map(|note| root::NoteRootCandidate {
        event: note.event_route.clone(),
        series: note.series_route.clone(),
    });
    let daily_rel = snapshot::daily_relative_path(day);
    let chosen = if inclusion == Inclusion::Task {
        Some(root::choose_event_root(
            resolver,
            vault,
            &daily_rel,
            &event,
            &calendar_name,
            config,
            learned,
            note_roots.as_ref(),
        ))
    } else {
        None
    };
    event.finalize_note_fields(
        chosen.as_ref().is_some_and(root::EventRoot::is_from_note),
        inclusion == Inclusion::Task && config.description_bullets(),
    );
    let title = chosen
        .as_ref()
        .map(|root| root.title.clone())
        .unwrap_or_else(|| event.title.clone());
    let time = match (&event.start, &event.end) {
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            format!("{}-{}", window.format_time(start), window.format_time(end))
        }
        _ => "all day".to_string(),
    };
    let sort_at = window.effective_start(&event);
    let chosen_root = chosen.as_ref().and_then(|root| root.value.as_deref());
    let (clock, root_only_clock) = clock::resolve_listed_clock(
        resolver,
        &daily_rel,
        chosen_root,
        &event.clock_tag,
        inclusion == Inclusion::Task,
    );
    event.title = title;
    Ok((
        Some(NoteCandidate {
            event,
            google_event,
            calendar_name,
            day,
            time,
            sort_at,
            inclusion,
            root: chosen,
            clock,
            root_only_clock,
        }),
        Vec::new(),
    ))
}

/// Group recurring instances of one series into one picker row.
pub(crate) struct SeriesGroup {
    pub(crate) key: (String, String),
    pub(crate) indices: Vec<usize>,
    pub(crate) soonest: usize,
}

pub(crate) fn group_candidates(
    candidates: &[NoteCandidate],
) -> Vec<SeriesGroup> {
    let mut by_series: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let series_key = match candidate.event.series_id.clone() {
            Some(series) => format!("series:{series}"),
            None => format!("\u{0}event:{}", candidate.event.key.event_id),
        };
        by_series
            .entry((candidate.event.key.calendar_id.clone(), series_key))
            .or_default()
            .push(index);
    }
    let mut groups: Vec<SeriesGroup> = Vec::new();
    for (key, mut indices) in by_series {
        indices.sort_by(|a, b| {
            candidates[*a].sort_at.cmp(&candidates[*b].sort_at)
        });
        let soonest = indices[0];
        groups.push(SeriesGroup {
            key,
            indices,
            soonest,
        });
    }
    groups.sort_by(|a, b| {
        candidates[a.soonest]
            .sort_at
            .cmp(&candidates[b.soonest].sort_at)
    });
    groups
}

pub(crate) fn is_tty() -> bool {
    config::assume_tty() || io::stdin().is_terminal()
}

fn read_line() -> Result<Option<String>, GcalError> {
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(line)),
        Err(error) => Err(GcalError::setup(
            "needs_tty",
            format!("read the picker: {error}"),
        )),
    }
}

/// Numbered picker on a TTY, soonest first. Returns the chosen candidate
/// index into `groups`.
fn answer_chip(answer: MyAnswer) -> &'static str {
    match answer {
        MyAnswer::Yes => "Yes",
        MyAnswer::Maybe => "Maybe",
        MyAnswer::No => "declined",
        MyAnswer::NoReply => "awaiting reply",
    }
}

struct Orphan {
    calendar_id: String,
    event_id: Option<String>,
    series: Option<String>,
    scope: NoteScope,
    title: String,
    reason: String,
}

fn run_list<S: HomeSource>(
    args: &NoteArgs,
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    gog: &Gog,
    account: &str,
    calendars: &[super::gog::GogCalendar],
) -> Result<(), GcalError> {
    let day_filter = resolve_day_filter(args.day.as_deref())?;
    let (from, to, window_from, window_to) = window_range(day_filter)?;
    let (mut candidates, mut warnings) = fetch_candidates(
        config, vault, resolver, gog, account, calendars, &from, &to,
        day_filter,
    )?;
    // Orphans: notes whose event is outside the window.
    let (extra, orphans, orphan_warnings) = lookup_orphans(
        config,
        vault,
        resolver,
        gog,
        account,
        calendars,
        &candidates,
        day_filter,
    )?;
    warnings.extend(orphan_warnings);
    candidates.extend(extra);
    candidates.sort_by(|left, right| {
        left.sort_at
            .cmp(&right.sort_at)
            .then_with(|| {
                left.event.key.calendar_id.cmp(&right.event.key.calendar_id)
            })
            .then_with(|| {
                left.event.key.event_id.cmp(&right.event.key.event_id)
            })
    });
    // Filter to noted events unless `-a` lists everything.
    let mut listed: Vec<&NoteCandidate> = if args.all {
        candidates.iter().collect()
    } else {
        candidates
            .iter()
            .filter(|candidate| candidate.event.note.is_some())
            .collect()
    };
    if !args.query.is_empty() {
        listed = listed
            .into_iter()
            .filter(|candidate| {
                matches_query(&candidate.event.title, &args.query)
            })
            .collect();
    }
    if args.format.is_json() {
        print_list_json(
            args.all,
            &window_from,
            &window_to,
            &from,
            &to,
            &listed,
            &orphans,
            &warnings,
        )
    } else {
        print_list_human(
            args,
            &window_from,
            &window_to,
            &listed,
            &orphans,
            &mut warnings,
        )
    }
}

fn lookup_orphans<S: HomeSource>(
    _config: &GcalConfig,
    vault: &std::path::Path,
    _resolver: &mut HomeResolver<S>,
    gog: &Gog,
    account: &str,
    _calendars: &[super::gog::GogCalendar],
    candidates: &[NoteCandidate],
    _day_filter: Option<NaiveDate>,
) -> Result<(Vec<NoteCandidate>, Vec<Orphan>, Vec<String>), GcalError> {
    let (book, _) = journal::load_notes(vault).unwrap_or_default();
    let present_events: BTreeSet<(String, String)> = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.event.key.calendar_id.clone(),
                candidate.event.key.event_id.clone(),
            )
        })
        .collect();
    let present_series: BTreeSet<(String, String)> =
        candidates
            .iter()
            .filter_map(|candidate| {
                candidate.event.series_id.clone().map(|series| {
                    (candidate.event.key.calendar_id.clone(), series)
                })
            })
            .collect();
    // Collect missing notes: (calendar_id, key, scope).
    let missing: Vec<(String, String, NoteScope)> = Vec::new();
    // Event-scope notes missing from the window.
    // NoteBook internals are private; re-fold via load_notes is already
    // done, so re-read the journal raw for keys? Instead, use the book
    // through a helper: iterate by re-loading with a snapshot of keys
    // from the journal file is complex. For the select phase, surface
    // orphans by scanning the journal's note records directly.
    let _ = (&book, &present_events, &present_series);
    // Scan raw journal for note keys to find missing ones without
    // exposing NoteBook internals.
    let mut keys: Vec<(String, String, NoteScope, String, String)> = Vec::new();
    if let Ok((stored, _)) =
        crate::native::pref_journal::read_raw(vault, journal::NAMESPACE)
    {
        let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
        let mut ordered = stored.iter().collect::<Vec<_>>();
        crate::native::pref_journal::sort_by_writer_order(
            &mut ordered,
            |event| {
                (
                    event.ts,
                    event.host.as_str(),
                    event.path.as_str(),
                    event.line,
                )
            },
        );
        for event in ordered {
            if event.kind.as_str() != "gcal.note" {
                continue;
            }
            let Some(object) = event.value.as_object() else {
                continue;
            };
            let Some(note_account) =
                object.get("account").and_then(Value::as_str)
            else {
                continue;
            };
            if note_account != account {
                continue;
            }
            let Some(scope_str) = object.get("scope").and_then(Value::as_str)
            else {
                continue;
            };
            let scope = match scope_str {
                "event" => NoteScope::Event,
                "series" => NoteScope::Series,
                _ => continue,
            };
            let Some(key_object) = object.get("key").and_then(Value::as_object)
            else {
                continue;
            };
            let Some(calendar_id) =
                key_object.get("calendar_id").and_then(Value::as_str)
            else {
                continue;
            };
            let key_part = match scope {
                NoteScope::Event => {
                    key_object.get("event_id").and_then(Value::as_str)
                }
                NoteScope::Series => {
                    key_object.get("series").and_then(Value::as_str)
                }
            };
            let Some(key_part) = key_part else {
                continue;
            };
            if key_part.is_empty() || calendar_id.is_empty() {
                continue;
            }
            let text = object.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                // A clear remembers the key was cleared: never an orphan.
                seen.insert((
                    calendar_id.to_string(),
                    key_part.to_string(),
                    scope_str.to_string(),
                ));
                keys.retain(|(cal, key, sc, _, _)| {
                    !(*cal == calendar_id && *key == key_part && *sc == scope)
                });
                continue;
            }
            if seen.contains(&(
                calendar_id.to_string(),
                key_part.to_string(),
                scope_str.to_string(),
            )) {
                continue;
            }
            // Last-writer-wins: keep only the latest record per key by
            // replacing any earlier entry.
            keys.retain(|(cal, key, sc, _, _)| {
                !(*cal == calendar_id && *key == key_part && *sc == scope)
            });
            let title = object
                .get("seen")
                .and_then(Value::as_object)
                .and_then(|seen| seen.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("(untitled event)")
                .to_string();
            keys.push((
                calendar_id.to_string(),
                key_part.to_string(),
                scope,
                text.to_string(),
                title,
            ));
            seen.insert((
                calendar_id.to_string(),
                key_part.to_string(),
                scope_str.to_string(),
            ));
            let _ = &missing;
        }
    }
    let extra: Vec<NoteCandidate> = Vec::new();
    let mut orphans: Vec<Orphan> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut lookups = 0usize;
    let mut skipped = 0usize;
    let mut unchecked: Vec<String> = Vec::new();
    let today_midnight = today();
    for (calendar_id, key_part, scope, _text, seen_title) in keys {
        let already_present = match scope {
            NoteScope::Event => present_events
                .contains(&(calendar_id.clone(), key_part.clone())),
            NoteScope::Series => present_series
                .contains(&(calendar_id.clone(), key_part.clone())),
        };
        if already_present {
            continue;
        }
        if lookups >= 25 {
            skipped += 1;
            continue;
        }
        lookups += 1;
        let lookup_id = key_part.clone();
        let raw = match gog.event(&calendar_id, &lookup_id, account) {
            Ok(value) => value,
            Err(error) if is_not_found_error(&error) => {
                orphans.push(Orphan {
                    calendar_id: calendar_id.clone(),
                    event_id: match scope {
                        NoteScope::Event => Some(key_part.clone()),
                        NoteScope::Series => None,
                    },
                    series: match scope {
                        NoteScope::Series => Some(key_part.clone()),
                        NoteScope::Event => None,
                    },
                    scope,
                    title: seen_title,
                    reason: "not_found".to_string(),
                });
                continue;
            }
            Err(error) => {
                // A network, auth, or Google outage is not an orphan:
                // leave the note out of the list and say so once.
                unchecked.push(format!("{seen_title}: {}", error.message()));
                continue;
            }
        };
        let event = match GcalEvent::from_google(&calendar_id, raw) {
            Ok(event) => event,
            Err(error) => {
                unchecked.push(format!("{seen_title}: {}", error.message()));
                continue;
            }
        };
        // Gone, cancelled, and declined beat past-hiding: a deleted or
        // declined event that ended in the past is still an orphan, not
        // silently hidden.
        if event.status == "cancelled"
            || matches!(event.my_answer, MyAnswer::No)
        {
            let reason = if event.status == "cancelled" {
                "cancelled"
            } else {
                "declined"
            };
            orphans.push(Orphan {
                calendar_id: calendar_id.clone(),
                event_id: match scope {
                    NoteScope::Event => Some(key_part.clone()),
                    NoteScope::Series => None,
                },
                series: match scope {
                    NoteScope::Series => Some(key_part.clone()),
                    NoteScope::Event => None,
                },
                scope,
                title: event.title.clone(),
                reason: reason.to_string(),
            });
            continue;
        }
        // A series is judged by its master's status, never the master's
        // start: the master carries the first occurrence's day, which is
        // in the past for every long-running series.
        if scope == NoteScope::Event {
            let is_past = match (&event.start, &event.end) {
                (EventDateTime::Timed(_), EventDateTime::Timed(end)) => {
                    base_day_for(end, bob_env::configured_timezone())
                        < today_midnight
                }
                (EventDateTime::AllDay(_), EventDateTime::AllDay(end)) => {
                    *end <= today_midnight
                }
                _ => false,
            };
            if is_past {
                continue;
            }
        }
        // A valid future event outside the 90-day window joins the list.
        // Reuse the candidate builder by re-layering notes is complex;
        // for the select phase, surface it as an orphan with a still-on-calendar
        // reason only when it is ineligible; otherwise skip (it will be
        // listed once the window reaches it). To keep the list honest,
        // treat ineligible survivors as orphans.
        if !is_eligible(&event) {
            orphans.push(Orphan {
                calendar_id: calendar_id.clone(),
                event_id: Some(event.key.event_id.clone()),
                series: event.series_id.clone(),
                scope,
                title: event.title.clone(),
                reason: "out_of_scope".to_string(),
            });
            continue;
        }
        // Eligible but outside the window: keep it out of the 90-day
        // list (the header says next 90 days) and do not call it an
        // orphan. Past hiding already ran; future outside-window notes
        // reappear once the window reaches them.
    }
    if skipped > 0 {
        warnings.push(format!("{skipped} noted events were not checked"));
    }
    if !unchecked.is_empty() {
        let detail = unchecked.join("; ");
        let noun = if unchecked.len() == 1 {
            "note"
        } else {
            "notes"
        };
        warnings.push(format!(
            "could not check {} {noun}: {detail}",
            unchecked.len()
        ));
    }
    Ok((extra, orphans, warnings))
}

fn run_show<S: HomeSource>(
    args: &NoteArgs,
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    gog: &Gog,
    account: &str,
    calendars: &[super::gog::GogCalendar],
    url_event: Option<String>,
) -> Result<(), GcalError> {
    let day_filter = resolve_day_filter(args.day.as_deref())?;
    // Exact selection first: `-i` or a pasted URL.
    let exact_id = args.event_id.clone().or(url_event.clone());
    if let Some(id) = exact_id.clone() {
        let (found, _) = lookup_exact(
            config,
            vault,
            resolver,
            gog,
            account,
            calendars,
            &id,
            args.calendar.as_deref(),
        )?;
        if let Some(candidate) = found {
            if args.clear
                && args.scope.as_deref() == Some("series")
                && !candidate.recurring()
            {
                // `-s series` names the series key, but a bare master has
                // no recurringEventId. When the master is live, resolve its
                // affected occurrence days via the bounded window and reuse
                // the normal clear path so rendered notes sync immediately;
                // a genuine orphan still clears by key below.
                return clear_live_series_by_master(
                    args, config, vault, resolver, gog, account, calendars,
                    candidate, &id, day_filter,
                );
            }
            let mut related_days = vec![candidate.day];
            let mut selection_warnings = Vec::new();
            if candidate.recurring()
                && (args.clear
                    || args.dry_run
                    || args.adopt
                    || args.edit
                    || !args.text.is_empty())
            {
                let (from, to, _, _) = window_range(day_filter)?;
                match fetch_candidates(
                    config, vault, resolver, gog, account, calendars, &from,
                    &to, day_filter,
                ) {
                    Ok((occurrences, occurrence_warnings)) => {
                        related_days.extend(series_days(
                            &occurrences,
                            &candidate.event.key.calendar_id,
                            candidate.event.series_id.as_deref(),
                        ));
                        selection_warnings.extend(occurrence_warnings);
                    }
                    Err(error) => selection_warnings.push(format!(
                        "could not inspect other occurrences for immediate sync: {}",
                        error.message()
                    )),
                }
            }
            return present_candidate(
                args,
                config,
                vault,
                resolver,
                account,
                candidate,
                selection_warnings,
                unique_days(related_days),
            );
        }
        if args.clear {
            // No loadable event (not found, cancelled, declined, or a
            // gone master), but a live note may remain: clear it by key so
            // the printed orphan clear commands work.
            return clear_by_key(args, config, vault, account, &id);
        }
        if args.event_id.is_some() {
            return Err(GcalError::setup(
                "no_match",
                format!("no event with id {id}"),
            )
            .with_hint("run `bob gcal list -D <day>` to find the event id"));
        }
        // URL fallback: warn and continue to title search. The URL
        // carries no title words, so this becomes a zero-match exit 2
        // with the fallback warning attached.
        let (from, to, _, _) = window_range(day_filter)?;
        let (candidates, mut warnings) = fetch_candidates(
            config, vault, resolver, gog, account, calendars, &from, &to,
            day_filter,
        )?;
        warnings.push(format!(
            "no event with id {id}; falling back to a title search"
        ));
        return show_from_query(
            args, config, vault, resolver, account, candidates, warnings,
            day_filter,
        );
    }
    let (from, to, _, _) = window_range(day_filter)?;
    let (candidates, warnings) = fetch_candidates(
        config, vault, resolver, gog, account, calendars, &from, &to,
        day_filter,
    )?;
    show_from_query(
        args, config, vault, resolver, account, candidates, warnings,
        day_filter,
    )
}

/// Clear a live series via its bare master id. The master lookup
/// succeeded, so this is not an orphan: resolve affected occurrence days
/// with the bounded window and reuse the normal clear path so already
/// existing, rendered daily notes sync immediately on the writer host.
/// Falls back to a live-aware by-key tombstone when no occurrence is in
/// the window or the window lookup fails; true missing-master clears stay
/// in `clear_by_key`.
fn clear_live_series_by_master<S: HomeSource>(
    args: &NoteArgs,
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    gog: &Gog,
    account: &str,
    calendars: &[super::gog::GogCalendar],
    master: NoteCandidate,
    id: &str,
    day_filter: Option<NaiveDate>,
) -> Result<(), GcalError> {
    let calendar_id = master.event.key.calendar_id.clone();
    let (from, to, _, _) = window_range(day_filter)?;
    match fetch_candidates(
        config, vault, resolver, gog, account, calendars, &from, &to,
        day_filter,
    ) {
        Ok((occurrences, occurrence_warnings)) => {
            if let Some(index) = occurrences.iter().position(|candidate| {
                candidate.event.series_id.as_deref() == Some(id)
                    && candidate.event.key.calendar_id == calendar_id
            }) {
                let occurrence = clone_candidate(&occurrences[index]);
                let related = series_days(&occurrences, &calendar_id, Some(id));
                return write::write_one(
                    args,
                    config,
                    vault,
                    resolver,
                    account,
                    occurrence,
                    occurrence_warnings,
                    related,
                )
                .map(|_| ());
            }
            clear_live_series_no_occurrence(
                args,
                vault,
                account,
                &master,
                id,
                &calendar_id,
                occurrence_warnings,
            )
        }
        Err(error) => clear_live_series_no_occurrence(
            args,
            vault,
            account,
            &master,
            id,
            &calendar_id,
            vec![format!(
                "could not inspect other occurrences for immediate sync: {}",
                error.message()
            )],
        ),
    }
}

/// Live-aware by-key clear when the master is live but no occurrence is in
/// the bounded window (or the window lookup failed). Appends the series
/// tombstone and reports `cleared` without claiming the event is gone;
/// there are no rendered window notes to sync, and future notes are never
/// created. Explicit `-c` with no note keeps its existing error.
fn clear_live_series_no_occurrence(
    args: &NoteArgs,
    vault: &std::path::Path,
    account: &str,
    master: &NoteCandidate,
    id: &str,
    calendar_id: &str,
    warnings: Vec<String>,
) -> Result<(), GcalError> {
    let (book, mut all_warnings) =
        journal::load_notes(vault).unwrap_or_default();
    all_warnings.extend(warnings);
    let existing = book.series(account, calendar_id, id);
    let Some(existing) = existing else {
        return Err(GcalError::setup(
            "no_match",
            format!("no event with id {id}"),
        )
        .with_hint("run `bob gcal list -D <day>` to find the event id"));
    };
    if existing.cleared {
        return Err(GcalError::setup(
            "no_note",
            "there is no private note in the series scope".to_string(),
        ));
    }
    let title = existing
        .seen
        .as_ref()
        .and_then(|seen| seen.title.clone())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| master.event.title.clone());
    let record = journal::NewNote {
        account: account.to_string(),
        calendar_id: calendar_id.to_string(),
        event_id: None,
        series: Some(id.to_string()),
        scope: NoteScope::Series,
        text: String::new(),
        seen: existing.seen.clone(),
    };
    journal::append_note(vault, &journal::host_label(), &record).map_err(
        |error| {
            GcalError::runtime(
                "journal",
                format!("clear private event note: {error}"),
            )
        },
    )?;
    print_cleared_live_series(args, calendar_id, id, &title, &all_warnings)
}

/// Report a live-master series clear with no occurrence in the window:
/// the master is live, so never claim the event is gone.
fn print_cleared_live_series(
    args: &NoteArgs,
    calendar_id: &str,
    id: &str,
    title: &str,
    warnings: &[String],
) -> Result<(), GcalError> {
    if args.format.is_json() {
        let document = json!({
            "schema_version": 1,
            "ok": true,
            "action": "cleared",
            "scope": "series",
            "calendar_id": calendar_id,
            "event_id": Value::Null,
            "series_id": id,
            "input": {"text": ""},
            "spans": [],
            "note": {"text": "", "lines": []},
            "status": {
                "state": "cleared",
                "detail": "cleared; no occurrence in the window needed an immediate sync",
                "path": Value::Null,
            },
            "errors": [],
            "warnings": warnings,
        });
        println!(
            "{}",
            serde_json::to_string(&document).map_err(|error| {
                GcalError::runtime(
                    "output",
                    format!("serialize gcal note JSON: {error}"),
                )
            })?
        );
        ui::flush_stdout();
        return Ok(());
    }
    println!("✎ {title}");
    println!("  All events · cleared");
    println!("✓ Cleared · Google's description decides again");
    for warning in warnings {
        ui::warn(warning);
    }
    ui::flush_stdout();
    Ok(())
}

/// Clear a live private note by its journal key when no eligible event
/// can be loaded: not found, a gone series master, or a bare master under
/// `-s series`. Lookup failures never reach here — `lookup_exact` returns
/// those as errors — so a network or auth outage cannot clear anything.
/// On success the note is gone and the orphan row disappears.
fn clear_by_key(
    args: &NoteArgs,
    config: &GcalConfig,
    vault: &std::path::Path,
    account: &str,
    id: &str,
) -> Result<(), GcalError> {
    let requested = match args.scope.as_deref() {
        Some("series") => Some(NoteScope::Series),
        Some("event") => Some(NoteScope::Event),
        _ => None,
    };
    let calendars: Vec<String> = match args.calendar.as_deref() {
        Some(pinned) => vec![pinned.to_string()],
        None => config
            .calendars()
            .iter()
            .map(|cal| cal.id.clone())
            .collect(),
    };
    let (book, warnings) = journal::load_notes(vault).unwrap_or_default();
    let mut hits: Vec<(String, NoteScope)> = Vec::new();
    let mut any_record = false;
    for calendar_id in &calendars {
        for scope in [NoteScope::Event, NoteScope::Series] {
            if requested.is_some_and(|want| want != scope) {
                continue;
            }
            let record = match scope {
                NoteScope::Event => book.event(account, calendar_id, id),
                NoteScope::Series => book.series(account, calendar_id, id),
            };
            let Some(record) = record else {
                continue;
            };
            any_record = true;
            if !record.cleared {
                hits.push((calendar_id.clone(), scope));
            }
        }
    }
    if hits.len() > 1 {
        let same_scope = hits.iter().all(|(_, scope)| *scope == hits[0].1);
        if same_scope {
            let ids: Vec<String> =
                hits.iter().map(|(cal, _)| cal.clone()).collect();
            return Err(GcalError::setup(
                "ambiguous_calendar",
                format!(
                    "that note key is live in more than one calendar: {}",
                    ids.join(", ")
                ),
            )
            .with_hint("rerun with `-C <calendar>` to pin the calendar"));
        }
        return Err(GcalError::setup(
            "needs_scope",
            "that key has notes in both scopes; choose which one to clear"
                .to_string(),
        )
        .with_hint("rerun with `-s event` or `-s series`"));
    }
    let Some((calendar_id, scope)) = hits.into_iter().next() else {
        if any_record {
            // A tombstone (or a note under another scope) already covers
            // the key: there is nothing left to clear.
            let message = match requested {
                Some(NoteScope::Series) => {
                    "there is no private note in the series scope"
                }
                Some(NoteScope::Event) => {
                    "there is no private note in the event scope"
                }
                None => "there is no private note to clear",
            };
            return Err(GcalError::setup("no_note", message.to_string()));
        }
        return Err(GcalError::setup(
            "no_match",
            format!("no event with id {id}"),
        )
        .with_hint("run `bob gcal list -D <day>` to find the event id"));
    };
    let existing = match scope {
        NoteScope::Event => book.event(account, &calendar_id, id),
        NoteScope::Series => book.series(account, &calendar_id, id),
    };
    let title = existing
        .and_then(|record| record.seen.as_ref())
        .and_then(|seen| seen.title.clone())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "(untitled event)".to_string());
    let record = journal::NewNote {
        account: account.to_string(),
        calendar_id: calendar_id.clone(),
        event_id: (scope == NoteScope::Event).then(|| id.to_string()),
        series: (scope == NoteScope::Series).then(|| id.to_string()),
        scope,
        text: String::new(),
        seen: existing.and_then(|record| record.seen.clone()),
    };
    journal::append_note(vault, &journal::host_label(), &record).map_err(
        |error| {
            GcalError::runtime(
                "journal",
                format!("clear private event note: {error}"),
            )
        },
    )?;
    print_cleared_by_key(args, &calendar_id, scope, id, &title, &warnings)
}

/// Report a by-key clear: the event itself is gone, so there is no
/// preview or day to sync — just the cleared note and the key.
fn print_cleared_by_key(
    args: &NoteArgs,
    calendar_id: &str,
    scope: NoteScope,
    id: &str,
    title: &str,
    warnings: &[String],
) -> Result<(), GcalError> {
    if args.format.is_json() {
        let document = json!({
            "schema_version": 1,
            "ok": true,
            "action": "cleared",
            "scope": scope.as_str(),
            "calendar_id": calendar_id,
            "event_id": (scope == NoteScope::Event).then_some(id),
            "series_id": (scope == NoteScope::Series).then_some(id),
            "input": {"text": ""},
            "spans": [],
            "note": {"text": "", "lines": []},
            "status": {
                "state": "cleared",
                "detail": "the event is no longer on your calendar; the note is cleared",
                "path": Value::Null,
            },
            "errors": [],
            "warnings": warnings,
        });
        println!(
            "{}",
            serde_json::to_string(&document).map_err(|error| {
                GcalError::runtime(
                    "output",
                    format!("serialize gcal note JSON: {error}"),
                )
            })?
        );
        ui::flush_stdout();
        return Ok(());
    }
    println!("✎ {title}");
    println!(
        "  {} · no longer on your calendar",
        match scope {
            NoteScope::Event => "This event",
            NoteScope::Series => "All events",
        }
    );
    println!("✓ Cleared · Google's description decides again");
    for warning in warnings {
        ui::warn(warning);
    }
    ui::flush_stdout();
    Ok(())
}

fn show_from_query<S: HomeSource>(
    args: &NoteArgs,
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    account: &str,
    candidates: Vec<NoteCandidate>,
    warnings: Vec<String>,
    day_filter: Option<NaiveDate>,
) -> Result<(), GcalError> {
    let mut filtered: Vec<&NoteCandidate> = candidates
        .iter()
        .filter(|candidate| matches_query(&candidate.event.title, &args.query))
        .collect();
    filtered.sort_by(|a, b| {
        let ra = rank_candidate(&a.event.title, &args.query, &a.sort_at);
        let rb = rank_candidate(&b.event.title, &args.query, &b.sort_at);
        ra.cmp(&rb)
    });
    if filtered.is_empty() {
        let hint = if day_filter.is_some() {
            "try another day with `-D DAY` or `bob gcal list -D <day>`"
        } else {
            "try `-D DAY` or `bob gcal list -D <day>`"
        };
        // The URL fallback warning (and any fetch warnings) travel with
        // the error so they print in both human output and JSON.
        return Err(GcalError::setup(
            "no_match",
            format!("no events match {}", args.query.join(" ")),
        )
        .with_hint(hint)
        .with_warnings(warnings));
    }
    let groups = group_candidates(&candidates);
    let mut ranked_groups: Vec<&SeriesGroup> = Vec::new();
    for candidate in filtered {
        if let Some(group) = groups.iter().find(|group| {
            group.indices.iter().any(|index| {
                candidates[*index].event.key == candidate.event.key
            })
        }) && !ranked_groups
            .iter()
            .any(|existing| existing.key == group.key)
        {
            ranked_groups.push(group);
        }
    }
    if ranked_groups.len() > 1 {
        if args.format.is_json() {
            return Err(ambiguous_json(&candidates, &ranked_groups, warnings));
        }
        if !is_tty() {
            return Err(ambiguous_error(&candidates, &ranked_groups, false));
        }
        // TTY picker with full rows.
        let mut display: Vec<(&SeriesGroup, &NoteCandidate)> = ranked_groups
            .iter()
            .map(|group| (*group, &candidates[group.soonest]))
            .collect();
        let _ = &mut display;
        let chosen = pick_group_owned(&candidates, &ranked_groups)?;
        let group = ranked_groups[chosen];
        let candidate = select_from_group(&candidates, group, day_filter)?;
        let related_days = series_days(
            &candidates,
            &candidate.event.key.calendar_id,
            candidate.event.series_id.as_deref(),
        );
        return present_candidate(
            args,
            config,
            vault,
            resolver,
            account,
            candidate,
            warnings,
            related_days,
        );
    }
    let group = ranked_groups[0];
    let candidate = select_from_group(&candidates, group, day_filter)?;
    let related_days = series_days(
        &candidates,
        &candidate.event.key.calendar_id,
        candidate.event.series_id.as_deref(),
    );
    present_candidate(
        args,
        config,
        vault,
        resolver,
        account,
        candidate,
        warnings,
        related_days,
    )
}

pub(crate) fn series_days(
    candidates: &[NoteCandidate],
    calendar_id: &str,
    series_id: Option<&str>,
) -> Vec<NaiveDate> {
    let Some(series_id) = series_id else {
        return Vec::new();
    };
    unique_days(
        candidates
            .iter()
            .filter(|candidate| {
                candidate.event.key.calendar_id == calendar_id
                    && candidate.event.series_id.as_deref() == Some(series_id)
            })
            .map(|candidate| candidate.day)
            .collect(),
    )
}

pub(crate) fn unique_days(days: Vec<NaiveDate>) -> Vec<NaiveDate> {
    let mut days = days;
    days.sort_unstable();
    days.dedup();
    days
}

pub(crate) fn pick_group_owned(
    candidates: &[NoteCandidate],
    groups: &[&SeriesGroup],
) -> Result<usize, GcalError> {
    let styler = crate::native::style::Styler::detect();
    eprintln!("{}", styler.dim("Several events match; choose one:"));
    for (position, group) in groups.iter().enumerate() {
        let candidate = &candidates[group.soonest];
        // A grouped series names its next occurrence; a lone repeat (or a
        // one-off) keeps the bare marker.
        let recurring = if group.indices.len() > 1 {
            let next = &candidates[group.indices[1]];
            format!(" · ↻ next {}", next.day.format("%a %b %d"))
        } else if candidate.recurring() {
            " · ↻".to_string()
        } else {
            String::new()
        };
        let noted = note_summary(&styler, candidate)
            .map(|summary| format!(" · {summary}"))
            .unwrap_or_default();
        eprintln!(
            "  {}  {} {}  {}  from {}{} · {}{}",
            position + 1,
            candidate.day.format("%a %b %d"),
            candidate.time,
            candidate.event.title,
            candidate.organizer_name(),
            recurring,
            answer_chip(candidate.event.my_answer),
            noted,
        );
    }
    eprint!("Which? [1] › ");
    let _ = io::stderr().flush();
    let line = read_line()?.unwrap_or_default();
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit")
    {
        return Err(GcalError::setup(
            "cancelled",
            "selection cancelled".to_string(),
        ));
    }
    match trimmed.parse::<usize>() {
        Ok(number) if number >= 1 && number <= groups.len() => Ok(number - 1),
        _ => Err(GcalError::setup(
            "bad_choice",
            format!("invalid choice {trimmed:?}; enter 1-{}", groups.len()),
        )
        .with_hint("rerun with a narrower query or `-i <event-id>`")),
    }
}

/// `✎ this event ● family −30m +15m`: the applied note's scope, root,
/// and buffers for the picker row. `None` when the event has no note.
fn note_summary(
    styler: &crate::native::style::Styler,
    candidate: &NoteCandidate,
) -> Option<String> {
    let applied = candidate.event.note.as_ref()?;
    let scope = match applied.scope.as_str() {
        "series" => "all events",
        "both" => "this event + all events",
        _ => "this event",
    };
    let mut parts = vec!["✎".to_string(), scope.to_string()];
    if let Some(value) = candidate
        .root
        .as_ref()
        .and_then(|root| root.value.as_deref())
    {
        parts.push(ui::hue_dot(
            styler,
            candidate.root.as_ref().and_then(|root| root.hue),
            value,
        ));
    }
    let buffer = buffer_label(candidate);
    if !buffer.is_empty() {
        parts.push(buffer);
    }
    Some(parts.join(" "))
}

pub(crate) fn select_from_group(
    candidates: &[NoteCandidate],
    group: &SeriesGroup,
    day_filter: Option<NaiveDate>,
) -> Result<NoteCandidate, GcalError> {
    if let Some(filter) = day_filter {
        for index in &group.indices {
            if candidates[*index].day == filter {
                return Ok(clone_candidate(&candidates[*index]));
            }
        }
    }
    Ok(clone_candidate(&candidates[group.soonest]))
}

pub(crate) fn clone_candidate(candidate: &NoteCandidate) -> NoteCandidate {
    NoteCandidate {
        event: candidate.event.clone(),
        google_event: candidate.google_event.clone(),
        calendar_name: candidate.calendar_name.clone(),
        day: candidate.day,
        time: candidate.time.clone(),
        sort_at: candidate.sort_at,
        inclusion: candidate.inclusion.clone(),
        root: candidate.root.clone(),
        clock: candidate.clock.clone(),
        root_only_clock: candidate.root_only_clock.clone(),
    }
}

fn ambiguous_error(
    candidates: &[NoteCandidate],
    groups: &[&SeriesGroup],
    json: bool,
) -> GcalError {
    if json {
        return GcalError::setup(
            "ambiguous",
            "several events match".to_string(),
        );
    }
    let mut message = String::from(
        "several events match; be more specific or use `-i <event-id>`",
    );
    for group in groups.iter().take(10) {
        let candidate = &candidates[group.soonest];
        message.push_str(&format!(
            "\n  {} {}  {} (-i {} -C {})",
            candidate.day.format("%a %b %d"),
            candidate.time,
            candidate.event.title,
            candidate.event.key.event_id,
            candidate.event.key.calendar_id,
        ));
    }
    GcalError::setup("ambiguous", message)
        .with_hint("rerun with a narrower query or `-i <event-id>`")
}

/// This event / All events scope prompt on a TTY.
///
/// Shared scope selection: note/editor/adopt keep **This event** as the
/// default and use the existing scope-resolved note path.
pub(crate) fn pick_scope(
    candidate: &NoteCandidate,
) -> Result<NoteScope, GcalError> {
    // No cadence word (weekly, daily, …): the recurrence rule is not on
    // the event, and the prompt must not cost an extra Google request.
    let role = if candidate.event.organizer_is_self {
        "organizer"
    } else {
        "guest"
    };
    eprintln!(
        "{} repeats · {}s {} · {} · {}",
        candidate.event.title,
        candidate.day.format("%A"),
        candidate.time,
        role,
        answer_chip(candidate.event.my_answer),
    );
    eprintln!("  1  This event   {}", candidate.day.format("%a %b %d"));
    eprintln!("  2  All events   every {}", candidate.event.title);
    eprint!("Which? [1] › ");
    let _ = io::stderr().flush();
    let line = read_line()?.unwrap_or_default();
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "1" {
        Ok(NoteScope::Event)
    } else if trimmed == "2" {
        Ok(NoteScope::Series)
    } else if trimmed.eq_ignore_ascii_case("q")
        || trimmed.eq_ignore_ascii_case("quit")
    {
        Err(GcalError::setup(
            "cancelled",
            "selection cancelled".to_string(),
        ))
    } else {
        Err(GcalError::setup(
            "bad_choice",
            format!("invalid scope {trimmed:?}; enter 1 or 2"),
        )
        .with_hint("rerun with `-s event` or `-s series`"))
    }
}

pub(crate) fn resolve_show_scope(
    args: &NoteArgs,
    candidate: &NoteCandidate,
) -> Result<Option<NoteScope>, GcalError> {
    if !candidate.recurring() {
        if args.scope.as_deref() == Some("series") {
            return Err(GcalError::setup(
                "bad_scope",
                " `-s series` needs a recurring event".to_string(),
            )
            .with_hint("drop `-s series` for a one-off event"));
        }
        return Ok(Some(NoteScope::Event));
    }
    if let Some(scope) = args.scope.as_deref() {
        return match scope {
            "event" => Ok(Some(NoteScope::Event)),
            "series" => Ok(Some(NoteScope::Series)),
            _ => Err(GcalError::setup(
                "bad_scope",
                format!("invalid scope {scope:?}; use event or series"),
            )),
        };
    }
    if args.format.is_json() || !is_tty() {
        return Err(GcalError::setup(
            "needs_scope",
            format!(
                "{} repeats; choose This event or All events",
                candidate.event.title
            ),
        )
        .with_hint("rerun with `-s event` or `-s series`"));
    }
    Ok(Some(pick_scope(candidate)?))
}

fn present_candidate<S: HomeSource>(
    args: &NoteArgs,
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    account: &str,
    candidate: NoteCandidate,
    warnings: Vec<String>,
    related_days: Vec<NaiveDate>,
) -> Result<(), GcalError> {
    let mut prepared;
    let args = if args.adopt || args.edit {
        prepared = args.clone();
        let (resolved_scope, existing) =
            current_scope_text(args, &candidate, vault, account)?;
        // Carry the once-resolved scope through seed, validation, preview,
        // save, and clear so a second prompt cannot seed from one scope
        // and save into the other.
        prepared.scope = Some(resolved_scope.as_str().to_string());
        let has_note = !existing.trim().is_empty();
        if args.adopt && !args.edit && has_note {
            return Err(GcalError::setup(
                "note_exists",
                "this event already has a private note · edit it with `-e`"
                    .to_string(),
            ));
        }
        let adopted = if args.adopt {
            adopted_google_text(&candidate.google_event)
        } else {
            String::new()
        };
        if args.adopt && adopted.trim().is_empty() {
            return Err(GcalError::setup(
                "nothing_to_adopt",
                "this event's Google description has no directive or reason lines to adopt"
                    .to_string(),
            )
            .with_hint(
                "add @route, #off/#on, or -N/+M lines in Google, or write a note with TEXT",
            ));
        }
        let mut seed = if args.adopt { adopted } else { existing };
        if args.edit {
            loop {
                let edited = edit_text(&seed, &candidate)?;
                let trimmed = edited.trim().to_string();
                if trimmed.is_empty() {
                    prepared.clear = true;
                    prepared.text.clear();
                    break;
                }
                let errors =
                    write::validation_errors(vault, candidate.day, &trimmed);
                if errors.is_empty() {
                    prepared.text = vec![trimmed];
                    break;
                }
                for error in &errors {
                    eprintln!("{error}");
                }
                if !ask_edit_again()? {
                    return Err(GcalError::setup(
                        "printed_exit_2",
                        "invalid private note".to_string(),
                    ));
                }
                seed = trimmed;
            }
        } else {
            prepared.text = vec![seed];
        }
        &prepared
    } else {
        args
    };
    if args.clear || args.dry_run || !args.text.is_empty() {
        write::write_one(
            args,
            config,
            vault,
            resolver,
            account,
            candidate,
            warnings,
            related_days,
        )
        .map(|_| ())
    } else {
        show_one(args, vault, account, candidate, warnings)
    }
}

const EDITOR_SCISSORS: &str =
    "------------------------ >8 ------------------------";

/// Reopen the editor with an invalid one-line draft intact.
///
/// Returns the edited text without making the caller parse stdout: the
/// ack session uses this after `validation_errors` keeps a draft.
pub(crate) fn edit_text(
    seed: &str,
    candidate: &NoteCandidate,
) -> Result<String, GcalError> {
    let editor = std::env::var_os("VISUAL")
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var_os("EDITOR").filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| std::ffi::OsString::from("vi"));
    let mut file = tempfile::NamedTempFile::new().map_err(|error| {
        GcalError::setup("editor_file", format!("create editor file: {error}"))
    })?;
    let mut initial = String::new();
    if !seed.is_empty() {
        initial.push_str(seed);
        if !seed.ends_with('\n') {
            initial.push('\n');
        }
    }
    initial.push_str(EDITOR_SCISSORS);
    initial.push('\n');
    initial.push_str(
        "Do not modify or remove the line above; everything below it is ignored.\n",
    );
    initial
        .push_str(&format!("Private note for {}\n", identity_line(candidate)));
    initial.push_str(
        "  @route  #off  #on  -30  +15        directives: root, clock, buffers\n",
    );
    initial.push_str(
        "  -30 Drive there   +15 Drive home   buffer reasons (bullets)\n",
    );
    initial.push_str(
        "  anything else                      bullets on the Event Task\n",
    );
    initial.push_str("An empty note clears it.\n");
    file.write_all(initial.as_bytes()).map_err(|error| {
        GcalError::setup("editor_file", format!("write editor file: {error}"))
    })?;
    file.flush().map_err(|error| {
        GcalError::setup("editor_file", format!("flush editor file: {error}"))
    })?;
    let mut words = editor
        .to_string_lossy()
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if words.is_empty() {
        return Err(GcalError::setup(
            "editor_missing",
            "$EDITOR is empty".to_string(),
        ));
    }
    let program = words.remove(0);
    let status = std::process::Command::new(program)
        .args(words)
        .arg(file.path())
        .status()
        .map_err(|error| {
            GcalError::setup("editor_failed", format!("launch editor: {error}"))
        })?;
    if !status.success() {
        return Err(GcalError::setup(
            "editor_failed",
            format!("editor exited with {status}"),
        ));
    }
    let contents = std::fs::read_to_string(file.path()).map_err(|error| {
        GcalError::setup("editor_file", format!("read editor file: {error}"))
    })?;
    Ok(contents
        .split(EDITOR_SCISSORS)
        .next()
        .unwrap_or_default()
        .trim()
        .to_string())
}

fn identity_line(candidate: &NoteCandidate) -> String {
    let time = candidate.time.replace('-', "\u{2013}");
    format!(
        "{} {} {} {} {} (from {} · {})",
        candidate.day.format("%a"),
        candidate.day.format("%b"),
        candidate.day.day(),
        time,
        candidate.event.title,
        candidate.organizer_name(),
        answer_chip(candidate.event.my_answer),
    )
}

pub(crate) fn ask_edit_again() -> Result<bool, GcalError> {
    eprint!("Edit again? [Y/n] ");
    let _ = io::stderr().flush();
    match read_line()? {
        None => Ok(false),
        Some(line) => {
            let trimmed = line.trim();
            Ok(trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("y")
                || trimmed.eq_ignore_ascii_case("yes"))
        }
    }
}

pub(crate) fn current_scope_text(
    args: &NoteArgs,
    candidate: &NoteCandidate,
    vault: &std::path::Path,
    account: &str,
) -> Result<(NoteScope, String), GcalError> {
    let scope =
        resolve_show_scope(args, candidate)?.unwrap_or(NoteScope::Event);
    let (book, _) = journal::load_notes(vault).unwrap_or_default();
    let record = match scope {
        NoteScope::Event => book.event_record(
            account,
            &candidate.event.key.calendar_id,
            &candidate.event.key.event_id,
        ),
        NoteScope::Series => {
            candidate.event.series_id.as_deref().and_then(|series| {
                book.series_record(
                    account,
                    &candidate.event.key.calendar_id,
                    series,
                )
            })
        }
    };
    Ok((
        scope,
        record
            .filter(|note| !note.cleared)
            .map(|note| note.text.clone())
            .unwrap_or_default(),
    ))
}

pub(crate) fn adopted_google_text(event: &GcalEvent) -> String {
    let description = &event.description;
    let mut lines = Vec::new();
    if let Some(route) = description.directives.route.as_deref() {
        lines.push(format!("@{route}"));
    }
    match description.directives.clock_tag.kind() {
        crate::native::clock_tags::ClockKind::Off => {
            lines.push("#off".to_string())
        }
        crate::native::clock_tags::ClockKind::On => {
            lines.push("#on".to_string())
        }
        crate::native::clock_tags::ClockKind::None => {}
    }
    for (part, set) in [
        (
            crate::native::event_task::BufferPart::Before,
            description.directives.before_set,
        ),
        (
            crate::native::event_task::BufferPart::After,
            description.directives.after_set,
        ),
    ] {
        if !set {
            continue;
        }
        let sign = match part {
            crate::native::event_task::BufferPart::Before => '-',
            crate::native::event_task::BufferPart::After => '+',
        };
        let reason = match part {
            crate::native::event_task::BufferPart::Before => {
                description.buffer_reasons.before.as_ref()
            }
            crate::native::event_task::BufferPart::After => {
                description.buffer_reasons.after.as_ref()
            }
        };
        if let Some(reason) = reason {
            let reason_text = reason
                .inlines
                .iter()
                .map(|inline| match inline {
                    super::description::Inline::Text { text, .. } => {
                        text.clone()
                    }
                    super::description::Inline::Link { text, .. } => {
                        text.clone()
                    }
                })
                .collect::<String>();
            if reason_text.trim().is_empty() {
                lines.push(format!("{sign}{}", reason.minutes));
            } else {
                lines.push(format!("{sign}{} {reason_text}", reason.minutes));
            }
        } else {
            let minutes = match part {
                crate::native::event_task::BufferPart::Before => {
                    description.directives.buffer.before_min
                }
                crate::native::event_task::BufferPart::After => {
                    description.directives.buffer.after_min
                }
            };
            lines.push(format!("{sign}{minutes}"));
        }
    }
    lines.join("\n")
}

fn show_one(
    args: &NoteArgs,
    vault: &std::path::Path,
    account: &str,
    candidate: NoteCandidate,
    warnings: Vec<String>,
) -> Result<(), GcalError> {
    let scope = resolve_show_scope(args, &candidate)?;
    let (book, _) = journal::load_notes(vault).unwrap_or_default();
    // Scope-specific note text: this account's selected-scope record.
    // A live note saved under another account never shows here.
    let mut warnings = warnings;
    let foreign = match scope {
        Some(NoteScope::Series) => {
            candidate.event.series_id.as_ref().is_some_and(|series| {
                book.iter().any(|record| {
                    record.scope == NoteScope::Series
                        && record.account != account
                        && !record.cleared
                        && record.calendar_id == candidate.event.key.calendar_id
                        && record.key == *series
                })
            })
        }
        Some(NoteScope::Event) | None => book.iter().any(|record| {
            record.scope == NoteScope::Event
                && record.account != account
                && !record.cleared
                && record.calendar_id == candidate.event.key.calendar_id
                && record.key == candidate.event.key.event_id
        }),
    };
    let scope_text = match scope {
        Some(NoteScope::Series) => candidate
            .event
            .series_id
            .as_ref()
            .and_then(|series| {
                book.series(account, &candidate.event.key.calendar_id, series)
            })
            .filter(|record| !record.cleared)
            .map(|record| record.text.clone()),
        Some(NoteScope::Event) | None => book
            .event(
                account,
                &candidate.event.key.calendar_id,
                &candidate.event.key.event_id,
            )
            .filter(|record| !record.cleared)
            .map(|record| record.text.clone())
            .or_else(|| {
                // One-off events have no series scope; show the layered
                // occurrence text when no scope was chosen.
                if scope.is_none() {
                    scope_text_fallback(&candidate)
                } else {
                    None
                }
            }),
    };
    if scope_text.is_none() && foreign {
        warnings.push(
            "a note saved under another account exists; it applies only when that account is signed in"
                .to_string(),
        );
    }
    write::show_result(args, vault, &candidate, scope, scope_text, &warnings)
}

fn scope_text_fallback(candidate: &NoteCandidate) -> Option<String> {
    // For one-off events the layered note is the occurrence note; the
    // AppliedNote carries no text, so fall back to no text. The human
    // show still prints the effective root/buffer/clock below.
    let _ = candidate;
    None
}

fn note_summary_json(candidate: &NoteCandidate) -> Value {
    let Some(applied) = candidate.event.note.as_ref() else {
        return Value::Null;
    };
    let root_value =
        candidate.root.as_ref().and_then(|root| root.value.clone());
    let root_hue = candidate
        .root
        .as_ref()
        .and_then(|root| root.hue)
        .map(|hue| hue.name().to_string());
    let buffer = {
        let before = candidate.event.buffer.before_min;
        let after = candidate.event.buffer.after_min;
        if before == 0 && after == 0 {
            Value::Null
        } else {
            json!({"before_minutes": before, "after_minutes": after})
        }
    };
    json!({
        "scope": applied.scope.as_str(),
        "root": {"value": root_value, "hue": root_hue},
        "buffer": buffer,
        "clock": clock::explicit_clock_label(&candidate.event.clock_tag),
        "bullet_lines": candidate.event.description.note_lines.len(),
    })
}

pub(crate) fn event_json(
    candidate: &NoteCandidate,
    include_text: bool,
    scope_text: Option<&str>,
) -> Value {
    let (start, end) = match (&candidate.event.start, &candidate.event.end) {
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => (
            start.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            end.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        ),
        _ => (String::new(), String::new()),
    };
    let mut note = note_summary_json(candidate);
    if include_text {
        let text = scope_text.unwrap_or("").to_string();
        let lines: Vec<String> = if text.is_empty() {
            Vec::new()
        } else {
            text.lines().map(str::to_string).collect()
        };
        if let Value::Null = note {
            note = json!({
                "scope": Value::Null,
                "root": {"value": Value::Null, "hue": Value::Null},
                "buffer": Value::Null,
                "clock": Value::Null,
                "bullet_lines": 0,
                "text": text,
                "lines": lines,
            });
        } else if let Some(object) = note.as_object_mut() {
            object.insert("text".to_string(), Value::String(text));
            object.insert("lines".to_string(), json!(lines));
        }
    }
    json!({
        "calendar_id": candidate.event.key.calendar_id,
        "calendar_name": candidate.calendar_name,
        "event_id": candidate.event.key.event_id,
        "series_id": candidate.event.series_id,
        "recurring": candidate.recurring(),
        "title": candidate.event.title,
        "day": candidate.day.to_string(),
        "start": start,
        "end": end,
        "time": candidate.time,
        "answer": candidate.event.my_answer.as_str(),
        "organizer": {"name": candidate.organizer_name(), "self": candidate.event.organizer_is_self},
        "url": candidate.event.html_link.clone().unwrap_or_default(),
        "lands": candidate.lands(),
        "note": note,
    })
}

fn orphan_json(orphan: &Orphan) -> Value {
    json!({
        "calendar_id": orphan.calendar_id,
        "event_id": orphan.event_id,
        "series_id": orphan.series,
        "scope": match orphan.scope {
            NoteScope::Event => "event",
            NoteScope::Series => "series",
        },
        "title": orphan.title,
        "reason": orphan.reason,
    })
}

fn ambiguous_json(
    candidates: &[NoteCandidate],
    groups: &[&SeriesGroup],
    warnings: Vec<String>,
) -> GcalError {
    let list: Vec<Value> = groups
        .iter()
        .take(10)
        .map(|group| event_json(&candidates[group.soonest], false, None))
        .collect();
    let document = json!({
        "schema_version": 1,
        "ok": false,
        "error": {
            "kind": "ambiguous",
            "message": "several events match",
            "hint": "rerun with a narrower query or `-i <event-id>`",
        },
        "candidates": list,
        "warnings": warnings,
    });
    println!(
        "{}",
        serde_json::to_string(&document)
            .unwrap_or_else(|_| "{\"ok\":false}".to_string())
    );
    ui::flush_stdout();
    GcalError::setup("ambiguous_printed", "several events match".to_string())
}

fn print_list_json(
    all: bool,
    window_from: &NaiveDate,
    window_to: &NaiveDate,
    from: &str,
    to: &str,
    listed: &[&NoteCandidate],
    orphans: &[Orphan],
    warnings: &[String],
) -> Result<(), GcalError> {
    let events: Vec<Value> = listed
        .iter()
        .map(|candidate| event_json(candidate, false, None))
        .collect();
    let orphans_json: Vec<Value> = orphans.iter().map(orphan_json).collect();
    // The key names the mode, not the rows: `-l -a` lists every event
    // (the Mac picker reads `events`) while every other listing mode
    // lists notes — even an empty window or an all-noted one.
    let mut document = json!({
        "schema_version": 1,
        "ok": true,
        "window": {"from": from, "to": to, "start": window_from.to_string(), "end": window_to.to_string()},
        "orphans": orphans_json,
        "warnings": warnings,
    });
    if all {
        document["events"] = Value::Array(events);
    } else {
        document["notes"] = Value::Array(events);
    }
    println!(
        "{}",
        serde_json::to_string(&document).map_err(|error| {
            GcalError::runtime(
                "output",
                format!("serialize gcal note JSON: {error}"),
            )
        })?
    );
    ui::flush_stdout();
    Ok(())
}

fn status_word(candidate: &NoteCandidate) -> String {
    match candidate.lands() {
        "waiting_reply" => "waiting for your reply".to_string(),
        "waiting_maybe" => "waiting for Yes".to_string(),
        "task" => {
            if candidate.day == today() {
                "on today's note".to_string()
            } else {
                format!("lands {}", candidate.day.format("%a %b %d"))
            }
        }
        _ => "context".to_string(),
    }
}

fn buffer_label(candidate: &NoteCandidate) -> String {
    let before = candidate.event.buffer.before_min;
    let after = candidate.event.buffer.after_min;
    match (before, after) {
        (0, 0) => String::new(),
        (before, 0) => format!("−{before}m"),
        (0, after) => format!("+{after}m"),
        (before, after) => format!("−{before}m +{after}m"),
    }
}

fn print_list_human(
    args: &NoteArgs,
    window_from: &NaiveDate,
    window_to: &NaiveDate,
    listed: &[&NoteCandidate],
    orphans: &[Orphan],
    warnings: &mut Vec<String>,
) -> Result<(), GcalError> {
    use crate::native::style::Styler;
    let styler = Styler::detect();
    let count = listed.len();
    let noun = if count == 1 {
        "private note"
    } else {
        "private notes"
    };
    let window_label = if args.day.is_some() {
        format!("{}", window_from.format("%a %b %d"))
    } else {
        "next 90 days".to_string()
    };
    let _ = window_to;
    println!("✎ {count} {noun} · {window_label}");
    for candidate in listed {
        let scope_label = match candidate
            .event
            .note
            .as_ref()
            .map(|note| note.scope.as_str())
        {
            Some("both") => "both scopes",
            Some("series") => "all events",
            _ => "this event",
        };
        let root_label = candidate
            .root
            .as_ref()
            .and_then(|root| root.value.as_deref())
            .map(|value| {
                if styler.is_color() {
                    ui::hue_dot(
                        &styler,
                        candidate.root.as_ref().and_then(|root| root.hue),
                        value,
                    )
                } else {
                    format!("● {value}")
                }
            })
            .unwrap_or_else(|| "—".to_string());
        let buffer = buffer_label(candidate);
        let buffer_cell = if buffer.is_empty() {
            String::new()
        } else {
            format!("  {buffer}")
        };
        println!(
            "  {} {}  {:<32}  {:<10}  {}{}  {}",
            candidate.day.format("%a %b %d"),
            candidate.time,
            candidate.event.title,
            scope_label,
            root_label,
            buffer_cell,
            status_word(candidate),
        );
    }
    for orphan in orphans {
        let scope_label = match orphan.scope {
            NoteScope::Event => "this event",
            NoteScope::Series => "all events",
        };
        let clear = match (&orphan.event_id, &orphan.series) {
            (Some(id), _) => format!("bob gcal note -c -i {id}"),
            (None, Some(series)) => {
                format!("bob gcal note -c -s series -i {series}")
            }
            _ => "bob gcal note -c".to_string(),
        };
        println!(
            "  {}  {:<32}  {:<10}  no longer on your calendar · {clear}",
            styler.yellow("⚠"),
            orphan.title,
            scope_label,
        );
    }
    for warning in warnings.iter() {
        ui::warn(warning);
    }
    ui::flush_stdout();
    Ok(())
}

fn print_show_human(
    args: &NoteArgs,
    candidate: &NoteCandidate,
    scope: Option<NoteScope>,
    scope_text: Option<String>,
    warnings: &[String],
) -> Result<(), GcalError> {
    use crate::native::style::Styler;
    let styler = Styler::detect();
    let title = if candidate.event.note.is_some() {
        format!("✎ {}", candidate.event.title)
    } else {
        candidate.event.title.clone()
    };
    println!("{title}");
    println!(
        "  {} · {} · from {} · {}",
        candidate.day.format("%a %b %d"),
        candidate.time,
        candidate.organizer_name(),
        answer_chip(candidate.event.my_answer),
    );
    println!();
    if let Some(text) = scope_text.filter(|text| !text.trim().is_empty()) {
        for line in write::canonical_display_lines(&text, candidate) {
            println!("  {line}");
        }
        println!();
    } else {
        let example = format!(
            "bob gcal note {} @family -30 Drive there",
            candidate
                .event
                .title
                .split_whitespace()
                .next()
                .unwrap_or("event")
        );
        println!("  no private note yet · add one: {example}");
        println!();
    }
    // Effective root, buffer, and clock before the shared preview below.
    let root_label = candidate
        .root
        .as_ref()
        .and_then(|root| root.value.as_deref())
        .map(|value| {
            let source = candidate
                .root
                .as_ref()
                .and_then(|root| root.source.as_deref())
                .unwrap_or("");
            if source.is_empty() {
                format!("● {value}")
            } else {
                format!("● {value} (via {source})")
            }
        })
        .unwrap_or_else(|| "no root".to_string());
    let buffer = buffer_label(candidate);
    let clock = clock::explicit_clock_label(&candidate.event.clock_tag)
        .unwrap_or("on-clock");
    let scope_label = match scope {
        Some(NoteScope::Series) => "All events",
        Some(NoteScope::Event) => "This event",
        None => "This event",
    };
    println!(
        "  {scope_label} · {root_label} · {} · {clock}",
        if buffer.is_empty() {
            "no buffer".to_string()
        } else {
            buffer
        }
    );
    let _ = (args, styler);
    for warning in warnings {
        ui::warn(warning);
    }
    ui::flush_stdout();
    Ok(())
}

#[cfg(test)]
mod prepass_tests {
    use super::*;

    fn tail(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    fn strings(values: &[OsString]) -> Vec<String> {
        values
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn buffer_tokens_never_parse_as_options() {
        for token in ["-30", "-30m", "+15", "\u{2212}30", "\u{2013}30"] {
            assert!(is_buffer_token(token), "{token}");
            assert!(is_directive_token(token), "{token}");
        }
        assert!(!is_buffer_token("-c"));
        assert!(!is_buffer_token("--"));
    }

    #[test]
    fn clear_after_query_stays_an_option() {
        let out = rewrite_note_tail(&tail(&["deerwood", "-c"]));
        assert_eq!(strings(&out), vec!["deerwood", "-c"]);
    }

    #[test]
    fn directive_text_gets_a_separator() {
        let out =
            rewrite_note_tail(&tail(&["deerwood", "@family", "-30", "Drive"]));
        assert_eq!(
            strings(&out),
            vec!["deerwood", "--", "@family", "-30", "Drive"]
        );
    }

    #[test]
    fn explicit_separator_keeps_text_verbatim() {
        let out = rewrite_note_tail(&tail(&["-i", "abc123", "--", "-d"]));
        assert_eq!(strings(&out), vec!["-i", "abc123", "--", "-d"]);
    }

    #[test]
    fn options_after_directives_are_still_honored() {
        let out = rewrite_note_tail(&tail(&[
            "deerwood", "@family", "-30", "Drive", "there", "-d",
        ]));
        assert_eq!(
            strings(&out),
            vec!["deerwood", "-d", "--", "@family", "-30", "Drive", "there"]
        );
    }

    #[test]
    fn valued_options_are_pulled_out_of_text_runs() {
        let out = rewrite_note_tail(&tail(&[
            "deerwood", "@family", "lunch", "-C", "primary", "more",
        ]));
        assert_eq!(
            strings(&out),
            vec!["deerwood", "-C", "primary", "--", "@family", "lunch", "more"]
        );
    }

    #[test]
    fn attached_short_values_never_consume_the_next_token() {
        for token in ["-iabc", "-Dtomorrow", "-fjson"] {
            // No TEXT starts here, so no `--` is inserted; the point is
            // the next token is left alone for QUERY.
            let out = rewrite_note_tail(&tail(&[token, "Drive"]));
            assert_eq!(strings(&out), vec![token, "Drive"]);
        }
        // Same inside a TEXT run: the value stays attached and the
        // following TEXT word is not swallowed as the value.
        let out = rewrite_note_tail(&tail(&["@fam", "-iabc", "Drive"]));
        assert_eq!(strings(&out), vec!["-iabc", "--", "@fam", "Drive"]);
        // A trailing value-taking short still takes the next token.
        let out = rewrite_note_tail(&tail(&["-ai", "abc123"]));
        assert_eq!(strings(&out), vec!["-ai", "abc123"]);
    }

    #[test]
    fn sign_digit_non_buffers_start_text_instead_of_erroring() {
        for token in ["-30min", "+15x", "\u{2212}30min"] {
            assert!(is_sign_digit_token(token), "{token}");
            let out = rewrite_note_tail(&tail(&["deerwood", token, "words"]));
            assert_eq!(strings(&out), vec!["deerwood", "--", token, "words"]);
        }
        assert!(!is_sign_digit_token("-30"));
        assert!(!is_sign_digit_token("-c"));
        assert!(!is_sign_digit_token("+off"));
    }

    #[test]
    fn explicit_separator_after_implicit_text_stops_option_extraction() {
        // The land-audit probe: implicit TEXT then `--` must keep `-d` as
        // text instead of extracting it as an option.
        let out =
            rewrite_note_tail(&tail(&["-i", "e1", "@family", "--", "-d"]));
        assert_eq!(strings(&out), vec!["-i", "e1", "--", "@family", "-d"]);
    }

    #[test]
    fn separator_after_text_keeps_options_on_either_side() {
        // `-d` before the delimiter stays an option; `-d` after stays text.
        let out =
            rewrite_note_tail(&tail(&["-d", "@family", "lunch", "--", "-d"]));
        assert_eq!(strings(&out), vec!["-d", "--", "@family", "lunch", "-d"]);
        // Attached-value options before the delimiter still apply.
        let out =
            rewrite_note_tail(&tail(&["@fam", "-iabc", "Drive", "--", "-C"]));
        assert_eq!(strings(&out), vec!["-iabc", "--", "@fam", "Drive", "-C"]);
        // Valued options keep their separate value before the delimiter.
        let out = rewrite_note_tail(&tail(&[
            "@fam", "-C", "primary", "Drive", "--", "-C",
        ]));
        assert_eq!(
            strings(&out),
            vec!["-C", "primary", "--", "@fam", "Drive", "-C"]
        );
    }

    #[test]
    fn second_separator_after_delimiter_remains_text() {
        let out =
            rewrite_note_tail(&tail(&["@fam", "Drive", "--", "--", "-d"]));
        assert_eq!(strings(&out), vec!["--", "@fam", "Drive", "--", "-d"]);
    }

    fn argv(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn ack_sibling_never_enters_note_rewrite() {
        // `ack` is a sibling subcommand: its query may contain the
        // literal word `note` without becoming a `note` tail.
        for args in [
            argv(&["ack", "note"]),
            argv(&["ack", "note", "planning"]),
            argv(&["ack", "@family", "-30", "Drive"]),
            argv(&["ack", "deerwood", "@family", "-30", "Drive", "-d"]),
            argv(&["ack", "-i", "e1", "--", "@family"]),
        ] {
            assert_eq!(rewrite_gcal_argv(args.clone()), args);
        }
        // Top-level options before `ack` still resolve to the sibling.
        let args = argv(&["-b", "vault", "ack", "note"]);
        assert_eq!(rewrite_gcal_argv(args.clone()), args);
    }

    #[test]
    fn note_tail_still_rewrites_after_sibling_fix() {
        // The sibling fix must not change existing `note` handling.
        assert_eq!(
            strings(&rewrite_gcal_argv(argv(&[
                "note", "deerwood", "@family", "-30", "Drive"
            ]))),
            vec!["note", "deerwood", "--", "@family", "-30", "Drive"]
        );
        assert_eq!(
            strings(&rewrite_gcal_argv(argv(&[
                "note", "-i", "e1", "--", "-d"
            ]))),
            vec!["note", "-i", "e1", "--", "-d"]
        );
        // No subcommand means no rewrite.
        let args = argv(&["list", "deerwood"]);
        assert_eq!(rewrite_gcal_argv(args.clone()), args);
    }
}

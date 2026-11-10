//! Private event note normalization, validation, preview, and persistence.

use super::*;
use crate::native::{
    gcal::{
        journal::{NewNote, NoteSeen},
        model::ParsedNote,
        plan,
        snapshot::{DaySnapshot, SnapshotEvent},
    },
    parent_notes,
};
use chrono::Timelike;

#[derive(Debug, Clone)]
struct Span {
    start: usize,
    end: usize,
    kind: &'static str,
}

#[derive(Debug, Clone)]
struct NoteIssue {
    start: usize,
    end: usize,
    kind: &'static str,
    message: String,
    suggestions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Before,
    After,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Self::Before => Self::After,
            Self::After => Self::Before,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }

    fn sign(self) -> char {
        match self {
            Self::Before => '-',
            Self::After => '+',
        }
    }
}

#[derive(Debug, Clone)]
struct BufferValue {
    side: Side,
    minutes: u32,
    reason: Option<String>,
    token: Span,
}

#[derive(Debug, Clone)]
struct RouteValue {
    value: String,
    span: Span,
}

#[derive(Debug, Clone)]
struct NormalizedNote {
    canonical: String,
    spans: Vec<Span>,
    routes: Vec<RouteValue>,
    clocks: Vec<(bool, Span)>,
    buffers: Vec<BufferValue>,
    prose: Vec<(String, Span)>,
    warnings: Vec<NoteIssue>,
    errors: Vec<NoteIssue>,
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    start: usize,
    end: usize,
}

pub(super) fn write_one<S: HomeSource>(
    args: &NoteArgs,
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    account: &str,
    candidate: NoteCandidate,
    mut warnings: Vec<String>,
    related_days: Vec<NaiveDate>,
) -> Result<(), GcalError> {
    let (book, journal_warnings) =
        journal::load_notes(vault).unwrap_or_default();
    warnings.extend(journal_warnings);
    let scope = if args.clear {
        choose_clear_scope(args, &candidate, &book, account)?
    } else {
        choose_write_scope(args, &candidate)?
    };
    let affected_days = if scope == NoteScope::Series {
        let mut days = related_days;
        days.push(candidate.day);
        unique_days(days)
    } else {
        vec![candidate.day]
    };
    let current = note_for_scope(&candidate, &book, account, scope)
        .filter(|note| !note.cleared)
        .map(|note| note.text.clone());

    if args.clear {
        if current.is_none() {
            return Err(GcalError::setup(
                "no_note",
                "there is no private note in that scope to clear".to_string(),
            ));
        }
        let clear_candidate = rebuild_candidate(
            &candidate, config, vault, resolver, account, &book, scope, None,
        )?;
        let rendered = preview(&clear_candidate, vault)?;
        append(&clear_candidate, vault, account, scope, String::new())?;
        let (status, status_detail, status_path, sync_warnings) =
            apply_if_possible(
                config,
                vault,
                &clear_candidate,
                &affected_days,
                scope == NoteScope::Series,
            );
        warnings.extend(sync_warnings);
        return print_result(
            args,
            &clear_candidate,
            scope,
            "cleared",
            "",
            None,
            &status,
            &status_detail,
            status_path.as_deref(),
            &warnings,
            &rendered,
            true,
        );
    }

    let input = if args.text.is_empty() {
        current.clone().unwrap_or_default()
    } else {
        args.text.join(" ")
    };
    let mut note = normalize(&input);
    validate(&mut note, vault, candidate.day);
    if note.canonical.is_empty() {
        note.errors.push(NoteIssue {
            start: 0,
            end: input.len(),
            kind: "empty",
            message: "a private note needs at least one directive, reason, or prose line".to_string(),
            suggestions: Vec::new(),
        });
    }
    warnings
        .extend(note.warnings.iter().map(|warning| warning.message.clone()));
    if note.errors.len() > 0 {
        return invalid_output(
            args, &candidate, scope, &input, &note, warnings,
        );
    }
    let candidate = rebuild_candidate(
        &candidate,
        config,
        vault,
        resolver,
        account,
        &book,
        scope,
        Some(&note.canonical),
    )?;
    // Compute the preview before saving or syncing. In particular, a new
    // row must use the same freshly minted block id that sync will write.
    let rendered = preview(&candidate, vault)?;
    let unchanged = current.as_deref() == Some(note.canonical.as_str());
    let action = if args.dry_run {
        "preview"
    } else if unchanged {
        "unchanged"
    } else {
        append(&candidate, vault, account, scope, note.canonical.clone())?;
        "saved"
    };
    let (status, status_detail, status_path, sync_warnings) =
        if action == "preview" {
            (
                "preview".to_string(),
                "nothing was saved".to_string(),
                None,
                Vec::new(),
            )
        } else if action == "unchanged" {
            (
                "unchanged".to_string(),
                "the canonical note is already saved".to_string(),
                None,
                Vec::new(),
            )
        } else {
            apply_if_possible(
                config,
                vault,
                &candidate,
                &affected_days,
                scope == NoteScope::Series,
            )
        };
    warnings.extend(sync_warnings);
    print_result(
        args,
        &candidate,
        scope,
        action,
        &input,
        Some(&note),
        &status,
        &status_detail,
        status_path.as_deref(),
        &warnings,
        &rendered,
        false,
    )
}

fn note_for_scope<'a>(
    candidate: &NoteCandidate,
    book: &'a journal::NoteBook,
    account: &str,
    scope: NoteScope,
) -> Option<&'a FoldedNote> {
    let record = match scope {
        NoteScope::Event => book.event(
            &candidate.event.key.calendar_id,
            &candidate.event.key.event_id,
        ),
        NoteScope::Series => {
            candidate.event.series_id.as_deref().and_then(|series| {
                book.series(&candidate.event.key.calendar_id, series)
            })
        }
    }?;
    (record.account == account).then_some(record)
}

fn choose_write_scope(
    args: &NoteArgs,
    candidate: &NoteCandidate,
) -> Result<NoteScope, GcalError> {
    if !candidate.recurring() {
        if args.scope.as_deref() == Some("series") {
            return Err(GcalError::setup(
                "bad_scope",
                "`-s series` needs a recurring event".to_string(),
            )
            .with_hint("drop `-s series` for a one-off event"));
        }
        return Ok(NoteScope::Event);
    }
    if let Some(scope) = args.scope.as_deref() {
        return match scope {
            "event" => Ok(NoteScope::Event),
            "series" => Ok(NoteScope::Series),
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
    pick_scope(candidate)
}

fn choose_clear_scope(
    args: &NoteArgs,
    candidate: &NoteCandidate,
    book: &journal::NoteBook,
    account: &str,
) -> Result<NoteScope, GcalError> {
    let event = book
        .event(
            &candidate.event.key.calendar_id,
            &candidate.event.key.event_id,
        )
        .filter(|note| !note.cleared && note.account == account);
    let series = candidate
        .event
        .series_id
        .as_deref()
        .and_then(|series| {
            book.series(&candidate.event.key.calendar_id, series)
        })
        .filter(|note| !note.cleared && note.account == account);
    if !candidate.recurring() {
        if args.scope.as_deref() == Some("series") {
            return Err(GcalError::setup(
                "bad_scope",
                "`-s series` needs a recurring event".to_string(),
            ));
        }
        return Ok(NoteScope::Event);
    }
    let available: Vec<NoteScope> = [
        event.map(|_| NoteScope::Event),
        series.map(|_| NoteScope::Series),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some(scope) = args.scope.as_deref() {
        let selected = match scope {
            "event" => NoteScope::Event,
            "series" => NoteScope::Series,
            _ => {
                return Err(GcalError::setup(
                    "bad_scope",
                    format!("invalid scope {scope:?}; use event or series"),
                ))
            }
        };
        if !available.contains(&selected) {
            return Err(GcalError::setup(
                "no_note",
                format!("there is no private note in the {scope} scope"),
            ));
        }
        return Ok(selected);
    }
    match available.as_slice() {
        [only] => Ok(*only),
        [] => Err(GcalError::setup(
            "no_note",
            "there is no private note to clear".to_string(),
        )),
        _ if args.format.is_json() || !is_tty() => Err(GcalError::setup(
            "needs_scope",
            format!(
                "{} has notes in both scopes; choose which one to clear",
                candidate.event.title
            ),
        )
        .with_hint("rerun with `-s event` or `-s series`")),
        _ => {
            eprintln!(
                "{} repeats · choose the note scope to clear:",
                candidate.event.title
            );
            if event.is_some() {
                eprintln!("  1  This event");
            }
            if series.is_some() {
                eprintln!("  2  All events");
            }
            eprint!("Which? › ");
            let _ = io::stderr().flush();
            let answer = read_line()?.unwrap_or_default();
            match answer.trim() {
                "1" if event.is_some() => Ok(NoteScope::Event),
                "2" if series.is_some() => Ok(NoteScope::Series),
                "q" | "Q" => Err(GcalError::setup(
                    "cancelled",
                    "selection cancelled".to_string(),
                )),
                other => Err(GcalError::setup(
                    "bad_choice",
                    format!("invalid scope {other:?}"),
                )),
            }
        }
    }
}

fn unique_days(mut days: Vec<NaiveDate>) -> Vec<NaiveDate> {
    days.sort_unstable();
    days.dedup();
    days
}

fn append(
    candidate: &NoteCandidate,
    vault: &std::path::Path,
    account: &str,
    scope: NoteScope,
    text: String,
) -> Result<(), GcalError> {
    let event = &candidate.google_event;
    let seen = NoteSeen {
        title: Some(event.title.clone()),
        start: Some(match &event.start {
            EventDateTime::Timed(value) => value.to_rfc3339(),
            EventDateTime::AllDay(value) => value.to_string(),
        }),
        ical_uid: event.ical_uid.clone(),
        original_start: event.original_start.as_ref().map(
            |value| match value {
                EventDateTime::Timed(value) => value.to_rfc3339(),
                EventDateTime::AllDay(value) => value.to_string(),
            },
        ),
    };
    let record = NewNote {
        account: account.to_string(),
        calendar_id: event.key.calendar_id.clone(),
        event_id: (scope == NoteScope::Event)
            .then(|| event.key.event_id.clone()),
        series: (scope == NoteScope::Series)
            .then(|| event.series_id.clone())
            .flatten(),
        scope,
        text,
        seen: Some(seen),
    };
    journal::append_note(vault, &journal::host_label(), &record).map_err(
        |error| {
            GcalError::runtime(
                "journal",
                format!("append private event note: {error}"),
            )
        },
    )?;
    Ok(())
}

fn rebuild_candidate<S: HomeSource>(
    original: &NoteCandidate,
    config: &GcalConfig,
    vault: &std::path::Path,
    resolver: &mut HomeResolver<S>,
    account: &str,
    book: &journal::NoteBook,
    selected_scope: NoteScope,
    replacement: Option<&str>,
) -> Result<NoteCandidate, GcalError> {
    let mut event = original.google_event.clone();
    let mut occurrence = matching_note(
        book.event(&event.key.calendar_id, &event.key.event_id),
        account,
    );
    let mut series = event.series_id.as_ref().and_then(|series_id| {
        matching_note(book.series(&event.key.calendar_id, series_id), account)
    });
    if selected_scope == NoteScope::Event {
        occurrence =
            replacement.map(|text| ParsedNote::parse(NoteScope::Event, text));
    } else {
        series =
            replacement.map(|text| ParsedNote::parse(NoteScope::Series, text));
    }
    event.apply_notes(occurrence.as_ref(), series.as_ref());
    let inclusion = classify::classify(
        &event,
        &DayWindow::new(original.day, bob_env::configured_timezone())?,
        config,
    );
    let learned_roots = journal::load_series_roots(vault).unwrap_or_default();
    let learned = event.series_id.as_ref().and_then(|id| {
        learned_roots.get(&(event.key.calendar_id.clone(), id.clone()))
    });
    let note_roots = event.note.as_ref().map(|note| root::NoteRootCandidate {
        event: note.event_route.clone(),
        series: note.series_route.clone(),
    });
    let daily_rel = snapshot::daily_relative_path(original.day);
    let mut chosen = if inclusion == Inclusion::Task {
        Some(root::choose_event_root(
            resolver,
            vault,
            &daily_rel,
            &event,
            &original.calendar_name,
            config,
            learned,
            note_roots.as_ref(),
        ))
    } else {
        None
    };
    let title = chosen
        .as_ref()
        .map(|root| root.title.clone())
        .unwrap_or_else(|| event.title.clone());
    event.title = title;
    let root_value = chosen.as_ref().and_then(|root| root.value.as_deref());
    let (clock, root_only_clock) = clock::resolve_listed_clock(
        resolver,
        &daily_rel,
        root_value,
        &event.clock_tag,
        inclusion == Inclusion::Task,
    );
    if let Some(root) = chosen.as_mut() {
        root.title = event.title.clone();
    }
    Ok(NoteCandidate {
        event,
        google_event: original.google_event.clone(),
        calendar_name: original.calendar_name.clone(),
        day: original.day,
        time: original.time.clone(),
        sort_at: original.sort_at,
        inclusion,
        root: chosen,
        clock,
        root_only_clock,
    })
}

fn apply_if_possible(
    config: &GcalConfig,
    vault: &std::path::Path,
    candidate: &NoteCandidate,
    affected_days: &[NaiveDate],
    series_scope: bool,
) -> (String, String, Option<String>, Vec<String>) {
    let writer_here = config
        .writer()
        .is_none_or(|writer| writer == journal::host_label());
    let mut warnings = Vec::new();
    let mut failures = Vec::new();
    let mut additional_waits = Vec::new();
    if series_scope && writer_here {
        for day in affected_days
            .iter()
            .copied()
            .filter(|day| *day != candidate.day)
        {
            let path = vault.join(snapshot::daily_relative_path(day));
            if !path.is_file() {
                continue;
            }
            match super::super::sync::apply_note_day(vault, day) {
                Ok((state, path, day_warnings, waiting)) => {
                    warnings.extend(day_warnings);
                    if state == "waiting" {
                        additional_waits.push(waiting.unwrap_or_else(|| {
                            format!("{path} is not ready to sync")
                        }));
                    }
                }
                Err(error) => {
                    failures.push(format!(
                        "{}: {}",
                        snapshot::daily_relative_path(day).display(),
                        error.message()
                    ));
                }
            }
        }
    }
    let mut result = apply_one_if_possible(config, vault, candidate);
    result.3.extend(warnings);
    if !failures.is_empty() {
        let details = failures.join("; ");
        result.0 = "sync_failed".to_string();
        result.1 = format!(
            "saved · immediate sync failed: {details} · run `bob gcal sync`"
        );
        result.3.push(format!("immediate sync failed: {details}"));
    } else if !additional_waits.is_empty() {
        result.1 = format!(
            "{} · other series days: {}",
            result.1,
            additional_waits.join("; ")
        );
    }
    if series_scope {
        result.1 = format!(
            "applies to every {} · {}",
            candidate.event.title, result.1
        );
    }
    result
}

fn apply_one_if_possible(
    config: &GcalConfig,
    vault: &std::path::Path,
    candidate: &NoteCandidate,
) -> (String, String, Option<String>, Vec<String>) {
    let path = vault.join(snapshot::daily_relative_path(candidate.day));
    let shown_path = snapshot::daily_relative_path(candidate.day)
        .display()
        .to_string();
    if candidate.lands() == "waiting_reply" {
        return (
            "waiting_reply".to_string(),
            "you haven't replied: it lands when you say Yes".to_string(),
            None,
            Vec::new(),
        );
    }
    if candidate.lands() == "waiting_maybe" {
        return (
            "waiting_maybe".to_string(),
            "you answered Maybe: it lands when you say Yes".to_string(),
            None,
            Vec::new(),
        );
    }
    if candidate.lands() != "task" {
        return (
            "context".to_string(),
            "this event is context, so it has no Event Task row".to_string(),
            None,
            Vec::new(),
        );
    }
    if config
        .writer()
        .is_some_and(|writer| writer != journal::host_label())
    {
        return (
            "saved_elsewhere".to_string(),
            format!(
                "saved · {shown_path} updates on the next sync on {}",
                config.writer().unwrap_or("the writer")
            ),
            Some(shown_path),
            Vec::new(),
        );
    }
    if !path.is_file() {
        return (
            "waiting_note".to_string(),
            format!("lands in {shown_path} when that day syncs"),
            None,
            Vec::new(),
        );
    }
    match super::super::sync::apply_note_day(vault, candidate.day) {
        Ok((state, path, warnings, waiting)) if state == "waiting" => (
            "waiting_note".to_string(),
            waiting.unwrap_or_else(|| {
                format!("saved · {path} is not ready to sync")
            }),
            Some(path),
            warnings,
        ),
        Ok((state, path, warnings, _))
            if state == "written" || state == "unchanged" =>
        {
            (
                "applied".to_string(),
                format!("↻ applied to {path}"),
                Some(path),
                warnings,
            )
        }
        Ok((state, path, warnings, _)) => (
            "saved_elsewhere".to_string(),
            format!("saved · {path} ({state})"),
            Some(path),
            warnings,
        ),
        Err(error) => (
            "sync_failed".to_string(),
            format!(
                "saved · immediate sync failed: {} · run `bob gcal sync`",
                error.message()
            ),
            Some(shown_path),
            vec![format!("immediate sync failed: {}", error.message())],
        ),
    }
}

fn normalize(input: &str) -> NormalizedNote {
    let mut out = NormalizedNote {
        canonical: String::new(),
        spans: Vec::new(),
        routes: Vec::new(),
        clocks: Vec::new(),
        buffers: Vec::new(),
        prose: Vec::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let mut line_start = 0usize;
    for line in input.split('\n') {
        let clean = line.trim_end_matches('\r');
        if !clean.trim().is_empty() {
            let first = words(clean, line_start).first().cloned();
            let split_semicolons = first
                .as_ref()
                .is_some_and(|token| directive(token.text.as_str()).is_some());
            if split_semicolons {
                let mut offset = 0usize;
                for part in clean.split_inclusive(';') {
                    let has_separator = part.ends_with(';');
                    let segment = if has_separator {
                        &part[..part.len() - 1]
                    } else {
                        part
                    };
                    parse_segment(segment, line_start + offset, &mut out);
                    if has_separator {
                        out.spans.push(Span {
                            start: line_start + offset + segment.len(),
                            end: line_start + offset + part.len(),
                            kind: "separator",
                        });
                    }
                    offset += part.len();
                }
            } else {
                let text = clean.trim().to_string();
                let start = line_start + clean.find(text.as_str()).unwrap_or(0);
                let end = start + text.len();
                if starts_with_unknown_hash(&text) {
                    out.warnings.push(issue(
                        start,
                        end,
                        "unknown_hash",
                        format!(
                            "{} is not a Bob directive; it will be a bullet",
                            text.split_whitespace().next().unwrap_or("#word")
                        ),
                    ));
                }
                if text.split_whitespace().any(|word| {
                    word.starts_with('-')
                        && word.len() > 1
                        && !is_buffer_token(word)
                }) {
                    out.warnings.push(issue(start, end, "option_like", "option-like text may look like a command option; put options before the event query".to_string()));
                }
                out.spans.push(Span {
                    start,
                    end,
                    kind: "prose",
                });
                out.prose.push((
                    text,
                    Span {
                        start,
                        end,
                        kind: "prose",
                    },
                ));
            }
        }
        line_start += line.len() + 1;
    }
    let route = out.routes.first().map(|route| route.value.clone());
    let clock_off = out.clocks.iter().any(|(off, _)| *off);
    let clock_on = out.clocks.iter().any(|(off, _)| !*off);
    if clock_off && clock_on {
        let span = out
            .clocks
            .iter()
            .find(|(off, _)| *off)
            .map(|(_, span)| span.clone())
            .unwrap_or(Span {
                start: 0,
                end: 0,
                kind: "clock",
            });
        out.errors.push(issue(
            span.start,
            span.end,
            "clock_conflict",
            "#off and #on cannot both be set".to_string(),
        ));
    }
    let mut directive_tokens = Vec::new();
    if let Some(route) = route {
        directive_tokens.push(format!("@{route}"));
    }
    if clock_on {
        directive_tokens.push("#on".to_string());
    } else if clock_off {
        directive_tokens.push("#off".to_string());
    }
    for side in [Side::Before, Side::After] {
        let first = out.buffers.iter().find(|value| value.side == side);
        if let Some(value) = first.filter(|value| value.reason.is_none()) {
            directive_tokens.push(format!("{}{}", side.sign(), value.minutes));
        }
    }
    let mut lines = Vec::new();
    if !directive_tokens.is_empty() {
        lines.push(directive_tokens.join(" "));
    }
    for side in [Side::Before, Side::After] {
        if let Some(value) = out
            .buffers
            .iter()
            .find(|value| value.side == side)
            .filter(|value| value.reason.is_some())
        {
            lines.push(format!(
                "{}{} {}",
                side.sign(),
                value.minutes,
                value.reason.as_deref().unwrap_or("")
            ));
        }
    }
    lines.extend(out.prose.iter().map(|(text, _)| text.clone()));
    out.canonical = lines.join("\n");
    out
}

fn words(input: &str, base: usize) -> Vec<Token> {
    let mut result = Vec::new();
    let mut cursor = 0usize;
    for text in input.split_whitespace() {
        let Some(relative) = input[cursor..].find(text) else {
            continue;
        };
        let start = cursor + relative;
        let end = start + text.len();
        result.push(Token {
            text: text.to_string(),
            start: base + start,
            end: base + end,
        });
        cursor = end;
    }
    result
}

fn directive(token: &str) -> Option<(&'static str, Option<(Side, u32)>)> {
    if token.starts_with('@') && token.len() > 1 {
        return Some(("route", None));
    }
    match token.to_ascii_lowercase().as_str() {
        "#off" | "#offclock" | "#off-clock" | "#untracked" => {
            return Some(("clock_off", None))
        }
        "#on" => return Some(("clock_on", None)),
        _ => {}
    }
    if let Some((side, minutes)) = buffer_value(token) {
        return Some(("buffer", Some((side, minutes))));
    }
    None
}

fn buffer_value(token: &str) -> Option<(Side, u32)> {
    let mut chars = token.chars();
    let sign = chars.next()?;
    let rest: String = chars.collect();
    let digits = rest
        .strip_suffix('m')
        .or_else(|| rest.strip_suffix('M'))
        .unwrap_or(&rest);
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let minutes = digits.parse::<u64>().ok()?.min(u64::from(u32::MAX)) as u32;
    Some((
        if sign == '-' || sign == '−' || sign == '–' {
            Side::Before
        } else {
            Side::After
        },
        minutes,
    ))
}

fn parse_segment(input: &str, base: usize, out: &mut NormalizedNote) {
    let tokens = words(input, base);
    if tokens.is_empty() {
        return;
    }
    let mut cursor = 0usize;
    loop {
        if cursor >= tokens.len() {
            return;
        }
        let begin = cursor;
        let mut last_buffer: Option<(Side, u32, Span)> = None;
        while cursor < tokens.len() {
            let token = &tokens[cursor];
            let Some((kind, buffer)) = directive(&token.text) else {
                break;
            };
            let span = Span {
                start: token.start,
                end: token.end,
                kind,
            };
            match kind {
                "route" => {
                    let value = token.text.trim_start_matches('@').to_string();
                    out.routes.push(RouteValue {
                        value,
                        span: span.clone(),
                    });
                }
                "clock_off" => {
                    out.clocks.push((true, span.clone()));
                    if token.text.eq_ignore_ascii_case("#offclock")
                        || token.text.eq_ignore_ascii_case("#off-clock")
                        || token.text.eq_ignore_ascii_case("#untracked")
                    {
                        out.warnings.push(issue(
                            token.start,
                            token.end,
                            "deprecated_clock",
                            format!("{} is deprecated; use #off", token.text),
                        ));
                    }
                }
                "clock_on" => out.clocks.push((false, span.clone())),
                "buffer" => {
                    let (side, minutes) =
                        buffer.expect("buffer directive has value");
                    check_buffer(minutes, token, out);
                    add_buffer(out, side, minutes, None, span.clone());
                    last_buffer = Some((side, minutes, span.clone()));
                }
                _ => {}
            }
            out.spans.push(span);
            cursor += 1;
        }
        if cursor == tokens.len() {
            return;
        }
        let Some((side, minutes, buffer_span)) =
            last_buffer.filter(|(_, _, span)| {
                cursor > begin && tokens[cursor - 1].start == span.start
            })
        else {
            let start = tokens[cursor].start;
            let end = tokens.last().map(|token| token.end).unwrap_or(start);
            let text_start = start.saturating_sub(base);
            let text_end = end.saturating_sub(base);
            let text = input
                .get(text_start..text_end)
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() {
                return;
            }
            let span = Span {
                start,
                end,
                kind: "prose",
            };
            if starts_with_unknown_hash(&text) {
                out.warnings.push(issue(
                    start,
                    end,
                    "unknown_hash",
                    format!(
                        "{} is not a Bob directive; it will be a bullet",
                        text.split_whitespace().next().unwrap_or("#word")
                    ),
                ));
            }
            if text.split_whitespace().any(|word| {
                word.starts_with('-')
                    && word.len() > 1
                    && !is_buffer_token(word)
            }) {
                out.warnings.push(issue(start, end, "option_like", "option-like text may look like a command option; put options before the event query".to_string()));
            }
            out.spans.push(span.clone());
            out.prose.push((text, span));
            return;
        };
        let mut next_reason = tokens.len();
        for index in cursor..tokens.len() {
            if let Some((other_side, _)) = buffer_value(&tokens[index].text)
                && other_side == side.other()
                && !out.buffers.iter().any(|value| value.side == other_side)
            {
                next_reason = index;
                break;
            }
        }
        let reason_end = if next_reason < tokens.len() {
            tokens[next_reason].start
        } else {
            tokens.last().unwrap().end
        };
        let raw_start = tokens[cursor].start;
        let text_start = raw_start.saturating_sub(base);
        let text_end = reason_end.saturating_sub(base);
        let reason = input
            .get(text_start..text_end)
            .unwrap_or("")
            .trim()
            .to_string();
        if reason.is_empty() {
            // A bare value is canonicalized into the directive line.
            if let Some(value) = out.buffers.iter_mut().find(|value| {
                value.side == side && value.token.start == buffer_span.start
            }) {
                value.reason = None;
            }
        } else {
            if minutes == 0 {
                out.errors.push(issue(
                    buffer_span.start,
                    buffer_span.end,
                    "reason_zero",
                    format!(
                        "a {} reason needs minutes greater than 0",
                        side.name()
                    ),
                ));
            }
            let reason_span = Span {
                start: raw_start,
                end: reason_end,
                kind: "reason",
            };
            let token_start = buffer_span.start.saturating_sub(base);
            let token_end = buffer_span.end.saturating_sub(base);
            let sign_len = input
                .get(token_start..token_end)
                .and_then(|value| value.chars().next())
                .map(char::len_utf8)
                .unwrap_or(1);
            out.spans.push(Span {
                start: buffer_span.start,
                end: buffer_span.start + sign_len,
                kind: "reason_sign",
            });
            out.spans.push(reason_span.clone());
            if let Some(value) = out.buffers.iter_mut().find(|value| {
                value.side == side && value.token.start == buffer_span.start
            }) {
                value.reason = Some(reason);
            } else {
                add_buffer(out, side, minutes, Some(reason), buffer_span);
            }
        }
        if next_reason == tokens.len() {
            return;
        }
        cursor = next_reason;
    }
}

fn check_buffer(minutes: u32, token: &Token, out: &mut NormalizedNote) {
    if minutes > 720 {
        out.errors.push(issue(
            token.start,
            token.end,
            "minutes_range",
            "buffer minutes must be between 0 and 720".to_string(),
        ));
    }
}

fn add_buffer(
    out: &mut NormalizedNote,
    side: Side,
    minutes: u32,
    reason: Option<String>,
    token: Span,
) {
    if let Some(previous) = out.buffers.iter().find(|value| value.side == side)
    {
        if previous.minutes != minutes {
            out.errors.push(issue(
                token.start,
                token.end,
                "buffer_conflict",
                format!(
                    "{} and {} set different {} buffer values",
                    format!("{}{}", side.sign(), previous.minutes),
                    format!("{}{}", side.sign(), minutes),
                    side.name()
                ),
            ));
        }
        return;
    }
    out.buffers.push(BufferValue {
        side,
        minutes,
        reason,
        token,
    });
}

fn starts_with_unknown_hash(text: &str) -> bool {
    text.split_whitespace().next().is_some_and(|word| {
        word.starts_with('#')
            && !matches!(
                word.to_ascii_lowercase().as_str(),
                "#off" | "#on" | "#offclock" | "#off-clock" | "#untracked"
            )
    })
}

fn issue(
    start: usize,
    end: usize,
    kind: &'static str,
    message: String,
) -> NoteIssue {
    NoteIssue {
        start,
        end,
        kind,
        message,
        suggestions: Vec::new(),
    }
}

fn validate(
    note: &mut NormalizedNote,
    vault: &std::path::Path,
    day: NaiveDate,
) {
    let original_route = note.routes.first().map(|route| route.value.clone());
    for route in &mut note.routes {
        let raw = route.value.clone();
        match parent_notes::resolve_parent(vault, &raw) {
            Ok(parent) => match root::validate_root_candidate(
                vault,
                &snapshot::daily_relative_path(day),
                &parent.route,
            ) {
                Ok(_) => route.value = parent.route,
                Err(error) => note.errors.push(issue(
                    route.span.start,
                    route.span.end,
                    "route_invalid",
                    format!("@{raw}: {}", error.message),
                )),
            },
            Err(error) => note.errors.push(issue(
                route.span.start,
                route.span.end,
                "route_unknown",
                format!("@{raw}: {}", error.message()),
            )),
        }
    }
    let routes: Vec<&RouteValue> = note.routes.iter().collect();
    if let Some(first) = routes.first() {
        for route in routes.iter().skip(1) {
            if route.value != first.value {
                note.errors.push(issue(
                    route.span.start,
                    route.span.end,
                    "route_conflict",
                    format!("@{} conflicts with @{}", first.value, route.value),
                ));
            }
        }
    }
    // Rebuild the route in the canonical text after aliases have resolved.
    if let (Some(before), Some(route)) = (original_route, note.routes.first()) {
        if before != route.value {
            note.canonical = note.canonical.replacen(
                &format!("@{before}"),
                &format!("@{}", route.value),
                1,
            );
        }
    }
    if note.canonical.len() > journal::MAX_NOTE_BYTES {
        note.errors.push(issue(
            0,
            note.canonical.len(),
            "too_large",
            format!(
                "canonical note text is {} bytes; the maximum is {}",
                note.canonical.len(),
                journal::MAX_NOTE_BYTES
            ),
        ));
    }
}

fn invalid_output(
    args: &NoteArgs,
    candidate: &NoteCandidate,
    scope: NoteScope,
    input: &str,
    note: &NormalizedNote,
    warnings: Vec<String>,
) -> Result<(), GcalError> {
    let message = note
        .errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    if args.format.is_json() {
        let errors = note.errors.iter().map(issue_json).collect::<Vec<_>>();
        println!("{}", serde_json::to_string(&json!({
            "schema_version": 1, "ok": false, "action": "invalid",
            "event": event_json(candidate, false, None),
            "scope": scope.as_str(), "input": {"text": input},
            "spans": note.spans.iter().map(span_json).collect::<Vec<_>>(),
            "errors": errors,
            "warnings": warnings,
            "error": {"kind": "invalid_note", "message": message, "hint": "fix the highlighted note text and retry"}
        })).unwrap_or_else(|_| "{\"ok\":false}".to_string()));
        ui::flush_stdout();
        return Err(GcalError::setup(
            "printed_exit_2",
            "invalid private note".to_string(),
        ));
    }
    Err(GcalError::setup("invalid_note", message)
        .with_hint("fix the note text and try again"))
}

fn print_result(
    args: &NoteArgs,
    candidate: &NoteCandidate,
    scope: NoteScope,
    action: &str,
    input: &str,
    note: Option<&NormalizedNote>,
    status: &str,
    status_detail: &str,
    status_path: Option<&str>,
    warnings: &[String],
    rendered: &Preview,
    cleared: bool,
) -> Result<(), GcalError> {
    let canonical = note.map(|note| note.canonical.as_str()).unwrap_or("");
    if args.format.is_json() {
        let note_lines = note.map(note_lines_json).unwrap_or_default();
        let effective = effective_json(candidate, rendered);
        let document = json!({
            "schema_version": 1,
            "ok": true,
            "action": action,
            "event": event_json(candidate, true, Some(canonical)),
            "scope": scope.as_str(),
            "input": {"text": input},
            "spans": note.map(|note| note.spans.iter().map(span_json).collect::<Vec<_>>()).unwrap_or_default(),
            "note": {"text": canonical, "lines": note_lines},
            "effective": effective,
            "preview": rendered.json,
            "status": {"state": status, "detail": status_detail, "path": status_path},
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
    } else {
        println!("✎ {}", candidate.event.title);
        println!(
            "  {} · {} · from {} · {}",
            candidate.day.format("%a %b %d"),
            candidate.time,
            candidate.organizer_name(),
            answer_chip(candidate.event.my_answer)
        );
        if !cleared && !canonical.is_empty() {
            println!();
            for line in canonical.lines() {
                println!("  {line}");
            }
        }
        if let Some(rail) = rendered.rail.as_deref() {
            println!();
            println!("  {rail}");
        }
        if let Some(row) = rendered.row.as_deref() {
            println!();
            println!("  lands as");
            println!("  {row}");
            for bullet in &rendered.bullets {
                println!("  {bullet}");
            }
        }
        println!();
        if cleared {
            println!("✓ Cleared · Google's description decides again");
        } else if action == "preview" {
            println!("Preview · nothing saved · {status_detail}");
        } else if action == "unchanged" {
            println!("✓ Unchanged · {status_detail}");
        } else {
            println!("✓ Noted · only in Bob · {status_detail}");
        }
        for warning in warnings {
            ui::warn(warning);
        }
        ui::flush_stdout();
    }
    Ok(())
}

pub(super) fn show_result(
    args: &NoteArgs,
    vault: &std::path::Path,
    candidate: &NoteCandidate,
    scope: Option<NoteScope>,
    scope_text: Option<String>,
    warnings: &[String],
) -> Result<(), GcalError> {
    let rendered = preview(candidate, vault)?;
    if args.format.is_json() {
        let effective = effective_json(candidate, &rendered);
        let document = json!({
            "schema_version": 1,
            "ok": true,
            "action": "show",
            "event": event_json(candidate, true, scope_text.as_deref()),
            "scope": scope.map(NoteScope::as_str),
            "effective": effective,
            "preview": rendered.json,
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
    } else {
        super::print_show_human(args, candidate, scope, scope_text, warnings)?;
        if let Some(rail) = rendered.rail {
            println!("\n  {rail}");
        }
        if let Some(row) = rendered.row {
            println!("\n  lands as\n  {row}");
            for bullet in rendered.bullets {
                println!("  {bullet}");
            }
        }
        ui::flush_stdout();
    }
    Ok(())
}

struct Preview {
    json: Value,
    row: Option<String>,
    bullets: Vec<String>,
    rail: Option<String>,
}

fn preview(
    candidate: &NoteCandidate,
    vault: &std::path::Path,
) -> Result<Preview, GcalError> {
    let window = DayWindow::new(candidate.day, bob_env::configured_timezone())?;
    let daily_rel = snapshot::daily_relative_path(candidate.day);
    let mut resolver = HomeResolver::new(DiskHomeSource::new(vault));
    let snapshot = DaySnapshot {
        day: candidate.day,
        window: window.clone(),
        vault: vault.to_path_buf(),
        daily_rel: daily_rel.clone(),
        events: vec![SnapshotEvent {
            event: candidate.event.clone(),
            inclusion: candidate.inclusion.clone(),
            root: candidate.root.clone(),
            calendar_name: candidate.calendar_name.clone(),
            time: candidate.time.clone(),
            sort_at: candidate.sort_at,
            clock: candidate.clock.clone(),
            root_only_clock: candidate.root_only_clock.clone(),
        }],
        account: candidate.event.key.calendar_id.clone(),
        series_roots: journal::load_series_roots(vault).unwrap_or_default(),
    };
    let planned = super::super::sync::build_occurrences(
        &snapshot,
        &GcalConfig::resolve()?,
        vault,
        &daily_rel,
        candidate.day == today(),
        &bob_env::current_datetime(),
        &mut resolver,
    )
    .into_iter()
    .next()
    .map(|(occurrence, _)| occurrence);
    let mut row = None;
    let mut bullets = Vec::new();
    let path = daily_rel.display().to_string();
    if let Some(occurrence) = planned {
        if occurrence.task {
            let contents = std::fs::read_to_string(vault.join(&daily_rel))
                .unwrap_or_default();
            let lines =
                contents.lines().map(str::to_string).collect::<Vec<_>>();
            let ids = plan::collect_block_ids(&lines);
            let existing_id =
                plan::preview_existing_block_id(&contents, &occurrence);
            let block_id = existing_id.clone().unwrap_or_else(|| {
                crate::native::capture_block_ids::mint_block_id(
                    &occurrence.id_hint,
                    &ids.iter().map(String::as_str).collect(),
                )
            });
            row = Some(plan::render_occurrence(
                &occurrence,
                &block_id,
                candidate.day,
            ));
            bullets = plan::preview_bullets(
                &contents,
                &occurrence,
                existing_id.as_deref(),
            );
        }
    }
    let rail = time_rail(candidate, &window, true);
    let json_rail = time_rail(candidate, &window, false);
    let (meeting_start, meeting_end, occupied_start, occupied_end, clipped) =
        time_bounds(candidate, &window);
    let json_preview = json!({
        "path": path,
        "row": row,
        "bullets": bullets,
        "meeting": {"start": meeting_start, "end": meeting_end},
        "occupied": {"start": occupied_start, "end": occupied_end, "clipped": clipped},
        "rail": json_rail,
    });
    Ok(Preview {
        json: json_preview,
        row,
        bullets,
        rail,
    })
}

fn time_bounds(
    candidate: &NoteCandidate,
    window: &DayWindow,
) -> (String, String, String, String, bool) {
    let (start, end) = match (&candidate.event.start, &candidate.event.end) {
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            (window.local(start), window.local(end))
        }
        _ => {
            return (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                false,
            )
        }
    };
    let start_min = (start.hour() * 60 + start.minute()) as i32;
    let end_min = (end.hour() * 60 + end.minute()) as i32
        + if end.date() > start.date() { 1440 } else { 0 };
    let before = candidate.event.buffer.before_min as i32;
    let after = candidate.event.buffer.after_min as i32;
    let raw_start = start_min - before;
    let raw_end = end_min + after;
    (
        format!("{:02}:{:02}", start.hour(), start.minute()),
        format!("{:02}:{:02}", end.hour(), end.minute()),
        format_minute(raw_start.max(0)),
        format_minute(raw_end.min(1440)),
        raw_start < 0 || raw_end > 1440,
    )
}

fn format_minute(value: i32) -> String {
    let minutes = value.clamp(0, 1440);
    if minutes == 1440 {
        "24:00".to_string()
    } else {
        format!("{:02}:{:02}", minutes / 60, minutes % 60)
    }
}

fn time_rail(
    candidate: &NoteCandidate,
    window: &DayWindow,
    color: bool,
) -> Option<String> {
    let (EventDateTime::Timed(start), EventDateTime::Timed(end)) =
        (&candidate.event.start, &candidate.event.end)
    else {
        return None;
    };
    let local_start = window.local(start);
    let local_end = window.local(end);
    let start_min = (local_start.hour() * 60 + local_start.minute()) as i32;
    let end_min = (local_end.hour() * 60 + local_end.minute()) as i32
        + if local_end.date() > local_start.date() {
            1440
        } else {
            0
        };
    let raw_start = start_min - candidate.event.buffer.before_min as i32;
    let raw_end = end_min + candidate.event.buffer.after_min as i32;
    let clipped_before = raw_start < 0;
    let clipped_after = raw_end > 1440;
    let occupied_start = raw_start.max(0);
    let occupied_end = raw_end.min(1440);
    let lengths = [
        (start_min - occupied_start).max(0) as usize,
        (end_min - start_min).max(1) as usize,
        (occupied_end - end_min).max(0) as usize,
    ];
    let required = lengths.iter().filter(|length| **length > 0).count();
    let route_hue = candidate.root.as_ref().and_then(|root| root.hue);
    let full_name = route_hue
        .map(|hue| hue.name().to_string())
        .unwrap_or_else(|| "unrouted".to_string());
    let mut hue = full_name.clone();
    let make_base = |name: &str| {
        if clipped_before {
            format!(
                "● {name} (clipped at 00:00) {} {} {}",
                format_minute(start_min),
                format_minute(end_min),
                format_minute(occupied_end)
            )
        } else if clipped_after {
            format!(
                "● {name} {} {} {} (clipped at 24:00)",
                format_minute(occupied_start),
                format_minute(start_min),
                format_minute(end_min)
            )
        } else {
            format!(
                "● {name} {} {} {} {}",
                format_minute(occupied_start),
                format_minute(start_min),
                format_minute(end_min),
                format_minute(occupied_end)
            )
        }
    };
    let empty_base = make_base("");
    let max_name = 48usize
        .saturating_sub(empty_base.chars().count())
        .saturating_sub(required);
    if hue.chars().count() > max_name {
        hue = crate::native::style::truncate(&hue, max_name);
    }
    let empty_base = make_base(&hue);
    let available = 48usize
        .saturating_sub(empty_base.chars().count())
        .max(required);
    let widths = proportional_rail_widths(lengths, available);
    let before = "╱".repeat(widths[0]);
    let meeting = "━".repeat(widths[1]);
    let after = "╱".repeat(widths[2]);
    let plain = if clipped_before {
        format!(
            "● {hue} (clipped at 00:00) {before}{} {meeting}{} {after}{}",
            format_minute(start_min),
            format_minute(end_min),
            format_minute(occupied_end),
        )
    } else if clipped_after {
        format!(
            "● {hue} {} {before}{} {meeting}{} {after}(clipped at 24:00)",
            format_minute(occupied_start),
            format_minute(start_min),
            format_minute(end_min),
        )
    } else {
        format!(
            "● {hue} {} {before}{} {meeting}{} {after}{}",
            format_minute(occupied_start),
            format_minute(start_min),
            format_minute(end_min),
            format_minute(occupied_end),
        )
    };
    if !color {
        return Some(plain);
    }
    let styler = crate::native::style::Styler::detect();
    let Some(root_hue) = route_hue else {
        return Some(plain);
    };
    let hue_code = format!("38;5;{}", root_hue.xterm());
    let dot = styler.paint(&hue_code, "●");
    let colored_hue = styler.paint(&hue_code, &hue);
    let wing_code = format!("2;38;5;{}", root_hue.xterm());
    let before = styler.paint(&wing_code, &before);
    let meeting = styler.paint(&hue_code, &meeting);
    let after = styler.paint(&wing_code, &after);
    let start = format_minute(start_min);
    let end = format_minute(end_min);
    let rail = if clipped_before {
        format!(
            "{dot} {colored_hue} (clipped at 00:00) {before}{start} {meeting}{end} {after}{}",
            format_minute(occupied_end)
        )
    } else if clipped_after {
        format!(
            "{dot} {colored_hue} {} {before}{start} {meeting}{end} {after}(clipped at 24:00)",
            format_minute(occupied_start)
        )
    } else {
        format!(
            "{dot} {colored_hue} {} {before}{start} {meeting}{end} {after}{}",
            format_minute(occupied_start),
            format_minute(occupied_end)
        )
    };
    Some(rail)
}

fn proportional_rail_widths(lengths: [usize; 3], width: usize) -> [usize; 3] {
    let total: usize = lengths.iter().sum();
    if total == 0 || width == 0 {
        return [0; 3];
    }
    let mut cells = lengths.map(|length| length * width / total);
    for index in 0..3 {
        if lengths[index] > 0 && cells[index] == 0 {
            cells[index] = 1;
        }
    }
    while cells.iter().sum::<usize>() > width {
        let Some(index) = (0..3)
            .filter(|index| lengths[*index] > 0 && cells[*index] > 1)
            .max_by_key(|index| {
                cells[*index] as i64 * total as i64
                    - lengths[*index] as i64 * width as i64
            })
        else {
            break;
        };
        cells[index] -= 1;
    }
    while cells.iter().sum::<usize>() < width {
        let index = (0..3)
            .filter(|index| lengths[*index] > 0)
            .max_by_key(|index| {
                lengths[*index] as i64 * width as i64
                    - cells[*index] as i64 * total as i64
            })
            .unwrap_or(1);
        cells[index] += 1;
    }
    cells
}

fn effective_json(candidate: &NoteCandidate, preview: &Preview) -> Value {
    let applied = candidate.event.note.as_ref();
    let buffer = candidate.event.buffer;
    let reasons = &candidate.event.description.buffer_reasons;
    json!({
        "root": {"value": candidate.root.as_ref().and_then(|root| root.value.clone()), "source": candidate.root.as_ref().and_then(|root| root.source.clone()), "kind": candidate.root.as_ref().and_then(|root| root.kind.json_label()), "hue": candidate.root.as_ref().and_then(|root| root.hue).map(|hue| hue.name())},
        "clock": clock::explicit_clock_label(&candidate.event.clock_tag),
        "buffer": {
            "before_minutes": buffer.before_min,
            "after_minutes": buffer.after_min,
            "before_reason": buffer_reason_text(reasons.before.as_ref()),
            "after_reason": buffer_reason_text(reasons.after.as_ref()),
        },
        "meeting": preview.json["meeting"],
        "occupied": preview.json["occupied"],
        "fields": applied.map(|note| note.fields.iter().map(|field| field.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
    })
}

fn buffer_reason_text(
    reason: Option<&crate::native::gcal::description::BufferReason>,
) -> Option<String> {
    reason.map(|reason| {
        reason
            .inlines
            .iter()
            .map(|inline| match inline {
                crate::native::gcal::description::Inline::Text {
                    text, ..
                }
                | crate::native::gcal::description::Inline::Link {
                    text, ..
                } => text.as_str(),
            })
            .collect()
    })
}

fn note_lines_json(note: &NormalizedNote) -> Vec<Value> {
    let mut lines = Vec::new();
    let route = note.routes.first().map(|route| route.value.as_str());
    let clocks: Vec<&str> = note
        .clocks
        .iter()
        .map(|(off, _)| if *off { "#off" } else { "#on" })
        .collect();
    let mut directives = Vec::new();
    if let Some(route) = route {
        directives.push(format!("@{route}"));
    }
    if let Some(clock) = clocks.last() {
        directives.push((*clock).to_string());
    }
    for side in [Side::Before, Side::After] {
        if let Some(value) = note
            .buffers
            .iter()
            .find(|value| value.side == side && value.reason.is_none())
        {
            directives.push(format!("{}{}", side.sign(), value.minutes));
        }
    }
    if !directives.is_empty() {
        lines.push(json!({"kind":"directives","text":directives.join(" ")}));
    }
    for side in [Side::Before, Side::After] {
        if let Some(value) = note
            .buffers
            .iter()
            .find(|value| value.side == side && value.reason.is_some())
        {
            lines.push(json!({"kind":"reason","side":side.name(),"minutes":value.minutes,"text":format!("{}{} {}", side.sign(), value.minutes, value.reason.as_deref().unwrap_or("") )}));
        }
    }
    for (text, _) in &note.prose {
        lines.push(json!({"kind":"prose","text":text}));
    }
    lines
}

fn span_json(span: &Span) -> Value {
    json!({"start":span.start,"end":span.end,"kind":span.kind})
}

fn issue_json(issue: &NoteIssue) -> Value {
    json!({"start":issue.start,"end":issue.end,"kind":issue.kind,"message":issue.message,"suggestions":issue.suggestions})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn normalization_shared_vectors() {
        let vectors: Vec<Value> = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/gcal_note/normalize.json"
        ))
        .expect("normalization vectors");
        let vault = tempfile::tempdir().expect("validation vault");
        for area in ["body", "family"] {
            std::fs::write(
                vault.path().join(format!("{area}.md")),
                "---\ntype: area\n---\n",
            )
            .expect("write area");
        }
        for vector in vectors {
            let input = vector["input"].as_str().expect("input");
            let mut actual = normalize(input);
            validate(
                &mut actual,
                vault.path(),
                NaiveDate::from_ymd_opt(2026, 12, 13).expect("day"),
            );
            assert_eq!(
                actual.canonical,
                vector["canonical"].as_str().expect("canonical"),
                "input: {input:?}"
            );
            let mut errors = actual
                .errors
                .iter()
                .map(|error| error.kind)
                .collect::<Vec<_>>();
            errors.sort_unstable();
            let mut expected = vector["errors"]
                .as_array()
                .expect("errors")
                .iter()
                .map(|kind| kind.as_str().expect("error kind"))
                .collect::<Vec<_>>();
            expected.sort_unstable();
            assert_eq!(errors, expected, "input: {input:?}");
            let mut warnings = actual
                .warnings
                .iter()
                .map(|warning| warning.kind)
                .collect::<Vec<_>>();
            warnings.sort_unstable();
            let mut expected_warnings = vector
                .get("warnings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|kind| kind.as_str().expect("warning kind"))
                .collect::<Vec<_>>();
            expected_warnings.sort_unstable();
            assert_eq!(warnings, expected_warnings, "input: {input:?}");
        }
    }

    #[test]
    fn canonical_note_size_is_limited_to_2048_bytes() {
        let mut note = normalize(&"x".repeat(journal::MAX_NOTE_BYTES + 1));
        validate(
            &mut note,
            std::path::Path::new("/missing-vault"),
            NaiveDate::from_ymd_opt(2026, 12, 13).expect("day"),
        );
        assert!(note.errors.iter().any(|error| error.kind == "too_large"));
    }

    #[test]
    fn reason_spans_are_half_open_utf8_offsets() {
        let input = "−30 Drive home";
        let note = normalize(input);
        let sign = note
            .spans
            .iter()
            .find(|span| span.kind == "reason_sign")
            .expect("reason sign");
        let reason = note
            .spans
            .iter()
            .find(|span| span.kind == "reason")
            .expect("reason");
        assert_eq!(&input[sign.start..sign.end], "−");
        assert_eq!(&input[reason.start..reason.end], "Drive home");
    }
}

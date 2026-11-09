//! Read-only `bob gcal list`: fetch a complete local-day snapshot, classify
//! every event, choose valid roots, then render a table or JSON envelope.

use chrono::{Duration, NaiveDate};
use serde_json::{json, Value};

use super::{
    classify::{self, Inclusion},
    clock,
    config::GcalConfig,
    description::bullets::render_bullets,
    diagnostics, journal,
    lifecycle::split_row_spans,
    model::MyAnswer,
    plan::journaled_title,
    skip::{restore_command, SkipReason},
    snapshot::{self, SnapshotEvent},
    ui, GcalError, ListArgs,
};
use crate::native::{
    env as bob_env, event_task,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    style::{self, Styler},
    templater::first_pending_tag,
};

type ListedEvent = SnapshotEvent;

pub(crate) fn run(args: &ListArgs) -> i32 {
    match list(args) {
        Ok(()) => 0,
        Err(error)
            if error.kind() == "not_logged_in" && !args.format.is_json() =>
        {
            eprintln!(
                "Google Calendar is not connected yet.\n\
                 Run `bob gcal login` to connect.\n\
                 Then try `bob gcal` again."
            );
            error.exit_code()
        }
        Err(error) => ui::report_error("list", &error, args.error_format()),
    }
}

fn snapshot_diagnostics(
    events: &[ListedEvent],
) -> (Vec<String>, Vec<diagnostics::Deprecation>) {
    let inputs: Vec<diagnostics::DiagnosticInput<'_>> = events
        .iter()
        .map(|listed| diagnostics::DiagnosticInput {
            calendar_id: &listed.event.key.calendar_id,
            event_id: &listed.event.key.event_id,
            title: &listed.event.title,
            clock_tag: &listed.event.clock_tag,
            deprecated_title_tags: &listed.event.deprecated_title_tags,
            deprecated_description_tags: &listed
                .event
                .deprecated_description_tags,
            root_value: listed
                .root
                .as_ref()
                .and_then(|root| root.value.as_deref()),
            root_only_off_clock: listed.root_only_clock.off_clock,
            root_only_note: listed.root_only_clock.note.as_deref(),
        })
        .collect();
    diagnostics::collect(&inputs)
}

fn list(args: &ListArgs) -> Result<(), GcalError> {
    let config = GcalConfig::resolve()?;
    let day =
        classify::resolve_day(&args.day, bob_env::current_datetime().date())?;
    let vault = args.bob_dir();
    let mut resolver = HomeResolver::new(DiskHomeSource::new(&vault));
    let snapshot =
        snapshot::fetch_day(vault.clone(), &config, day, &mut resolver)?;
    let (diag_warnings, deprecations) = snapshot_diagnostics(&snapshot.events);
    let visible_calendars: Vec<Value> = config
        .calendars()
        .iter()
        .map(|calendar| {
            let name = snapshot
                .events
                .iter()
                .find(|event| event.event.key.calendar_id == calendar.id)
                .map(|event| event.calendar_name.clone())
                .unwrap_or_else(|| calendar.id.clone());
            json!({"id": calendar.id, "name": name})
        })
        .collect();
    let events = &snapshot.events;
    let counts = counts(events);
    // The VAULT column reads the same journal sync writes, plus the
    // day's note. A missing note means every row is still waiting;
    // an unreadable journal degrades to unjournaled display with a
    // stderr warning rather than failing this read-only command.
    let (journal_state, journal_warnings, newest) =
        match journal::load_state(&vault) {
            Ok(loaded) => loaded,
            Err(error) => {
                ui::warn(&format!("could not read the gcal journal: {error}"));
                Default::default()
            }
        };
    let note = std::fs::read_to_string(vault.join(&snapshot.daily_rel)).ok();
    let note_lines: Vec<&str> = note
        .as_deref()
        .map(|contents| contents.lines().collect())
        .unwrap_or_default();
    // A note Templater has not rendered is about to be overwritten, so
    // sync refuses it; list shows the same state instead of Bob state.
    let unrendered = note
        .as_deref()
        .is_some_and(|contents| first_pending_tag(contents).is_some());
    let note_view: NoteView<'_> = match (&note, unrendered) {
        (_, true) => Err(WAITING_FOR_TEMPLATER),
        (Some(_), false) => Ok(note_lines.as_slice()),
        (None, false) => Err(WAITING_FOR_NOTE),
    };
    let day_string = day.to_string();
    let is_today = day == bob_env::current_datetime().date();
    let cells: Vec<VaultCell> = events
        .iter()
        .map(|event| {
            vault_cell(
                event,
                &snapshot.window,
                &journal_state,
                &day_string,
                note_view,
                is_today,
            )
        })
        .collect();
    // Missed meetings stay open and surface here; nothing auto-cancels.
    let past_open = past_open_meetings(&vault, day);
    if args.format.is_json() {
        print_json(
            day,
            &config,
            &visible_calendars,
            events,
            &counts,
            args.include_omitted,
            &cells,
            last_sync_json(&newest),
            &past_open,
            &journal_warnings,
            &diag_warnings,
            &deprecations,
        )
    } else {
        print_table(
            day,
            &config,
            &visible_calendars,
            events,
            &counts,
            args.include_omitted,
            &cells,
            (day, is_today),
            &last_sync_human(&newest, unrendered),
            &journal_warnings,
            &diag_warnings,
            &past_open,
        )
    }
}

/// Open Event Tasks still sitting in the previous 7 daily notes, newest
/// first, as (day, title). Missed meetings are surfaced, never
/// auto-cancelled.
fn past_open_meetings(
    vault: &std::path::Path,
    today: NaiveDate,
) -> Vec<(NaiveDate, String)> {
    let mut open = Vec::new();
    for back in 1..=7 {
        let day = today - Duration::days(back);
        let Ok(contents) = std::fs::read_to_string(
            vault.join(snapshot::daily_relative_path(day)),
        ) else {
            continue;
        };
        let lines: Vec<&str> = contents.lines().collect();
        let Some(section) = event_task::events_section_range(&lines) else {
            continue;
        };
        for line in lines.get(section).unwrap_or(&[]) {
            if event_task::parse_event_span(line).is_none() {
                continue;
            }
            let status = line
                .trim_start()
                .strip_prefix("- [")
                .and_then(|rest| rest.chars().next());
            if !matches!(status, Some(' ') | Some('*')) {
                continue;
            }
            let title = journaled_title(line);
            if !title.is_empty() {
                open.push((day, title));
            }
        }
    }
    open
}

const WAITING_FOR_NOTE: &str = "waiting for note";
const WAITING_FOR_TEMPLATER: &str = "waiting for Templater";

/// The day's note lines, or the label every VAULT cell shows while the
/// note is missing or still unrendered.
type NoteView<'a> = Result<&'a [&'a str], &'static str>;

/// One event's VAULT cell and, when sync leaves it out, the shared skip
/// reason behind it.
struct VaultCell {
    vault: String,
    skip: Option<SkipReason>,
}

impl VaultCell {
    fn skip_json(&self) -> Value {
        self.skip
            .as_ref()
            .map(SkipReason::json)
            .unwrap_or(Value::Null)
    }

    fn removed_by_you(&self) -> bool {
        self.skip
            .as_ref()
            .is_some_and(|skip| skip.code == "removed_by_you")
    }
}

/// What sync did, or would do, with one event: `task · MEET`, `task`,
/// `context`, `waiting for note`, or `removed by you 05:28`, each task
/// with ` · off-clock` (or ` · off-clock via love`) appended when it is
/// off-clock. Omitted events keep their omission reason. A tombstoned
/// row whose line is back reads as managed: the next sync adopts it.
fn vault_cell(
    event: &ListedEvent,
    window: &classify::DayWindow,
    journal: &std::collections::BTreeMap<
        (String, String),
        journal::OccurrenceState,
    >,
    day: &str,
    note: NoteView<'_>,
    is_today: bool,
) -> VaultCell {
    if let Inclusion::Omitted(reason) = &event.inclusion {
        return VaultCell {
            vault: reason.clone(),
            skip: Some(SkipReason::omitted(reason, &event.event, window)),
        };
    }
    let lines = match note {
        Ok(lines) => lines,
        Err(label) => {
            return VaultCell {
                vault: label.to_string(),
                skip: None,
            };
        }
    };
    let key = journal::occurrence_key(
        &event.event.key.calendar_id,
        &event.event.key.event_id,
    );
    let state = journal.get(&(key, day.to_string()));
    let task = event.inclusion == Inclusion::Task;
    // Whether the journaled row is in the note. A context row without a
    // URL is matched by its rendered text, which list does not render, so
    // its presence is unknown: only a tombstone can say it is removed.
    let present: Option<bool> =
        state.and_then(|state| state.row.as_ref()).and_then(|row| {
            if !task && event_url(event).is_none_or(str::is_empty) {
                return None;
            }
            let block_id = if task { row.block_id.as_str() } else { "" };
            Some(row_present(lines, block_id, event_url(event), task))
        });
    let removed = match state {
        Some(state) if state.gone && present != Some(true) => {
            Some(SkipReason::removed(state, window.day, is_today))
        }
        Some(state) if !state.gone && present == Some(false) => {
            Some(SkipReason::removed_by_you(None, window.day, is_today))
        }
        _ => None,
    };
    if let Some(skip) = removed {
        return VaultCell {
            vault: skip.vault_label(),
            skip: Some(skip),
        };
    }
    // Context rows strip the token, so only tasks show the off-clock mark;
    // JSON booleans carry the effective clock for tasks and tag-only
    // values for context/omitted events.
    let off_clock_suffix = if task {
        clock::off_clock_status_suffix(&event.clock)
    } else {
        String::new()
    };
    let vault = match &event.inclusion {
        Inclusion::Omitted(_) => unreachable!("handled above"),
        Inclusion::Context(_) => "context".to_string(),
        Inclusion::Task => {
            if let Some(row) = state.and_then(|state| state.row.as_ref()) {
                if meet_link_present(lines, &row.block_id) {
                    format!("task · MEET{off_clock_suffix}")
                } else {
                    format!("task{off_clock_suffix}")
                }
            } else {
                match adopted_block_id(lines, event_url(event)) {
                    Some(block_id) if meet_link_present(lines, &block_id) => {
                        format!("task · MEET{off_clock_suffix}")
                    }
                    _ => format!("task{off_clock_suffix}"),
                }
            }
        }
    };
    VaultCell { vault, skip: None }
}

fn event_url(event: &ListedEvent) -> Option<&str> {
    event.event.html_link.as_deref()
}

/// Bullet lines sync would write for one event: the rendered, capped
/// count for Task inclusions in `bullets` mode, 0 otherwise. JSON
/// carries only this count, never description text.
fn description_lines(config: &GcalConfig, event: &ListedEvent) -> usize {
    if event.inclusion != Inclusion::Task || !config.description_bullets() {
        return 0;
    }
    render_bullets(
        &event.event.description,
        config.description_lines(),
        event.event.html_link.as_deref(),
    )
    .bullets
    .len()
}

/// A row is present when its journaled `^block_id` matches, or its
/// `📅` URL appears on a top-level row. An indented description bullet
/// carrying the event URL (the overflow line) never counts.
fn row_present(
    lines: &[&str],
    block_id: &str,
    url: Option<&str>,
    task: bool,
) -> bool {
    if task && !block_id.is_empty() {
        let suffix = format!("^{block_id}");
        if lines.iter().any(|line| {
            line.trim_end().strip_suffix(suffix.as_str()).is_some_and(
                |before| before.chars().last().is_some_and(char::is_whitespace),
            )
        }) {
            return true;
        }
    }
    url.filter(|url| !url.is_empty()).is_some_and(|url| {
        lines
            .iter()
            .any(|line| line.contains(url) && split_row_spans(line).is_some())
    })
}

/// The `^block_id` of the top-level note line carrying `url`, for
/// unjournaled rows.
fn adopted_block_id(lines: &[&str], url: Option<&str>) -> Option<String> {
    let url = url.filter(|url| !url.is_empty())?;
    lines
        .iter()
        .find(|line| line.contains(url) && split_row_spans(line).is_some())
        .and_then(|line| trailing_block_id(line))
}

fn trailing_block_id(line: &str) -> Option<String> {
    let trimmed = line.trim_end();
    let caret = trimmed.rfind('^')?;
    let id = trimmed.get(caret + 1..)?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || (caret > 0
            && !trimmed[..caret]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace))
    {
        return None;
    }
    Some(id.to_string())
}

fn meet_link_present(lines: &[&str], block_id: &str) -> bool {
    let link = format!("[[#^{block_id}]]");
    let range = match crate::native::pomodoro::pomodoros_section_range(lines) {
        Some(range) => range,
        None => return false,
    };
    range
        .filter_map(|index| lines.get(index))
        .any(|line| line.contains(link.as_str()))
}

/// `{"before_minutes", "after_minutes"}` for a buffered task, else
/// `null`: context rows never carry a buffer, matching gcal's null style.
fn buffer_json(event: &ListedEvent) -> Value {
    let buffer = if event.inclusion == Inclusion::Task {
        event.event.buffer
    } else {
        event_task::EventBuffer::default()
    };
    if buffer.is_empty() {
        Value::Null
    } else {
        json!({
            "before_minutes": buffer.before_min,
            "after_minutes": buffer.after_min,
        })
    }
}

/// Human buffer label for a task row's TIME cell, if it carries one.
fn buffer_label(event: &ListedEvent) -> Option<String> {
    if event.inclusion != Inclusion::Task {
        return None;
    }
    event.event.buffer.label()
}

/// Header freshness: the host-local status file, else the newest
/// journal record's host and time, else never synced.
fn last_sync_human(
    newest: &Option<(chrono::DateTime<chrono::FixedOffset>, String)>,
    unrendered: bool,
) -> String {
    if unrendered {
        return WAITING_FOR_TEMPLATER.to_string();
    }
    if let Some(status) = journal::read_status() {
        if status.outcome == "waiting" {
            return match status.reason.as_deref() {
                Some("note_unrendered") => WAITING_FOR_TEMPLATER,
                _ => WAITING_FOR_NOTE,
            }
            .to_string();
        }
        return format!(
            "synced {} on {}",
            journal::ago(&status.ts),
            status.host,
        );
    }
    match newest {
        Some((ts, host)) => {
            format!("synced {} on {}", journal::ago(ts), host)
        }
        None => "never synced".to_string(),
    }
}

fn last_sync_json(
    newest: &Option<(chrono::DateTime<chrono::FixedOffset>, String)>,
) -> Value {
    if let Some(status) = journal::read_status() {
        return json!({
            "outcome": status.outcome,
            "reason": status.reason,
            "host": status.host,
            "ts": status.ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        });
    }
    match newest {
        Some((ts, host)) => json!({
            "outcome": "unknown",
            "host": host,
            "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        }),
        None => Value::Null,
    }
}

#[derive(Default)]
struct Counts {
    tasks: usize,
    context: usize,
    omitted: usize,
    awaiting_reply: usize,
}

fn counts(events: &[ListedEvent]) -> Counts {
    let mut counts = Counts::default();
    for event in events {
        match &event.inclusion {
            Inclusion::Task => counts.tasks += 1,
            Inclusion::Context(_) => counts.context += 1,
            Inclusion::Omitted(_) => counts.omitted += 1,
        }
        if event.inclusion.reason() == Some("reply in Calendar") {
            counts.awaiting_reply += 1;
        }
    }
    counts
}

#[allow(clippy::too_many_arguments)]
fn print_json(
    day: NaiveDate,
    config: &GcalConfig,
    calendars: &[Value],
    events: &[ListedEvent],
    counts: &Counts,
    include_omitted: bool,
    cells: &[VaultCell],
    last_sync: Value,
    past_open: &[(NaiveDate, String)],
    journal_warnings: &[String],
    diag_warnings: &[String],
    deprecations: &[diagnostics::Deprecation],
) -> Result<(), GcalError> {
    let rows: Vec<Value> = events
        .iter()
        .zip(cells)
        .filter(|(event, _)| include_omitted || !matches!(event.inclusion, Inclusion::Omitted(_)))
        .map(|(event, cell)| {
            let root = event.root.as_ref();
            json!({
                "time": event.time,
                "title": event.event.title,
                "answer": event.event.my_answer.as_str(),
                "inclusion": event.inclusion.name(),
                "reason": event.inclusion.reason(),
                "clock": clock::explicit_clock_label(&event.event.clock_tag),
                "clock_source": event.clock.source,
                "clock_note": event.clock.note,
                "off_clock": event.clock.off_clock,
                "untracked": event.clock.off_clock,
                "buffer": buffer_json(event),
                "description_lines": description_lines(config, event),
                "root": {
                    "value": root.and_then(|root| root.value.as_deref()),
                    "source": root.and_then(|root| root.source.as_deref()),
                    "warnings": root.map(|root| root.warnings.as_slice()).unwrap_or(&[]),
                },
                "vault": cell.vault,
                "skip": cell.skip_json(),
            })
        })
        .collect();
    let past: Vec<Value> = past_open
        .iter()
        .map(|(day, title)| json!({"day": day.to_string(), "title": title}))
        .collect();
    let mut warnings: Vec<String> = journal_warnings.to_vec();
    warnings.extend(diag_warnings.iter().cloned());
    write_stdout(&json!({
        "schema_version": 1,
        "day": day.to_string(),
        "calendars": calendars,
        "events": rows,
        "counts": {
            "tasks": counts.tasks,
            "context": counts.context,
            "omitted": counts.omitted,
            "awaiting_reply": counts.awaiting_reply,
        },
        "last_sync": last_sync,
        "past_open": past,
        "warnings": warnings,
        "deprecations": deprecations,
    }))
}

#[allow(clippy::too_many_arguments)]
fn print_table(
    day: NaiveDate,
    config: &GcalConfig,
    calendars: &[Value],
    events: &[ListedEvent],
    counts: &Counts,
    include_omitted: bool,
    cells: &[VaultCell],
    (day_for_hint, is_today): (NaiveDate, bool),
    sync_state: &str,
    journal_warnings: &[String],
    diag_warnings: &[String],
    past_open: &[(NaiveDate, String)],
) -> Result<(), GcalError> {
    let styler = Styler::detect();
    let calendar_names = calendars
        .iter()
        .filter_map(|calendar| calendar.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(", ");
    let mut output = format!(
        "{}\n",
        styler.cyan(&format!(
            "Google Calendar · {} · {calendar_names} · {sync_state}",
            day.format("%a %Y-%m-%d")
        ))
    );
    let headers = ["TIME", "EVENT", "RSVP", "ROOT", "VAULT"];
    let mut rows = Vec::new();
    let shown: Vec<(&ListedEvent, &VaultCell)> = events
        .iter()
        .zip(cells)
        .filter(|(event, _)| {
            include_omitted || !matches!(event.inclusion, Inclusion::Omitted(_))
        })
        .collect();
    for (event, cell) in &shown {
        let root_label = event
            .root
            .as_ref()
            .and_then(|root| root.value.as_ref())
            .map(|root| format!("● {root}"))
            .unwrap_or_else(|| "—".to_string());
        let vault = cell.vault.clone();
        let time = match buffer_label(event) {
            Some(label) => format!("{} {label}", event.time),
            None => event.time.clone(),
        };
        // The bullet count rides plain here so column widths are
        // computed before styling, the way `buffer_label` does.
        let title = match description_lines(config, event) {
            0 => event.event.title.clone(),
            lines => format!("{} ≡ {lines}", event.event.title),
        };
        rows.push([
            time,
            title,
            answer_label(
                event.event.my_answer,
                event.event.organizer_or_no_guests,
            ),
            root_label,
            vault,
        ]);
    }
    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            rows.iter()
                .map(|row| style::display_width(&row[column]))
                .chain(std::iter::once(headers[column].len()))
                .max()
                .unwrap_or(headers[column].len())
        })
        .collect();
    output.push_str(
        &styler.dim(&format_row(&headers.map(str::to_string), &widths)),
    );
    output.push('\n');
    for ((event, _), row) in shown.iter().zip(rows.iter()) {
        let mut cells = row.clone();
        if let Some(label) = buffer_label(event) {
            cells[0] = format!("{} {}", event.time, styler.dim(&label));
        }
        match description_lines(config, event) {
            0 => {}
            lines => {
                cells[1] = format!(
                    "{} {}",
                    event.event.title,
                    styler.dim(&format!("≡ {lines}"))
                );
            }
        }
        if let Some(root) = event.root.as_ref()
            && let Some(value) = root.value.as_deref()
        {
            cells[3] = ui::hue_dot(&styler, root.hue, value);
        }
        output.push_str(&format_row(&cells, &widths));
        output.push('\n');
        if let Some(root) = event.root.as_ref() {
            for warning in &root.warnings {
                output.push_str(&styler.yellow(&format!(
                    "  warning · {} · {warning}",
                    event.event.title
                )));
                output.push('\n');
            }
        }
    }
    output.push_str(&format!(
        "{} tasks · {} context · {} awaiting your reply\n",
        counts.tasks, counts.context, counts.awaiting_reply
    ));
    let removed = cells.iter().filter(|cell| cell.removed_by_you()).count();
    if removed > 0 {
        let whose = if is_today {
            "today's".to_string()
        } else {
            format!("{day_for_hint}'s")
        };
        output.push_str(&format!(
            "{removed} removed by you · {} restores {whose} Events\n",
            styler.cyan(&restore_command(day_for_hint, is_today)),
        ));
    }
    if !past_open.is_empty() {
        let items: Vec<String> = past_open
            .iter()
            .map(|(day, title)| format!("{} {title}", day.format("%a")))
            .collect();
        let noun = if past_open.len() == 1 {
            "past meeting still open"
        } else {
            "past meetings still open"
        };
        output.push_str(&format!(
            "{} {noun}: {} · close or cancel them in their notes\n",
            past_open.len(),
            items.join(", ")
        ));
    }
    for warning in journal_warnings.iter().chain(diag_warnings.iter()) {
        ui::warn(warning);
    }
    write_text_stdout(&output)
}

fn answer_label(answer: MyAnswer, own: bool) -> String {
    match answer {
        MyAnswer::Yes if own => "✓ own".to_string(),
        MyAnswer::Yes => "✓ going".to_string(),
        MyAnswer::Maybe => "? maybe".to_string(),
        MyAnswer::No => "✗ declined".to_string(),
        MyAnswer::NoReply => "· no reply".to_string(),
    }
}

fn format_row(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| style::pad_right(cell, *width))
        .collect::<Vec<_>>()
        .join("  ")
}

fn write_stdout(value: &Value) -> Result<(), GcalError> {
    let text = serde_json::to_string(value).map_err(|error| {
        GcalError::runtime(
            "output",
            format!("serialize gcal list JSON: {error}"),
        )
    })?;
    write_text_stdout(&format!("{text}\n"))
}

fn write_text_stdout(text: &str) -> Result<(), GcalError> {
    use std::io::Write;
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(GcalError::runtime(
            "output",
            format!("write gcal list output: {error}"),
        )),
    }
}

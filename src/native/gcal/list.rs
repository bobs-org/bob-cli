//! Read-only `bob gcal list`: fetch a complete local-day snapshot, classify
//! every event, choose valid roots, then render a table or JSON envelope.

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    classify::{self, Inclusion},
    config::GcalConfig,
    journal,
    model::MyAnswer,
    snapshot::{self, SnapshotEvent},
    ui, GcalError, ListArgs,
};
use crate::native::{
    env as bob_env,
    style::{self, Styler},
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

fn list(args: &ListArgs) -> Result<(), GcalError> {
    let config = GcalConfig::resolve()?;
    let day =
        classify::resolve_day(&args.day, bob_env::current_datetime().date())?;
    let vault = args.bob_dir();
    let snapshot = snapshot::fetch_day(vault.clone(), &config, day)?;
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
    let has_note = note.is_some();
    let day_string = day.to_string();
    if args.format.is_json() {
        print_json(
            day,
            &visible_calendars,
            events,
            &counts,
            args.include_omitted,
            &journal_state,
            &day_string,
            has_note.then_some(note_lines.as_slice()),
            last_sync_json(&newest),
        )
    } else {
        print_table(
            day,
            &visible_calendars,
            events,
            &counts,
            args.include_omitted,
            &journal_state,
            &day_string,
            has_note.then_some(note_lines.as_slice()),
            &last_sync_human(&newest),
            &journal_warnings,
        )
    }
}

/// What sync did, or would do, with one event: `task · MEET`, `task`,
/// `context`, `waiting for note`, or `removed by you`. Omitted events
/// keep their omission reason.
fn vault_status(
    event: &ListedEvent,
    journal: &std::collections::BTreeMap<
        (String, String),
        journal::OccurrenceState,
    >,
    day: &str,
    note: Option<&[&str]>,
) -> String {
    if let Inclusion::Omitted(reason) = &event.inclusion {
        return reason.clone();
    }
    let Some(lines) = note else {
        return "waiting for note".to_string();
    };
    let key = journal::occurrence_key(
        &event.event.key.calendar_id,
        &event.event.key.event_id,
    );
    let state = journal.get(&(key, day.to_string()));
    if state.is_some_and(|state| state.gone) {
        return "removed by you".to_string();
    }
    match &event.inclusion {
        Inclusion::Omitted(_) => unreachable!("filtered above"),
        Inclusion::Context(_) => {
            if state.and_then(|state| state.row.as_ref()).is_some()
                && !row_present(lines, "", event_url(event), false)
            {
                return "removed by you".to_string();
            }
            "context".to_string()
        }
        Inclusion::Task => {
            if let Some(row) = state.and_then(|state| state.row.as_ref()) {
                if !row_present(lines, &row.block_id, event_url(event), true) {
                    return "removed by you".to_string();
                }
                if meet_link_present(lines, &row.block_id) {
                    return "task · MEET".to_string();
                }
                return "task".to_string();
            }
            match adopted_block_id(lines, event_url(event)) {
                Some(block_id) if meet_link_present(lines, &block_id) => {
                    "task · MEET".to_string()
                }
                _ => "task".to_string(),
            }
        }
    }
}

fn event_url(event: &ListedEvent) -> Option<&str> {
    event.event.html_link.as_deref()
}

/// A row is present when its journaled `^block_id` matches, or its
/// `📅` URL appears in the note.
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
    url.filter(|url| !url.is_empty())
        .is_some_and(|url| lines.iter().any(|line| line.contains(url)))
}

/// The `^block_id` of the note line carrying `url`, for unjournaled rows.
fn adopted_block_id(lines: &[&str], url: Option<&str>) -> Option<String> {
    let url = url.filter(|url| !url.is_empty())?;
    lines
        .iter()
        .find(|line| line.contains(url))
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

/// Header freshness: the host-local status file, else the newest
/// journal record's host and time, else never synced.
fn last_sync_human(
    newest: &Option<(chrono::DateTime<chrono::FixedOffset>, String)>,
) -> String {
    if let Some(status) = journal::read_status() {
        if status.outcome == "waiting" {
            return "waiting for note".to_string();
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
    calendars: &[Value],
    events: &[ListedEvent],
    counts: &Counts,
    include_omitted: bool,
    journal: &std::collections::BTreeMap<
        (String, String),
        journal::OccurrenceState,
    >,
    day_string: &str,
    note: Option<&[&str]>,
    last_sync: Value,
) -> Result<(), GcalError> {
    let rows: Vec<Value> = events
        .iter()
        .filter(|event| include_omitted || !matches!(event.inclusion, Inclusion::Omitted(_)))
        .map(|event| {
            let root = event.root.as_ref();
            json!({
                "time": event.time,
                "title": event.event.title,
                "answer": event.event.my_answer.as_str(),
                "inclusion": event.inclusion.name(),
                "reason": event.inclusion.reason(),
                "root": {
                    "value": root.and_then(|root| root.value.as_deref()),
                    "source": root.and_then(|root| root.source.as_deref()),
                    "warnings": root.map(|root| root.warnings.as_slice()).unwrap_or(&[]),
                },
                "vault": vault_status(event, journal, day_string, note),
            })
        })
        .collect();
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
    }))
}

#[allow(clippy::too_many_arguments)]
fn print_table(
    day: NaiveDate,
    calendars: &[Value],
    events: &[ListedEvent],
    counts: &Counts,
    include_omitted: bool,
    journal: &std::collections::BTreeMap<
        (String, String),
        journal::OccurrenceState,
    >,
    day_string: &str,
    note: Option<&[&str]>,
    sync_state: &str,
    journal_warnings: &[String],
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
    for event in events.iter().filter(|event| {
        include_omitted || !matches!(event.inclusion, Inclusion::Omitted(_))
    }) {
        let root_label = event
            .root
            .as_ref()
            .and_then(|root| root.value.as_ref())
            .map(|root| format!("● {root}"))
            .unwrap_or_else(|| "—".to_string());
        let vault = vault_status(event, journal, day_string, note);
        rows.push([
            event.time.clone(),
            event.event.title.clone(),
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
    for (event, row) in events
        .iter()
        .filter(|event| {
            include_omitted || !matches!(event.inclusion, Inclusion::Omitted(_))
        })
        .zip(rows.iter())
    {
        let mut cells = row.clone();
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
    for warning in journal_warnings {
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

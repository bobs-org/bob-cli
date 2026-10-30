//! Read-only `bob gcal list`: fetch a complete local-day snapshot, classify
//! every event, choose valid roots, then render a table or JSON envelope.

use std::{cmp::Ordering, path::PathBuf};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use serde_json::{json, Value};

use super::{
    calendars::resolve_account,
    classify::{self, DayWindow, Inclusion},
    config::GcalConfig,
    gog::Gog,
    model::{EventDateTime, GcalEvent, MyAnswer},
    root::{self, EventRoot},
    ui, GcalError, ListArgs,
};
use crate::native::{
    env as bob_env,
    style::{self, Styler},
};

struct ListedEvent {
    event: GcalEvent,
    inclusion: Inclusion,
    root: Option<EventRoot>,
    time: String,
    sort_at: DateTime<FixedOffset>,
}

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
    let timezone = bob_env::configured_timezone();
    let window = DayWindow::new(day, timezone)?;
    let vault = args.bob_dir();
    let daily_rel = daily_relative_path(day);
    let gog = Gog::resolve(config.timeout())?;
    let _ = gog.require_version()?;
    let account = resolve_account(&gog, &config)?;
    let calendars = gog.calendars(&account)?;

    let mut visible_calendars = Vec::new();
    let mut events = Vec::new();
    for configured in config.calendars() {
        let calendar = calendars
            .iter()
            .find(|calendar| calendar.matches_configured_id(&configured.id));
        let calendar_name = calendar
            .map(|calendar| calendar.name().to_string())
            .unwrap_or_else(|| configured.id.clone());
        visible_calendars.push(json!({
            "id": configured.id,
            "name": calendar_name,
        }));

        let raw_events = gog.events(
            &configured.id,
            &window.from_rfc3339(),
            &window.to_rfc3339(),
        )?;
        for raw_event in raw_events {
            let event = GcalEvent::from_google(&configured.id, raw_event)?;
            let inclusion = classify::classify(&event, &window, &config);
            let root = if inclusion == Inclusion::Task {
                Some(root::choose_event_root(
                    &vault,
                    &daily_rel,
                    &event,
                    &calendar_name,
                    &config,
                ))
            } else {
                None
            };
            let title = root
                .as_ref()
                .map(|chosen| chosen.title.clone())
                .unwrap_or_else(|| event.title.clone());
            let time = event_time(&event, &window);
            let sort_at = window.effective_start(&event);
            let mut event = event;
            event.title = title;
            events.push(ListedEvent {
                event,
                inclusion,
                root,
                time,
                sort_at,
            });
        }
    }
    events.sort_by(compare_events);
    let counts = counts(&events);
    if args.format.is_json() {
        print_json(
            day,
            &visible_calendars,
            &events,
            &counts,
            args.include_omitted,
        )
    } else {
        print_table(
            day,
            &visible_calendars,
            &events,
            &counts,
            args.include_omitted,
        )
    }
}

fn daily_relative_path(day: NaiveDate) -> PathBuf {
    PathBuf::from(format!(
        "{}/{:04}{:02}{:02}.md",
        day.year(),
        day.year(),
        day.month(),
        day.day()
    ))
}

fn event_time(event: &GcalEvent, window: &DayWindow) -> String {
    match (&event.start, &event.end) {
        (EventDateTime::AllDay(_), EventDateTime::AllDay(_)) => {
            "all day".to_string()
        }
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            format!("{}-{}", window.format_time(start), window.format_time(end))
        }
        _ => "all day".to_string(),
    }
}

fn compare_events(left: &ListedEvent, right: &ListedEvent) -> Ordering {
    let left_all_day = matches!(left.event.start, EventDateTime::AllDay(_));
    let right_all_day = matches!(right.event.start, EventDateTime::AllDay(_));
    left_all_day
        .cmp(&right_all_day)
        .reverse()
        .then_with(|| left.sort_at.cmp(&right.sort_at))
        .then_with(|| {
            left.event.key.calendar_id.cmp(&right.event.key.calendar_id)
        })
        .then_with(|| left.event.key.event_id.cmp(&right.event.key.event_id))
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

fn print_json(
    day: NaiveDate,
    calendars: &[Value],
    events: &[ListedEvent],
    counts: &Counts,
    include_omitted: bool,
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
    }))
}

fn print_table(
    day: NaiveDate,
    calendars: &[Value],
    events: &[ListedEvent],
    counts: &Counts,
    include_omitted: bool,
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
            "Google Calendar · {} · {calendar_names}",
            day.format("%a %Y-%m-%d")
        ))
    );
    let headers = ["TIME", "EVENT", "RSVP", "ROOT", "SOURCE", "SYNC WOULD"];
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
        let source = event
            .root
            .as_ref()
            .and_then(|root| root.source.clone())
            .unwrap_or_default();
        let action = match &event.inclusion {
            Inclusion::Task => "task".to_string(),
            Inclusion::Context(reason) => format!("context · {reason}"),
            Inclusion::Omitted(reason) => reason.clone(),
        };
        rows.push([
            event.time.clone(),
            event.event.title.clone(),
            answer_label(
                event.event.my_answer,
                event.event.organizer_or_no_guests,
            ),
            root_label,
            source,
            action,
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

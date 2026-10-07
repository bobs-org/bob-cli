//! Human and JSON rendering for `bob ref jobs list`.

use chrono::Local;
use serde_json::json;

use crate::native::style::{
    display_width, pad_right, terminal_width, truncate, Styler,
};

use super::spool::{parse_time, JobState, JobsView, ListedJob};

/// `list --format` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListFormat {
    Human,
    Json,
}

impl ListFormat {
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "json" => ListFormat::Json,
            _ => ListFormat::Human,
        }
    }
}

/// Short age without `ago`, e.g. `40 s`, `2 min`, `1 h`, `3 d`.
pub(crate) fn age_short_secs(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{} s", secs.max(1))
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86400 * 2 {
        format!("{} h", secs / 3600)
    } else {
        format!("{} d", secs / 86400)
    }
}

/// `40 s ago`, `2 min ago`, …; `time unknown` when unparseable.
pub(crate) fn age_phrase(value: &str) -> String {
    match parse_time(value) {
        Some(when) => {
            let secs = (Local::now() - when).num_seconds();
            format!("{} ago", age_short_secs(secs))
        }
        None => "time unknown".to_string(),
    }
}

/// `started …` basis for a clipping row.
fn started_phrase(job: &ListedJob) -> String {
    job.started_at
        .as_deref()
        .map(age_phrase)
        .unwrap_or_else(|| age_phrase(&job.created_at))
}

/// Print the `list` view. Read-only; always succeeds.
pub(crate) fn run_list(view: &JobsView, all: bool, format: ListFormat) -> i32 {
    match format {
        ListFormat::Json => print_list_json(view),
        ListFormat::Human => print_list_human(view, all),
    }
    0
}

fn state_label(state: JobState) -> &'static str {
    match state {
        JobState::Pending => "pending",
        JobState::Clipping => "clipping",
        JobState::Clipped => "clipped",
        JobState::InLibrary => "in library",
        JobState::AlreadyQueued => "already queued",
        JobState::FellBack => "fell back",
        JobState::Stuck => "stuck",
    }
}

fn state_glyph(state: JobState) -> &'static str {
    match state {
        JobState::Pending => "◷",
        JobState::Clipping => "⟳",
        JobState::Clipped => "✓",
        JobState::InLibrary => "≡",
        JobState::AlreadyQueued => "≡",
        JobState::FellBack => "↩",
        JobState::Stuck => "✗",
    }
}

fn header_counts(view: &JobsView) -> Vec<(&'static str, usize)> {
    let order = [
        (JobState::Pending, "pending"),
        (JobState::Clipping, "clipping"),
        (JobState::Clipped, "clipped"),
        (JobState::InLibrary, "in library"),
        (JobState::AlreadyQueued, "already queued"),
        (JobState::FellBack, "fell back"),
        (JobState::Stuck, "stuck"),
    ];
    order
        .iter()
        .filter_map(|(state, label)| {
            view.counts
                .get(state.as_str())
                .map(|count| (*label, *count))
        })
        .filter(|(_, count)| *count > 0)
        .collect()
}

fn print_list_human(view: &JobsView, all: bool) {
    let window = if all { "all" } else { "last 7 days" };
    let counts = header_counts(view);
    if counts.is_empty() {
        println!(
            "bob ref · jobs · nothing pending · nothing in the last 7 days"
        );
        return;
    }
    let parts = counts
        .iter()
        .map(|(label, count)| format!("{count} {label}"))
        .collect::<Vec<_>>()
        .join(" · ");
    println!("bob ref · jobs · {parts} · {window}");
    println!();
    let styler = Styler::detect();
    let width = terminal_width();
    let state_width = view
        .jobs
        .iter()
        .map(|job| display_width(state_label(job.state)))
        .max()
        .unwrap_or(0);
    for job in &view.jobs {
        let detail = row_detail(job, &styler);
        let display = row_display(job, &detail, state_width, width);
        let glyph = state_glyph(job.state);
        let state = pad_right(
            &paint_state(job.state, &styler),
            state_width + extra_paint_width(job.state, &styler),
        );
        println!("  {glyph} {state}  {display}  {detail}");
    }
    println!();
    println!(
        "next: bob ref scan turns clipped PDFs into ref notes (the Mac runs it every 15 minutes)"
    );
}

/// Paint the state word; plain when color is off.
fn paint_state(state: JobState, styler: &Styler) -> String {
    let label = state_label(state);
    match state {
        JobState::Pending => styler.yellow(label),
        JobState::Clipping => styler.cyan(label),
        JobState::Clipped => styler.green(label),
        JobState::InLibrary => styler.dim(label),
        JobState::AlreadyQueued => styler.dim(label),
        JobState::FellBack => styler.yellow(label),
        JobState::Stuck => styler.red(label),
    }
}

/// Extra columns the paint adds, so padding stays aligned when color
/// is on (zero when plain).
fn extra_paint_width(state: JobState, styler: &Styler) -> usize {
    if styler.is_color() {
        display_width(&paint_state(state, styler))
            - display_width(state_label(state))
    } else {
        0
    }
}

fn row_detail(job: &ListedJob, styler: &Styler) -> String {
    let dimmed = |text: String| styler.dim(&text);
    match job.state {
        JobState::Pending => {
            dimmed(format!("queued {}", age_phrase(&job.created_at)))
        }
        JobState::Clipping => {
            dimmed(format!("started {}", started_phrase(job)))
        }
        JobState::Clipped => dimmed(format!(
            "{} · {}",
            job.pdf.as_deref().unwrap_or("?"),
            job.finished_at
                .as_deref()
                .map(age_phrase)
                .unwrap_or_else(|| "time unknown".to_string())
        )),
        JobState::InLibrary => dimmed(format!(
            "{} · {}",
            job.note.as_deref().unwrap_or("?"),
            job.finished_at
                .as_deref()
                .map(age_phrase)
                .unwrap_or_else(|| "time unknown".to_string())
        )),
        JobState::AlreadyQueued => dimmed(format!(
            "{} · {}",
            job.pdf.as_deref().unwrap_or("?"),
            job.finished_at
                .as_deref()
                .map(age_phrase)
                .unwrap_or_else(|| "time unknown".to_string())
        )),
        JobState::FellBack => {
            let kind = job
                .error
                .as_ref()
                .map(|error| error.kind.as_str())
                .unwrap_or("failed");
            let target = job.fallback_target.as_deref().unwrap_or("?");
            dimmed(format!(
                "{kind} → {target} · {}",
                job.finished_at
                    .as_deref()
                    .map(age_phrase)
                    .unwrap_or_else(|| "time unknown".to_string())
            ))
        }
        JobState::Stuck => {
            let message = job
                .fallback_error
                .clone()
                .or_else(|| {
                    job.error.as_ref().map(|error| error.message.clone())
                })
                .unwrap_or_else(|| "unknown error".to_string());
            dimmed(format!(
                "fallback write failed: {} · bob ref jobs run",
                truncate(&first_line(&message), 60)
            ))
        }
    }
}

fn first_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn row_display(
    job: &ListedJob,
    detail: &str,
    state_width: usize,
    width: usize,
) -> String {
    // `  <glyph> <state>  <display>  <detail>`, truncated to the
    // terminal width. The display is cyan, like capture's URLs.
    let styler = Styler::detect();
    let reserved = 2 + 2 + state_width + 2 + 2 + display_width(detail);
    let available = width.saturating_sub(reserved).max(8);
    let text = truncate(&job.display, available);
    styler.cyan(&text)
}

fn print_list_json(view: &JobsView) {
    let jobs = view
        .jobs
        .iter()
        .map(|job| {
            let mut entry = json!({
                "id": job.id,
                "state": job.state.as_str(),
                "url": job.url,
                "display": job.display,
                "created_at": job.created_at,
            });
            if let Some(started) = &job.started_at {
                entry["started_at"] = json!(started);
            }
            if let Some(finished) = &job.finished_at {
                entry["finished_at"] = json!(finished);
            }
            if let Some(pdf) = &job.pdf {
                entry["pdf"] = json!(pdf);
            }
            if let Some(note) = &job.note {
                entry["note"] = json!(note);
            }
            if let Some(error) = &job.error {
                entry["error"] = json!({
                    "kind": error.kind,
                    "message": error.message,
                    "retryable": error.retryable,
                });
            }
            if let Some(target) = &job.fallback_target
                && matches!(
                    job.state,
                    JobState::Pending
                        | JobState::Clipping
                        | JobState::FellBack
                        | JobState::Stuck
                )
            {
                entry["fallback"] = json!({ "relative_target": target });
            }
            if let Some(path) = &job.path {
                entry["path"] = json!(path.to_string_lossy());
            }
            entry
        })
        .collect::<Vec<_>>();
    let summary = json!({
        "pending": view.counts.get("pending").copied().unwrap_or(0),
        "clipping": view.counts.get("clipping").copied().unwrap_or(0),
        "clipped": view.counts.get("clipped").copied().unwrap_or(0),
        "in_library": view.counts.get("in_library").copied().unwrap_or(0),
        "already_queued": view.counts.get("already_queued").copied().unwrap_or(0),
        "fell_back": view.counts.get("fell_back").copied().unwrap_or(0),
        "stuck": view.counts.get("stuck").copied().unwrap_or(0),
    });
    println!(
        "{}",
        json!({
            "schema_version": super::spool::SCHEMA_VERSION,
            "ok": true,
            "jobs": jobs,
            "summary": summary,
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn age_buckets_match_human_table() {
        assert_eq!(age_short_secs(0), "1 s");
        assert_eq!(age_short_secs(40), "40 s");
        assert_eq!(age_short_secs(120), "2 min");
        assert_eq!(age_short_secs(3600), "1 h");
        assert_eq!(age_short_secs(5 * 3600), "5 h");
        assert_eq!(age_short_secs(3 * 86400), "3 d");
    }

    #[test]
    fn empty_view_counts_nothing() {
        let view = JobsView {
            jobs: Vec::new(),
            counts: HashMap::new(),
        };
        assert!(header_counts(&view).is_empty());
    }
}

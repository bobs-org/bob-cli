//! `bob gcal sync`: one bounded, idempotent reconciliation of one day.
//!
//! Fetch outside any lock; any failure means the snapshot is
//! incomplete, so write nothing and exit 1. Then take the vault lock,
//! re-read and re-plan, write through preimage-checked
//! `write_staged_files` with a single retry, and append the journal
//! only after the note write succeeds.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use chrono::{NaiveDate, Timelike};
use serde_json::{json, Value};

use super::{
    classify::{self, Inclusion},
    config::GcalConfig,
    gog::Gog,
    journal::{self, OccurrenceState},
    lifecycle::{AbsentVerdict, LifecycleItem},
    model::{EventDateTime, GcalEvent},
    plan::{self, PlannedOccurrence},
    render,
    snapshot::{self},
    ui, GcalError, SyncArgs,
};
use crate::native::{
    capture::{write_staged_files, CaptureFailureKind, StagedTextFile},
    env as bob_env, ob,
    pomodoro::pomodoros_section_range,
    pref_journal,
    style::Styler,
};

pub(crate) fn run(args: &SyncArgs) -> i32 {
    let format = args.error_format();
    match sync(args) {
        Ok(report) => {
            print_report(&report, args);
            0
        }
        Err(error) => {
            record_last_error(args, &error);
            ui::report_error("sync", &error, format)
        }
    }
}

/// Remember a failed run in host-local status so `doctor` can show the
/// last error. Best effort: a broken config must not hide the error.
fn record_last_error(args: &SyncArgs, error: &GcalError) {
    let today = bob_env::current_datetime().date();
    let Ok(day) = classify::resolve_day(&args.day, today) else {
        return;
    };
    record_status(
        &journal::host_label(),
        day,
        "error",
        &SyncReport {
            day,
            path: String::new(),
            status: "error",
            rows: Vec::new(),
            entry_line: None,
            entry_created: false,
            links_added: 0,
            warnings: Vec::new(),
            quiet: true,
            json: false,
            diff: None,
        },
        Some(format!("{}: {}", error.kind(), error.message())),
    );
}

struct ReportRow {
    action: &'static str,
    kind: &'static str,
    time: String,
    title: String,
    root: Option<String>,
    source: Option<String>,
}

struct SyncReport {
    day: NaiveDate,
    path: String,
    status: &'static str,
    rows: Vec<ReportRow>,
    entry_line: Option<usize>,
    entry_created: bool,
    links_added: usize,
    warnings: Vec<String>,
    quiet: bool,
    json: bool,
    diff: Option<String>,
}

fn sync(args: &SyncArgs) -> Result<SyncReport, GcalError> {
    let config = GcalConfig::resolve()?;
    let host = journal::host_label();
    if !args.dry_run
        && let Some(writer) = config.writer()
        && writer != host
    {
        return Err(GcalError::setup(
            "writer",
            format!("this host is {host}; gcal.writer is {writer}"),
        )
        .with_hint(&format!(
            "run `bob gcal sync` on {writer}, or set gcal.writer: {host}"
        )));
    }
    let today = bob_env::current_datetime().date();
    let day = classify::resolve_day(&args.day, today)?;
    let is_today = day == today;
    let vault = args.bob_dir();

    // Fetch outside any lock; an incomplete snapshot writes nothing.
    let snapshot = snapshot::fetch_day(vault, &config, day)?;
    let vault = snapshot.vault.clone();
    let day = snapshot.day;
    let daily_rel = snapshot.daily_rel.clone();
    let note_path = vault.join(&daily_rel);
    let display_path = display_rel(&vault, &note_path);

    let contents = match std::fs::read_to_string(&note_path) {
        Ok(contents) => contents,
        Err(_) => {
            return waiting(
                args,
                &host,
                day,
                &display_path,
                &journal_warnings(&vault)?,
            );
        }
    };
    let refs: Vec<&str> = contents.lines().collect();
    if pomodoros_section_range(&refs).is_none() {
        return waiting(
            args,
            &host,
            day,
            &display_path,
            &journal_warnings(&vault)?,
        );
    }

    let (journal_state, mut warnings, _) = journal::load_state(&vault)
        .map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("read the gcal journal: {error}"),
            )
        })?;
    let now = bob_env::current_datetime();
    let mut occurrences = Vec::new();
    for (occurrence, warning) in
        build_occurrences(&snapshot, &vault, &daily_rel, is_today, &now)
    {
        if let Some(warning) = warning {
            warnings.push(warning);
        }
        occurrences.push(occurrence);
    }
    // Lifecycle verdicts for journaled occurrences with no included
    // snapshot row. A failed lookup makes the snapshot incomplete, so
    // nothing is written.
    let lifecycle =
        resolve_absent(&snapshot, &occurrences, &journal_state, &day)?;

    let plan = plan::plan_sync(&plan::SyncPlanInput {
        contents: &contents,
        is_today,
        occurrences: occurrences.clone(),
        journal: &journal_state,
        day_string: day.to_string(),
        today: now.date(),
        lifecycle: lifecycle.clone(),
        series_roots: &snapshot.series_roots,
    });

    if args.dry_run {
        let diff = unified_diff(&contents, &plan.contents, &display_path);
        return Ok(SyncReport {
            day,
            path: display_path,
            status: "dry_run",
            rows: report_rows(&plan),
            entry_line: plan.pomodoro_entry_line,
            entry_created: plan.pomodoro_created,
            links_added: plan.links_added.len(),
            warnings,
            quiet: args.quiet,
            json: args.format.is_json(),
            diff: Some(diff),
        });
    }

    if !plan.changed {
        let report = SyncReport {
            day,
            path: display_path.clone(),
            status: "unchanged",
            rows: Vec::new(),
            entry_line: None,
            entry_created: false,
            links_added: 0,
            warnings,
            quiet: args.quiet,
            json: args.format.is_json(),
            diff: None,
        };
        record_status(&host, day, "unchanged", &report, None);
        return Ok(report);
    }

    // Take the vault lock, re-read and re-plan, then write through the
    // preimage-checked staged writer with a single retry.
    let styler = Styler::detect();
    let _lock = match ob::acquire_lock_waiting(Duration::from_secs(30), || {
        eprintln!(
            "{}",
            styler.dim("waiting for another vault maintenance run…")
        );
    }) {
        Ok(lock) => lock,
        Err(error) => {
            return Err(GcalError::runtime(
                "lock",
                format!("acquire the vault lock: {error}"),
            )
            .with_hint("another run held the lock for 30 s; try again"));
        }
    };
    let mut plan = plan;
    let mut current = contents.clone();
    for attempt in 0..2 {
        if plan.contents == current {
            // Journal-only change (tombstones, adoptions): the note
            // is already byte-identical, so write nothing to it.
            break;
        }
        let staged = StagedTextFile {
            target: note_path.clone(),
            target_existed: true,
            original_target: current.clone(),
            updated_target: plan.contents.clone(),
        };
        match write_staged_files(std::slice::from_ref(&staged)) {
            Ok(()) => break,
            Err(error)
                if error.typed == Some(CaptureFailureKind::NoteChanged)
                    && attempt == 0 =>
            {
                current =
                    std::fs::read_to_string(&note_path).map_err(|error| {
                        GcalError::runtime(
                            "note",
                            format!("re-read {}: {error}", note_path.display()),
                        )
                    })?;
                plan = plan::plan_sync(&plan::SyncPlanInput {
                    contents: &current,
                    is_today,
                    occurrences: occurrences.clone(),
                    journal: &journal_state,
                    day_string: day.to_string(),
                    today: now.date(),
                    lifecycle: lifecycle.clone(),
                    series_roots: &snapshot.series_roots,
                });
                if !plan.changed {
                    let report = SyncReport {
                        day,
                        path: display_path.clone(),
                        status: "unchanged",
                        rows: Vec::new(),
                        entry_line: None,
                        entry_created: false,
                        links_added: 0,
                        warnings,
                        quiet: args.quiet,
                        json: args.format.is_json(),
                        diff: None,
                    };
                    record_status(&host, day, "unchanged", &report, None);
                    return Ok(report);
                }
            }
            Err(error) => {
                return Err(GcalError::runtime(
                    "note",
                    format!("write {}: {}", note_path.display(), error.message),
                ));
            }
        }
    }

    // Append the journal only after the note write succeeds.
    append_journal(&vault, &host, &plan, &display_path, day)?;

    for kept in &plan.kept {
        warnings.push(format!("{}: {}", kept.title, kept.detail));
    }

    let report = SyncReport {
        day,
        path: display_path,
        status: "written",
        rows: report_rows(&plan),
        entry_line: plan.pomodoro_entry_line,
        entry_created: plan.pomodoro_created,
        links_added: plan.links_added.len(),
        warnings,
        quiet: args.quiet,
        json: args.format.is_json(),
        diff: None,
    };
    record_status(&host, day, "written", &report, None);
    Ok(report)
}

fn waiting(
    args: &SyncArgs,
    host: &str,
    day: NaiveDate,
    display_path: &str,
    warnings: &[String],
) -> Result<SyncReport, GcalError> {
    let report = SyncReport {
        day,
        path: display_path.to_string(),
        status: "waiting",
        rows: Vec::new(),
        entry_line: None,
        entry_created: false,
        links_added: 0,
        warnings: warnings.to_vec(),
        quiet: args.quiet,
        json: args.format.is_json(),
        diff: None,
    };
    if !args.dry_run {
        record_status(host, day, "waiting", &report, None);
    }
    Ok(report)
}

fn journal_warnings(vault: &std::path::Path) -> Result<Vec<String>, GcalError> {
    journal::load_state(vault)
        .map(|(_, warnings, _)| warnings)
        .map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("read the gcal journal: {error}"),
            )
        })
}

/// Build time-ordered plan occurrences from the snapshot, deciding
/// roots, the `[type:: MEET]` field, badges, span hashes, and MEET
/// eligibility. Returns each occurrence with its root warnings.
fn build_occurrences(
    snapshot: &snapshot::DaySnapshot,
    vault: &std::path::Path,
    daily_rel: &std::path::Path,
    is_today: bool,
    now: &chrono::NaiveDateTime,
) -> Vec<(PlannedOccurrence, Option<String>)> {
    let mut out = Vec::new();
    for event in &snapshot.events {
        if matches!(event.inclusion, Inclusion::Omitted(_)) {
            continue;
        }
        let task = event.inclusion == Inclusion::Task;
        let (all_day, start_min, time_label) =
            row_time(&snapshot.window, &event.event);
        let root_value =
            event.root.as_ref().and_then(|root| root.value.clone());
        let root_source =
            event.root.as_ref().and_then(|root| root.source.clone());
        let needs_type = if task {
            needs_meet_type(
                vault,
                daily_rel,
                &time_label,
                &event.event.title,
                root_value.as_deref(),
            )
        } else {
            false
        };
        let badges = render::badge_text(
            event.event.html_link.as_deref(),
            event.event.join_link.as_deref(),
            event.event.location.as_deref(),
        );
        let mut spans = BTreeMap::new();
        spans.insert("time".to_string(), journal::span_hash(&time_label));
        spans.insert(
            "title".to_string(),
            journal::span_hash(&event.event.title),
        );
        spans.insert("badges".to_string(), journal::span_hash(&badges));
        let qualifier = if task {
            None
        } else {
            context_qualifier(event.inclusion.reason())
        };
        let link_eligible =
            task && is_today && !has_ended(&event.event, &snapshot.window, now);
        let warnings = event
            .root
            .as_ref()
            .map(|root| {
                if root.warnings.is_empty() {
                    None
                } else {
                    Some(
                        root.warnings
                            .iter()
                            .map(|warning| {
                                format!(
                                    "root for {}: {warning}",
                                    event.event.title
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; "),
                    )
                }
            })
            .unwrap_or(None);
        out.push((
            PlannedOccurrence {
                calendar_id: event.event.key.calendar_id.clone(),
                event_id: event.event.key.event_id.clone(),
                ical_uid: event.event.ical_uid.clone(),
                series_id: event.event.series_id.clone(),
                id_hint: event.event.title.clone(),
                task,
                time_label,
                title: event.event.title.clone(),
                html_link: event.event.html_link.clone(),
                join_link: event.event.join_link.clone(),
                location: event.event.location.clone(),
                root: root_value,
                source: root_source,
                qualifier,
                spans,
                needs_type,
                link_eligible,
                all_day,
                start_min,
            },
            warnings,
        ));
    }
    out
}

/// `(all_day, start_min, time_label)` for one event in vault-local time.
fn row_time(
    window: &classify::DayWindow,
    event: &super::model::GcalEvent,
) -> (bool, i32, String) {
    match (&event.start, &event.end) {
        (EventDateTime::AllDay(_), EventDateTime::AllDay(_)) => {
            (true, 0, "all day".to_string())
        }
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            let local_start = window.local(start);
            let local_end = window.local(end);
            let label = format!(
                "{:02}{:02}-{:02}{:02}",
                local_start.hour(),
                local_start.minute(),
                local_end.hour(),
                local_end.minute(),
            );
            let start_min =
                local_start.hour() as i32 * 60 + local_start.minute() as i32;
            (false, start_min, label)
        }
        _ => (true, 0, "all day".to_string()),
    }
}

fn needs_meet_type(
    vault: &std::path::Path,
    daily_rel: &std::path::Path,
    time_label: &str,
    title: &str,
    root: Option<&str>,
) -> bool {
    let mut probe = format!("- [ ] #task **{time_label}** {title}");
    if let Some(root) = root {
        probe = crate::native::task_root::set_task_root(&probe, root);
    }
    render::needs_meet_type(vault, daily_rel, &probe)
}

/// Verdicts for journaled occurrences with no included snapshot row:
/// declined, deleted, or moved events cancel or remove untouched rows;
/// anything else is left alone and reported once. A journaled row the
/// user already deleted tombstones without a lookup. A failed lookup
/// makes the whole snapshot incomplete, so nothing is written.
fn resolve_absent(
    snapshot: &snapshot::DaySnapshot,
    occurrences: &[PlannedOccurrence],
    journal: &BTreeMap<(String, String), OccurrenceState>,
    day: &NaiveDate,
) -> Result<Vec<LifecycleItem>, GcalError> {
    use std::collections::HashSet;
    let day_string = day.to_string();
    let present: HashSet<(String, String)> = occurrences
        .iter()
        .map(|occurrence| {
            (occurrence.calendar_id.clone(), occurrence.event_id.clone())
        })
        .collect();
    let mut missing: Vec<(String, String)> = journal
        .iter()
        .filter(|((_, entry_day), state)| {
            entry_day == &day_string && !state.gone
        })
        .filter_map(|((key, _), _)| {
            key.split_once('\u{1f}').map(|(calendar, event)| {
                (calendar.to_string(), event.to_string())
            })
        })
        .filter(|key| !present.contains(key))
        .collect();
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    let mut transport: Option<Gog> = None;
    let mut out = Vec::new();
    for (calendar_id, event_id) in missing {
        let verdict = match snapshot.events.iter().find(|listed| {
            listed.event.key.calendar_id == calendar_id
                && listed.event.key.event_id == event_id
        }) {
            Some(listed) => match &listed.inclusion {
                Inclusion::Omitted(reason) if reason == "declined" => {
                    AbsentVerdict::Declined
                }
                Inclusion::Omitted(reason) if reason == "cancelled" => {
                    AbsentVerdict::Deleted
                }
                Inclusion::Omitted(reason) if reason == "reply in Calendar" => {
                    AbsentVerdict::LeftAlone(
                        "awaiting your reply in Google Calendar".to_string(),
                    )
                }
                Inclusion::Omitted(reason) => {
                    AbsentVerdict::LeftAlone(reason.clone())
                }
                _ => continue,
            },
            None => {
                if transport.is_none() {
                    let config = GcalConfig::resolve()?;
                    transport = Some(Gog::resolve(config.timeout())?);
                }
                let gog = transport.as_ref().expect("gog resolved");
                lookup_verdict(gog, snapshot, &calendar_id, &event_id, day)?
            }
        };
        out.push(LifecycleItem {
            calendar_id,
            event_id,
            verdict,
        });
    }
    Ok(out)
}

/// Look up one journaled occurrence that fell out of the day snapshot:
/// deleted (404/410, or `status: cancelled`), moved to another day,
/// or declined. A failed lookup is an incomplete snapshot: the caller
/// writes nothing.
fn lookup_verdict(
    gog: &Gog,
    snapshot: &snapshot::DaySnapshot,
    calendar_id: &str,
    event_id: &str,
    day: &NaiveDate,
) -> Result<AbsentVerdict, GcalError> {
    let raw = match gog.event(calendar_id, event_id, &snapshot.account) {
        Err(error) if error.kind() == "not_found" => {
            return Ok(AbsentVerdict::Deleted);
        }
        Err(error) => {
            return Err(GcalError::runtime(
                "incomplete_snapshot",
                format!(
                    "look up {calendar_id}/{event_id}: {}",
                    error.message()
                ),
            ));
        }
        Ok(raw) => raw,
    };
    let event = GcalEvent::from_google(calendar_id, raw).map_err(|error| {
        GcalError::runtime(
            "incomplete_snapshot",
            format!("parse {calendar_id}/{event_id}: {}", error.message()),
        )
    })?;
    if event.status == "cancelled" {
        return Ok(AbsentVerdict::Deleted);
    }
    let starts = match &event.start {
        EventDateTime::Timed(start) => snapshot.window.local(start).date(),
        EventDateTime::AllDay(date) => *date,
    };
    if starts != *day {
        return Ok(AbsentVerdict::Moved(
            starts.format("%a %Y-%m-%d").to_string(),
        ));
    }
    if event.my_answer == super::model::MyAnswer::No {
        return Ok(AbsentVerdict::Declined);
    }
    Ok(AbsentVerdict::LeftAlone(
        "still listed on this day".to_string(),
    ))
}

/// Whether the timed event already ended at sync time, in vault-local
/// wall time.
fn has_ended(
    event: &super::model::GcalEvent,
    window: &classify::DayWindow,
    now: &chrono::NaiveDateTime,
) -> bool {
    match &event.end {
        EventDateTime::Timed(end) => window.local(end) <= *now,
        EventDateTime::AllDay(_) => false,
    }
}

/// Context-row qualifier from the inclusion reason.
fn context_qualifier(reason: Option<&str>) -> Option<String> {
    match reason {
        None | Some("all-day") => None,
        Some(reason) => reason
            .strip_prefix("all-day · ")
            .map(str::to_string)
            .or_else(|| Some(reason.to_string())),
    }
}

fn report_rows(plan: &plan::SyncPlan) -> Vec<ReportRow> {
    let mut rows = Vec::new();
    for added in &plan.rows_added {
        rows.push(ReportRow {
            action: "added",
            kind: if added.row_kind == "task" {
                "task"
            } else {
                "context"
            },
            time: added.time_label.clone(),
            title: added.title.clone(),
            root: added.root.clone(),
            source: added.source.clone(),
        });
    }
    for adopted in &plan.rows_adopted {
        rows.push(ReportRow {
            action: "adopted",
            kind: if adopted.row_kind == "task" {
                "task"
            } else {
                "context"
            },
            time: adopted.time_label.clone(),
            title: adopted.title.clone(),
            root: adopted.root.clone(),
            source: adopted.source.clone(),
        });
    }
    for _ in &plan.rows_gone {
        rows.push(ReportRow {
            action: "removed",
            kind: "task",
            time: String::new(),
            title: String::new(),
            root: None,
            source: None,
        });
    }
    for updated in &plan.rows_updated {
        rows.push(ReportRow {
            action: "updated",
            kind: if updated.row_kind == "task" {
                "task"
            } else {
                "context"
            },
            time: updated.time_label.clone(),
            title: updated.title.clone(),
            root: updated.root.clone(),
            source: updated.source.clone(),
        });
    }
    for cancelled in &plan.rows_cancelled {
        rows.push(ReportRow {
            action: "cancelled",
            kind: "task",
            time: cancelled.time_label.clone(),
            title: format!("{} ({})", cancelled.title, cancelled.reason),
            root: None,
            source: None,
        });
    }
    for removed in &plan.rows_removed {
        rows.push(ReportRow {
            action: "removed",
            kind: "context",
            time: String::new(),
            title: removed.reason.clone(),
            root: None,
            source: None,
        });
    }
    for upgraded in &plan.rows_upgraded {
        rows.push(ReportRow {
            action: "upgraded",
            kind: "task",
            time: upgraded.time_label.clone(),
            title: upgraded.title.clone(),
            root: upgraded.root.clone(),
            source: upgraded.source.clone(),
        });
    }
    for learned in &plan.learned {
        rows.push(ReportRow {
            action: "learned",
            kind: "series",
            time: String::new(),
            title: learned.title.clone(),
            root: Some(learned.root.clone()),
            source: Some(format!("learned from your {} edit", learned.day)),
        });
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn append_row_record(
    vault: &std::path::Path,
    host: &str,
    day: &str,
    display_path: &str,
    calendar_id: &str,
    event_id: &str,
    ical_uid: Option<&str>,
    series_id: Option<&str>,
    block_id: &str,
    url: Option<&str>,
    row_kind: &str,
    spans: &BTreeMap<String, String>,
    root: Option<&str>,
    source: Option<&str>,
) -> Result<(), GcalError> {
    journal::append_row(
        vault,
        host,
        &journal::NewRow {
            calendar_id: calendar_id.to_string(),
            event_id: event_id.to_string(),
            ical_uid: ical_uid.map(str::to_string),
            series_id: series_id.map(str::to_string),
            day: day.to_string(),
            path: display_path.to_string(),
            block_id: block_id.to_string(),
            url: url.map(str::to_string),
            row_kind: row_kind.to_string(),
            spans: spans.clone(),
            root: root.map(str::to_string),
            source: source.map(str::to_string),
        },
    )
    .map(|_| ())
    .map_err(|error| {
        GcalError::runtime(
            "journal",
            format!("append the gcal journal: {error}"),
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn append_mark_record(
    vault: &std::path::Path,
    host: &str,
    kind: &str,
    calendar_id: &str,
    event_id: &str,
    day: &str,
    block_id: Option<&str>,
    reason: Option<&str>,
) -> Result<(), GcalError> {
    journal::append_mark(
        vault,
        host,
        kind,
        &journal::NewMark {
            calendar_id: calendar_id.to_string(),
            event_id: event_id.to_string(),
            day: day.to_string(),
            block_id: block_id.map(str::to_string),
            reason: reason.map(str::to_string),
        },
    )
    .map(|_| ())
    .map_err(|error| {
        GcalError::runtime(
            "journal",
            format!("append the gcal journal: {error}"),
        )
    })
}

fn append_journal(
    vault: &std::path::Path,
    host: &str,
    plan: &plan::SyncPlan,
    display_path: &str,
    day: NaiveDate,
) -> Result<(), GcalError> {
    let day_string = day.to_string();
    for added in &plan.rows_added {
        append_row_record(
            vault,
            host,
            &day_string,
            display_path,
            &added.calendar_id,
            &added.event_id,
            added.ical_uid.as_deref(),
            added.series_id.as_deref(),
            &added.block_id,
            added.url.as_deref(),
            &added.row_kind,
            &added.spans,
            added.root.as_deref(),
            added.source.as_deref(),
        )?;
    }
    for adopted in &plan.rows_adopted {
        append_row_record(
            vault,
            host,
            &day_string,
            display_path,
            &adopted.calendar_id,
            &adopted.event_id,
            adopted.ical_uid.as_deref(),
            adopted.series_id.as_deref(),
            &adopted.block_id,
            adopted.url.as_deref(),
            &adopted.row_kind,
            &adopted.spans,
            adopted.root.as_deref(),
            adopted.source.as_deref(),
        )?;
    }
    for updated in &plan.rows_updated {
        append_row_record(
            vault,
            host,
            &day_string,
            display_path,
            &updated.calendar_id,
            &updated.event_id,
            updated.ical_uid.as_deref(),
            updated.series_id.as_deref(),
            &updated.block_id,
            updated.url.as_deref(),
            &updated.row_kind,
            &updated.spans,
            updated.root.as_deref(),
            updated.source.as_deref(),
        )?;
    }
    for upgraded in &plan.rows_upgraded {
        append_row_record(
            vault,
            host,
            &day_string,
            display_path,
            &upgraded.calendar_id,
            &upgraded.event_id,
            upgraded.ical_uid.as_deref(),
            upgraded.series_id.as_deref(),
            &upgraded.block_id,
            upgraded.url.as_deref(),
            &upgraded.row_kind,
            &upgraded.spans,
            upgraded.root.as_deref(),
            upgraded.source.as_deref(),
        )?;
    }
    for adopted in &plan.rows_adopted {
        if adopted.link_adopted {
            journal::append_mark(
                vault,
                host,
                "gcal.link",
                &journal::NewMark {
                    calendar_id: adopted.calendar_id.clone(),
                    event_id: adopted.event_id.clone(),
                    day: day_string.clone(),
                    block_id: Some(adopted.block_id.clone()),
                    reason: None,
                },
            )
            .map_err(|error| {
                GcalError::runtime(
                    "journal",
                    format!("append the gcal journal: {error}"),
                )
            })?;
        }
    }
    for gone in plan.rows_gone.iter().chain(plan.rows_removed.iter()) {
        append_mark_record(
            vault,
            host,
            "gcal.gone",
            &gone.calendar_id,
            &gone.event_id,
            &day_string,
            Some(gone.block_id.as_str()).filter(|id| !id.is_empty()),
            Some(gone.reason.as_str()),
        )?;
    }
    for kept in &plan.kept {
        append_mark_record(
            vault,
            host,
            "gcal.flag",
            &kept.calendar_id,
            &kept.event_id,
            &day_string,
            None,
            Some(kept.flag.as_str()),
        )?;
    }
    for learned in &plan.learned {
        journal::append_series_root(
            vault,
            host,
            &learned.calendar_id,
            &learned.series,
            &learned.root,
            &learned.day,
            &learned.block_id,
        )
        .map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("append the gcal journal: {error}"),
            )
        })?;
    }
    for link in &plan.links_added {
        journal::append_mark(
            vault,
            host,
            "gcal.link",
            &journal::NewMark {
                calendar_id: link.calendar_id.clone(),
                event_id: link.event_id.clone(),
                day: day_string.clone(),
                block_id: Some(link.block_id.clone()),
                reason: None,
            },
        )
        .map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("append the gcal journal: {error}"),
            )
        })?;
    }
    Ok(())
}

fn record_status(
    host: &str,
    day: NaiveDate,
    outcome: &str,
    report: &SyncReport,
    error: Option<String>,
) {
    let tasks_added = report
        .rows
        .iter()
        .filter(|row| row.action == "added" && row.kind == "task")
        .count();
    let context_added = report
        .rows
        .iter()
        .filter(|row| row.action == "added" && row.kind == "context")
        .count();
    let _ = journal::write_status(&journal::SyncStatus {
        ts: pref_journal::event_timestamp(),
        host: host.to_string(),
        day: day.to_string(),
        outcome: outcome.to_string(),
        tasks_added,
        context_added,
        links_added: report.links_added,
        error,
    });
}

fn display_rel(vault: &std::path::Path, path: &PathBuf) -> String {
    path.strip_prefix(vault)
        .map(|rel| rel.display().to_string().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

fn print_report(report: &SyncReport, _args: &SyncArgs) {
    if report.json {
        print_json(report);
    } else if let Some(diff) = report.diff.as_deref() {
        print!("{diff}");
    } else {
        print_human(report);
    }
}

fn print_json(report: &SyncReport) {
    let rows: Vec<Value> = report
        .rows
        .iter()
        .map(|row| {
            json!({
                "action": row.action,
                "kind": row.kind,
                "time": row.time,
                "title": row.title,
                "root": row.root,
                "source": row.source,
            })
        })
        .collect();
    println!(
        "{}",
        json!({
            "schema_version": 1,
            "ok": true,
            "day": report.day.to_string(),
            "path": report.path,
            "status": report.status,
            "rows": rows,
            "pomodoro": {
                "entry_line": report.entry_line,
                "created": report.entry_created,
                "links_added": report.links_added,
            },
            "warnings": report.warnings,
        })
    );
}

/// Unified diff of the dry-run plan, `---`/`+++` plus `@@` hunks.
pub(crate) fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out = format!("--- {path}\n+++ {path}\n");
    let mut unified = diff.unified_diff();
    unified.context_radius(3);
    for hunk in unified.iter_hunks() {
        out.push_str(&format!("{}\n", hunk.header()));
        for change in hunk.iter_changes() {
            let value = change.value().trim_end_matches(['\r', '\n']);
            out.push_str(&format!("{}{value}\n", change.tag()));
        }
    }
    out
}

fn print_human(report: &SyncReport) {
    if report.quiet {
        return;
    }
    let styler = Styler::detect();
    if report.status == "waiting" {
        println!("bob gcal sync · {}", report.path);
        println!("waiting for {}", report.path);
        return;
    }
    println!("bob gcal sync · {}", report.path);
    for row in &report.rows {
        let sigil = match row.action {
            "added" => "+",
            "adopted" => "=",
            "updated" => "~",
            "upgraded" => "^",
            "learned" => "*",
            _ => "-",
        };
        let time = human_time(&row.time);
        let root = row
            .root
            .as_deref()
            .map(|root| format!("  ● {root}"))
            .unwrap_or_default();
        let source = row
            .source
            .as_deref()
            .map(|source| format!("  via {source}"))
            .unwrap_or_default();
        let detail = match row.action {
            "removed" if row.kind == "task" => "removed by you".to_string(),
            "removed" => format!("removed ({})", row.title),
            _ => format!("{time}  {}{root}{source}", row.title),
        };
        println!("  {sigil} {:<7} {detail}", row.kind);
    }
    if report.links_added > 0 {
        let made = if report.entry_created { "created " } else { "" };
        println!(
            "  {} MEET     {}() — MEET · linked {}",
            if report.entry_created { "+" } else { "=" },
            made,
            report.links_added,
        );
    }
    let count = |action: &str, kind: &str| {
        report
            .rows
            .iter()
            .filter(|row| row.action == action && row.kind == kind)
            .count()
    };
    let tasks = count("added", "task");
    let context = count("added", "context");
    let summary = if report.status == "unchanged" {
        "✓ Events unchanged · Pomodoros unchanged".to_string()
    } else {
        let mut parts = vec![
            format!("+{tasks} tasks"),
            format!("+{context} context"),
            format!(
                "~{} updated",
                count("updated", "task") + count("updated", "context")
            ),
            format!("-{} cancelled", count("cancelled", "task")),
            format!("^{} upgraded", count("upgraded", "task")),
        ];
        parts.retain(|part| {
            !part.starts_with("+0")
                && !part.starts_with("~0")
                && !part.starts_with("-0")
                && !part.starts_with("^0")
        });
        if parts.is_empty() {
            parts.push("+0 tasks".to_string());
        }
        format!(
            "✓ Events {} · Pomodoros +{} entry, +{} links",
            parts.join(", "),
            usize::from(report.entry_created),
            report.links_added,
        )
    };
    println!("{summary}");
    for warning in &report.warnings {
        eprintln!("{}", styler.dim(&format!("warning: {warning}")));
    }
}

/// `0800-0830` → `08:00–08:30`; `all day` stays as written.
fn human_time(time: &str) -> String {
    if time.len() == 9 && time.as_bytes().get(4) == Some(&b'-') {
        format!(
            "{}:{}–{}:{}",
            &time[..2],
            &time[2..4],
            &time[5..7],
            &time[7..9]
        )
    } else {
        time.to_string()
    }
}

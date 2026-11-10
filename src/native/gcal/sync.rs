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
    description::bullets::render_bullets,
    diagnostics,
    gog::Gog,
    journal::{self, OccurrenceState},
    lifecycle::{AbsentVerdict, LifecycleItem},
    model::{EventDateTime, GcalEvent},
    plan::{self, LinkPhase, MeetPomodoroEntry, PlannedOccurrence},
    render, replaced,
    skip::{self, SkipReason, SkippedEvent},
    snapshot::{self},
    stamp::{self, Stamp},
    ui, GcalError, SyncArgs,
};
use crate::native::{
    capture::{write_staged_files, CaptureFailureKind, StagedTextFile},
    env as bob_env, ob,
    pomodoro::pomodoros_section_range,
    pomodoro_homes::{DiskHomeSource, HomeResolver, HomeSource},
    pref_journal,
    style::Styler,
    templater::{first_pending_tag, PendingTag},
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
            entries: Vec::new(),
            links_added: 0,
            links_refiled: 0,
            links_sorted: 0,
            warnings: Vec::new(),
            deprecations: Vec::new(),
            quiet: true,
            force: args.force,
            json: false,
            diff: None,
            stamp: None,
            stamp_refreshed: false,
            waiting: None,
            skipped: Vec::new(),
            in_sync: 0,
            held: None,
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
    off_clock: bool,
    untracked: bool,
    buffer: crate::native::event_task::EventBuffer,
    /// Bullet lines this run wrote for the row, 0 otherwise.
    description_lines: usize,
    /// Why a removed row left the note, for `removed` rows.
    skip: Option<SkipReason>,
    /// Additional detail such as a span reset.
    detail: Option<String>,
    /// Private-note summary; never includes note text.
    note: Value,
}

struct SyncReport {
    day: NaiveDate,
    path: String,
    status: &'static str,
    rows: Vec<ReportRow>,
    entries: Vec<MeetPomodoroEntry>,
    links_added: usize,
    links_refiled: usize,
    links_sorted: usize,
    warnings: Vec<String>,
    deprecations: Vec<diagnostics::Deprecation>,
    quiet: bool,
    force: bool,
    json: bool,
    diff: Option<String>,
    stamp: Option<Stamp>,
    stamp_refreshed: bool,
    /// Why the run wrote nothing, for `waiting` results only.
    waiting: Option<WaitingReason>,
    /// Every Google event this run leaves out of the note, with why.
    skipped: Vec<SkippedEvent>,
    /// Included events already in the note that needed no change.
    in_sync: usize,
    held: Option<HeldSummary>,
}

struct HeldSummary {
    rows: usize,
    written_at: String,
    detail: String,
    hint: String,
}

/// Deprecation diagnostics for the complete fetched snapshot, before any
/// mutation or no-op filtering. Context and omitted occurrences keep their
/// Google-input evidence; an observed alias still warns for unchanged,
/// dry-run, and waiting results.
fn snapshot_diagnostics(
    snapshot: &snapshot::DaySnapshot,
) -> (Vec<String>, Vec<diagnostics::Deprecation>) {
    let inputs: Vec<diagnostics::DiagnosticInput<'_>> = snapshot
        .events
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

fn sync(args: &SyncArgs) -> Result<SyncReport, GcalError> {
    let config = GcalConfig::resolve()?;
    let host = journal::host_label();
    if (!args.dry_run || args.force)
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
    let mut resolver = HomeResolver::new(DiskHomeSource::new(&vault));

    // Fetch outside any lock; an incomplete snapshot writes nothing.
    let snapshot = snapshot::fetch_day(vault, &config, day, &mut resolver)?;
    let (diag_warnings, deprecations) = snapshot_diagnostics(&snapshot);
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
                WaitingReason::NoteMissing,
                journal_warnings(&vault)?
                    .into_iter()
                    .chain(diag_warnings)
                    .collect(),
                deprecations,
            );
        }
    };
    if let Some(reason) = WaitingReason::of_contents(&contents) {
        return waiting(
            args,
            &host,
            day,
            &display_path,
            reason,
            journal_warnings(&vault)?
                .into_iter()
                .chain(diag_warnings)
                .collect(),
            deprecations,
        );
    }
    stamp::account_guard(&contents, &snapshot.account, day, &display_path)?;

    let (journal_state, mut warnings, _) = journal::load_state(&vault)
        .map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("read the gcal journal: {error}"),
            )
        })?;
    let (note_book, note_warnings) =
        journal::load_notes(&vault).unwrap_or_default();
    warnings.extend(note_warnings);
    let malformed = journal::malformed_count(&vault);
    // Diagnostics stay ephemeral: they never create journal writes and
    // never make an otherwise unchanged plan changed.
    warnings.extend(diag_warnings);
    if !args.force
        && let Some(held) = replaced_events_hold(&contents, day, &journal_state)
    {
        let mut skipped = Vec::new();
        for ((key, row_day), state) in &journal_state {
            if row_day != &day.to_string() || state.gone {
                continue;
            }
            if state.row.is_none() {
                continue;
            }
            let Some(event) = snapshot.events.iter().find(|event| {
                journal::occurrence_key(
                    &event.event.key.calendar_id,
                    &event.event.key.event_id,
                ) == *key
            }) else {
                continue;
            };
            skipped.push(SkippedEvent {
                time: row_time(&snapshot.window, &event.event).3,
                title: event.event.title.clone(),
                calendar_id: event.event.key.calendar_id.clone(),
                event_id: event.event.key.event_id.clone(),
                reason: SkipReason::events_replaced(day, is_today),
            });
        }
        skipped.sort_by(|a, b| a.time.cmp(&b.time));
        let report = SyncReport {
            day,
            path: display_path,
            status: "held",
            rows: Vec::new(),
            entries: Vec::new(),
            links_added: 0,
            links_refiled: 0,
            links_sorted: 0,
            warnings,
            deprecations,
            quiet: args.quiet,
            force: false,
            json: args.format.is_json(),
            diff: None,
            stamp: None,
            stamp_refreshed: false,
            waiting: None,
            skipped,
            in_sync: 0,
            held: Some(held),
        };
        if !args.dry_run {
            record_status(&host, day, "held", &report, None);
        }
        return Ok(report);
    }
    let now = bob_env::current_datetime();
    let mut occurrences = Vec::new();
    for (occurrence, warning) in build_occurrences(
        &snapshot,
        &config,
        &vault,
        &daily_rel,
        is_today,
        &now,
        &mut resolver,
    ) {
        if let Some(warning) = warning {
            warnings.push(warning);
        }
        occurrences.push(occurrence);
    }
    let mut reset = args.force.then(|| {
        super::reset::prepare(
            &contents,
            &day.to_string(),
            &journal_state,
            &occurrences,
        )
    });
    let active_journal = reset
        .as_ref()
        .map(|reset| &reset.journal)
        .unwrap_or(&journal_state);
    // Lifecycle verdicts for journaled occurrences with no included
    // snapshot row. A failed lookup makes the snapshot incomplete, so
    // nothing is written.
    let lifecycle =
        resolve_absent(&snapshot, &occurrences, active_journal, &day)?;

    let plan = plan::plan_with_clock(
        &plan::SyncPlanInput {
            contents: &contents,
            is_today,
            occurrences: occurrences.clone(),
            journal: active_journal,
            notes: &note_book,
            malformed,
            day_string: day.to_string(),
            today: now.date(),
            lifecycle: lifecycle.clone(),
            reset: reset.as_ref(),
            series_roots: &snapshot.series_roots,
            account: &snapshot.account,
            now,
        },
        Some(daily_rel.as_path()),
        Some(&mut resolver),
    );

    if args.dry_run {
        let diff = unified_diff(&contents, &plan.contents, &display_path);
        let skips = day_skips(&snapshot, active_journal, &plan, is_today);
        return Ok(SyncReport {
            day,
            path: display_path,
            status: "dry_run",
            rows: report_rows(&plan, &snapshot, is_today),
            entries: plan.pomodoro_entries.clone(),
            links_added: plan.links_added.len(),
            links_refiled: plan.links_refiled.len(),
            links_sorted: plan.links_sorted,
            warnings: merge_warnings(warnings, &plan.warnings),
            deprecations,
            quiet: args.quiet,
            force: args.force,
            json: args.format.is_json(),
            diff: Some(diff),
            stamp: plan.stamp.clone(),
            stamp_refreshed: plan.stamp_refreshed,
            waiting: None,
            skipped: skips.skipped,
            in_sync: skips.in_sync,
            held: None,
        });
    }

    let reset_record = reset.as_ref().is_some_and(|reset| reset.had_state);
    if !plan.changed && !reset_record {
        let skips = day_skips(&snapshot, active_journal, &plan, is_today);
        let report = SyncReport {
            day,
            path: display_path.clone(),
            status: "unchanged",
            rows: Vec::new(),
            entries: Vec::new(),
            links_added: 0,
            links_refiled: 0,
            links_sorted: 0,
            warnings,
            deprecations,
            quiet: args.quiet,
            force: args.force,
            json: args.format.is_json(),
            diff: None,
            stamp: plan.stamp.clone(),
            stamp_refreshed: plan.stamp_refreshed,
            waiting: None,
            skipped: skips.skipped,
            in_sync: skips.in_sync,
            held: None,
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
                // Templater may have replaced the note while the plan
                // waited for the lock; never write into an unrendered one.
                if let Some(reason) = WaitingReason::of_contents(&current) {
                    return waiting(
                        args,
                        &host,
                        day,
                        &display_path,
                        reason,
                        warnings,
                        deprecations,
                    );
                }
                stamp::account_guard(
                    &current,
                    &snapshot.account,
                    day,
                    &display_path,
                )?;
                resolver.refresh();
                reset = args.force.then(|| {
                    super::reset::prepare(
                        &current,
                        &day.to_string(),
                        &journal_state,
                        &occurrences,
                    )
                });
                let active_journal = reset
                    .as_ref()
                    .map(|reset| &reset.journal)
                    .unwrap_or(&journal_state);
                let retry_lifecycle = resolve_absent(
                    &snapshot,
                    &occurrences,
                    active_journal,
                    &day,
                )?;
                plan = plan::plan_with_clock(
                    &plan::SyncPlanInput {
                        contents: &current,
                        is_today,
                        occurrences: occurrences.clone(),
                        journal: active_journal,
                        notes: &note_book,
                        malformed,
                        day_string: day.to_string(),
                        today: now.date(),
                        lifecycle: retry_lifecycle,
                        reset: reset.as_ref(),
                        series_roots: &snapshot.series_roots,
                        account: &snapshot.account,
                        now,
                    },
                    Some(daily_rel.as_path()),
                    Some(&mut resolver),
                );
                let reset_record =
                    reset.as_ref().is_some_and(|reset| reset.had_state);
                if !plan.changed && !reset_record {
                    let skips =
                        day_skips(&snapshot, active_journal, &plan, is_today);
                    let report = SyncReport {
                        day,
                        path: display_path.clone(),
                        status: "unchanged",
                        rows: Vec::new(),
                        entries: Vec::new(),
                        links_added: 0,
                        links_refiled: 0,
                        links_sorted: 0,
                        warnings: warnings.clone(),
                        deprecations: deprecations.clone(),
                        quiet: args.quiet,
                        force: args.force,
                        json: args.format.is_json(),
                        diff: None,
                        stamp: plan.stamp.clone(),
                        stamp_refreshed: plan.stamp_refreshed,
                        waiting: None,
                        skipped: skips.skipped,
                        in_sync: skips.in_sync,
                        held: None,
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
    append_journal(
        &vault,
        &host,
        &plan,
        &display_path,
        day,
        reset.as_ref().is_some_and(|reset| reset.had_state),
    )?;

    for kept in &plan.kept {
        warnings.push(format!("{}: {}", kept.title, kept.detail));
    }
    warnings.extend(plan.warnings.iter().cloned());

    let active_journal = reset
        .as_ref()
        .map(|reset| &reset.journal)
        .unwrap_or(&journal_state);
    let skips = day_skips(&snapshot, active_journal, &plan, is_today);
    let report = SyncReport {
        day,
        path: display_path,
        status: "written",
        rows: report_rows(&plan, &snapshot, is_today),
        entries: plan.pomodoro_entries.clone(),
        links_added: plan.links_added.len(),
        links_refiled: plan.links_refiled.len(),
        links_sorted: plan.links_sorted,
        warnings,
        deprecations,
        quiet: args.quiet,
        force: args.force,
        json: args.format.is_json(),
        diff: None,
        stamp: plan.stamp.clone(),
        stamp_refreshed: plan.stamp_refreshed,
        waiting: None,
        skipped: skips.skipped,
        in_sync: skips.in_sync,
        held: None,
    };
    record_status(&host, day, "written", &report, None);
    Ok(report)
}

fn waiting(
    args: &SyncArgs,
    host: &str,
    day: NaiveDate,
    display_path: &str,
    reason: WaitingReason,
    warnings: Vec<String>,
    deprecations: Vec<diagnostics::Deprecation>,
) -> Result<SyncReport, GcalError> {
    let report = SyncReport {
        day,
        path: display_path.to_string(),
        status: "waiting",
        rows: Vec::new(),
        entries: Vec::new(),
        links_added: 0,
        links_refiled: 0,
        links_sorted: 0,
        warnings,
        deprecations,
        quiet: args.quiet,
        force: args.force,
        json: args.format.is_json(),
        diff: None,
        stamp: None,
        stamp_refreshed: false,
        waiting: Some(reason),
        skipped: Vec::new(),
        in_sync: 0,
        held: None,
    };
    if !args.dry_run {
        record_status(host, day, "waiting", &report, None);
    }
    Ok(report)
}

/// Why sync left a day's note untouched. Day-level, never per-event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitingReason {
    /// The daily note does not exist yet.
    NoteMissing,
    /// The note exists but has no `## Pomodoros` section.
    PomodorosMissing,
    /// Templater has not rendered the note: a `<%` tag is still in it.
    NoteUnrendered(PendingTag),
}

impl WaitingReason {
    /// The guard for note contents, in order: an unrendered note first,
    /// because a raw template already has a `## Pomodoros` section.
    pub(crate) fn of_contents(contents: &str) -> Option<Self> {
        if let Some(tag) = first_pending_tag(contents) {
            return Some(Self::NoteUnrendered(tag));
        }
        let lines: Vec<&str> = contents.lines().collect();
        pomodoros_section_range(&lines)
            .is_none()
            .then_some(Self::PomodorosMissing)
    }

    /// Stable JSON `reason` code.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NoteMissing => "note_missing",
            Self::PomodorosMissing => "pomodoros_missing",
            Self::NoteUnrendered(_) => "note_unrendered",
        }
    }

    pub(crate) fn detail(&self) -> String {
        match self {
            Self::NoteMissing => "the daily note doesn't exist yet".to_string(),
            Self::PomodorosMissing => {
                "it has no ## Pomodoros section yet".to_string()
            }
            Self::NoteUnrendered(tag) => format!(
                "Templater hasn't finished rendering it (line {} still has {})",
                tag.line, tag.excerpt
            ),
        }
    }

    pub(crate) fn hint(&self) -> &'static str {
        match self {
            Self::NoteMissing => {
                "nothing was written; rerun bob gcal sync once the note exists"
            }
            Self::PomodorosMissing => {
                "nothing was written; rerun bob gcal sync once the section exists"
            }
            Self::NoteUnrendered(_) => {
                "nothing was written; rerun bob gcal sync once the note is rendered"
            }
        }
    }

    /// Line of the pending tag, for an unrendered note.
    pub(crate) fn line(&self) -> Option<usize> {
        match self {
            Self::NoteUnrendered(tag) => Some(tag.line),
            _ => None,
        }
    }
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
/// roots, the `[type:: MEET]` field, badges, span hashes, description
/// bullets, and MEET eligibility. Returns each occurrence with its root
/// warnings.
fn build_occurrences<S: HomeSource>(
    snapshot: &snapshot::DaySnapshot,
    config: &GcalConfig,
    vault: &std::path::Path,
    daily_rel: &std::path::Path,
    is_today: bool,
    now: &chrono::NaiveDateTime,
    resolver: &mut HomeResolver<S>,
) -> Vec<(PlannedOccurrence, Option<String>)> {
    let mut out = Vec::new();
    for event in &snapshot.events {
        if matches!(event.inclusion, Inclusion::Omitted(_)) {
            continue;
        }
        let task = event.inclusion == Inclusion::Task;
        let (all_day, start_min, end_min, time_label) =
            row_time(&snapshot.window, &event.event);
        let root_value =
            event.root.as_ref().and_then(|root| root.value.clone());
        let root_source =
            event.root.as_ref().and_then(|root| root.source.clone());
        let needs_type = if task {
            needs_meet_type(
                resolver,
                vault,
                daily_rel,
                &time_label,
                &event.event.title,
                root_value.as_deref(),
            )
        } else {
            false
        };
        let off_clock = event.event.off_clock;
        let buffer = task_buffer(&event.event, task);
        let badges = render::badge_text(
            event.event.html_link.as_deref(),
            event.event.join_link.as_deref(),
            event.event.location.as_deref(),
            task.then_some(&event.event.clock_tag),
            buffer,
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
        let link_phase = if task {
            Some(link_phase_for(
                &event.event,
                &snapshot.window,
                is_today,
                now,
            ))
        } else {
            None
        };
        // Task occurrences in `bullets` mode carry their rendered,
        // capped bullets (`Some` even when empty); context rows and
        // `off` mode carry none.
        let description = if task && config.description_bullets() {
            Some(render_bullets(
                &event.event.description,
                config.description_lines(),
                event.event.html_link.as_deref(),
            ))
        } else {
            None
        };
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
        let note = task
            .then(|| event.event.note.as_ref().map(|note| note.provenance()))
            .flatten();
        out.push((
            PlannedOccurrence {
                calendar_id: event.event.key.calendar_id.clone(),
                event_id: event.event.key.event_id.clone(),
                ical_uid: event.event.ical_uid.clone(),
                series_id: event.event.series_id.clone(),
                note,
                id_hint: event.event.title.clone(),
                task,
                time_label,
                title: event.event.title.clone(),
                html_link: event.event.html_link.clone(),
                join_link: event.event.join_link.clone(),
                location: event.event.location.clone(),
                root: root_value,
                source: root_source,
                root_kind: event
                    .root
                    .as_ref()
                    .map(|root| root.kind)
                    .unwrap_or_default(),
                qualifier,
                spans,
                needs_type,
                off_clock,
                clock_tag: event.event.clock_tag.clone(),
                buffer,
                description,
                link_phase,
                all_day,
                start_min,
                end_min,
            },
            warnings,
        ));
    }
    out
}

/// The buffer one snapshot event syncs: task rows carry Google's buffer,
/// context rows never do.
fn task_buffer(
    event: &super::model::GcalEvent,
    task: bool,
) -> crate::native::event_task::EventBuffer {
    if task {
        event.buffer
    } else {
        crate::native::event_task::EventBuffer::default()
    }
}

/// `(all_day, start_min, end_min, time_label)` for one event in vault-local time.
pub(super) fn row_time(
    window: &classify::DayWindow,
    event: &super::model::GcalEvent,
) -> (bool, i32, i32, String) {
    match (&event.start, &event.end) {
        (EventDateTime::AllDay(_), EventDateTime::AllDay(_)) => {
            (true, 0, 0, "all day".to_string())
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
            let start_min = minutes_from_midnight(window.day, local_start);
            let end_min = minutes_from_midnight(window.day, local_end);
            (false, start_min, end_min, label)
        }
        _ => (true, 0, 0, "all day".to_string()),
    }
}

fn minutes_from_midnight(day: NaiveDate, local: chrono::NaiveDateTime) -> i32 {
    let midnight = day.and_hms_opt(0, 0, 0).expect("midnight");
    (local - midnight).num_minutes() as i32
}

/// Phase at sync time on the buffered span: past means the buffered
/// end passed, current means the buffered span covers now (today only).
/// Timed close classification still uses the bare meeting span.
fn link_phase_for(
    event: &super::model::GcalEvent,
    window: &classify::DayWindow,
    is_today: bool,
    now: &chrono::NaiveDateTime,
) -> LinkPhase {
    match (&event.start, &event.end) {
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            let local_start = window.local(start);
            let local_end = window.local(end);
            let buffered_start = local_start
                - chrono::Duration::minutes(i64::from(event.buffer.before_min));
            let buffered_end = local_end
                + chrono::Duration::minutes(i64::from(event.buffer.after_min));
            if buffered_end <= *now {
                LinkPhase::Past
            } else if buffered_start <= *now && is_today {
                LinkPhase::Current
            } else {
                LinkPhase::Future
            }
        }
        _ if window.day < now.date() => LinkPhase::Past,
        _ => LinkPhase::Future,
    }
}

fn needs_meet_type<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
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
    render::needs_meet_type(resolver, vault, daily_rel, &probe)
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
                    transport = Some(Gog::resolve(
                        crate::native::google::GCAL,
                        config.gog(),
                        config.timeout(),
                    )?);
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

/// Bullet lines one inserted row wrote this run: the journaled line
/// count for a `Tracked` record, 0 otherwise.
fn written_description_lines(state: &journal::DescriptionState) -> usize {
    match state {
        journal::DescriptionState::Tracked { lines, .. } => *lines,
        _ => 0,
    }
}

/// Skipped events and the in-sync count for the finished plan.
fn day_skips(
    snapshot: &snapshot::DaySnapshot,
    journal: &BTreeMap<(String, String), OccurrenceState>,
    plan: &plan::SyncPlan,
    is_today: bool,
) -> skip::DaySkips {
    skip::summarize(
        &snapshot.events,
        &snapshot.window,
        journal,
        plan,
        is_today,
        &|event| row_time(&snapshot.window, event).3,
    )
}

/// Detect a whole-section replacement: active rows were journaled, none
/// survives in the note, and Bob's recognized Events stamp disappeared.
fn replaced_events_hold(
    contents: &str,
    day: NaiveDate,
    journal: &BTreeMap<(String, String), OccurrenceState>,
) -> Option<HeldSummary> {
    let detected = replaced::detect(contents, &day.to_string(), journal)?;
    let written_at = detected.written_at.format("%H:%M").to_string();
    let count = detected.rows;
    let restore = if day == bob_env::current_datetime().date() {
        "bob gcal sync -F".to_string()
    } else {
        format!("bob gcal sync -D {day} -F")
    };
    let event_day = if day == bob_env::current_datetime().date() {
        "today's".to_string()
    } else {
        format!("{day}'s")
    };
    Some(HeldSummary {
        rows: count,
        detail: format!(
            "{event_day} Events section was replaced: the {count} rows Bob wrote at {written_at} and its ↻ stamp are gone"
        ),
        hint: format!(
            "another writer (Templater, a sync conflict) likely replaced the note; nothing was tombstoned · {restore} rewrites the day's Events (then delete any rows you don't want)"
        ),
        written_at,
    })
}

fn listed_note(
    snapshot: &snapshot::DaySnapshot,
    calendar_id: &str,
    event_id: &str,
) -> Value {
    snapshot
        .events
        .iter()
        .find(|listed| {
            listed.event.key.calendar_id == calendar_id
                && listed.event.key.event_id == event_id
        })
        .map(|listed| super::model::note_summary_json(&listed.event))
        .unwrap_or(Value::Null)
}

fn report_rows(
    plan: &plan::SyncPlan,
    snapshot: &snapshot::DaySnapshot,
    is_today: bool,
) -> Vec<ReportRow> {
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
            off_clock: added.off_clock,
            untracked: added.off_clock,
            buffer: added.buffer,
            description_lines: written_description_lines(&added.description),
            skip: None,
            detail: added.detail.clone(),
            note: listed_note(snapshot, &added.calendar_id, &added.event_id),
        });
    }
    for restored in &plan.rows_restored {
        rows.push(ReportRow {
            action: "restored",
            kind: if restored.row_kind == "task" {
                "task"
            } else {
                "context"
            },
            time: restored.time_label.clone(),
            title: restored.title.clone(),
            root: restored.root.clone(),
            source: restored.source.clone(),
            off_clock: restored.off_clock,
            untracked: restored.off_clock,
            buffer: restored.buffer,
            description_lines: written_description_lines(&restored.description),
            skip: None,
            detail: restored.detail.clone(),
            note: listed_note(
                snapshot,
                &restored.calendar_id,
                &restored.event_id,
            ),
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
            off_clock: adopted.off_clock,
            untracked: adopted.off_clock,
            buffer: adopted.buffer,
            description_lines: 0,
            skip: None,
            detail: None,
            note: listed_note(
                snapshot,
                &adopted.calendar_id,
                &adopted.event_id,
            ),
        });
    }
    for gone in &plan.rows_gone {
        // A lifecycle tombstone has no occurrence in the plan; borrow
        // the time and title from the snapshot when the event is there.
        let listed = snapshot.events.iter().find(|listed| {
            listed.event.key.calendar_id == gone.calendar_id
                && listed.event.key.event_id == gone.event_id
        });
        let time = match (gone.time_label.is_empty(), listed) {
            (true, Some(listed)) => row_time(&snapshot.window, &listed.event).3,
            _ => gone.time_label.clone(),
        };
        let title = match (gone.title.is_empty(), listed) {
            (true, Some(listed)) => listed.event.title.clone(),
            _ => gone.title.clone(),
        };
        let skip = if gone.reason == "line_removed" {
            SkipReason::removed_by_you(None, snapshot.day, is_today)
        } else {
            SkipReason::removed_in_google(&gone.reason)
        };
        rows.push(ReportRow {
            action: "removed",
            kind: "task",
            time,
            title,
            root: None,
            source: None,
            off_clock: false,
            untracked: false,
            buffer: crate::native::event_task::EventBuffer::default(),
            description_lines: 0,
            skip: Some(skip),
            detail: None,
            note: listed_note(snapshot, &gone.calendar_id, &gone.event_id),
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
            off_clock: updated.off_clock,
            untracked: updated.off_clock,
            buffer: updated.buffer,
            description_lines: updated.wrote_description_lines,
            skip: None,
            detail: updated.detail.clone(),
            note: listed_note(
                snapshot,
                &updated.calendar_id,
                &updated.event_id,
            ),
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
            off_clock: false,
            untracked: false,
            buffer: crate::native::event_task::EventBuffer::default(),
            description_lines: 0,
            skip: None,
            detail: None,
            note: Value::Null,
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
            off_clock: false,
            untracked: false,
            buffer: crate::native::event_task::EventBuffer::default(),
            description_lines: 0,
            skip: Some(SkipReason::removed_in_google(&removed.reason)),
            detail: None,
            note: listed_note(
                snapshot,
                &removed.calendar_id,
                &removed.event_id,
            ),
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
            off_clock: upgraded.off_clock,
            untracked: upgraded.off_clock,
            buffer: upgraded.buffer,
            description_lines: written_description_lines(&upgraded.description),
            skip: None,
            detail: upgraded.detail.clone(),
            note: listed_note(
                snapshot,
                &upgraded.calendar_id,
                &upgraded.event_id,
            ),
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
            off_clock: false,
            untracked: false,
            buffer: crate::native::event_task::EventBuffer::default(),
            description_lines: 0,
            skip: None,
            detail: None,
            note: Value::Null,
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
    description: &journal::DescriptionState,
    note: Option<&journal::NoteProvenance>,
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
            description: description.clone(),
            note: note.cloned(),
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
    reset: bool,
) -> Result<(), GcalError> {
    let day_string = day.to_string();
    if reset {
        journal::append_reset(vault, host, &day_string).map_err(|error| {
            GcalError::runtime(
                "journal",
                format!("append the gcal reset record: {error}"),
            )
        })?;
    }
    for added in plan.rows_added.iter().chain(&plan.rows_restored) {
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
            &added.description,
            added.note.as_ref(),
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
            &adopted.description,
            adopted.note.as_ref(),
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
            &updated.description,
            updated.note.as_ref(),
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
            &upgraded.description,
            upgraded.note.as_ref(),
        )?;
    }
    for refreshed in &plan.journal_refreshes {
        append_row_record(
            vault,
            host,
            &day_string,
            display_path,
            &refreshed.calendar_id,
            &refreshed.event_id,
            refreshed.ical_uid.as_deref(),
            refreshed.series_id.as_deref(),
            &refreshed.block_id,
            refreshed.url.as_deref(),
            &refreshed.row_kind,
            &refreshed.spans,
            refreshed.root.as_deref(),
            refreshed.source.as_deref(),
            &refreshed.description,
            refreshed.note.as_ref(),
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
        links_refiled: report.links_refiled,
        error,
        reason: report
            .waiting
            .as_ref()
            .map(|reason| reason.code().to_string()),
        forced: report.force,
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
        print_warnings_stderr(report);
    } else {
        print_human(report);
    }
}

fn print_warnings_stderr(report: &SyncReport) {
    let styler = Styler::detect();
    for warning in &report.warnings {
        eprintln!("{}", styler.dim(&format!("warning: {warning}")));
    }
}

/// `{"before_minutes", "after_minutes"}` for a buffered row, else
/// `null`, matching gcal's null style for absent values.
fn buffer_json(buffer: crate::native::event_task::EventBuffer) -> Value {
    if buffer.is_empty() {
        Value::Null
    } else {
        json!({
            "before_minutes": buffer.before_min,
            "after_minutes": buffer.after_min,
        })
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
                "off_clock": row.off_clock,
                "untracked": row.untracked,
                "buffer": buffer_json(row.buffer),
                "description_lines": row.description_lines,
                "detail": row.detail.as_deref().or_else(|| {
                    row.skip.as_ref().map(|skip| skip.detail.as_str())
                }),
                "note": row.note,
            })
        })
        .collect();
    let skipped: Vec<Value> =
        report.skipped.iter().map(SkippedEvent::json).collect();
    let stamp = report.stamp.as_ref().map(|stamp| {
        json!({
            "account": stamp.account,
            "synced_at": stamp.json_synced_at(),
            "refreshed": report.stamp_refreshed,
        })
    });
    println!(
        "{}",
        json!({
            "schema_version": 1,
            "ok": true,
            "day": report.day.to_string(),
            "path": report.path,
            "status": report.status,
            "force": report.force,
            "rows": rows,
            "in_sync": report.in_sync,
            "skipped": skipped,
            "pomodoro": {
                "entry_line": first_open_entry(report).map(|entry| entry.line),
                "created": first_open_entry(report)
                    .is_some_and(|entry| entry.created),
                "links_added": report.links_added,
                "links_refiled": report.links_refiled,
                "links_sorted": report.links_sorted,
                "entries": json_entries(report),
            },
            "waiting": report.waiting.as_ref().map(|reason| json!({
                "reason": reason.code(),
                "detail": reason.detail(),
                "hint": reason.hint(),
                "line": reason.line(),
            })),
            "held": report.held.as_ref().map(|held| json!({
                "rows": held.rows,
                "written_at": held.written_at,
                "detail": held.detail,
                "hint": held.hint,
            })),
            "warnings": report.warnings,
            "deprecations": report.deprecations,
            "stamp": stamp,
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
    let styler = Styler::detect();
    if report.quiet {
        // Quiet suppresses success chatter while warnings stay visible.
        print_warnings_stderr(report);
        return;
    }
    if report.status == "waiting" {
        let reset_suffix = if report.force { " · reset" } else { "" };
        println!("bob gcal sync · {}{reset_suffix}", report.path);
        match report.waiting.as_ref() {
            Some(reason) => {
                println!("waiting for {} · {}", report.path, reason.detail());
                println!(
                    "  {}",
                    styler.dim(&format!("hint: {}", reason.hint()))
                );
            }
            None => println!("waiting for {}", report.path),
        }
        print_warnings_stderr(report);
        return;
    }
    if report.status == "held" {
        let held = report.held.as_ref().expect("held report details");
        println!("bob gcal sync · {}", report.path);
        println!("held · {}", held.detail);
        println!("  {}", styler.dim(&format!("hint: {}", held.hint)));
        for skipped in &report.skipped {
            println!("{}", skipped.human_line(&styler, &human_time));
        }
        print_warnings_stderr(report);
        return;
    }
    let reset_suffix = if report.force { " · reset" } else { "" };
    println!("bob gcal sync · {}{reset_suffix}", report.path);
    for row in &report.rows {
        let sigil = match row.action {
            "added" => "+",
            "adopted" => "=",
            "updated" => "~",
            "upgraded" => "^",
            "restored" => "↺",
            "learned" => "*",
            _ => "-",
        };
        let time = match row.buffer.label() {
            Some(label) => format!("{} {label}", human_time(&row.time)),
            None => human_time(&row.time),
        };
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
        let detail = match (row.action, row.skip.as_ref()) {
            ("removed", Some(skip)) if row.kind == "task" => {
                removed_row_text(row, skip, &styler)
            }
            ("removed", _) => format!("removed ({})", row.title),
            _ => {
                let detail = row
                    .detail
                    .as_deref()
                    .map(|detail| format!("  {}", styler.dim(detail)))
                    .unwrap_or_default();
                format!("{time}  {}{root}{source}{detail}", row.title)
            }
        };
        let bullets = if row.description_lines > 0 {
            styler.dim(&format!(" ≡ {}", row.description_lines))
        } else {
            String::new()
        };
        println!("  {sigil} {:<7} {detail}{bullets}", row.kind);
    }
    for entry in &report.entries {
        println!("  {}", meet_human_line(entry));
    }
    for skipped in &report.skipped {
        println!("{}", skipped.human_line(&styler, &human_time));
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
    let created_entries =
        report.entries.iter().filter(|entry| entry.created).count();
    let stamp_only = report.status == "written"
        && report.rows.is_empty()
        && report.links_added == 0
        && report.links_refiled == 0
        && report.links_sorted == 0
        && created_entries == 0;
    let sort_only = report.status == "written"
        && report.rows.is_empty()
        && report.links_added == 0
        && report.links_refiled == 0
        && report.links_sorted > 0
        && created_entries == 0;
    let refile_only = report.status == "written"
        && report.links_refiled > 0
        && report.links_added == 0
        && report.links_sorted == 0
        && created_entries == 0
        && !report.rows.is_empty()
        && report.rows.iter().all(|row| row.action == "updated");
    let summary = if report.status == "unchanged" {
        "✓ Events unchanged · Pomodoros unchanged".to_string()
    } else if refile_only {
        format!(
            "✓ Events updated · Pomodoros ↪{} refiled",
            report.links_refiled
        )
    } else if sort_only {
        format!(
            "✓ Events unchanged · Pomodoros ↕{} links sorted",
            report.links_sorted
        )
    } else if stamp_only {
        "✓ Events unchanged · stamp refreshed".to_string()
    } else {
        let mut parts = vec![
            format!("+{tasks} tasks"),
            format!("+{context} context"),
            format!(
                "~{} updated",
                count("updated", "task") + count("updated", "context")
            ),
            format!("-{} cancelled", count("cancelled", "task")),
            format!("-{} removed", count("removed", "task")),
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
        let entry_word = if created_entries == 1 {
            "entry"
        } else {
            "entries"
        };
        let mut pomodoros = format!(
            "✓ Events {} · Pomodoros +{created_entries} {entry_word}, +{} links",
            parts.join(", "),
            report.links_added,
        );
        if report.links_sorted > 0 {
            pomodoros.push_str(&format!(", ↕{} sorted", report.links_sorted));
        }
        if report.links_refiled > 0 {
            pomodoros.push_str(&format!(", ↪{} refiled", report.links_refiled));
        }
        pomodoros
    };
    let summary = if report.force && report.status == "written" {
        if report.rows.is_empty()
            && report.links_added == 0
            && report.links_refiled == 0
            && report.links_sorted == 0
            && report.entries.is_empty()
            && !report.stamp_refreshed
        {
            "✓ Events reset · nothing to rewrite".to_string()
        } else if report.rows.is_empty()
            && report.links_added == 0
            && report.links_refiled == 0
            && report.links_sorted == 0
            && report.stamp_refreshed
        {
            "✓ Events reset · stamp refreshed".to_string()
        } else {
            let restored =
                count("restored", "task") + count("restored", "context");
            let updated =
                count("updated", "task") + count("updated", "context");
            let added = count("added", "task") + count("added", "context");
            let mut changes = Vec::new();
            if restored > 0 {
                changes.push(format!("↺{restored} restored"));
            }
            if updated > 0 {
                changes.push(format!("~{updated} reset"));
            }
            if added > 0 {
                changes.push(format!("+{added} added"));
            }
            if changes.is_empty() {
                changes.push("rows reconciled".to_string());
            }
            format!(
                "✓ Events reset · {} · Pomodoros +{} links",
                changes.join(", "),
                report.links_added
            )
        }
    } else {
        summary
    };
    println!("{summary}{}", counts_tail(report));
    if let Some(stamp) = &report.stamp {
        println!("{}", styler.dim(&stamp.echo(report.day)));
    }
    print_warnings_stderr(report);
}

/// ` · 3 in sync · 2 skipped`, dropping zero counts so a quiet day stays
/// short.
fn counts_tail(report: &SyncReport) -> String {
    let mut tail = String::new();
    if report.in_sync > 0 {
        tail.push_str(&format!(" · {} in sync", report.in_sync));
    }
    if !report.skipped.is_empty() {
        tail.push_str(&format!(" · {} skipped", report.skipped.len()));
    }
    tail
}

/// A removed task row's text: `{time}  {title}  removed by you · restore
/// with bob gcal sync -F`. Parts the plan could not name are left out.
fn removed_row_text(
    row: &ReportRow,
    skip: &SkipReason,
    styler: &Styler,
) -> String {
    let mut parts = Vec::new();
    if !row.time.is_empty() {
        parts.push(human_time(&row.time));
    }
    if !row.title.is_empty() {
        parts.push(row.title.clone());
    }
    let mut tail = styler.dim(&skip.detail);
    if let Some(hint) = &skip.hint {
        tail.push_str(&styler.dim(" · "));
        tail.push_str(&hint.styled(styler));
    }
    parts.push(tail);
    parts.join("  ")
}

fn merge_warnings(mut warnings: Vec<String>, extra: &[String]) -> Vec<String> {
    warnings.extend(extra.iter().cloned());
    warnings
}

fn first_open_entry(report: &SyncReport) -> Option<&MeetPomodoroEntry> {
    report.entries.iter().find(|entry| entry.state.is_open())
}

fn json_entries(report: &SyncReport) -> Vec<Value> {
    report
        .entries
        .iter()
        .map(|entry| {
            json!({
                "state": entry.state.as_str(),
                "line": entry.line,
                "time": entry.time,
                "created": entry.created,
                "links": entry.links,
            })
        })
        .collect()
}

fn meet_human_line(entry: &MeetPomodoroEntry) -> String {
    let sigil = if entry.created { "+" } else { "=" };
    let body = match (entry.created, entry.state, entry.time.as_deref()) {
        (true, plan::MeetEntryState::Planned, _) => {
            "created () — MEET".to_string()
        }
        (false, plan::MeetEntryState::Planned, _) => "() — MEET".to_string(),
        (_, state, Some(time)) => {
            format!("{} {} — MEET", state.as_str(), human_time(time))
        }
        (_, state, None) => format!("{} () — MEET", state.as_str()),
    };
    format!("{sigil} MEET     {body} · linked {}", entry.links)
}

/// `0800-0830` → `08:00–08:30`; `all day` stays as written.
pub(super) fn human_time(time: &str) -> String {
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

#[cfg(test)]
mod waiting_tests {
    use super::*;

    const RENDERED: &str = "---\ndate: 2026-11-09\n---\n## Pomodoros\n\n";

    #[test]
    fn rendered_note_with_pomodoros_is_ready() {
        assert_eq!(WaitingReason::of_contents(RENDERED), None);
    }

    #[test]
    fn unrendered_check_precedes_the_pomodoros_check() {
        // The raw template already has Pomodoros; the tag must still win.
        let raw = "---\ndate: <% tp.date.now() %>\n---\n## Pomodoros\n";
        let reason = WaitingReason::of_contents(raw).expect("waiting");
        assert_eq!(reason.code(), "note_unrendered");
        assert_eq!(reason.line(), Some(2));
        // Without Pomodoros the tag is still the reported reason.
        let reason = WaitingReason::of_contents("<% x %>\n").expect("waiting");
        assert_eq!(reason.code(), "note_unrendered");
    }

    #[test]
    fn rendered_note_without_pomodoros_waits_for_the_section() {
        let reason = WaitingReason::of_contents("# Day\n").expect("waiting");
        assert_eq!(reason.code(), "pomodoros_missing");
        assert_eq!(reason.line(), None);
    }

    #[test]
    fn locked_reread_guard_refuses_a_replaced_note() {
        // The locked re-read after a NoteChanged retry uses the same
        // guard: a note Templater replaced mid-run is refused.
        let replaced = format!("{RENDERED}<%* tR += 'x' %>\n");
        assert!(matches!(
            WaitingReason::of_contents(&replaced),
            Some(WaitingReason::NoteUnrendered(_))
        ));
    }

    #[test]
    fn missing_note_has_its_own_code() {
        assert_eq!(WaitingReason::NoteMissing.code(), "note_missing");
    }
}

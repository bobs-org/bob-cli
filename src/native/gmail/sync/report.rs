//! Human and JSON reports for `bob gmail sync`.
//!
//! The JSON document is a superset of pull's schema 1: it keeps `ok`,
//! `dry_run`, `pulled`, `results`, and `warnings`, and adds `watch`,
//! `problems`, and `summary`. Consumers of scan and apply outcomes use
//! those structs directly rather than parsing this document.

use serde_json::{json, Value};

use super::archive_preview::{
    ArchiveGate, ArchivePlan, ArchivePreviewRow, PreviewDecision, ACTION_KEPT,
    ACTION_PAUSED, ACTION_WOULD_ARCHIVE, KEPT_CONSENT_REMOVED, KEPT_MANUAL,
    REASON_BACKLOG, REASON_ELIGIBLE,
};
use super::archive_projection::RecentArchiveSnapshot;
use super::archive_scan::ArchiveScan;
use super::archive_state::ArchiveJournal;
use super::file::{FilingFailure, FilingOutcome, FilingResult};
use super::scan::WatchScan;
use super::watch_apply::{WatchGoneOutcome, WatchThreadOutcome};
use super::ThreadKey;
use crate::native::env as bob_env;
use crate::native::gmail::body::inert_single_line;
use crate::native::gmail::cli::SyncArgs;
use crate::native::gmail::route::RouteProblem;
use crate::native::gmail::{save, ui, GmailError};
use crate::native::style::Styler;

const AWAITING_TAG: &str = "#awaiting_reply";

/// One thread row in the JSON `results` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncResult {
    pub(crate) thread_id: String,
    pub(crate) note: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) task_file: Option<String>,
    pub(crate) route: Option<String>,
}

/// One route-problem row in the JSON `problems` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncProblem {
    pub(crate) thread_id: String,
    pub(crate) note: Option<String>,
    pub(crate) label: String,
    pub(crate) kind: String,
    pub(crate) message: String,
    pub(crate) hint: String,
    pub(crate) first_seen: bool,
}

/// The document `bob gmail sync` prints, plus the flags the printer needs.
/// One task locator in an archive JSON row: path, line, and block id.
/// Task prose never leaves the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveTaskRef {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: Option<String>,
}

/// Archive settings that clients should not need to re-read from config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveConfigSummary {
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) max: u32,
}

/// A typed restore hint for an open task whose mail still has Bob's receipt.
/// This core phase only advises the user; it does not provide an undo CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveHint {
    pub(crate) kind: &'static str,
    pub(crate) thread_id: String,
    pub(crate) note: String,
}

pub(crate) fn archive_restore_hints(
    scan: &ArchiveScan,
    journal: &ArchiveJournal,
) -> Vec<ArchiveHint> {
    let mut hints = std::collections::BTreeMap::new();
    for receipt in journal.owned_receipts() {
        if let Some(evidence) = scan.evidence(&receipt.key)
            && evidence.has_open()
        {
            hints
                .entry(receipt.key.thread_id().to_string())
                .or_insert_with(|| ArchiveHint {
                    kind: "restore_to_inbox",
                    thread_id: receipt.key.thread_id().to_string(),
                    note: evidence.note.clone(),
                });
        }
    }
    hints.into_values().collect()
}

/// One archive preview row: `would_archive`, `kept`, or `paused`. Live
/// `archived`, `kept`, `let_go`, and `restored` rows arrive with the apply phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveRow {
    pub(crate) thread_id: String,
    pub(crate) note: String,
    pub(crate) action: &'static str,
    pub(crate) reason: &'static str,
    /// What the row would do without the pause: `would_archive` or
    /// `kept:<reason>`. Live `archived` rows carry `would_archive`,
    /// live `kept` rows carry `kept:<reason>`, and live `let_go` rows
    /// carry `let_go`.
    pub(crate) decision: String,
    pub(crate) messages: Vec<String>,
    pub(crate) tasks: Vec<ArchiveTaskRef>,
}

fn preview_decision(row: &ArchivePreviewRow) -> String {
    match &row.decision {
        PreviewDecision::Archive { .. } => ACTION_WOULD_ARCHIVE.to_string(),
        PreviewDecision::Kept { reason } => format!("kept:{reason}"),
    }
}

impl ArchiveRow {
    /// Map a preview row to its display action. A paused gate rewrites
    /// every actionable row to `paused` with the gate reason: a paused
    /// batch contains no mutation of any kind. The underlying decision is
    /// kept so the confirmation shows what each row would do.
    fn from_plan(plan: &ArchivePlan, row: &ArchivePreviewRow) -> Self {
        let (action, reason) = match &plan.gate {
            ArchiveGate::Open { .. } => match &row.decision {
                PreviewDecision::Archive { .. } => (
                    ACTION_WOULD_ARCHIVE,
                    if row.historical {
                        REASON_BACKLOG
                    } else {
                        REASON_ELIGIBLE
                    },
                ),
                PreviewDecision::Kept { reason } => (ACTION_KEPT, *reason),
            },
            ArchiveGate::Paused { reason } => (ACTION_PAUSED, *reason),
        };
        let decision = preview_decision(row);
        ArchiveRow {
            thread_id: row.key.thread_id().to_string(),
            note: row.note.clone(),
            action,
            reason,
            decision,
            messages: row.messages(),
            tasks: row
                .tasks
                .iter()
                .map(|task| ArchiveTaskRef {
                    path: task.path.clone(),
                    line: task.line,
                    block_id: task.block_id.clone(),
                })
                .collect(),
        }
    }

    /// Manually archived threads stay human-silent: there is nothing to
    /// confirm and nothing Bob did.
    fn human_silent(&self) -> bool {
        self.action == ACTION_KEPT
            && matches!(self.reason, KEPT_MANUAL | KEPT_CONSENT_REMOVED)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncReport {
    pub(crate) ok: bool,
    pub(crate) dry_run: bool,
    pub(crate) pulled: usize,
    pub(crate) results: Vec<SyncResult>,
    pub(crate) watch: Vec<WatchThreadOutcome>,
    pub(crate) gone: Vec<WatchGoneOutcome>,
    pub(crate) problems: Vec<SyncProblem>,
    pub(crate) warnings: Vec<String>,
    pub(crate) watching: usize,
    pub(crate) awaiting: usize,
    pub(crate) first_seen: bool,
    /// Guard or vault lock was busy: exit 0 with no human output.
    pub(crate) busy: bool,
    /// Archive rows: preview rows on dry-run, live outcomes on live runs.
    pub(crate) archive: Vec<ArchiveRow>,
    /// Canonical digest over the previewed set, when any row is actionable.
    /// Kept only for dry-run and paused runs; live successful runs omit it.
    pub(crate) archive_digest: Option<String>,
    /// Archive configuration projected for clients in schema 1.
    pub(crate) archive_config: Option<ArchiveConfigSummary>,
    /// Reopen advice stays separate from the established watch action list.
    pub(crate) archive_hints: Vec<ArchiveHint>,
    /// Archive planning was skipped with `-n` on an archive-enabled host.
    pub(crate) archive_skipped: bool,
    /// The live archive phase failed after filing/watch work completed.
    /// Keeping it on the report preserves those rows and the error's exit
    /// code instead of replacing the whole sync report.
    pub(crate) archive_error: Option<GmailError>,
    /// Historical archive activity in the previous configured local day.
    /// `None` is unavailable data, never an empty-history assertion.
    pub(crate) recent_archive: Option<RecentArchiveSnapshot>,
}

impl SyncReport {
    /// A quiet skip: no vault writes, no journal, exit 0.
    pub(crate) fn busy(dry_run: bool) -> Self {
        Self {
            ok: true,
            dry_run,
            pulled: 0,
            results: Vec::new(),
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: true,
            archive: Vec::new(),
            archive_digest: None,
            archive_config: None,
            archive_hints: Vec::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        }
    }

    pub(crate) fn set_archive_config(
        &mut self,
        enabled: bool,
        label: &str,
        max: u32,
    ) {
        self.archive_config = Some(ArchiveConfigSummary {
            enabled,
            label: label.to_string(),
            max,
        });
    }

    /// Attach a read-only dry-run archive preview. A paused gate fails the
    /// run (exit 1) with its rows and digest, and never mutates.
    pub(crate) fn attach_archive_preview(&mut self, plan: &ArchivePlan) {
        self.archive = plan
            .rows
            .iter()
            .map(|row| ArchiveRow::from_plan(plan, row))
            .collect();
        self.archive_digest = plan.digest.clone();
        if plan.gate.is_paused() {
            self.ok = false;
        }
    }

    /// Attach a live archive apply outcome. Filing results are already in
    /// the report; archive rows, digest, warnings, and failure state join
    /// them. A paused batch or any isolated failure fails the run without
    /// rolling back durable filings. The digest is kept only for paused
    /// batches; live successful runs omit it.
    pub(crate) fn attach_live_archive(
        &mut self,
        rows: Vec<ArchiveRow>,
        digest: Option<String>,
        warnings: Vec<String>,
        failed: bool,
        deferred_empty: bool,
        hints: Vec<ArchiveHint>,
    ) {
        self.archive = rows;
        self.archive_hints = hints;
        let paused = self.archive.iter().any(|row| row.action == "paused");
        if paused && self.archive_digest.is_none() {
            self.archive_digest = digest;
        }
        self.warnings.extend(warnings);
        let paused = self.archive.iter().any(|row| row.action == "paused");
        if failed || paused || !deferred_empty {
            self.ok = false;
        }
    }

    pub(crate) fn attach_archive_error(&mut self, error: GmailError) {
        self.warnings.extend(error.warnings().iter().cloned());
        let mut warning = format!(
            "Gmail archive phase failed: {}: {}",
            error.kind(),
            error.message()
        );
        if let Some(hint) = error.hint() {
            warning.push_str(&format!("; hint: {hint}"));
        }
        self.warnings.push(warning);
        self.archive_error = Some(error);
        self.ok = false;
    }

    pub(crate) fn exit_code(&self) -> i32 {
        if let Some(error) = &self.archive_error {
            return error.exit_code();
        }
        if self.ok {
            0
        } else {
            1
        }
    }
}

/// Build the report from apply's outcome plus enrolled discovery rows.
///
/// Counts come from typed filings, not the public status string:
/// `pulled` counts only `filed`/`would_file` rows (including filed
/// route-problem rows); fetch and planning failures never count. Watching
/// uses the apply boundary's revalidated snapshot so a task closed during
/// preparation leaves the summary, plus this run's newly filed and adopted
/// threads. Failed and deferred adoptions never count as newly enrolled
/// watch.
pub(crate) fn from_outcome(
    dry_run: bool,
    outcome: FilingOutcome,
    extra_results: Vec<SyncResult>,
    extra_warnings: Vec<String>,
    watchers: &WatchScan,
    account: &str,
    enrollment: &super::enroll::EnrollmentFold,
) -> SyncReport {
    // Membership source is explicit:
    // - the revalidated apply snapshot when apply ran;
    // - the projected membership in dry-run (also `applied`);
    // - otherwise the run's own scan.
    // Never fall back to an empty default when apply was skipped.
    let snapshot = if outcome.applied {
        &outcome.watch_snapshot
    } else {
        watchers
    };
    let extra_failed = extra_results.iter().any(|row| row.status == "problem");
    let mut results: Vec<SyncResult> = extra_results;
    let mut seen: std::collections::BTreeSet<String> =
        results.iter().map(|row| row.thread_id.clone()).collect();
    let mut problems = Vec::new();
    let mut first_seen = false;
    for result in &outcome.results {
        if !seen.insert(result.key.thread_id().to_string()) {
            continue;
        }
        if let Some(problem) = &result.problem {
            problems.push(sync_problem(
                result.key.thread_id(),
                result.note.as_deref(),
                problem,
                result.first_seen,
            ));
            first_seen |= result.first_seen;
        }
        results.push(from_filing(result));
    }
    for failure in &outcome.failures {
        if !seen.insert(failure.key.thread_id().to_string()) {
            continue;
        }
        results.push(from_failure(failure));
    }

    let mut warnings = extra_warnings;
    warnings.extend(outcome.warnings.clone());
    // Surface filing failure kind/message as body-free warnings.
    for failure in &outcome.failures {
        warnings.push(format!(
            "skipped Gmail thread {}: {}: {}",
            failure.key.thread_id(),
            failure.kind,
            failure.message
        ));
    }

    results.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));
    problems.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));

    // Typed durable filings only; fetch/planning failures never count.
    let pulled = outcome
        .results
        .iter()
        .filter(|row| matches!(row.status, "filed" | "would_file"))
        .count();
    let failed = outcome.failed
        || !outcome.failures.is_empty()
        || extra_failed
        || !outcome.deferred.is_empty();
    first_seen |= problems.iter().any(|problem| problem.first_seen);

    let watching = watching_count(&outcome, snapshot, account, enrollment);
    let awaiting = awaiting_count(&outcome, snapshot, account, enrollment);

    SyncReport {
        ok: !failed && !first_seen,
        dry_run,
        pulled,
        results,
        watch: outcome.watch_threads.clone(),
        gone: outcome.watch_gone.clone(),
        problems,
        warnings,
        watching,
        awaiting,
        first_seen,
        busy: outcome.busy,
        archive: Vec::new(),
        archive_digest: None,
        archive_config: None,
        archive_hints: Vec::new(),
        archive_skipped: false,
        archive_error: None,
        recent_archive: None,
    }
}

pub(crate) fn print(args: &SyncArgs, report: &SyncReport) {
    if args.format.is_json() {
        println!("{}", json_document(report));
        ui::flush_stdout();
        return;
    }
    // A busy skip is quiet only when preparation recorded no failure.
    // Failures and deferrals already on the report still print (and keep
    // exit 1) even though apply wrote nothing.
    if report.busy && report.ok {
        return;
    }
    let styler = Styler::detect();
    if !args.quiet {
        println!("{}", styler.cyan(&header(report)));
        for row in &report.results {
            if let Some(line) = human_row(row, &report.problems) {
                println!("  {}", color_row(&styler, row.status, &line));
            }
        }
        for row in &report.watch {
            if row.action != "unchanged"
                && let Some(line) = watch_human_row(row)
            {
                println!("  {}", watch_color_row(&styler, row.action, &line));
            }
        }
        for row in &report.gone {
            if let Some(line) = gone_human_row(row) {
                println!("  {}", styler.yellow(&line));
            }
        }
    } else {
        for row in &report.results {
            if row.status == "problem"
                && let Some(line) = human_row(row, &report.problems)
            {
                println!("  {}", styler.yellow(&line));
            }
        }
        for row in &report.watch {
            if row.action == "deferred_running"
                && let Some(line) = watch_human_row(row)
            {
                println!("  {}", styler.yellow(&line));
            }
        }
    }
    // The archive preview prints in quiet mode too: a paused batch must
    // explain its exit 1, and a dry-run preview is the explicitly
    // requested output. Manually archived threads stay silent.
    print_archive_preview(&styler, report, args.quiet);
    for warning in &report.warnings {
        eprintln!("{}", styler.dim(warning));
    }
    ui::flush_stdout();
}

fn print_archive_preview(styler: &Styler, report: &SyncReport, quiet: bool) {
    let mut shown = false;
    for row in &report.archive {
        let Some(line) = archive_human_row(row) else {
            continue;
        };
        shown = true;
        let colored = match row.action {
            ACTION_WOULD_ARCHIVE | "archived" => styler.green(&line),
            _ => styler.yellow(&line),
        };
        println!("  {colored}");
    }
    if !quiet {
        for hint in &report.archive_hints {
            println!(
                "  {}",
                styler.blue(&format!(
                    "↩ reopened {} → {} (archived mail stays in Gmail; restore stays manual: Move to Inbox in Gmail)",
                    hint.thread_id,
                    inert_single_line(&hint.note)
                ))
            );
        }
    }
    // The digest is only meaningful for dry-run and paused runs. A quiet
    // live run whose rows are silent must not leak it.
    let show_digest = report.dry_run
        || report.archive.iter().any(|row| row.action == ACTION_PAUSED);
    if show_digest && let Some(digest) = &report.archive_digest {
        shown = true;
        println!("  {}", styler.dim(&format!("archive digest {digest}")));
        if report.archive.iter().any(|row| row.action == ACTION_PAUSED) {
            println!(
                "  {}",
                styler
                    .dim(&format!("confirm with: bob gmail sync -c {digest}"))
            );
        }
    }
    if !shown && report.archive_skipped && !quiet {
        println!("  {}", styler.dim("archive planning skipped (-n)"));
    }
}

fn archive_human_row(row: &ArchiveRow) -> Option<String> {
    if row.human_silent() {
        return None;
    }
    let messages = if row.messages.len() == 1 {
        "1 message".to_string()
    } else {
        format!("{} messages", row.messages.len())
    };
    let note = inert_single_line(&row.note);
    let dest = if note.is_empty() {
        String::new()
    } else {
        format!(" → {note}")
    };
    match row.action {
        ACTION_WOULD_ARCHIVE => Some(format!(
            "✉ would archive {}{dest} ({messages})",
            row.thread_id,
        )),
        "archived" => {
            Some(format!("✉ archived {}{dest} ({messages})", row.thread_id,))
        }
        "let_go" => {
            if dest.is_empty() {
                Some(format!(
                    "✉ back in inbox {} ({})",
                    row.thread_id, row.reason,
                ))
            } else {
                Some(format!(
                    "✉ back in inbox {} ({}){dest}",
                    row.thread_id, row.reason,
                ))
            }
        }
        "restored" => {
            Some(format!("✉ restored {}{dest} ({messages})", row.thread_id,))
        }
        ACTION_KEPT => {
            if dest.is_empty() {
                Some(format!("✉ kept {} ({})", row.thread_id, row.reason,))
            } else {
                Some(
                    format!("✉ kept {} ({}){dest}", row.thread_id, row.reason,),
                )
            }
        }
        ACTION_PAUSED => {
            let decision = if row.decision.is_empty() {
                String::new()
            } else {
                format!("; {}", row.decision)
            };
            if dest.is_empty() {
                Some(format!(
                    "⏸ paused {} ({}{decision})",
                    row.thread_id, row.reason,
                ))
            } else {
                Some(format!(
                    "⏸ paused {} ({}{decision}){dest}",
                    row.thread_id, row.reason,
                ))
            }
        }
        _ => None,
    }
}

fn json_document(report: &SyncReport) -> Value {
    let results: Vec<Value> = report
        .results
        .iter()
        .map(|row| {
            json!({
                "thread_id": row.thread_id,
                "note": row.note,
                "link": row.link,
                "status": row.status,
                "task_file": row.task_file,
                "route": row.route,
            })
        })
        .collect();
    let problems: Vec<Value> = report
        .problems
        .iter()
        .map(|problem| {
            json!({
                "thread_id": problem.thread_id,
                "note": problem.note,
                "label": problem.label,
                "kind": problem.kind,
                "message": problem.message,
                "hint": problem.hint,
                "first_seen": problem.first_seen,
            })
        })
        .collect();
    let archive: Vec<Value> = report
        .archive
        .iter()
        .map(|row| {
            json!({
                "thread_id": row.thread_id,
                "note": row.note,
                "action": row.action,
                "reason": row.reason,
                "decision": row.decision,
                "messages": row.messages,
                "tasks": row.tasks.iter().map(|task| {
                    json!({
                        "path": task.path,
                        "line": task.line,
                        "block_id": task.block_id,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut document = json!({
        "schema_version": 1,
        "ok": report.ok,
        "dry_run": report.dry_run,
        "pulled": report.pulled,
        "results": results,
        "watch": report.watch.iter().map(watch_json).collect::<Vec<_>>(),
        "problems": problems,
        "archive": archive,
        "archive_hints": report.archive_hints.iter().map(|hint| json!({
            "kind": hint.kind,
            "thread_id": hint.thread_id,
            "note": hint.note,
            "message": "Task reopened; archived mail stays in Gmail; restore stays manual: Move to Inbox in Gmail.",
        })).collect::<Vec<_>>(),
        "summary": {
            "watching": report.watching,
            "awaiting": report.awaiting,
            "archived": report
                .archive
                .iter()
                .filter(|row| row.action == "archived")
                .count(),
        },
        "recent_archive": report.recent_archive.as_ref().map(|recent| {
            json!({
                "since": recent.since,
                "count": recent.count,
                "items": recent.items.iter().map(|item| json!({
                    "thread_id": item.thread_id,
                    "note": item.note,
                    "archived_at": item.archived_at,
                    "message_count": item.message_count,
                    "label": {
                        "name": item.label.name,
                        "id": item.label.id,
                    },
                    "archive_event_id": item.archive_event_id,
                    "state": item.state,
                })).collect::<Vec<_>>(),
            })
        }),
        "warnings": report.warnings,
    });
    // The digest is only meaningful for dry-run and paused runs. A quiet
    // live run whose rows are silent must not leak it.
    let show_digest = report.dry_run
        || report.archive.iter().any(|row| row.action == "paused");
    if show_digest && let Some(digest) = &report.archive_digest {
        document["archive_digest"] = json!(digest);
    }
    if report.archive_skipped {
        document["archive_skipped"] = json!(true);
    }
    if let Some(config) = &report.archive_config {
        document["archive_enabled"] = json!(config.enabled);
        document["archive_label"] = json!(config.label);
        document["archive_max"] = json!(config.max);
    }
    if let Some(error) = &report.archive_error {
        document["archive_error"] = json!({
            "kind": error.kind(),
            "message": error.message(),
            "hint": error.hint(),
        });
    }
    document
}

fn watch_json(row: &WatchThreadOutcome) -> Value {
    let tasks: Vec<Value> = row
        .tasks
        .iter()
        .map(|task| {
            json!({
                "path": task.path,
                "line": task.line,
                "block_id": task.block_id,
            })
        })
        .collect();
    let mut value = json!({
        "thread_id": row.key.thread_id(),
        "note": row.note,
        "tasks": tasks,
        "turn": row.turn,
        "turn_since": row.turn_since,
        "action": row.action,
    });
    if let Some(nudge_on) = &row.nudge_on {
        value["nudge_on"] = json!(nudge_on);
    }
    value
}

fn watch_human_row(row: &WatchThreadOutcome) -> Option<String> {
    let action = match row.action {
        "held" => "awaiting reply",
        "lifted" => "replied",
        "bounced" => "bounced",
        "resurfaced" => "resurfaced",
        "deferred_running" => "deferred (running task)",
        "refreshed" => "refreshed",
        _ => return None,
    };
    let who = row.who.as_deref().unwrap_or("Unknown sender");
    let title = row.title.as_deref().unwrap_or(row.key.thread_id());
    let note = row
        .note
        .as_deref()
        .map(|path| format!(" → {path}"))
        .unwrap_or_default();
    let nudge = row
        .nudge_on
        .as_deref()
        .map(|date| format!(" (nudge {date})"))
        .unwrap_or_default();
    Some(format!("✉ {action:<23} {who} — {title}{nudge}{note}"))
}

fn gone_human_row(row: &WatchGoneOutcome) -> Option<String> {
    let who = row.who.as_deref().unwrap_or("Unknown sender");
    let title = row.title.as_deref().unwrap_or(row.key.thread_id());
    let note = row
        .note
        .as_deref()
        .map(|path| format!(" → {path}"))
        .unwrap_or_default();
    Some(format!("⚠ gone                    {who} — {title}{note}"))
}

fn watch_color_row(styler: &Styler, action: &str, line: &str) -> String {
    match action {
        "lifted" => styler.blue(line),
        "bounced" | "deferred_running" => styler.yellow(line),
        _ => styler.green(line),
    }
}

fn from_filing(result: &FilingResult) -> SyncResult {
    let status = if result.problem.is_some() {
        "problem"
    } else {
        result.status
    };
    SyncResult {
        thread_id: result.key.thread_id().to_string(),
        note: result.note.clone(),
        link: result.link.clone(),
        status,
        task_file: result.task_file.clone(),
        route: result.route.clone(),
    }
}

fn from_failure(failure: &FilingFailure) -> SyncResult {
    SyncResult {
        thread_id: failure.key.thread_id().to_string(),
        note: None,
        link: None,
        status: "problem",
        task_file: None,
        route: None,
    }
}

fn sync_problem(
    thread_id: &str,
    note: Option<&str>,
    problem: &RouteProblem,
    first_seen: bool,
) -> SyncProblem {
    SyncProblem {
        thread_id: thread_id.to_string(),
        note: note.map(str::to_string),
        label: problem.label.clone(),
        kind: problem.kind.as_str().to_string(),
        message: problem.message.clone(),
        hint: problem.hint.clone(),
        first_seen,
    }
}

fn watching_count(
    outcome: &FilingOutcome,
    watchers: &WatchScan,
    account: &str,
    enrollment: &super::enroll::EnrollmentFold,
) -> usize {
    // Distinct enrolled threads with open watchers, including this run's
    // successful and projected filings and adoptions. Failed or deferred
    // adoptions never count as newly enrolled watch, and fetch failures
    // with no durable row never count.
    let mut keys: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for key in watchers.threads().filter(|key| key.is_account(account)) {
        // The revalidated snapshot lists open watchers; only enrolled
        // threads count, and enrollment never expires.
        if enrollment.is_enrolled(key) {
            keys.insert(key.thread_id().to_string());
        }
    }
    for result in &outcome.results {
        if matches!(result.status, "filed" | "would_file" | "adopted") {
            // Status strings never imply membership: recovered closed
            // tasks, pending save/origin/enrollment, and deferred
            // adoptions stay out of the watching count.
            if !result.watch_member {
                continue;
            }
            if outcome.deferred.contains(&result.key) {
                continue;
            }
            keys.insert(result.key.thread_id().to_string());
        }
    }
    keys.len()
}

fn awaiting_count(
    outcome: &FilingOutcome,
    watchers: &WatchScan,
    account: &str,
    enrollment: &super::enroll::EnrollmentFold,
) -> usize {
    let watching: std::collections::BTreeSet<String> = {
        let mut keys: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for key in watchers.threads().filter(|key| key.is_account(account)) {
            if enrollment.is_enrolled(key) {
                keys.insert(key.thread_id().to_string());
            }
        }
        for result in &outcome.results {
            if matches!(result.status, "filed" | "would_file" | "adopted")
                && result.watch_member
                && !outcome.deferred.contains(&result.key)
            {
                keys.insert(result.key.thread_id().to_string());
            }
        }
        keys
    };
    let mut counted = std::collections::BTreeSet::new();
    for id in watching {
        let Some(key) = ThreadKey::new(account, &id) else {
            continue;
        };
        if has_awaiting(watchers, &key) {
            counted.insert(id);
        }
    }
    counted.len()
}

fn has_awaiting(watchers: &WatchScan, key: &ThreadKey) -> bool {
    watchers.watchers(key).any(|task| {
        task.text
            .split_whitespace()
            .any(|token| token == AWAITING_TAG)
    })
}

fn header(report: &SyncReport) -> String {
    let time = local_time();
    let watching = if report.watching == 1 {
        "1 thread".to_string()
    } else {
        format!("{} threads", report.watching)
    };
    with_nothing(
        report,
        format!(
            "✉ gmail sync · {time} · watching {watching} · {} awaiting reply",
            report.awaiting
        ),
    )
}

fn with_nothing(report: &SyncReport, header: String) -> String {
    if report.results.iter().any(|row| {
        matches!(
            row.status,
            "filed"
                | "would_file"
                | "adopted"
                | "problem"
                | "refiled"
                | "would_refile"
        )
    }) || report.watch.iter().any(|row| row.action != "unchanged")
        || !report.gone.is_empty()
        || report.archive.iter().any(|row| !row.human_silent())
        || !report.archive_hints.is_empty()
    {
        header
    } else {
        format!("{header} · nothing new")
    }
}

fn local_time() -> String {
    let utc = save::current_utc();
    let tz = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    utc.with_timezone(&tz).format("%H:%M").to_string()
}

fn human_row(row: &SyncResult, problems: &[SyncProblem]) -> Option<String> {
    let filed_problem = (row.status == "problem")
        .then(|| {
            problems
                .iter()
                .find(|problem| problem.thread_id == row.thread_id)
        })
        .flatten();
    let action = match row.status {
        "filed" => "filed",
        "would_file" => "would file",
        "adopted" => "adopted",
        "refiled" => "refiled",
        "would_refile" => "would refile",
        "problem" if filed_problem.is_some() => "route",
        "problem" => "failed",
        _ => return None,
    };
    let name = display_name(row);
    let dest = row
        .task_file
        .as_deref()
        .or(row.note.as_deref())
        .unwrap_or("saved note unavailable");
    let glyph = match row.status {
        "adopted" => "⇢",
        "refiled" | "would_refile" => "↗",
        "problem" => "⚠",
        _ => "+",
    };
    if let Some(problem) = filed_problem {
        // Filed route problems show the resolver's label, message, and
        // hint so Bryan can fix the label without opening JSON.
        let mut line = format!(
            "{glyph} {action:<10} {name} → {dest} — `{}` {}",
            problem.label, problem.message
        );
        if !problem.hint.is_empty() {
            line.push_str(" — ");
            line.push_str(&problem.hint);
        }
        return Some(line);
    }
    if row.status == "problem" {
        // Remote fetch or planning failures have no durable row; keep the
        // per-thread row and point at warnings for the body-free diagnostic.
        return Some(format!(
            "{glyph} {action:<10} {name} → {dest} (see warnings)"
        ));
    }
    let extra = match row.status {
        "adopted" => " (existing task)",
        _ => "",
    };
    Some(format!("{glyph} {action:<10} {name} → {dest}{extra}"))
}

fn display_name(row: &SyncResult) -> String {
    if let Some(link) = &row.link
        && let Some((_, label)) = link.rsplit_once('|')
    {
        return label.trim_end_matches(']').to_string();
    }
    if let Some(link) = &row.link {
        return link.clone();
    }
    row.thread_id.clone()
}

fn color_row(styler: &Styler, status: &str, line: &str) -> String {
    match status {
        "problem" => styler.yellow(line),
        "adopted" => styler.blue(line),
        _ => styler.green(line),
    }
}

#[cfg(test)]
mod tests {
    use super::super::scan::{LinkPlacement, WatchTarget, WatchingTask};
    use super::super::watch_apply::{WatchGoneOutcome, WatchTaskRef};
    use super::*;

    fn filed() -> SyncResult {
        SyncResult {
            thread_id: "aa01".into(),
            note: Some("mail/2026/a.md".into()),
            link: Some("[[mail/2026/a.md|✉️ Lease · Ada]]".into()),
            status: "filed",
            task_file: Some("job.md".into()),
            route: Some("job".into()),
        }
    }

    #[test]
    fn json_keeps_pull_fields_and_adds_watch_problems_summary() {
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 1,
            results: vec![filed()],
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 1,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: Vec::new(),
            archive_digest: None,
            archive_config: None,
            archive_hints: Vec::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        };
        let value = json_document(&report);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], true);
        assert_eq!(value["pulled"], 1);
        assert_eq!(value["results"][0]["status"], "filed");
        assert_eq!(value["results"][0]["task_file"], "job.md");
        assert_eq!(value["results"][0]["route"], "job");
        assert!(value["watch"].as_array().unwrap().is_empty());
        assert_eq!(value["summary"]["watching"], 1);
        assert_eq!(value["summary"]["awaiting"], 0);
        assert!(value["problems"].as_array().unwrap().is_empty());
    }

    #[test]
    fn archive_json_is_additive_and_contains_only_ids_and_task_locators() {
        let report = SyncReport {
            ok: true,
            dry_run: true,
            pulled: 0,
            results: Vec::new(),
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: vec![ArchiveRow {
                thread_id: "aa01".into(),
                note: "mail/2026/aa01.md".into(),
                action: "archived",
                reason: "eligible",
                decision: "would_archive".to_string(),
                messages: vec!["00000000000000a0".into()],
                tasks: vec![ArchiveTaskRef {
                    path: "job.md".into(),
                    line: 12,
                    block_id: Some("follow-up".into()),
                }],
            }],
            archive_digest: Some("abcd".into()),
            archive_config: Some(ArchiveConfigSummary {
                enabled: true,
                label: "bob/closed".into(),
                max: 10,
            }),
            archive_hints: vec![ArchiveHint {
                kind: "restore_to_inbox",
                thread_id: "bb02".into(),
                note: "mail/2026/bb02.md".into(),
            }],
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        };
        let value = json_document(&report);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["archive_enabled"], true);
        assert_eq!(value["archive_label"], "bob/closed");
        assert_eq!(value["archive_max"], 10);
        assert_eq!(value["archive_digest"], "abcd");
        assert_eq!(value["archive"][0]["action"], "archived");
        assert_eq!(
            value["archive"][0]["messages"],
            json!(["00000000000000a0"])
        );
        assert_eq!(value["archive"][0]["tasks"][0]["path"], "job.md");
        assert_eq!(value["archive"][0]["tasks"][0]["line"], 12);
        assert_eq!(value["archive"][0]["tasks"][0]["block_id"], "follow-up");
        assert_eq!(value["archive_hints"][0]["kind"], "restore_to_inbox");
        assert_eq!(value["summary"]["archived"], 1);
    }

    #[test]
    fn archive_activity_breaks_quiet_and_safe_noop_rows_stay_silent() {
        let manual = ArchiveRow {
            thread_id: "aa01".into(),
            note: "mail/2026/aa01.md".into(),
            action: ACTION_KEPT,
            reason: KEPT_MANUAL,
            decision: format!("kept:{KEPT_MANUAL}"),
            messages: Vec::new(),
            tasks: Vec::new(),
        };
        let no_label = ArchiveRow {
            reason: KEPT_CONSENT_REMOVED,
            ..manual.clone()
        };
        let unsaved = ArchiveRow {
            reason: "unsaved",
            ..manual.clone()
        };
        assert_eq!(archive_human_row(&manual), None);
        assert_eq!(archive_human_row(&no_label), None);
        assert!(archive_human_row(&unsaved).is_some());

        let quiet_report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 0,
            results: Vec::new(),
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: vec![ArchiveRow {
                action: "let_go",
                reason: "replied",
                decision: "let_go".to_string(),
                ..manual
            }],
            archive_digest: None,
            archive_config: None,
            archive_hints: Vec::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        };
        assert!(!with_nothing(&quiet_report, "header".into())
            .contains("nothing new"));
        assert!(archive_human_row(&quiet_report.archive[0])
            .unwrap()
            .contains("back in inbox"));
    }

    #[test]
    fn watch_json_and_human_rows_keep_the_typed_polling_contract() {
        let row = WatchThreadOutcome {
            key: ThreadKey::new("me@example.com", "aa01").unwrap(),
            note: Some("mail/2026/aa01.md".into()),
            title: Some("Lease renewal".into()),
            who: Some("Dana Whitfield".into()),
            tasks: vec![WatchTaskRef {
                path: "job.md".into(),
                line: 8,
                block_id: Some("follow-up".into()),
            }],
            turn: Some("theirs".into()),
            turn_since: Some("2026-11-05T09:00:00-05:00".into()),
            action: "held",
            nudge_on: Some("2026-11-06".into()),
        };
        let value = watch_json(&row);
        assert_eq!(value["thread_id"], "aa01");
        assert_eq!(value["note"], "mail/2026/aa01.md");
        assert_eq!(value["tasks"][0]["path"], "job.md");
        assert_eq!(value["tasks"][0]["line"], 8);
        assert_eq!(value["tasks"][0]["block_id"], "follow-up");
        assert_eq!(value["turn"], "theirs");
        assert_eq!(value["turn_since"], "2026-11-05T09:00:00-05:00");
        assert_eq!(value["action"], "held");
        assert_eq!(value["nudge_on"], "2026-11-06");

        for (action, expected) in [
            ("held", "awaiting reply"),
            ("lifted", "replied"),
            ("bounced", "bounced"),
            ("deferred_running", "deferred (running task)"),
        ] {
            let row = WatchThreadOutcome {
                action,
                ..row.clone()
            };
            let line = watch_human_row(&row).expect("visible watch action");
            assert!(line.contains(expected), "{line}");
            assert!(line.contains("Dana Whitfield"), "{line}");
            assert!(line.contains("Lease renewal"), "{line}");
        }

        let gone = gone_human_row(&WatchGoneOutcome {
            key: ThreadKey::new("me@example.com", "aa01").unwrap(),
            note: Some("mail/2026/aa01.md".into()),
            title: Some("Lease renewal".into()),
            who: Some("Dana Whitfield".into()),
        })
        .expect("gone warning row");
        assert!(gone.contains("gone"), "{gone}");
        assert!(gone.contains("Dana Whitfield — Lease renewal"), "{gone}");
    }

    #[test]
    fn pulled_counts_route_problems_and_human_hides_already_rows() {
        let problem = SyncResult {
            thread_id: "bb02".into(),
            note: Some("mail/2026/b.md".into()),
            link: Some("[[mail/2026/b.md|✉️ Kickoff · Lee]]".into()),
            status: "problem",
            task_file: Some("gmail_inbox.md".into()),
            route: None,
        };
        let filed_problem = SyncProblem {
            thread_id: "bb02".into(),
            note: Some("mail/2026/b.md".into()),
            label: "#/missing".into(),
            kind: "unknown_note".into(),
            message: "unknown note".into(),
            hint: "create the note".into(),
            first_seen: true,
        };
        let already = SyncResult {
            thread_id: "cc03".into(),
            note: Some("mail/2026/c.md".into()),
            link: None,
            status: "already_filed",
            task_file: Some("job.md".into()),
            route: Some("job".into()),
        };
        assert_eq!(human_row(&already, &[]), None);
        let line = human_row(&problem, std::slice::from_ref(&filed_problem))
            .expect("problem row");
        assert!(line.contains("route"), "{line}");
        assert!(line.contains("gmail_inbox.md"), "{line}");
        assert!(line.contains("#/missing"), "{line}");
        assert!(line.contains("unknown note"), "{line}");
        assert!(line.contains("create the note"), "{line}");
        let remote = SyncResult {
            thread_id: "dd04".into(),
            note: None,
            link: None,
            status: "problem",
            task_file: None,
            route: None,
        };
        let failed_line = human_row(&remote, &[]).expect("remote failure");
        assert!(failed_line.contains("failed"), "{failed_line}");
        assert!(failed_line.contains("see warnings"), "{failed_line}");
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 1,
            results: vec![filed()],
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 1,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: Vec::new(),
            archive_digest: None,
            archive_config: None,
            archive_hints: Vec::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        };
        let one = header(&report);
        assert!(one.contains("watching 1 thread ·"), "{one}");
        assert!(!one.contains("1 threads"), "{one}");
        let many = header(&SyncReport {
            watching: 2,
            ..report
        });
        assert!(many.contains("watching 2 threads ·"), "{many}");
    }

    #[test]
    fn nothing_new_when_only_already_enrolled() {
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 0,
            results: vec![SyncResult {
                thread_id: "aa01".into(),
                note: None,
                link: None,
                status: "already_pulled",
                task_file: None,
                route: None,
            }],
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: Vec::new(),
            archive_digest: None,
            archive_config: None,
            archive_hints: Vec::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        };
        let text = with_nothing(&report, "header".into());
        assert!(text.ends_with("nothing new"), "{text}");
    }

    #[test]
    fn refiled_rows_print_and_suppress_nothing_new() {
        let refiled = SyncResult {
            thread_id: "aa01".into(),
            note: Some("mail/2026/a.md".into()),
            link: Some("[[mail/2026/a.md|✉️ Lease · Ada]]".into()),
            status: "refiled",
            task_file: Some("job.md".into()),
            route: Some("job".into()),
        };
        let line = human_row(&refiled, &[]).expect("refiled row");
        assert!(line.contains("refiled"), "{line}");
        assert!(line.contains("job.md"), "{line}");
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 0,
            results: vec![refiled],
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 1,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: Vec::new(),
            archive_digest: None,
            archive_config: None,
            archive_hints: Vec::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        };
        let text = with_nothing(&report, "header".into());
        assert!(!text.contains("nothing new"), "{text}");
    }

    fn empty_fold() -> super::super::enroll::EnrollmentFold {
        let temp = tempfile::tempdir().unwrap();
        super::super::enroll::EnrollmentFold::load(temp.path()).unwrap()
    }

    fn filing_key(id: &str) -> ThreadKey {
        ThreadKey::new("me@example.com", id).unwrap()
    }

    fn filed_result(id: &str) -> FilingResult {
        FilingResult {
            key: filing_key(id),
            note: Some(format!("mail/2026/{id}.md")),
            link: Some(format!("[[mail/2026/{id}.md|✉️ Title · Who]]")),
            task_file: Some("job.md".into()),
            status: "filed",
            route: Some("job".into()),
            via: Some(super::super::enroll::FiledVia::Route),
            problem: None,
            first_seen: false,
            watch_member: true,
        }
    }

    #[test]
    fn permanent_fetch_failure_alone_counts_nothing_and_fails() {
        let fold = empty_fold();
        let outcome = FilingOutcome::default();
        let extra = vec![SyncResult {
            thread_id: "aa01".into(),
            note: None,
            link: None,
            status: "problem",
            task_file: None,
            route: None,
        }];
        let report = from_outcome(
            false,
            outcome,
            extra,
            vec!["skipped Gmail thread aa01: not_found".into()],
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 0);
        assert_eq!(report.watching, 0);
        assert!(!report.ok);
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn mixed_success_and_failure_counts_only_durable_filings() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.results.push(filed_result("aa01"));
        outcome.failures.push(FilingFailure {
            key: filing_key("bb02"),
            kind: "not_found".into(),
            message: "gone".into(),
        });
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 1);
        assert_eq!(report.watching, 1);
        assert!(!report.ok);
        assert!(report.warnings.iter().any(|w| w.contains("not_found")));
    }

    #[test]
    fn pending_journal_without_deferred_is_not_watching() {
        let fold = empty_fold();
        let mut pending = filed_result("aa01");
        pending.watch_member = false;
        let mut outcome = FilingOutcome::default();
        outcome.results.push(pending);
        outcome.failed = true;
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 0);
        assert_eq!(report.awaiting, 0);
        assert!(!report.ok);
    }

    #[test]
    fn deferred_failed_enrollment_is_not_newly_enrolled_watch() {
        let fold = empty_fold();
        let mut adopted = filed_result("aa01");
        adopted.status = "adopted";
        adopted.via = None;
        // A successful adoption counts as watch ...
        let mut outcome = FilingOutcome::default();
        outcome.results.push(adopted.clone());
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 1);
        assert!(report.ok);
        // ... but when its enrollment append failed, the key is deferred:
        // still reported as a failure, never as newly enrolled watch.
        let mut outcome = FilingOutcome::default();
        outcome.results.push(adopted);
        outcome.deferred.push(filing_key("aa01"));
        outcome.failed = true;
        outcome
            .warnings
            .push("could not append Gmail enrollment journal: test".into());
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            vec!["could not append Gmail enrollment journal: test".into()],
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 0);
        assert!(!report.ok);
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn filed_route_problem_counts_once_with_first_seen_failure() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.results.push(FilingResult {
            problem: Some(RouteProblem {
                kind:
                    crate::native::gmail::route::RouteProblemKind::UnknownNote,
                label: "#/missing".into(),
                message: "unknown note".into(),
                hint: "create the note".into(),
            }),
            first_seen: true,
            ..filed_result("aa01")
        });
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 1);
        assert_eq!(report.problems.len(), 1);
        assert!(!report.ok);
        assert!(report.problems[0].first_seen);
    }

    #[test]
    fn dry_run_parity_counts_projected_filings() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.results.push(FilingResult {
            status: "would_file",
            ..filed_result("aa01")
        });
        let report = from_outcome(
            true,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 1);
        assert_eq!(report.watching, 1);
        assert!(report.dry_run);
    }

    fn enrolled_fold_with(
        id: &str,
    ) -> (tempfile::TempDir, super::super::enroll::EnrollmentFold) {
        use std::fs;
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".bob/gmail");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("athena.jsonl"),
            format!(
                "{{\"v\":1,\"ts\":\"2026-11-01T09:00:00-04:00\",\"host\":\"athena\",\
                 \"kind\":\"gmail.filed\",\"value\":{{\"account\":\"me@example.com\",\
                 \"thread_id\":\"{id}\",\"note\":\"mail/2026/{id}.md\",\
                 \"task_file\":\"job.md\",\"via\":\"route\",\"route\":\"job\"}}}}\n"
            ),
        )
        .unwrap();
        let fold =
            super::super::enroll::EnrollmentFold::load(temp.path()).unwrap();
        (temp, fold)
    }

    fn scan_with(id: &str) -> WatchScan {
        WatchScan::from_tasks_for_test(vec![WatchingTask {
            path: "job.md".into(),
            line: 1,
            text: format!("- [ ] #task [[mail/2026/{id}.md]]"),
            block_id: None,
            status: ' ',
            targets: vec![WatchTarget {
                key: filing_key(id),
                note: format!("mail/2026/{id}.md"),
                placement: LinkPlacement::TaskLine,
            }],
        }])
    }

    #[test]
    fn skipped_apply_keeps_the_run_scan_when_fetches_were_deferred() {
        let (_temp, fold) = enrolled_fold_with("aa01");
        let scan = scan_with("aa01");
        let mut outcome = FilingOutcome::default();
        outcome.failed = true;
        outcome.deferred.push(filing_key("bb02"));
        // Apply skipped: empty snapshot must not zero watching/awaiting.
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            vec!["deferred remaining Gmail fetches".into()],
            &scan,
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 1);
        assert!(!report.ok);
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn watcher_closed_between_scan_and_apply_is_not_counted() {
        let (_temp, fold) = enrolled_fold_with("aa01");
        let initial = scan_with("aa01");
        let mut outcome = FilingOutcome::default();
        outcome.applied = true;
        outcome.watch_snapshot = WatchScan::default();
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &initial,
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 0);
        assert_eq!(report.awaiting, 0);
    }

    #[test]
    fn planning_failure_warning_includes_kind_and_message() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.failures.push(FilingFailure {
            key: filing_key("aa01"),
            kind: "note_changed".into(),
            message: "the adopted mail note or task disappeared during filing"
                .into(),
        });
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert!(
            report.warnings.iter().any(|warning| {
                warning.contains("note_changed")
                    && warning.contains("disappeared during filing")
            }),
            "{:?}",
            report.warnings
        );
        assert!(!report.ok);
    }

    #[test]
    fn dry_run_and_live_counts_match_on_the_same_projected_row() {
        let fold = empty_fold();
        let mut live = FilingOutcome::default();
        live.applied = true;
        live.results.push(filed_result("aa01"));
        let mut dry = FilingOutcome::default();
        dry.applied = true;
        dry.results.push(FilingResult {
            status: "would_file",
            ..filed_result("aa01")
        });
        let live_report = from_outcome(
            false,
            live,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        let dry_report = from_outcome(
            true,
            dry,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(live_report.pulled, dry_report.pulled);
        assert_eq!(live_report.watching, dry_report.watching);
        assert_eq!(live_report.pulled, 1);
        assert!(dry_report.dry_run);
        assert!(!live_report.dry_run);
    }

    fn surfaces_row(
        action: &'static str,
        reason: &'static str,
        decision: &str,
    ) -> ArchiveRow {
        ArchiveRow {
            thread_id: "aa01".into(),
            note: "mail/2026/aa01.md".into(),
            action,
            reason,
            decision: decision.to_string(),
            messages: vec!["00000000000000a0".into()],
            tasks: vec![ArchiveTaskRef {
                path: "job.md".into(),
                line: 12,
                block_id: Some("follow-up".into()),
            }],
        }
    }

    fn surfaces_report(
        dry_run: bool,
        rows: Vec<ArchiveRow>,
        digest: Option<String>,
    ) -> SyncReport {
        SyncReport {
            ok: true,
            dry_run,
            pulled: 0,
            results: Vec::new(),
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: rows,
            archive_digest: digest,
            archive_config: None,
            archive_hints: Vec::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        }
    }

    #[test]
    fn live_success_omits_digest_in_json_and_hides_quiet_leak() {
        let live = surfaces_report(
            false,
            vec![surfaces_row("archived", "eligible", "would_archive")],
            Some("abcd".into()),
        );
        let value = json_document(&live);
        assert!(
            value.get("archive_digest").is_none(),
            "live success omits digest: {value}"
        );
        assert_eq!(value["archive"][0]["decision"], "would_archive");
    }

    #[test]
    fn dry_run_and_paused_runs_keep_digest() {
        let dry = surfaces_report(
            true,
            vec![surfaces_row("would_archive", "eligible", "would_archive")],
            Some("abcd".into()),
        );
        assert_eq!(
            json_document(&dry)["archive_digest"],
            "abcd",
            "dry-run keeps digest"
        );
        let paused = surfaces_report(
            false,
            vec![surfaces_row(
                "paused",
                "backlog_unconfirmed",
                "would_archive",
            )],
            Some("abcd".into()),
        );
        assert_eq!(
            json_document(&paused)["archive_digest"],
            "abcd",
            "paused keeps digest"
        );
    }

    #[test]
    fn paused_rows_show_gate_reason_and_decision_without_dangling_arrow() {
        let row =
            surfaces_row("paused", "backlog_unconfirmed", "would_archive");
        let line = archive_human_row(&row).expect("paused row");
        assert!(line.contains("backlog_unconfirmed"), "{line}");
        assert!(line.contains("would_archive"), "{line}");
        assert!(line.contains("mail/2026/aa01.md"), "{line}");
        assert!(!line.trim_end().ends_with('→'), "{line}");
        let kept = surfaces_row("paused", "over_limit", "kept:unsaved");
        let line = archive_human_row(&kept).expect("paused kept");
        assert!(line.contains("kept:unsaved"), "{line}");
    }

    #[test]
    fn kept_and_let_go_rows_render_note_without_dangling_arrow() {
        let kept = surfaces_row("kept", "unsaved", "kept:unsaved");
        let line = archive_human_row(&kept).expect("kept row");
        assert!(line.contains("mail/2026/aa01.md"), "{line}");
        assert!(!line.trim_end().ends_with('→'), "{line}");
        let let_go = surfaces_row("let_go", "replied", "let_go");
        let line = archive_human_row(&let_go).expect("let-go row");
        assert!(line.contains("back in inbox"), "{line}");
        assert!(line.contains("mail/2026/aa01.md"), "{line}");
        assert!(!line.trim_end().ends_with('→'), "{line}");
        let value = json_document(&surfaces_report(false, vec![kept], None));
        assert_eq!(value["archive"][0]["note"], "mail/2026/aa01.md");
        assert_eq!(
            value["archive"][0]["messages"],
            serde_json::json!(["00000000000000a0"])
        );
        assert_eq!(value["archive"][0]["tasks"][0]["path"], "job.md");
    }

    #[test]
    fn no_archive_skip_serializes_additively() {
        let mut report = surfaces_report(false, Vec::new(), None);
        report.archive_skipped = true;
        let value = json_document(&report);
        assert_eq!(value["archive_skipped"], true);
    }
}

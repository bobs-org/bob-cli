//! Watch transaction: apply prepared turns through the filing transaction.
//!
//! Remote preparation lives in [`super::watch`]; this module never performs
//! a Gmail request, never takes a lock, and never commits. Callers hold
//! their lock while they reload snapshots, call [`plan_watch_edits`],
//! stage the returned file edits with their own scoped commit, and append
//! the returned markers only after those writes are durable.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::{
    hold,
    scan::{WatchScan, WatchingTask},
    watch::{PreparedWatch, TaskEffect},
    watch_state::{AppliedEffect, AppliedTurn, WatchJournal},
    ThreadKey,
};
use crate::native::gmail::{
    note::parse as note_parse,
    turn::{Net, Turn},
};

/// Typed per-thread watch outcome for the transaction boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchThreadOutcome {
    pub(crate) key: ThreadKey,
    pub(crate) note: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) who: Option<String>,
    pub(crate) tasks: Vec<WatchTaskRef>,
    pub(crate) turn: Option<String>,
    pub(crate) turn_since: Option<String>,
    pub(crate) action: &'static str,
    pub(crate) nudge_on: Option<String>,
}

/// A first report that a watched Gmail thread disappeared. Gone is surfaced
/// as a warning/human row and deliberately has no `watch[]` action value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchGoneOutcome {
    pub(crate) key: ThreadKey,
    pub(crate) note: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) who: Option<String>,
}

/// One task row a thread outcome touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchTaskRef {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: Option<String>,
}

/// Body-free pending watch evidence in the host-local recovery file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RecoveryWatch {
    pub(crate) account: String,
    pub(crate) thread_id: String,
    pub(crate) message_id: String,
    pub(crate) internal_date: i64,
    pub(crate) turn: String,
    pub(crate) effect: String,
    #[serde(default)]
    pub(crate) nudge_on: Option<String>,
    #[serde(default)]
    pub(crate) unblocked_on: Option<String>,
    #[serde(default = "default_true")]
    pub(crate) pending_tasks: bool,
    #[serde(default = "default_true")]
    pub(crate) pending_marker: bool,
}

fn default_true() -> bool {
    true
}

impl RecoveryWatch {
    pub(crate) fn key(&self) -> Option<ThreadKey> {
        ThreadKey::new(&self.account, &self.thread_id)
    }
}

/// One file edit the caller stages with its own scoped commit.
#[derive(Debug, Clone)]
pub(crate) struct WatchFileEdit {
    pub(crate) target: PathBuf,
    pub(crate) original: String,
    pub(crate) updated: String,
    pub(crate) existed: bool,
}

/// Markers to append after the caller's writes are durable.
#[derive(Debug, Clone, Default)]
pub(crate) struct WatchReceipts {
    pub(crate) turns: Vec<AppliedTurn>,
    pub(crate) gone: Vec<ThreadKey>,
}

/// One filing task file snapshot for coalescing.
#[derive(Debug, Clone)]
pub(crate) struct FilingTaskSnapshot {
    pub(crate) target: PathBuf,
    pub(crate) original: Option<String>,
    pub(crate) updated: String,
    /// New task rows from this transaction, with their just-filed identities.
    pub(crate) tasks: Vec<WatchingTask>,
}

/// Inputs for the shared local planner.
pub(crate) struct WatchPlanInput<'a> {
    pub(crate) vault: &'a Path,
    pub(crate) timezone: chrono_tz::Tz,
    pub(crate) reply_wait_days: u32,
    pub(crate) today: NaiveDate,
    pub(crate) blocked_ok: bool,
    pub(crate) blocked_reason: Option<String>,
    pub(crate) running: hold::RunningLookup,
    pub(crate) watchers: &'a WatchScan,
    pub(crate) prepared: &'a [PreparedWatch],
    pub(crate) journal: &'a WatchJournal,
    pub(crate) filing_tasks: &'a BTreeMap<PathBuf, FilingTaskSnapshot>,
    pub(crate) pending: &'a [RecoveryWatch],
}

/// Result of [`plan_watch_edits`].
#[derive(Debug, Default)]
pub(crate) struct WatchPlan {
    pub(crate) edits: Vec<WatchFileEdit>,
    pub(crate) outcomes: Vec<WatchThreadOutcome>,
    pub(crate) gone: Vec<WatchGoneOutcome>,
    pub(crate) snapshot: WatchScan,
    pub(crate) receipts: WatchReceipts,
    pub(crate) pending: Vec<RecoveryWatch>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failed: bool,
}

pub(crate) const ACTION_HELD: &str = "held";
pub(crate) const ACTION_LIFTED: &str = "lifted";
pub(crate) const ACTION_BOUNCED: &str = "bounced";
pub(crate) const ACTION_RESURFACED: &str = "resurfaced";
pub(crate) const ACTION_DEFERRED_RUNNING: &str = "deferred_running";
pub(crate) const ACTION_REFRESHED: &str = "refreshed";
pub(crate) const ACTION_UNCHANGED: &str = "unchanged";

/// Plan watch task edits and marker receipts without locking or committing.
pub(crate) fn plan_watch_edits(input: WatchPlanInput<'_>) -> WatchPlan {
    let mut plan = WatchPlan::default();
    let mut prepared_by_key: BTreeMap<ThreadKey, &PreparedWatch> =
        BTreeMap::new();
    for watch in input.prepared {
        prepared_by_key.insert(watch.observation.key.clone(), watch);
    }
    // Pending turns superseded by a newer prepared turn drop; the fresh
    // turn replaces them.
    let mut pending_by_key: BTreeMap<ThreadKey, &RecoveryWatch> =
        BTreeMap::new();
    for pending in input.pending {
        let Some(key) = pending.key() else {
            plan.warnings.push(format!(
                "skipping stored Gmail watch recovery with an invalid identity: {}",
                pending.thread_id
            ));
            plan.failed = true;
            continue;
        };
        if let Some(prepared) = prepared_by_key.get(&key) {
            let fresh_id =
                prepared.observation.folded.newest_counted_id.as_deref();
            let fresh_date = prepared
                .observation
                .folded
                .newest_internal_date
                .unwrap_or(0);
            let same = fresh_id
                .is_some_and(|id| id.eq_ignore_ascii_case(&pending.message_id))
                && fresh_date == pending.internal_date;
            if !same && fresh_id.is_some() {
                continue;
            }
        }
        pending_by_key.insert(key, pending);
    }

    let projected = projected_tasks(input.watchers, input.filing_tasks);
    let projected_scan = WatchScan::from_tasks(projected.clone());
    let intents = super::watch::plan_tasks(&projected_scan, input.prepared);

    let mut buffers: BTreeMap<PathBuf, FileBuffer> = BTreeMap::new();
    for (relative, snapshot) in input.filing_tasks {
        buffers.insert(
            relative.clone(),
            FileBuffer {
                target: snapshot.target.clone(),
                original: snapshot.original.clone(),
                updated: snapshot.updated.clone(),
            },
        );
    }

    // Map each intent back to its projected task to find its threads.
    let task_by_location: BTreeMap<
        (String, usize, Option<String>),
        &WatchingTask,
    > = projected
        .iter()
        .map(|task| {
            ((task.path.clone(), task.line, task.block_id.clone()), task)
        })
        .collect();

    let mut deferred_threads: BTreeSet<ThreadKey> = BTreeSet::new();
    let mut changed_tasks: Vec<(
        ThreadKey,
        WatchTaskRef,
        &'static str,
        Option<String>,
    )> = Vec::new();

    for intent in &intents {
        let Some(task) = task_by_location
            .get(&(intent.path.clone(), intent.line, intent.block_id.clone()))
            .copied()
        else {
            continue;
        };
        let thread_keys: Vec<ThreadKey> = task
            .targets
            .iter()
            .map(|target| target.key.clone())
            .collect();
        if thread_keys.is_empty() {
            continue;
        }
        let Some(effect) = intent.effect.clone() else {
            if intent.deferred_unknown {
                for key in &thread_keys {
                    deferred_threads.insert(key.clone());
                }
            }
            continue;
        };
        // Marker-only retries skip task rewrites: the tasks are already
        // durable and only the journal append remains. This preserves the
        // original [unblocked::] date across a later-day recovery instead
        // of refetching or reapplying the completed effect.
        let mut marker_only = true;
        for key in &thread_keys {
            let Some(prepared) = prepared_by_key.get(key) else {
                marker_only = false;
                break;
            };
            let folded = &prepared.observation.folded;
            let (Some(fresh_id), Some(fresh_date)) = (
                folded.newest_counted_id.clone(),
                folded.newest_internal_date,
            ) else {
                marker_only = false;
                break;
            };
            let Some(pending) = pending_by_key.get(key) else {
                marker_only = false;
                break;
            };
            if pending.pending_tasks
                || !pending.message_id.eq_ignore_ascii_case(&fresh_id)
                || pending.internal_date != fresh_date
            {
                marker_only = false;
                break;
            }
        }
        if marker_only {
            continue;
        }
        // Revalidate: reread the line under the lock. Closed or deleted
        // tasks always win and write nothing.
        let live = match resolve_live_task(input.vault, task, &mut buffers) {
            Some(live) => live,
            None => continue,
        };
        match effect {
            TaskEffect::Hold { nudge_at } => {
                let nudge = hold::nudge_date_for_send(
                    nudge_at,
                    input.timezone,
                    input.reply_wait_days,
                );
                if hold::is_running_task(
                    &input.running,
                    &live.path,
                    live.block_id.as_deref(),
                ) {
                    for key in &thread_keys {
                        deferred_threads.insert(key.clone());
                    }
                    // Pending retention happens once in the final pass so a
                    // running deferral never duplicates its turn evidence.
                    if matches!(
                        input.running,
                        hold::RunningLookup::Ambiguous(_)
                    ) {
                        plan.warnings.push(
                            "deferring Gmail reply holds while today's ledger is ambiguous".to_string(),
                        );
                    }
                    continue;
                }
                let req = hold::HoldRequest {
                    nudge_date: nudge,
                    today: input.today,
                    blocked_ok: input.blocked_ok,
                    blocked_reason: input.blocked_reason.clone(),
                };
                match apply_hold_to_buffer(
                    input.vault,
                    &mut buffers,
                    &live,
                    &req,
                ) {
                    HoldApply::Applied => {
                        for key in &thread_keys {
                            changed_tasks.push((
                                key.clone(),
                                WatchTaskRef {
                                    path: live.path.clone(),
                                    line: live.line,
                                    block_id: live.block_id.clone(),
                                },
                                ACTION_HELD,
                                Some(nudge.format("%Y-%m-%d").to_string()),
                            ));
                        }
                    }
                    HoldApply::Noop => {}
                    HoldApply::Refused { reason } => {
                        plan.warnings.push(format!(
                            "skipped Gmail reply hold for {}:{}: {}",
                            live.path, live.line, reason
                        ));
                    }
                    HoldApply::Missing => {
                        for key in &thread_keys {
                            deferred_threads.insert(key.clone());
                        }
                        continue;
                    }
                }
            }
            TaskEffect::Lifted | TaskEffect::Bounced => {
                match apply_lift_to_buffer(
                    input.vault,
                    &mut buffers,
                    &live,
                    input.today,
                ) {
                    LiftApply::Applied { unblocked, lifted } => {
                        // Unheld Ready rows that still resurface report
                        // `resurfaced`; held rows report lift/bounce.
                        let action = match effect {
                            TaskEffect::Bounced => ACTION_BOUNCED,
                            _ if lifted => ACTION_LIFTED,
                            _ if unblocked => ACTION_RESURFACED,
                            _ => ACTION_LIFTED,
                        };
                        for key in &thread_keys {
                            changed_tasks.push((
                                key.clone(),
                                WatchTaskRef {
                                    path: live.path.clone(),
                                    line: live.line,
                                    block_id: live.block_id.clone(),
                                },
                                action,
                                None,
                            ));
                        }
                    }
                    LiftApply::Noop { lifted } => {
                        if lifted {
                            let action = match effect {
                                TaskEffect::Bounced => ACTION_BOUNCED,
                                _ => ACTION_LIFTED,
                            };
                            for key in &thread_keys {
                                changed_tasks.push((
                                    key.clone(),
                                    WatchTaskRef {
                                        path: live.path.clone(),
                                        line: live.line,
                                        block_id: live.block_id.clone(),
                                    },
                                    action,
                                    None,
                                ));
                            }
                        }
                    }
                    LiftApply::Missing => {
                        for key in &thread_keys {
                            deferred_threads.insert(key.clone());
                        }
                        continue;
                    }
                }
            }
        }
    }

    // Mail-note turn refreshes: frontmatter-only updates stage as file edits
    // so an unchanged note stays byte-identical.
    for watch in input.prepared {
        if watch.report_gone {
            continue;
        }
        let Some(update) = &watch.turn_update else {
            continue;
        };
        let relative = PathBuf::from(&update.path);
        let target = input.vault.join(&relative);
        let buffer = buffers.entry(relative.clone()).or_insert_with(|| {
            let original = fs::read_to_string(&target).ok();
            let updated = original.clone().unwrap_or_default();
            FileBuffer {
                target: target.clone(),
                original,
                updated,
            }
        });
        if buffer.original.is_none() && buffer.updated.is_empty() {
            plan.warnings.push(format!(
                "deferred Gmail watch refresh for a missing mail note: {}",
                update.path
            ));
            plan.failed = true;
            continue;
        }
        let current = buffer.updated.clone();
        match note_parse::update_turn_fields(
            &current,
            update.turn.as_deref(),
            update.turn_since.as_deref(),
        ) {
            Ok(Some(next)) => {
                buffer.updated = next;
            }
            Ok(None) => {}
            Err(_) => {
                plan.warnings.push(format!(
                    "skipped Gmail watch turn update for a damaged mail note: {}",
                    update.path
                ));
            }
        }
    }

    for buffer in buffers.values() {
        // Only emit edits for files this planner changed beyond the filing
        // snapshots it started from. Filing-owned buffers already contain
        // filing's own edits; emit only when watch added more.
        let _ = buffer;
    }
    // Emit every changed buffer relative to its on-disk original. Filing
    // snapshots are part of the same scoped commit, so the caller merges
    // these with its own staged list and dedups by path (watch wins for
    // task files it touched because its buffer started from the filing
    // snapshot).
    for (relative, buffer) in &buffers {
        let on_disk_original = buffer.original.clone().unwrap_or_default();
        // Skip buffers that only carry filing's own snapshot when this
        // planner made no watch change: compare against the filing input.
        let filing_updated =
            input.filing_tasks.get(relative).map(|s| s.updated.as_str());
        let watch_changed = match filing_updated {
            Some(filing_text) => buffer.updated != filing_text,
            None => buffer.updated != on_disk_original,
        };
        if watch_changed {
            plan.edits.push(WatchFileEdit {
                target: buffer.target.clone(),
                original: on_disk_original,
                updated: buffer.updated.clone(),
                existed: buffer.original.is_some(),
            });
        }
    }

    // Per-thread marker advancement.
    let mut terminal: BTreeSet<ThreadKey> = BTreeSet::new();
    for watch in input.prepared {
        if watch.report_gone {
            plan.gone.push(WatchGoneOutcome {
                key: watch.observation.key.clone(),
                note: watch.note_path.clone(),
                title: watch.title.clone(),
                who: watch.who.clone(),
            });
            plan.warnings.push(format!(
                "Gmail watch thread {} is gone",
                watch.observation.key.thread_id()
            ));
            if !input.journal.was_reported_gone(&watch.observation.key) {
                plan.receipts.gone.push(watch.observation.key.clone());
            }
            continue;
        }
        let folded = &watch.observation.folded;
        if matches!(folded.net, Net::Unknown(_)) {
            continue;
        }
        if folded.newest_counted_id.is_none() {
            continue;
        }
        terminal.insert(watch.observation.key.clone());
    }
    for key in &deferred_threads {
        terminal.remove(key);
    }
    for watch in input.prepared {
        if watch.report_gone {
            continue;
        }
        if !terminal.contains(&watch.observation.key) {
            continue;
        }
        let folded = &watch.observation.folded;
        let (Some(message_id), Some(internal_date)) = (
            folded.newest_counted_id.clone(),
            folded.newest_internal_date,
        ) else {
            continue;
        };
        if let Some(marker) = input.journal.marker(&watch.observation.key) {
            if marker.message_id.eq_ignore_ascii_case(&message_id)
                && marker.internal_date == internal_date
            {
                terminal.remove(&watch.observation.key);
                continue;
            }
            if (marker.internal_date, marker.message_id.as_str())
                > (internal_date, message_id.as_str())
            {
                terminal.remove(&watch.observation.key);
                continue;
            }
        }
        let (turn, effect) = match folded.turn {
            Some(Turn::Theirs) => (Turn::Theirs, AppliedEffect::Hold),
            Some(Turn::Yours) => match folded.net {
                Net::Bounced => (Turn::Yours, AppliedEffect::Bounced),
                _ => (Turn::Yours, AppliedEffect::Lift),
            },
            None => continue,
        };
        plan.receipts.turns.push(AppliedTurn {
            key: watch.observation.key.clone(),
            message_id: message_id.to_ascii_lowercase(),
            internal_date,
            turn,
            effect,
        });
    }
    // Remove receipts for threads that lost terminal status above.
    plan.receipts
        .turns
        .retain(|receipt| terminal.contains(&receipt.key));

    // Typed per-thread outcomes.
    for watch in input.prepared {
        if watch.report_gone {
            continue;
        }
        let folded = &watch.observation.folded;
        let turn = folded.turn.map(|turn| turn.as_str().to_string());
        let turn_since = folded.turn_since.map(|time| {
            time.with_timezone(&input.timezone)
                .format("%Y-%m-%dT%H:%M:%S%:z")
                .to_string()
        });
        let is_terminal = terminal.contains(&watch.observation.key);
        let has_receipt = plan
            .receipts
            .turns
            .iter()
            .any(|receipt| receipt.key == watch.observation.key);
        let mut action = ACTION_UNCHANGED;
        let mut nudge_on = None;
        // Task-level results win over the thread net: an unheld Ready row
        // that resurfaces reports `resurfaced`, not `lifted`.
        let mut task_action: Option<&'static str> = None;
        for (key, _, action, _) in &changed_tasks {
            if key != &watch.observation.key {
                continue;
            }
            if *action == ACTION_LIFTED
                || *action == ACTION_BOUNCED
                || *action == ACTION_HELD
            {
                task_action = Some(*action);
                break;
            }
            if *action == ACTION_RESURFACED && task_action.is_none() {
                task_action = Some(*action);
            }
        }
        if deferred_threads.contains(&watch.observation.key) {
            action = ACTION_DEFERRED_RUNNING;
            if matches!(folded.net, Net::Hold) {
                if let Some(send) = folded.last_send_at {
                    nudge_on = Some(
                        hold::nudge_date_for_send(
                            send,
                            input.timezone,
                            input.reply_wait_days,
                        )
                        .format("%Y-%m-%d")
                        .to_string(),
                    );
                }
            }
        } else if let Some(task_action) = task_action {
            action = task_action;
            if matches!(folded.net, Net::Hold) {
                if let Some(send) = folded.last_send_at {
                    nudge_on = Some(
                        hold::nudge_date_for_send(
                            send,
                            input.timezone,
                            input.reply_wait_days,
                        )
                        .format("%Y-%m-%d")
                        .to_string(),
                    );
                }
            }
        } else if is_terminal && has_receipt {
            action = match folded.net {
                Net::Hold => ACTION_HELD,
                Net::Lift => ACTION_LIFTED,
                Net::Bounced => ACTION_BOUNCED,
                Net::Nothing => ACTION_REFRESHED,
                Net::Unknown(_) => ACTION_UNCHANGED,
            };
            if matches!(folded.net, Net::Hold) {
                if let Some(send) = folded.last_send_at {
                    nudge_on = Some(
                        hold::nudge_date_for_send(
                            send,
                            input.timezone,
                            input.reply_wait_days,
                        )
                        .format("%Y-%m-%d")
                        .to_string(),
                    );
                }
            }
        }
        let tasks: Vec<WatchTaskRef> = changed_tasks
            .iter()
            .filter(|(key, _, _, _)| key == &watch.observation.key)
            .map(|(_, task_ref, _, _)| task_ref.clone())
            .collect();
        let note = watch.note_path.clone().or_else(|| {
            input
                .filing_tasks
                .values()
                .flat_map(|snapshot| snapshot.tasks.iter())
                .flat_map(|task| task.targets.iter())
                .find(|target| target.key == watch.observation.key)
                .map(|target| target.note.clone())
        });
        // Override deferred display with the task-level nudge when known.
        if action == ACTION_DEFERRED_RUNNING {
            if let Some((_, _, _, nudge)) = changed_tasks
                .iter()
                .find(|(key, _, _, _)| key == &watch.observation.key)
            {
                nudge_on = nudge.clone();
            }
        }
        if action == ACTION_HELD {
            if let Some((_, _, _, nudge)) = changed_tasks
                .iter()
                .find(|(key, _, _, _)| key == &watch.observation.key)
            {
                nudge_on = nudge.clone();
            }
        }
        plan.outcomes.push(WatchThreadOutcome {
            key: watch.observation.key.clone(),
            note,
            title: watch.title.clone(),
            who: watch.who.clone(),
            tasks,
            turn,
            turn_since,
            action,
            nudge_on,
        });
    }

    // Pending retention for threads that did not advance.
    for watch in input.prepared {
        if watch.report_gone {
            continue;
        }
        if terminal.contains(&watch.observation.key)
            && plan
                .receipts
                .turns
                .iter()
                .any(|receipt| receipt.key == watch.observation.key)
        {
            continue;
        }
        // Unknown with no new move needs no recovery; deferred and
        // not-yet-terminal turns do.
        let folded = &watch.observation.folded;
        if matches!(folded.net, Net::Unknown(_)) {
            continue;
        }
        let (Some(message_id), Some(internal_date)) = (
            folded.newest_counted_id.clone(),
            folded.newest_internal_date,
        ) else {
            continue;
        };
        if !deferred_threads.contains(&watch.observation.key)
            && matches!(folded.net, Net::Nothing)
        {
            continue;
        }
        let (turn_text, effect_text) = match folded.turn {
            Some(Turn::Theirs) => ("theirs", "hold"),
            Some(Turn::Yours) => match folded.net {
                Net::Bounced => ("yours", "bounced"),
                _ => ("yours", "lift"),
            },
            None => continue,
        };
        // A newer prepared turn already supersedes pending; only retain
        // when this turn is still the newest known.
        plan.pending.push(RecoveryWatch {
            account: watch.observation.key.account().to_string(),
            thread_id: watch.observation.key.thread_id().to_string(),
            message_id: message_id.to_ascii_lowercase(),
            internal_date,
            turn: turn_text.to_string(),
            effect: effect_text.to_string(),
            nudge_on: None,
            unblocked_on: None,
            pending_tasks: true,
            pending_marker: true,
        });
    }

    let mut snapshot_tasks = projected;
    for task in &mut snapshot_tasks {
        let relative = PathBuf::from(&task.path);
        let Some(buffer) = buffers.get(&relative) else {
            continue;
        };
        if let Some(line) = read_line(&buffer.updated, task.line) {
            task.status = checkbox_status(&line).unwrap_or(task.status);
            task.text = line;
        }
    }
    plan.snapshot = WatchScan::from_tasks(snapshot_tasks);

    plan
}

/// Non-locking planner for send finalization.
pub(crate) fn plan_for_send(
    vault: &Path,
    timezone: chrono_tz::Tz,
    reply_wait_days: u32,
    today: NaiveDate,
    running: hold::RunningLookup,
    watchers: &WatchScan,
    prepared: &[PreparedWatch],
    journal: &WatchJournal,
    pending: &[RecoveryWatch],
) -> WatchPlan {
    let (blocked_ok, blocked_reason) = hold::blocked_status_for_hold(vault);
    plan_watch_edits(WatchPlanInput {
        vault,
        timezone,
        reply_wait_days,
        today,
        blocked_ok,
        blocked_reason,
        running,
        watchers,
        prepared,
        journal,
        filing_tasks: &BTreeMap::new(),
        pending,
    })
}

#[derive(Debug)]
struct LiveTask {
    path: String,
    line: usize,
    block_id: Option<String>,
}

struct FileBuffer {
    target: PathBuf,
    original: Option<String>,
    updated: String,
}

enum HoldApply {
    Applied,
    Noop,
    Refused { reason: String },
    Missing,
}

enum LiftApply {
    Applied { unblocked: bool, lifted: bool },
    Noop { lifted: bool },
    Missing,
}

fn apply_hold_to_buffer(
    vault: &Path,
    buffers: &mut BTreeMap<PathBuf, FileBuffer>,
    live: &LiveTask,
    req: &hold::HoldRequest,
) -> HoldApply {
    let relative = PathBuf::from(&live.path);
    let target = vault.join(&relative);
    let buffer = buffers.entry(relative.clone()).or_insert_with(|| {
        let original = fs::read_to_string(&target).ok();
        let updated = original.clone().unwrap_or_default();
        FileBuffer {
            target: target.clone(),
            original,
            updated,
        }
    });
    let Some(line_text) = read_line(&buffer.updated, live.line) else {
        return HoldApply::Missing;
    };
    if !line_matches_task(&line_text, live) {
        if relocate_by_block(&buffer.updated, live).is_none() {
            return HoldApply::Missing;
        }
        // Relocation found a unique block-id row elsewhere; defer rather
        // than writing at the stale line number.
        return HoldApply::Missing;
    }
    let outcome = hold::apply_hold(&line_text, req);
    if let Some(reason) = outcome.refused {
        return HoldApply::Refused { reason };
    }
    if !outcome.changed {
        return HoldApply::Noop;
    }
    replace_line(buffer, live.line, &outcome.line);
    HoldApply::Applied
}

fn apply_lift_to_buffer(
    vault: &Path,
    buffers: &mut BTreeMap<PathBuf, FileBuffer>,
    live: &LiveTask,
    today: NaiveDate,
) -> LiftApply {
    let relative = PathBuf::from(&live.path);
    let target = vault.join(&relative);
    let buffer = buffers.entry(relative.clone()).or_insert_with(|| {
        let original = fs::read_to_string(&target).ok();
        let updated = original.clone().unwrap_or_default();
        FileBuffer {
            target: target.clone(),
            original,
            updated,
        }
    });
    let Some(line_text) = read_line(&buffer.updated, live.line) else {
        return LiftApply::Missing;
    };
    if !line_matches_task(&line_text, live) {
        if relocate_by_block(&buffer.updated, live).is_none() {
            return LiftApply::Missing;
        }
        return LiftApply::Missing;
    }
    let scope =
        hold::unblock_scope_for(&PathBuf::from(&live.path), &line_text, false);
    let outcome = hold::apply_lift(
        &line_text,
        &hold::LiftRequest {
            today,
            scope: &scope,
        },
    );
    if !outcome.changed {
        return LiftApply::Noop {
            lifted: outcome.lifted,
        };
    }
    replace_line(buffer, live.line, &outcome.line);
    LiftApply::Applied {
        unblocked: outcome.unblocked_written,
        lifted: outcome.lifted,
    }
}

fn replace_line(buffer: &mut FileBuffer, line: usize, next: &str) {
    let mut lines: Vec<String> =
        buffer.updated.lines().map(str::to_string).collect();
    if line == 0 || line > lines.len() {
        return;
    }
    // Preserve a trailing carriage return through the hold/lift writers,
    // which already carry it; only the newline style is normalized here.
    lines[line - 1] = next.to_string();
    let trailing_newline = buffer.updated.ends_with('\n');
    let mut rebuilt = lines.join("\n");
    if trailing_newline {
        rebuilt.push('\n');
    }
    buffer.updated = rebuilt;
}

fn read_line(contents: &str, line: usize) -> Option<String> {
    if line == 0 {
        return None;
    }
    contents.lines().nth(line - 1).map(str::to_string)
}

fn line_matches_task(line: &str, live: &LiveTask) -> bool {
    if let Some(block_id) = &live.block_id {
        if !block_id.is_empty() && line.contains(block_id.as_str()) {
            return true;
        }
        return line.contains("#task");
    }
    line.trim_start().starts_with('-') && line.contains("#task")
}

fn relocate_by_block(contents: &str, live: &LiveTask) -> Option<String> {
    let block_id = live.block_id.as_deref()?;
    if block_id.is_empty() {
        return None;
    }
    let mut matches = Vec::new();
    for line in contents.lines() {
        if line.contains(block_id) && line.contains("#task") {
            matches.push(line.to_string());
        }
    }
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn resolve_live_task(
    vault: &Path,
    task: &WatchingTask,
    buffers: &mut BTreeMap<PathBuf, FileBuffer>,
) -> Option<LiveTask> {
    let relative = PathBuf::from(&task.path);
    let target = vault.join(&relative);
    let buffer = buffers.entry(relative.clone()).or_insert_with(|| {
        let original = fs::read_to_string(&target).ok();
        let updated = original.clone().unwrap_or_default();
        FileBuffer {
            target: target.clone(),
            original,
            updated,
        }
    });
    let contents = buffer.updated.clone();
    let line_text = read_line(&contents, task.line)?;
    let status = checkbox_status(&line_text)?;
    if matches!(status, 'x' | 'X' | '-') {
        return None;
    }
    if !line_text.contains("#task") {
        let _ = relocate_by_block(
            &contents,
            &LiveTask {
                path: task.path.clone(),
                line: task.line,
                block_id: task.block_id.clone(),
            },
        )?;
        // Relocation found the row elsewhere; defer rather than writing at
        // the stale line number. The next run replans from the fresh scan.
        return None;
    }
    Some(LiveTask {
        path: task.path.clone(),
        line: task.line,
        block_id: task.block_id.clone(),
    })
}

fn checkbox_status(line: &str) -> Option<char> {
    let trimmed = line.trim_start();
    let after_dash = trimmed.strip_prefix('-')?;
    let after_dash = after_dash.trim_start();
    let inner = after_dash.strip_prefix('[')?.chars().next()?;
    Some(inner)
}

fn projected_tasks(
    watchers: &WatchScan,
    filing_tasks: &BTreeMap<PathBuf, FilingTaskSnapshot>,
) -> Vec<WatchingTask> {
    let mut tasks: Vec<WatchingTask> = watchers.tasks().to_vec();
    for snapshot in filing_tasks.values() {
        tasks.extend(snapshot.tasks.iter().cloned());
    }
    tasks.sort_by(|left, right| {
        left.path.cmp(&right.path).then(left.line.cmp(&right.line))
    });
    tasks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::sync::scan::{LinkPlacement, WatchTarget};
    use chrono::{TimeZone, Utc};

    fn key(id: &str) -> ThreadKey {
        ThreadKey::new("me@example.com", id).unwrap()
    }

    fn task(path: &str, line: usize, keys: &[ThreadKey]) -> WatchingTask {
        WatchingTask {
            path: path.to_string(),
            line,
            text: format!("- [ ] #task [[mail|M]] {line}"),
            block_id: None,
            status: ' ',
            targets: keys
                .iter()
                .map(|key| WatchTarget {
                    key: key.clone(),
                    note: format!("mail/{}.md", key.thread_id()),
                    placement: LinkPlacement::TaskLine,
                })
                .collect(),
        }
    }

    fn folded_hold(
        send: chrono::DateTime<Utc>,
    ) -> crate::native::gmail::turn::ThreadTurn {
        crate::native::gmail::turn::ThreadTurn {
            net: Net::Hold,
            turn: Some(Turn::Theirs),
            turn_since: Some(send),
            newest_counted_id: Some("aa02".to_string()),
            newest_internal_date: Some(20),
            last_send_at: Some(send),
        }
    }

    fn folded_lift() -> crate::native::gmail::turn::ThreadTurn {
        crate::native::gmail::turn::ThreadTurn {
            net: Net::Lift,
            turn: Some(Turn::Yours),
            turn_since: Some(Utc.timestamp_millis_opt(30).unwrap()),
            newest_counted_id: Some("aa03".to_string()),
            newest_internal_date: Some(30),
            last_send_at: None,
        }
    }

    fn prepared(
        key: ThreadKey,
        folded: crate::native::gmail::turn::ThreadTurn,
    ) -> PreparedWatch {
        PreparedWatch {
            observation: super::super::watch::ThreadObservation { key, folded },
            note_path: None,
            title: None,
            who: None,
            refresh: None,
            turn_update: None,
            report_gone: false,
            refresh_failure: None,
        }
    }

    fn write_task(vault: &std::path::Path, path: &str, line: &str) {
        let target = vault.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&target, format!("{line}\n")).unwrap();
    }

    #[test]
    fn empty_preparation_plans_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let scan = WatchScan::from_tasks_for_test(Vec::new());
        let journal = WatchJournal::load(dir.path()).unwrap();
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running: hold::RunningLookup::NoSession,
            watchers: &scan,
            prepared: &[],
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert!(plan.edits.is_empty());
        assert!(plan.receipts.turns.is_empty());
        assert!(plan.pending.is_empty());
    }

    #[test]
    fn outgoing_send_holds_task_and_advances_marker_in_one_plan() {
        let dir = tempfile::tempdir().unwrap();
        let thread = key("aa01");
        write_task(
            dir.path(),
            "job.md",
            "- [ ] #task [[mail/aa01|M]] [created:: 2026-11-07]",
        );
        let scan = WatchScan::from_tasks_for_test(vec![task(
            "job.md",
            1,
            &[thread.clone()],
        )]);
        let journal = WatchJournal::load(dir.path()).unwrap();
        let send = Utc.timestamp_millis_opt(10).unwrap();
        let watches = vec![prepared(thread.clone(), folded_hold(send))];
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running: hold::RunningLookup::NoSession,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert_eq!(plan.edits.len(), 1, "one task file changes");
        let edit = &plan.edits[0];
        assert!(edit.updated.contains("#awaiting_reply"), "{}", edit.updated);
        assert!(edit.updated.contains("[scheduled::"), "{}", edit.updated);
        assert_eq!(plan.receipts.turns.len(), 1, "marker advances");
        assert_eq!(plan.receipts.turns[0].key, thread);
        assert!(plan.pending.is_empty(), "no deferral");
        assert_eq!(plan.outcomes.len(), 1);
        assert_eq!(plan.outcomes[0].action, ACTION_HELD);
    }

    #[test]
    fn running_hold_defers_without_losing_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let thread = key("aa01");
        write_task(
            dir.path(),
            "job.md",
            "- [ ] #task [[mail/aa01|M]] [created:: 2026-11-07] ^task1",
        );
        let mut watching = task("job.md", 1, &[thread.clone()]);
        watching.block_id = Some("task1".to_string());
        let scan = WatchScan::from_tasks_for_test(vec![watching]);
        let journal = WatchJournal::load(dir.path()).unwrap();
        let send = Utc.timestamp_millis_opt(10).unwrap();
        let watches = vec![prepared(thread.clone(), folded_hold(send))];
        let running = hold::RunningLookup::Session(
            [("job.md".to_string(), "task1".to_string())]
                .into_iter()
                .collect(),
        );
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert!(plan.edits.is_empty(), "running hold writes nothing");
        assert!(plan.receipts.turns.is_empty(), "marker must not advance");
        assert_eq!(plan.pending.len(), 1, "turn retained without losing it");
        assert_eq!(plan.outcomes[0].action, ACTION_DEFERRED_RUNNING);
    }

    #[test]
    fn closed_task_writes_nothing_but_consumes_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let thread = key("aa01");
        write_task(
            dir.path(),
            "job.md",
            "- [x] #task [[mail/aa01|M]] [created:: 2026-11-07]",
        );
        let scan = WatchScan::from_tasks_for_test(vec![task(
            "job.md",
            1,
            &[thread.clone()],
        )]);
        let journal = WatchJournal::load(dir.path()).unwrap();
        let send = Utc.timestamp_millis_opt(10).unwrap();
        let watches = vec![prepared(thread.clone(), folded_hold(send))];
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running: hold::RunningLookup::NoSession,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert!(plan.edits.is_empty(), "closed rows are untouched");
        assert_eq!(plan.receipts.turns.len(), 1, "closed still consumes");
    }

    #[test]
    fn already_held_row_is_a_byte_identical_noop_that_still_advances() {
        let dir = tempfile::tempdir().unwrap();
        let thread = key("aa01");
        // Nudge for the send below: send 1970-01-01 + 3 days.
        let line = "- [?] #task #awaiting_reply [[mail/aa01|M]] [created:: 2026-11-07] [scheduled:: 1970-01-04]";
        write_task(dir.path(), "job.md", line);
        let scan = WatchScan::from_tasks_for_test(vec![task(
            "job.md",
            1,
            &[thread.clone()],
        )]);
        let journal = WatchJournal::load(dir.path()).unwrap();
        let send = Utc.timestamp_millis_opt(10).unwrap();
        let watches = vec![prepared(thread.clone(), folded_hold(send))];
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running: hold::RunningLookup::NoSession,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert!(plan.edits.is_empty(), "correct hold must not churn");
        assert_eq!(plan.receipts.turns.len(), 1, "noop still consumes");
    }

    #[test]
    fn reply_lifts_and_resurfaces_unheld_ready_rows() {
        let dir = tempfile::tempdir().unwrap();
        let held_thread = key("aa01");
        let fresh_thread = key("bb02");
        write_task(
            dir.path(),
            "job.md",
            "- [?] #task #awaiting_reply [[m|M]] [fresh:: 2026-11-05] [created:: 2026-11-07] [scheduled:: 2026-11-10]",
        );
        write_task(
            dir.path(),
            "body.md",
            "- [ ] #task [[m|M]] [fresh:: 2026-11-05]",
        );
        let scan = WatchScan::from_tasks_for_test(vec![
            task("job.md", 1, &[held_thread.clone()]),
            task("body.md", 1, &[fresh_thread.clone()]),
        ]);
        let journal = WatchJournal::load(dir.path()).unwrap();
        let watches = vec![
            prepared(held_thread.clone(), folded_lift()),
            prepared(fresh_thread.clone(), folded_lift()),
        ];
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 8).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running: hold::RunningLookup::NoSession,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert_eq!(plan.edits.len(), 2, "both rows change once");
        assert_eq!(plan.receipts.turns.len(), 2);
        let actions: std::collections::BTreeSet<&str> =
            plan.outcomes.iter().map(|outcome| outcome.action).collect();
        assert!(actions.contains(ACTION_LIFTED), "{actions:?}");
        assert!(actions.contains(ACTION_RESURFACED), "{actions:?}");
    }

    #[test]
    fn two_tasks_on_one_thread_only_running_defers_and_catches_up() {
        let dir = tempfile::tempdir().unwrap();
        let thread = key("aa01");
        write_task(
            dir.path(),
            "job.md",
            "- [ ] #task [[mail/aa01|M]] [created:: 2026-11-07] ^idle\n- [ ] #task [[mail/aa01|M]] [created:: 2026-11-07] ^busy\n",
        );
        // Scan sees two rows on lines 1 and 2 watching the same thread.
        let idle = WatchingTask {
            path: "job.md".to_string(),
            line: 1,
            text: "- [ ] #task [[mail/aa01|M]] [created:: 2026-11-07] ^idle"
                .to_string(),
            block_id: Some("idle".to_string()),
            status: ' ',
            targets: vec![WatchTarget {
                key: thread.clone(),
                note: "mail/aa01.md".to_string(),
                placement: LinkPlacement::TaskLine,
            }],
        };
        let busy = WatchingTask {
            path: "job.md".to_string(),
            line: 2,
            text: "- [ ] #task [[mail/aa01|M]] [created:: 2026-11-07] ^busy"
                .to_string(),
            block_id: Some("busy".to_string()),
            status: ' ',
            targets: vec![WatchTarget {
                key: thread.clone(),
                note: "mail/aa01.md".to_string(),
                placement: LinkPlacement::TaskLine,
            }],
        };
        let scan = WatchScan::from_tasks_for_test(vec![idle, busy]);
        let journal = WatchJournal::load(dir.path()).unwrap();
        let send = Utc.timestamp_millis_opt(10).unwrap();
        let watches = vec![prepared(thread.clone(), folded_hold(send))];
        // Only ^busy runs: the eligible task changes, the running one
        // defers, and the marker must not advance.
        let running = hold::RunningLookup::Session(
            [("job.md".to_string(), "busy".to_string())]
                .into_iter()
                .collect(),
        );
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert_eq!(plan.edits.len(), 1, "only idle changes: {plan:?}");
        assert!(plan.receipts.turns.is_empty(), "deferral keeps the turn");
        assert_eq!(plan.pending.len(), 1);
        // After the session closes, the same turn plans both rows in one
        // coalesced file write and advances the marker.
        let idle_only = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running: hold::RunningLookup::NoSession,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &[],
        });
        assert_eq!(idle_only.edits.len(), 1, "one file coalesces both rows");
        assert_eq!(idle_only.receipts.turns.len(), 1, "marker advances");
        assert!(idle_only.pending.is_empty());
        let updated = &idle_only.edits[0].updated;
        assert_eq!(
            updated.matches("#awaiting_reply").count(),
            2,
            "both rows held: {updated}"
        );
    }

    #[test]
    fn newer_turn_supersedes_obsolete_pending_hold() {
        let dir = tempfile::tempdir().unwrap();
        let thread = key("aa01");
        write_task(
            dir.path(),
            "job.md",
            "- [ ] #task [[mail/aa01|M]] [created:: 2026-11-07]",
        );
        let scan = WatchScan::from_tasks_for_test(vec![task(
            "job.md",
            1,
            &[thread.clone()],
        )]);
        let journal = WatchJournal::load(dir.path()).unwrap();
        let pending = vec![RecoveryWatch {
            account: "me@example.com".to_string(),
            thread_id: "aa01".to_string(),
            message_id: "aa01".to_string(),
            internal_date: 10,
            turn: "theirs".to_string(),
            effect: "hold".to_string(),
            nudge_on: None,
            unblocked_on: None,
            pending_tasks: true,
            pending_marker: true,
        }];
        // Fresh turn is newer (aa02 @ 20): the obsolete aa01 pending drops.
        let send = Utc.timestamp_millis_opt(10).unwrap();
        let watches = vec![prepared(thread.clone(), folded_hold(send))];
        let plan = plan_watch_edits(WatchPlanInput {
            vault: dir.path(),
            timezone: chrono_tz::UTC,
            reply_wait_days: 3,
            today: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            blocked_ok: true,
            blocked_reason: None,
            running: hold::RunningLookup::NoSession,
            watchers: &scan,
            prepared: &watches,
            journal: &journal,
            filing_tasks: &BTreeMap::new(),
            pending: &pending,
        });
        // Fresh aa02 is still a hold for the open task: one edit, one
        // receipt, and the stale aa01 pending is replaced, not duplicated.
        assert_eq!(plan.edits.len(), 1);
        assert_eq!(plan.pending.len(), 0, "{:?}", plan.pending);
    }
}

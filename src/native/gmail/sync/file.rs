//! Routed mail-note and task filing through one vault transaction.
//!
//! Remote work is complete before [`apply`] is called. The live path then
//! holds the maintenance lock while it refreshes enrollment, the mail index,
//! destinations, and open-task membership; plans all notes and task edits;
//! writes the coalesced batch; commits only owned paths; and appends the
//! enrollment journal last. Dry-run uses the same planner without a guard,
//! lock, directory creation, or journal write.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use chrono::{DateTime, Utc};

use super::{
    enroll::{
        self, AdoptionIntent, CandidateDecision, EnrollmentEvent,
        EnrollmentFold, EnrollmentOrigin, FiledVia, Membership,
    },
    hold,
    retry::{Clock, SharedBudget, Sleeper},
    scan::{self, LinkPlacement, WatchScan, WatchTarget, WatchingTask},
    watch::{PreparedWatch, WatchFailure},
    watch_apply::{
        self, FilingTaskSnapshot, RecoveryWatch, WatchGoneOutcome,
        WatchThreadOutcome,
    },
    watch_state::WatchJournal,
    ThreadKey,
};
use crate::native::{
    attachments::journal as attachment_journal,
    capture::{self, StagedTextFile},
    env as bob_env,
    gmail::{
        note::index::MailIndex,
        resolve,
        route::{RouteContext, RouteProblem, RouteVerdict},
        save::{self, PreparedSave, SaveEventFacts, StagedSaveNote},
        GmailError,
    },
    ob, pref_journal, vault_sync,
};

/// Body-free transaction ownership evidence for commit and journal recovery.
///
/// Written when a scoped commit or a journal append fails, before any
/// enrollment is recorded. The next live run finishes the original scoped
/// commit (owned mail notes, task files, images, and canonical hubs only)
/// and its deferred save, origin, and enrollment events before recording
/// new enrollment, without requiring the failed transaction's SHA, a push,
/// or the enrollment label. Unrelated staged dirt, custom hubs, and the
/// vault AGENTS warning are never blanket commit targets.
///
/// The evidence lives in host-local state beside the guard
/// (`$XDG_STATE_HOME/bob/gmail/sync-recovery-<digest>.json`), never in the
/// vault: it describes this host's uncommitted working tree.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct RecoveryData {
    threads: Vec<RecoveryThread>,
    commit_paths: Vec<StoredCommitPath>,
    /// Body-free pending watch turns. Older evidence without this section
    /// loads with an empty list.
    #[serde(default)]
    watch: Vec<RecoveryWatch>,
}

/// One owned path as sync wrote it, with the SHA-256 of its bytes. Replay
/// commits the path only while its bytes still match; a path whose bytes
/// changed (a hub customized after the failure, a user's later edit) is
/// left out of the commit with a body-free warning, dropped from the
/// evidence, and still finishes its thread's events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredCommitPath {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RecoveryThread {
    account: String,
    thread_id: String,
    note: String,
    task_file: String,
    via: String,
    route: Option<String>,
    created: bool,
    added: usize,
    origins: Vec<(String, String, String)>,
    adopted: bool,
    problem_label: Option<String>,
    problem_kind: Option<String>,
    problem_message: Option<String>,
    problem_hint: Option<String>,
    first_seen: bool,
    /// The `gmail.save` event still needs appending. Filed threads only:
    /// adoptions never produce save receipts.
    #[serde(default)]
    pending_save: bool,
    /// Attachment-origin events still needing an append.
    #[serde(default)]
    pending_origins: Vec<(String, String, String)>,
    /// The enrollment event (`gmail.filed` or `gmail.adopted`) still needs
    /// appending, after the save and origin stages are done.
    #[serde(default)]
    pending_enrollment: bool,
}

/// Dispatch peek for host-local recovery evidence. Distinguishes a missing
/// or empty file from an unreadable path so idle `-l 0` still enters apply
/// when the evidence cannot be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryPresence {
    /// No evidence file, or an empty file that load treats as no work.
    Absent,
    /// A non-empty regular file that apply must replay.
    Pending,
    /// A directory, unreadable path, or other load error: apply must still
    /// run so the authoritative load can fail the run body-free.
    Unreadable,
}

/// Whether this host has unreplayed or unreadable sync-recovery evidence.
/// A read-only peek for dispatch: the authoritative load happens under the
/// guard and the vault lock inside [`apply`].
pub(crate) fn recovery_presence(
    vault: &Path,
    account: &str,
) -> RecoveryPresence {
    let path = recovery_path(vault, account);
    match fs::metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            RecoveryPresence::Absent
        }
        Err(_) => RecoveryPresence::Unreadable,
        Ok(metadata) if metadata.is_file() => {
            if metadata.len() == 0 {
                RecoveryPresence::Absent
            } else {
                RecoveryPresence::Pending
            }
        }
        Ok(_) => RecoveryPresence::Unreadable,
    }
}

#[cfg(test)]
#[derive(Default)]
struct RecoveryIo {
    /// Succeed this many writes before any injected failure.
    succeed_writes: usize,
    fail_writes: usize,
    fail_clears: usize,
}

#[cfg(test)]
thread_local! {
    static RECOVERY_IO: std::cell::RefCell<RecoveryIo> =
        const { std::cell::RefCell::new(RecoveryIo {
            succeed_writes: 0,
            fail_writes: 0,
            fail_clears: 0,
        }) };
}

fn injected_write_failure() -> Option<io::Error> {
    #[cfg(test)]
    {
        RECOVERY_IO.with(|io| {
            let mut io = io.borrow_mut();
            if io.succeed_writes > 0 {
                io.succeed_writes -= 1;
                return None;
            }
            if io.fail_writes > 0 {
                io.fail_writes -= 1;
                Some(io::Error::other("injected recovery write failure"))
            } else {
                None
            }
        })
    }
    #[cfg(not(test))]
    None
}

fn injected_clear_failure() -> Option<io::Error> {
    #[cfg(test)]
    {
        RECOVERY_IO.with(|io| {
            let mut io = io.borrow_mut();
            if io.fail_clears > 0 {
                io.fail_clears -= 1;
                Some(io::Error::other("injected recovery clear failure"))
            } else {
                None
            }
        })
    }
    #[cfg(not(test))]
    None
}

/// The host-local evidence path for `account` and `vault`.
///
/// The digest is SHA-256 over the normalized account plus the canonical
/// vault path, in the same style as the guard path, so different accounts
/// and different vault checkouts never share evidence.
fn recovery_path(vault: &Path, account: &str) -> PathBuf {
    let normalized = account.trim().to_ascii_lowercase();
    let canonical =
        fs::canonicalize(vault).unwrap_or_else(|_| vault.to_path_buf());
    let mut keyed = normalized.into_bytes();
    keyed.push(0);
    keyed.extend_from_slice(canonical.to_string_lossy().as_bytes());
    let digest = save::sha256(&keyed);
    bob_env::state_home().join(format!("bob/gmail/sync-recovery-{digest}.json"))
}

/// Load the evidence. A missing file is empty evidence; an unreadable or
/// corrupt file, or any other load failure, is a body-free error the caller
/// turns into a warning plus failure state — never a silent ignore.
fn load_recovery(vault: &Path, account: &str) -> Result<RecoveryData, String> {
    let path = recovery_path(vault, account);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RecoveryData::default())
        }
        Err(error) => {
            return Err(format!("could not read {}: {error}", path.display()))
        }
    };
    if bytes.is_empty() {
        return Ok(RecoveryData::default());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))
}

static RECOVERY_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Write the evidence atomically (temp file plus rename in the same
/// directory). Callers turn a failure into a body-free warning plus
/// failure state.
fn write_recovery(
    vault: &Path,
    account: &str,
    data: &RecoveryData,
) -> io::Result<()> {
    if let Some(error) = injected_write_failure() {
        return Err(error);
    }
    let path = recovery_path(vault, account);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(data)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(
        ".sync-recovery-{}-{}.tmp",
        std::process::id(),
        RECOVERY_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::write(&temp, &bytes).and_then(|()| {
        fs::rename(&temp, &path).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

/// Delete the evidence file. A missing file is fine; any other failure is
/// returned so the caller can warn instead of silently ignoring it.
fn clear_recovery(vault: &Path, account: &str) -> io::Result<()> {
    if let Some(error) = injected_clear_failure() {
        return Err(error);
    }
    match fs::remove_file(recovery_path(vault, account)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Persist the evidence, or delete the file when nothing remains pending.
/// Every error becomes a body-free warning plus failure state on the
/// outcome — never a silent ignore. Returns whether this persist succeeded
/// so a checkpoint failure can stop later journal stages.
fn persist_recovery(
    outcome: &mut FilingOutcome,
    vault: &Path,
    account: &str,
    data: &RecoveryData,
) -> bool {
    let empty = data.threads.is_empty() && data.commit_paths.is_empty();
    let result = if empty {
        clear_recovery(vault, account)
    } else {
        write_recovery(vault, account, data)
    };
    if let Err(error) = result {
        outcome.failed = true;
        outcome.warnings.push(format!(
            "could not persist Gmail sync recovery evidence: {error}"
        ));
        false
    } else {
        true
    }
}

/// Whether a `gmail.save` event for this account/thread/note is already in
/// the vault journal. Recovery replay uses this to skip a save that landed
/// before its checkpoint failed; independent current-run events are never
/// globally deduplicated.
fn save_event_present(
    vault: &Path,
    account: &str,
    thread_id: &str,
    note: &str,
) -> bool {
    let Ok((events, _)) = pref_journal::read_raw(vault, save::GMAIL_JOURNAL)
    else {
        return false;
    };
    events.iter().any(|event| {
        event.kind == "gmail.save"
            && event
                .value
                .get("value")
                .map(|value| {
                    value.get("account").and_then(serde_json::Value::as_str)
                        == Some(account)
                        && value
                            .get("thread_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(thread_id)
                        && value.get("note").and_then(serde_json::Value::as_str)
                            == Some(note)
                })
                .unwrap_or(false)
    })
}

/// Whether an attachment-origin event for this saved path and hash already
/// exists with `via=gmail`. Recovery replay skips that origin; independent
/// imports are never globally deduplicated.
fn origin_event_present(vault: &Path, saved: &str, sha256: &str) -> bool {
    let Ok((events, _)) = attachment_journal::read_journal(vault) else {
        return false;
    };
    events.iter().any(|event| {
        event.saved == saved && event.sha256 == sha256 && event.via == "gmail"
    })
}

/// Re-scan open watchers after a live persist so recovered closed or
/// deleted tasks drop out of membership and newly filed tasks appear.
fn refresh_watch_membership(
    vault: &Path,
    mail_folder: &Path,
    link_template: &str,
    outcome: &mut FilingOutcome,
) {
    let Ok(index) = resolve::scan_index(vault, mail_folder, link_template)
    else {
        return;
    };
    let Ok(watchers) = scan::scan_watching_tasks(vault, mail_folder, &index)
    else {
        return;
    };
    outcome.watch_snapshot = watchers;
    for result in &mut outcome.results {
        if result.watch_member
            && matches!(result.status, "filed" | "would_file" | "adopted")
            && !outcome.watch_snapshot.is_watched(&result.key)
        {
            result.watch_member = false;
        }
    }
}

/// A fully prepared thread and the route resolved from its fetched labels.
/// No Gmail request is performed by this module.
pub(crate) struct FilingCandidate {
    pub(crate) save: PreparedSave,
    pub(crate) route: RouteVerdict,
}

/// Inputs from discovery and the read-only preflight scan. The live apply
/// rereads these snapshots under the vault lock; dry-run consumes them as-is.
pub(crate) struct FilingInput<'a> {
    pub(crate) vault: &'a Path,
    /// Gmail account being synced. Keys the host-local recovery evidence
    /// beside the per-account guard.
    pub(crate) account: &'a str,
    pub(crate) mail_folder: &'a Path,
    pub(crate) link_template: &'a str,
    pub(crate) inbox: &'a str,
    pub(crate) candidates: Vec<FilingCandidate>,
    pub(crate) adoptions: &'a [AdoptionIntent],
    pub(crate) enrollment: &'a EnrollmentFold,
    pub(crate) watchers: &'a WatchScan,
    pub(crate) dry_run: bool,
    pub(crate) saved_at: DateTime<Utc>,
    pub(crate) timezone: chrono_tz::Tz,
    pub(crate) budget: &'a SharedBudget,
    /// Prepared watched threads from the read-only preparation phase. The
    /// transaction owns their task/note writes, marker advancement, and
    /// recovery; polling wires this field next phase.
    pub(crate) watches: Vec<PreparedWatch>,
    /// Isolated preparation failures that must not discard independent
    /// successes. Reported as failures with body-free warnings.
    pub(crate) watch_failures: Vec<WatchFailure>,
    /// Threads whose metadata reads deferred when the retry budget gave up.
    pub(crate) watch_deferred: Vec<ThreadKey>,
    /// Calendar-day wait for hold nudges. Read from Gmail config by the
    /// command; tests pass the default.
    pub(crate) reply_wait_days: u32,
}

/// One thread's result for the command report. Paths are vault-relative and
/// the `link` contains the inert mail-note wikilink returned by save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilingResult {
    pub(crate) key: ThreadKey,
    pub(crate) note: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) task_file: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) route: Option<String>,
    pub(crate) via: Option<FiledVia>,
    pub(crate) problem: Option<RouteProblem>,
    pub(crate) first_seen: bool,
    /// Whether this row belongs in the watching/awaiting counts. New
    /// filings and adoptions start true; recovered closed or deleted
    /// tasks and rows whose save/origin/enrollment stages are still
    /// pending stay false. Reports never infer membership from status.
    pub(crate) watch_member: bool,
}

/// One candidate that could not be planned independently. Gmail errors are
/// already body-free and redacted by the shared transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilingFailure {
    pub(crate) key: ThreadKey,
    pub(crate) kind: String,
    pub(crate) message: String,
}

/// Results of the combined planning and apply boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FilingOutcome {
    pub(crate) results: Vec<FilingResult>,
    pub(crate) failures: Vec<FilingFailure>,
    pub(crate) warnings: Vec<String>,
    pub(crate) deferred: Vec<ThreadKey>,
    /// A busy vault lock is a quiet success/skip for sync.
    pub(crate) busy: bool,
    /// Enrollment journal failures are recoverable by the next adoption scan
    /// but make this run fail, as the durable row is not yet enrolled.
    pub(crate) failed: bool,
    pub(crate) changed: bool,
    /// Whether this outcome came from live apply or dry-run planning.
    /// Reports use the revalidated snapshot when this is set; otherwise they
    /// keep the run's own scan so a skipped apply cannot zero membership.
    pub(crate) applied: bool,
    /// The watch scan the apply boundary revalidated under the vault lock
    /// (or the dry-run input scan). Reports must use this snapshot for live
    /// membership, not the pre-fetch scan, so a task closed during
    /// preparation leaves the summary.
    pub(crate) watch_snapshot: WatchScan,
    /// Typed per-thread watch outcomes. The polling phase renders these
    /// into `watch[]` rows; the transaction records them here.
    pub(crate) watch_threads: Vec<WatchThreadOutcome>,
    /// First-time gone reports; they are warning/human rows, not `watch[]`
    /// actions because the public watch action set has no `gone` value.
    pub(crate) watch_gone: Vec<WatchGoneOutcome>,
}

#[derive(Debug, Clone)]
struct Destination {
    relative: PathBuf,
    via: FiledVia,
    route: Option<String>,
    problem: Option<RouteProblem>,
    verdict: RouteVerdict,
}

struct DestinationSnapshot {
    target: PathBuf,
    original: Option<String>,
    updated: String,
}

struct AttemptPlan {
    outcome: FilingOutcome,
    works: BTreeMap<(String, String), PreparedSave>,
    destinations: BTreeMap<ThreadKey, Destination>,
    plans: Vec<StagedSaveNote>,
    staged: Vec<StagedTextFile>,
    task_commit_paths: Vec<PathBuf>,
    auxiliary_commit_paths: Vec<PathBuf>,
    enrollment_events: Vec<EnrollmentEvent>,
    watch_turns: Vec<crate::native::gmail::sync::watch_state::AppliedTurn>,
    watch_gone: Vec<ThreadKey>,
    watch_pending: Vec<RecoveryWatch>,
}

impl AttemptPlan {
    fn into_retry_candidates(mut self) -> Vec<FilingCandidate> {
        self.works
            .into_iter()
            .filter_map(|((account, thread_id), save)| {
                let key = ThreadKey::new(&account, &thread_id)?;
                let destination = self.destinations.remove(&key)?;
                Some(FilingCandidate {
                    save,
                    route: destination.verdict,
                })
            })
            .collect()
    }
}

/// Recheck local snapshots, plan every successful candidate together, and
/// persist the result as one scoped transaction. All callers must complete
/// Gmail fetching and body preparation before entering this function.
pub(crate) fn apply<S: Sleeper, C: Clock>(
    input: FilingInput<'_>,
    sleeper: &S,
    clock: &C,
) -> Result<FilingOutcome, GmailError> {
    if input.dry_run {
        let index = resolve::scan_index(
            input.vault,
            input.mail_folder,
            input.link_template,
        )?;
        // Dry-run reads the evidence to project recovery rows but never
        // writes or deletes it. A load failure still fails the run.
        let mut outcome = FilingOutcome::default();
        let recovery = match load_recovery(input.vault, input.account) {
            Ok(recovery) => recovery,
            Err(message) => {
                outcome.failed = true;
                outcome.warnings.push(format!(
                    "Gmail sync recovery evidence is unusable: {message}"
                ));
                RecoveryData::default()
            }
        };
        let in_flight = in_flight_keys(&recovery);
        let watch_journal = WatchJournal::load(input.vault).unwrap_or_default();
        let mut plan = build_attempt(
            input.vault,
            input.mail_folder,
            input.link_template,
            input.inbox,
            input.candidates,
            input.adoptions,
            input.enrollment,
            input.watchers,
            &index,
            true,
            false,
            input.saved_at,
            input.timezone,
            &in_flight,
            input.watches,
            input.watch_failures,
            input.watch_deferred,
            input.reply_wait_days,
            &watch_journal,
            &recovery.watch,
        )?;
        let mut invalid = 0usize;
        for thread in &recovery.threads {
            let Some(key) = ThreadKey::new(&thread.account, &thread.thread_id)
            else {
                invalid += 1;
                continue;
            };
            if plan.outcome.results.iter().any(|result| result.key == key) {
                continue;
            }
            plan.outcome.results.push(recovery_row(
                thread,
                &key,
                &index,
                input.watchers,
                true,
            ));
        }
        if invalid > 0 {
            plan.outcome.failed = true;
            plan.outcome.warnings.push(format!(
                "skipped {invalid} stored Gmail sync recovery thread(s) with an invalid identity"
            ));
        }
        plan.outcome.failed |= outcome.failed;
        plan.outcome.warnings.extend(outcome.warnings);
        plan.outcome
            .results
            .sort_by(|left, right| left.key.cmp(&right.key));
        return Ok(plan.outcome);
    }

    let Some(_lock) = acquire_vault_lock(input.budget, sleeper, clock)? else {
        return Ok(FilingOutcome {
            busy: true,
            ..FilingOutcome::default()
        });
    };

    // The authoritative evidence load happens here, under the guard and the
    // vault lock. A load failure fails the run but still files the current
    // work; replay simply has nothing to finish this time.
    let mut pre_failed = false;
    let mut pre_warnings = Vec::new();
    let recovery = match load_recovery(input.vault, input.account) {
        Ok(recovery) => recovery,
        Err(message) => {
            pre_failed = true;
            pre_warnings.push(format!(
                "Gmail sync recovery evidence is unusable: {message}"
            ));
            RecoveryData::default()
        }
    };
    let in_flight = in_flight_keys(&recovery);

    let mut candidates = input.candidates;
    let mut watches = input.watches;
    let mut watch_failures = input.watch_failures;
    let mut watch_deferred = input.watch_deferred;
    let reply_wait_days = input.reply_wait_days;
    // Watch keys for deferral when a stale-line retry drops watch work.
    let watch_keys: Vec<ThreadKey> = watches
        .iter()
        .map(|watch| watch.observation.key.clone())
        .chain(watch_failures.iter().map(|failure| failure.key.clone()))
        .chain(watch_deferred.iter().cloned())
        .collect();
    for attempt in 0..2 {
        let enrollment = EnrollmentFold::load(input.vault).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not reread the Gmail enrollment journal: {error}"),
            )
        })?;
        let index = resolve::scan_index(
            input.vault,
            input.mail_folder,
            input.link_template,
        )?;
        let watchers =
            scan::scan_watching_tasks(input.vault, input.mail_folder, &index)?;
        let watch_journal = WatchJournal::load(input.vault).unwrap_or_default();
        let child_env = ob::child_env();
        let is_worktree = ob::detect_git_worktree(input.vault, &child_env)
            .map_err(|error| {
                GmailError::runtime(
                    "git",
                    format!("could not determine whether the vault is a Git worktree: {error}"),
                )
            })?;
        // First attempt plans watch work; a stale-line retry drops it and
        // defers every watched thread so the next run replans from fresh
        // snapshots instead of writing at a stale line number.
        let attempt_watches = if attempt == 0 {
            std::mem::take(&mut watches)
        } else {
            Vec::new()
        };
        let attempt_failures = if attempt == 0 {
            std::mem::take(&mut watch_failures)
        } else {
            Vec::new()
        };
        let attempt_deferred = if attempt == 0 {
            std::mem::take(&mut watch_deferred)
        } else {
            watch_keys.clone()
        };

        let plan = build_attempt(
            input.vault,
            input.mail_folder,
            input.link_template,
            input.inbox,
            std::mem::take(&mut candidates),
            input.adoptions,
            &enrollment,
            &watchers,
            &index,
            false,
            is_worktree,
            input.saved_at,
            input.timezone,
            &in_flight,
            attempt_watches,
            attempt_failures,
            attempt_deferred,
            reply_wait_days,
            &watch_journal,
            &recovery.watch,
        )?;
        match persist_attempt(
            input.vault,
            input.account,
            &child_env,
            is_worktree,
            plan,
            &index,
            &recovery,
        ) {
            Ok(mut outcome) => {
                outcome.failed |= pre_failed;
                outcome.warnings.extend(pre_warnings);
                refresh_watch_membership(
                    input.vault,
                    input.mail_folder,
                    input.link_template,
                    &mut outcome,
                );
                return Ok(outcome);
            }
            Err((error, plan))
                if error.kind() == "note_changed" && attempt == 0 =>
            {
                candidates = plan.into_retry_candidates();
            }
            Err((error, plan)) if error.kind() == "note_changed" => {
                let mut outcome = plan.outcome;
                for result in &mut outcome.results {
                    if matches!(result.status, "filed" | "adopted") {
                        outcome.deferred.push(result.key.clone());
                    }
                }
                outcome
                    .results
                    .retain(|result| !outcome.deferred.contains(&result.key));
                outcome.failed = true;
                outcome.changed = false;
                outcome.warnings.push(
                    "vault files changed again during filing; remaining threads were deferred".to_string(),
                );
                let _ = error;
                outcome.failed |= pre_failed;
                outcome.warnings.extend(pre_warnings);
                return Ok(outcome);
            }
            Err((error, _)) => {
                if pre_warnings.is_empty() {
                    return Err(error);
                }
                // The run fails either way; keep the evidence-load warning
                // visible since an Err return carries no report warnings.
                return Err(error.with_hint(&pre_warnings.join(" ")));
            }
        }
    }

    unreachable!("the filing retry loop returns after its bounded attempts")
}

fn build_attempt(
    vault: &Path,
    mail_folder: &Path,
    link_template: &str,
    inbox: &str,
    candidates: Vec<FilingCandidate>,
    initial_adoptions: &[AdoptionIntent],
    enrollment: &EnrollmentFold,
    watchers: &WatchScan,
    index: &MailIndex,
    dry_run: bool,
    is_worktree: bool,
    saved_at: DateTime<Utc>,
    timezone: chrono_tz::Tz,
    in_flight: &BTreeSet<ThreadKey>,
    mut watches: Vec<PreparedWatch>,
    watch_failures: Vec<WatchFailure>,
    watch_deferred: Vec<ThreadKey>,
    reply_wait_days: u32,
    watch_journal: &WatchJournal,
    pending_watch: &[RecoveryWatch],
) -> Result<AttemptPlan, GmailError> {
    let membership =
        Membership::new(enrollment, watchers).with_in_flight(in_flight);
    let route_context = RouteContext::load(vault);
    let initial_adoption_keys: BTreeSet<_> = initial_adoptions
        .iter()
        .map(|intent| intent.key.clone())
        .collect();
    let candidate_keys: BTreeSet<_> = candidates
        .iter()
        .filter_map(|candidate| candidate_key(candidate).ok())
        .collect();
    let mut outcome = FilingOutcome::default();
    outcome
        .warnings
        .extend(enrollment.warnings().iter().cloned());
    outcome.warnings.extend(watchers.warnings().iter().cloned());
    let mut works = BTreeMap::new();
    let mut destinations = BTreeMap::new();
    let mut enrollment_events = Vec::new();
    let mut enrolled_events = BTreeSet::new();

    for candidate in candidates {
        let key = match candidate_key(&candidate) {
            Ok(key) => key,
            Err(error) => {
                outcome.warnings.push(format!(
                    "skipped a prepared Gmail candidate with an invalid identity: {}",
                    error.message()
                ));
                continue;
            }
        };
        match membership.decide(&key) {
            CandidateDecision::Enrolled(origin) => {
                outcome
                    .results
                    .push(already_enrolled(&key, origin, enrollment, index));
            }
            // A thread with pending evidence is an in-flight filing. It is
            // never filed again; the replay appends its recorded event.
            CandidateDecision::InFlight => continue,
            CandidateDecision::Adopt(intent) => {
                record_adoption(
                    vault,
                    &intent,
                    index,
                    &mut outcome,
                    &mut enrollment_events,
                    &mut enrolled_events,
                );
            }
            CandidateDecision::File => {
                if initial_adoption_keys.contains(&key) {
                    outcome.deferred.push(key);
                    continue;
                }
                match destination_for_route(
                    vault,
                    inbox,
                    &candidate.route,
                    &route_context,
                ) {
                    Ok(destination) => {
                        destinations.insert(key.clone(), destination);
                        works.insert(key.index_key(), candidate.save);
                    }
                    Err(error) => outcome.failures.push(FilingFailure {
                        key,
                        kind: error.kind().to_string(),
                        message: error.message().to_string(),
                    }),
                }
            }
        }
    }

    // A task that was open when Gmail fetching began may have been closed or
    // deleted during that work. Never turn that stale adoption into a new
    // filing. Recheck each initial adoption from the fresh scan above.
    for intent in initial_adoptions {
        if candidate_keys.contains(&intent.key) {
            continue;
        }
        match membership.decide(&intent.key) {
            CandidateDecision::Enrolled(origin) => outcome
                .results
                .push(already_enrolled(&intent.key, origin, enrollment, index)),
            // In-flight adoptions are owned by the replay, which appends
            // their recorded enrollment event.
            CandidateDecision::InFlight => continue,
            CandidateDecision::Adopt(current) => record_adoption(
                vault,
                &current,
                index,
                &mut outcome,
                &mut enrollment_events,
                &mut enrolled_events,
            ),
            CandidateDecision::File => {
                outcome.deferred.push(intent.key.clone());
            }
        }
    }

    let mut route_parent_links = BTreeMap::new();
    for (key, destination) in &destinations {
        if let Some(route) = &destination.route {
            route_parent_links.insert(key.index_key(), format!("[[{route}]]"));
        }
    }

    // Probe each candidate independently so one corrupt or ambiguous saved
    // note does not discard other valid candidates. The final batch pass below
    // restores shared filename and attachment collision accounting.
    let keys: Vec<_> = works.keys().cloned().collect();
    for key in keys {
        let Some(work) = works.remove(&key) else {
            continue;
        };
        let thread_key = ThreadKey::new(&key.0, &key.1)
            .expect("prepared save identity was checked above");
        let mut one = BTreeMap::from([(key.clone(), work)]);
        let parent = route_parent_links
            .get(&key)
            .map(|parent| BTreeMap::from([(key.clone(), parent.clone())]))
            .unwrap_or_default();
        match save::plan_prepared_saves_with_parent_links(
            vault,
            mail_folder,
            link_template,
            &mut one,
            index,
            None,
            &parent,
            false,
            saved_at,
            timezone,
        ) {
            Ok(_) => works.extend(one),
            Err(error) => {
                destinations.remove(&thread_key);
                outcome.failures.push(FilingFailure {
                    key: thread_key,
                    kind: error.kind().to_string(),
                    message: error.message().to_string(),
                });
            }
        }
    }

    let mut plans = match save::plan_prepared_saves_with_parent_links(
        vault,
        mail_folder,
        link_template,
        &mut works,
        index,
        None,
        &route_parent_links,
        false,
        saved_at,
        timezone,
    ) {
        Ok(plans) => plans,
        Err(error) => {
            for (account, thread_id) in works.keys() {
                if let Some(key) = ThreadKey::new(account, thread_id) {
                    outcome.failures.push(FilingFailure {
                        key,
                        kind: error.kind().to_string(),
                        message: error.message().to_string(),
                    });
                }
            }
            works.clear();
            destinations.clear();
            Vec::new()
        }
    };

    let mut staged = plans
        .iter()
        .filter(|plan| plan.changed)
        .map(save::staged_note)
        .collect::<Result<Vec<_>, _>>()?;
    let mut snapshots = load_destinations(vault, inbox, &destinations)?;
    let today = bob_env::current_datetime()
        .date()
        .format("%Y-%m-%d")
        .to_string();
    for plan in &plans {
        let Some(key) = ThreadKey::new(&plan.account, &plan.thread_id) else {
            continue;
        };
        let Some(destination) = destinations.get(&key) else {
            continue;
        };
        let Some(snapshot) = snapshots.get_mut(&destination.relative) else {
            continue;
        };
        let block =
            task_block(&plan.link, &today, destination.problem.as_ref());
        snapshot.updated =
            capture::insert_task_line(&snapshot.updated, &block).0;
    }
    let mut task_commit_paths = Vec::new();
    for snapshot in snapshots.values() {
        if snapshot.original.as_deref() != Some(snapshot.updated.as_str()) {
            task_commit_paths.push(
                snapshot
                    .target
                    .strip_prefix(vault)
                    .map_err(|_| {
                        GmailError::runtime(
                            "refused",
                            "a task destination escaped the vault".to_string(),
                        )
                    })?
                    .to_path_buf(),
            );
            staged.push(StagedTextFile {
                target: snapshot.target.clone(),
                target_existed: snapshot.original.is_some(),
                original_target: snapshot.original.clone().unwrap_or_default(),
                updated_target: snapshot.updated.clone(),
            });
        }
    }

    for plan in &plans {
        let Some(key) = ThreadKey::new(&plan.account, &plan.thread_id) else {
            continue;
        };
        let Some(destination) = destinations.get(&key) else {
            continue;
        };
        let note_path = path_relative(vault, &plan.path)?;
        outcome.results.push(FilingResult {
            key: key.clone(),
            note: Some(note_path.clone()),
            link: Some(plan.link.clone()),
            task_file: Some(save::path_to_slashes(&destination.relative)),
            status: if dry_run { "would_file" } else { "filed" },
            route: destination.route.clone(),
            via: Some(destination.via),
            problem: destination.problem.clone(),
            first_seen: destination.problem.is_some(),
            watch_member: true,
        });
        if !dry_run {
            enrollment_events.push(EnrollmentEvent::Filed {
                key,
                note: note_path,
                task_file: save::path_to_slashes(&destination.relative),
                via: destination.via,
                route: destination.route.clone(),
            });
        }
    }

    // Plan auxiliary hubs only when this run writes at least one mail
    // note. Adoption-only, failure-only, and recovery-only runs must not
    // create hubs or append the AGENTS warning; a replay's stored hub paths
    // rejoin the scoped commit through the recovery evidence instead.
    let auxiliary = if plans.iter().any(|plan| plan.changed) {
        let child_env = ob::child_env();
        save::plan_auxiliary_files(
            vault,
            mail_folder,
            is_worktree,
            &child_env,
            true,
        )?
    } else {
        save::AuxiliaryFiles {
            staged: Vec::new(),
            commit_paths: Vec::new(),
        }
    };
    staged.extend(auxiliary.staged.iter().map(save::clone_staged));
    save::validate_write_targets(vault, &plans, &works, &staged)?;

    for failure in &watch_failures {
        outcome.failures.push(FilingFailure {
            key: failure.key.clone(),
            kind: failure.kind.clone(),
            message: failure.message.clone(),
        });
        outcome.warnings.push(format!(
            "skipped Gmail thread {}: {}: {}",
            failure.key.thread_id(),
            failure.kind,
            failure.message
        ));
    }
    for key in &watch_deferred {
        if !outcome.deferred.contains(key) {
            outcome.deferred.push(key.clone());
        }
    }
    for watch in &watches {
        if let Some(failure) = &watch.refresh_failure {
            outcome.failures.push(FilingFailure {
                key: failure.key.clone(),
                kind: failure.kind.clone(),
                message: failure.message.clone(),
            });
            outcome.warnings.push(format!(
                "skipped Gmail watch refresh for {}: {}: {}",
                failure.key.thread_id(),
                failure.kind,
                failure.message
            ));
        }
    }

    // Stage body-bearing watch refreshes. Each refresh moves out of the
    // owned watches vector so no Clone bound is needed; one damaged note
    // never discards the other watches.
    let mut refreshes: Vec<(ThreadKey, PreparedSave)> = Vec::new();
    for watch in &mut watches {
        if !works.contains_key(&watch.observation.key.index_key())
            && let Some(refresh) = watch.refresh.take()
        {
            let key = watch.observation.key.clone();
            refreshes.push((key, refresh));
        }
    }
    let mut watch_refresh_plans: Vec<StagedSaveNote> = Vec::new();
    for (key, refresh) in refreshes {
        let mut one = BTreeMap::from([(key.index_key(), refresh)]);
        match save::plan_prepared_saves_with_parent_links(
            vault,
            mail_folder,
            link_template,
            &mut one,
            index,
            None,
            &BTreeMap::new(),
            false,
            saved_at,
            timezone,
        ) {
            Ok(mut made) => watch_refresh_plans.append(&mut made),
            Err(error) => {
                outcome.failures.push(FilingFailure {
                    key: key.clone(),
                    kind: error.kind().to_string(),
                    message: error.message().to_string(),
                });
                outcome.warnings.push(format!(
                    "skipped Gmail watch refresh for {}: {}: {}",
                    key.thread_id(),
                    error.kind(),
                    error.message()
                ));
            }
        }
    }
    for plan in &watch_refresh_plans {
        match save::staged_note(plan) {
            Ok(staged_note) => staged.push(staged_note),
            Err(error) => {
                if let Some(key) =
                    ThreadKey::new(&plan.account, &plan.thread_id)
                {
                    outcome.failures.push(FilingFailure {
                        key,
                        kind: error.kind().to_string(),
                        message: error.message().to_string(),
                    });
                }
            }
        }
    }
    plans.extend(watch_refresh_plans);

    let today = bob_env::current_datetime().date();
    let (blocked_ok, blocked_reason) = hold::blocked_status_for_hold(vault);
    let running = hold::load_running_tasks(vault);
    let mut filing_tasks: BTreeMap<PathBuf, FilingTaskSnapshot> =
        BTreeMap::new();
    for (relative, snapshot) in &snapshots {
        let original_lines: BTreeSet<String> = snapshot
            .original
            .as_deref()
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        let mut projected = Vec::new();
        for plan in &plans {
            let Some(key) = ThreadKey::new(&plan.account, &plan.thread_id)
            else {
                continue;
            };
            let Some(destination) = destinations.get(&key) else {
                continue;
            };
            if destination.relative != *relative {
                continue;
            }
            let Some((line_index, line)) =
                snapshot.updated.lines().enumerate().find(|(_, line)| {
                    line.contains(&plan.link)
                        && line.contains("#task")
                        && !original_lines.contains(*line)
                })
            else {
                continue;
            };
            projected.push(WatchingTask {
                path: save::path_to_slashes(relative),
                line: line_index + 1,
                text: line.to_string(),
                block_id: None,
                status: ' ',
                targets: vec![WatchTarget {
                    key,
                    note: path_relative(vault, &plan.path)?,
                    placement: LinkPlacement::TaskLine,
                }],
            });
        }
        if !projected.is_empty() {
            filing_tasks.insert(
                relative.clone(),
                FilingTaskSnapshot {
                    target: snapshot.target.clone(),
                    original: snapshot.original.clone(),
                    updated: snapshot.updated.clone(),
                    tasks: projected,
                },
            );
        }
    }
    let watch_plan =
        watch_apply::plan_watch_edits(watch_apply::WatchPlanInput {
            vault,
            timezone,
            reply_wait_days,
            today,
            blocked_ok,
            blocked_reason,
            running,
            watchers,
            prepared: &watches,
            journal: watch_journal,
            filing_tasks: &filing_tasks,
            pending: pending_watch,
        });
    outcome.warnings.extend(watch_plan.warnings);
    outcome.failed |= watch_plan.failed;
    outcome.watch_snapshot = watch_plan.snapshot.clone();
    for edit in watch_plan.edits {
        if let Some(existing) =
            staged.iter_mut().find(|file| file.target == edit.target)
        {
            existing.updated_target = edit.updated;
        } else {
            let existed = edit.existed
                || fs::symlink_metadata(&edit.target)
                    .is_ok_and(|metadata| metadata.is_file());
            staged.push(StagedTextFile {
                target: edit.target.clone(),
                target_existed: existed,
                original_target: edit.original,
                updated_target: edit.updated,
            });
        }
        if let Ok(relative) = edit.target.strip_prefix(vault) {
            let relative = relative.to_path_buf();
            if !task_commit_paths.contains(&relative) {
                task_commit_paths.push(relative);
            }
        }
    }
    apply_born_held(
        vault,
        timezone,
        reply_wait_days,
        today,
        blocked_ok,
        &mut staged,
        &mut task_commit_paths,
        &mut outcome,
        &works,
    );
    outcome.watch_threads = watch_plan.outcomes;
    outcome.watch_gone = watch_plan.gone;
    for key in &watch_deferred {
        if !outcome.deferred.contains(key) {
            outcome.deferred.push(key.clone());
        }
    }

    outcome
        .results
        .sort_by(|left, right| left.key.cmp(&right.key));
    outcome
        .failures
        .sort_by(|left, right| left.key.cmp(&right.key));
    outcome.deferred.sort();
    outcome.deferred.dedup();
    outcome.changed = !staged.is_empty() || !enrollment_events.is_empty();
    outcome.applied = true;

    Ok(AttemptPlan {
        outcome,
        works,
        destinations,
        plans,
        staged,
        task_commit_paths,
        auxiliary_commit_paths: auxiliary.commit_paths,
        enrollment_events,
        watch_turns: watch_plan.receipts.turns,
        watch_gone: watch_plan.receipts.gone,
        watch_pending: watch_plan.pending,
    })
}

/// Newly filed rows are born held when their thread is outgoing-last.
/// The filing snapshot already contains the new task line; this pass folds
/// the filed thread with no marker and applies the hold in the same staged
/// batch so the commit stays scoped to one transaction.
fn apply_born_held(
    vault: &Path,
    timezone: chrono_tz::Tz,
    reply_wait_days: u32,
    today: chrono::NaiveDate,
    blocked_ok: bool,
    staged: &mut Vec<StagedTextFile>,
    task_commit_paths: &mut Vec<PathBuf>,
    outcome: &mut FilingOutcome,
    works: &BTreeMap<(String, String), PreparedSave>,
) {
    use crate::native::gmail::turn as gmail_turn;

    for result in &outcome.results {
        if !matches!(result.status, "filed" | "would_file") {
            continue;
        }
        if !result.watch_member {
            continue;
        }
        let Some(note_path) = result.note.clone() else {
            continue;
        };
        let Some(task_path) = result.task_file.clone() else {
            continue;
        };
        let Some(link) = result.link.clone() else {
            continue;
        };
        let Some(save) = works.get(&result.key.index_key()) else {
            continue;
        };
        let folded = gmail_turn::fold(
            &save.confirmed.thread.messages,
            result.key.account(),
            None,
        );
        if !matches!(folded.net, gmail_turn::Net::Hold) {
            continue;
        }
        let Some(send_at) = folded.last_send_at else {
            continue;
        };
        let nudge =
            hold::nudge_date_for_send(send_at, timezone, reply_wait_days);
        let req = hold::HoldRequest {
            nudge_date: nudge,
            today,
            blocked_ok,
            blocked_reason: None,
        };
        let task_target = vault.join(PathBuf::from(&task_path));
        let Some(staged_file) =
            staged.iter_mut().find(|file| file.target == task_target)
        else {
            continue;
        };
        // Find the new task line by its link and apply the hold in place.
        let mut lines: Vec<String> = staged_file
            .updated_target
            .lines()
            .map(str::to_string)
            .collect();
        let trailing_newline = staged_file.updated_target.ends_with('\n');
        let mut changed = false;
        for line in &mut lines {
            if !line.contains(link.as_str()) || !line.contains("#task") {
                continue;
            }
            let applied = hold::apply_hold(line, &req);
            if applied.changed {
                *line = applied.line;
                changed = true;
                break;
            }
        }
        if changed {
            let mut rebuilt = lines.join("\n");
            if trailing_newline {
                rebuilt.push('\n');
            }
            staged_file.updated_target = rebuilt;
            if let Ok(relative) = task_target.strip_prefix(vault) {
                let relative = relative.to_path_buf();
                if !task_commit_paths.contains(&relative) {
                    task_commit_paths.push(relative);
                }
            }
            let _ = note_path;
        }
    }
}

fn persist_attempt(
    vault: &Path,
    account: &str,
    child_env: &ob::ChildEnv,
    is_worktree: bool,
    mut plan: AttemptPlan,
    index: &MailIndex,
    recovery: &RecoveryData,
) -> Result<FilingOutcome, (GmailError, AttemptPlan)> {
    let facts = match save::save_event_facts(
        vault,
        child_env,
        is_worktree,
        &plan.plans,
        &plan.works,
    ) {
        Ok(facts) => facts,
        Err(error) => return Err((error, plan)),
    };

    let mut commit_paths =
        match commit_paths(vault, child_env, is_worktree, &plan, &facts) {
            Ok(paths) => paths,
            Err(error) => return Err((error, plan)),
        };
    let created_images = match save::write_prepared_batch(
        vault,
        &plan.works,
        &plan.plans,
        &plan.staged,
    ) {
        Ok(paths) => paths,
        Err(error) => {
            // A failed text batch must remove only images this attempt
            // created, preserving the save transaction's rollback rule.
            return Err((error, plan));
        }
    };
    commit_paths
        .extend(created_images.into_iter().map(|(relative, _)| relative));
    commit_paths.sort();
    commit_paths.dedup();

    // Finish a prior failed transaction's scoped commit before new
    // enrollment is recorded. Stored recovery paths rejoin only while
    // their bytes still match what sync wrote; a path the user edited
    // after the failure is left out with a warning and dropped from the
    // evidence, while unrelated dirt, custom hubs, and the vault AGENTS
    // warning stay out. Rechecked under the lock so edits after the
    // failed attempt are never overwritten.
    commit_paths.extend(replay_commit_paths(
        &mut plan.outcome,
        vault,
        child_env,
        is_worktree,
        &recovery.commit_paths,
        &commit_paths,
    ));
    commit_paths.sort();
    commit_paths.dedup();
    // SHA-256 of each owned path's bytes as sync wrote them, so a later
    // replay commits a path only while it still matches.
    let commit_hashes = if is_worktree {
        snapshot_commit_paths(vault, &commit_paths)
    } else {
        Vec::new()
    };

    if is_worktree && !commit_paths.is_empty() {
        let count = plan.enrollment_events.len() + recovery.threads.len();
        if let Err(error) = ob::commit_paths(
            vault,
            child_env,
            &format!("gmail: sync {count} threads"),
            &commit_paths,
        ) {
            // Retain body-free ownership evidence so replay can finish the
            // scoped commit and its deferred events before enrollment.
            // Filed threads keep every stage pending; adopted threads keep
            // only enrollment (adoptions never produce save receipts).
            let mut threads: Vec<RecoveryThread> = recovery.threads.clone();
            for result in &plan.outcome.results {
                if !matches!(result.status, "filed" | "adopted") {
                    continue;
                }
                if threads.iter().any(|thread| {
                    thread.account == result.key.account()
                        && thread.thread_id == result.key.thread_id()
                }) {
                    continue;
                }
                threads.push(new_filed_evidence(result, &facts));
            }
            // Enrollment events without a result row still need retention.
            for event in &plan.enrollment_events {
                let key = event.key();
                if threads.iter().any(|thread| {
                    thread.account == key.account()
                        && thread.thread_id == key.thread_id()
                }) {
                    continue;
                }
                threads.push(new_event_evidence(event, &facts));
            }
            // Best effort: the run already fails with the uncommitted
            // error below, which carries the replay hint.
            // Watch work shares the same scoped commit: retain pending turns
            // (tasks are already written, markers never advanced) alongside
            // filing evidence so replay finishes both without resending.
            let watch = dedup_watch_pending(
                recovery
                    .watch
                    .iter()
                    .cloned()
                    .chain(plan.watch_pending.iter().cloned())
                    .chain(
                        plan.watch_turns
                            .iter()
                            .map(|turn| recovery_watch_for_turn(turn, false)),
                    )
                    .chain(plan.watch_gone.iter().filter_map(|key| {
                        Some(RecoveryWatch {
                            account: key.account().to_string(),
                            thread_id: key.thread_id().to_string(),
                            message_id: String::new(),
                            internal_date: 0,
                            turn: "gone".to_string(),
                            effect: "gone".to_string(),
                            nudge_on: None,
                            unblocked_on: None,
                            pending_tasks: false,
                            pending_marker: true,
                        })
                    })),
            );
            let persist_error = write_recovery(
                vault,
                account,
                &RecoveryData {
                    threads,
                    commit_paths: commit_hashes,
                    watch,
                },
            )
            .err();
            let mut message = format!(
                "Gmail files were written but could not be committed: {error}"
            );
            if let Some(persist_error) = persist_error {
                message.push_str(&format!(
                    "; could not persist Gmail sync recovery evidence: {persist_error}"
                ));
            }
            return Err((
                GmailError::runtime("uncommitted", message).with_hint(
                    "run `bob gmail sync` again to finish the scoped commit",
                ),
                plan,
            ));
        }
    }

    let host = vault_sync::hostname_slug();
    // Enrollment last, after the vault writes it describes are durable.
    // With in-flight skips above, current keys and recovery keys are
    // disjoint; keep the defensive guard so a current Adopted event for a
    // recovery key is still superseded by the recorded filing.
    let recovery_keys: BTreeSet<(String, String)> = recovery
        .threads
        .iter()
        .map(|thread| (thread.account.clone(), thread.thread_id.clone()))
        .collect();
    let mut current_events = std::mem::take(&mut plan.enrollment_events);
    current_events.retain(|event| {
        let key = event.key();
        let pair = (key.account().to_string(), key.thread_id().to_string());
        !recovery_keys.contains(&pair)
    });

    // One thread's journal work: this run's filings plus the recovery
    // replay. Each thread's stages complete in order — `gmail.save`, then
    // each attachment-origin event, then enrollment — and a thread's
    // enrollment is appended only after its save and origin stages are
    // done. After each successful append the stage is dropped from the
    // evidence and persisted immediately, so a retry never re-appends
    // anything that already succeeded.
    struct JournalWork {
        key: ThreadKey,
        thread: RecoveryThread,
        staged: Option<StagedSaveNote>,
        save_facts: SaveEventFacts,
        enrollment: enroll::EnrollmentEvent,
        from_recovery: bool,
    }
    let mut works: Vec<JournalWork> = Vec::new();
    let journaled_notes: Vec<&StagedSaveNote> = plan
        .plans
        .iter()
        .filter(|note| {
            note.changed
                || note.path.strip_prefix(vault).ok().is_some_and(|relative| {
                    commit_paths.iter().any(|path| path == relative)
                })
        })
        .collect();
    // Current-run filings: every stage is pending. A save or origin
    // failure keeps the stages in the evidence and blocks that thread's
    // enrollment until a replay succeeds.
    for note in journaled_notes {
        let Some(key) = ThreadKey::new(&note.account, &note.thread_id) else {
            plan.outcome.failed = true;
            plan.outcome.warnings.push(
                "skipped a filed Gmail thread with an invalid identity"
                    .to_string(),
            );
            continue;
        };
        let Some(event_facts) = facts.get(&key.index_key()) else {
            continue;
        };
        let Some(enrollment) = current_events
            .iter()
            .find(|event| event.key() == &key)
            .cloned()
        else {
            continue;
        };
        let Some(result) =
            plan.outcome.results.iter().find(|result| result.key == key)
        else {
            continue;
        };
        // The stored thread mirrors the result row, with every stage
        // pending: a save or origin failure keeps the stages in the
        // evidence and blocks that thread's enrollment until a replay
        // succeeds.
        works.push(JournalWork {
            key,
            thread: new_filed_evidence(result, &facts),
            staged: Some(note.clone()),
            save_facts: event_facts.clone(),
            enrollment,
            from_recovery: false,
        });
    }
    // Current-run adoptions carry no save or origin stages: adoptions never
    // produce `gmail.save` receipts.
    for event in &current_events {
        let key = event.key().clone();
        if works.iter().any(|work| work.key == key) {
            continue;
        }
        if !matches!(event, enroll::EnrollmentEvent::Adopted { .. }) {
            continue;
        }
        works.push(JournalWork {
            key,
            thread: new_event_evidence(event, &facts),
            staged: None,
            save_facts: SaveEventFacts {
                created: false,
                added_count: 0,
                origins: Vec::new(),
            },
            enrollment: event.clone(),
            from_recovery: false,
        });
    }
    // Recovery replay: only the stages still pending in the evidence run.
    for thread in &recovery.threads {
        let Some(key) = ThreadKey::new(&thread.account, &thread.thread_id)
        else {
            plan.outcome.failed = true;
            plan.outcome.warnings.push(
                "skipped stored Gmail sync recovery evidence with an invalid identity"
                    .to_string(),
            );
            continue;
        };
        if works.iter().any(|work| work.key == key) {
            continue;
        }
        let Some(enrollment) = recovery_enrollment_event(thread) else {
            plan.outcome.failed = true;
            plan.outcome.warnings.push(
                "skipped stored Gmail sync recovery evidence with an invalid identity"
                    .to_string(),
            );
            continue;
        };
        if thread.adopted {
            works.push(JournalWork {
                key,
                thread: RecoveryThread {
                    pending_save: false,
                    pending_origins: Vec::new(),
                    ..thread.clone()
                },
                staged: None,
                save_facts: SaveEventFacts {
                    created: false,
                    added_count: 0,
                    origins: Vec::new(),
                },
                enrollment,
                from_recovery: true,
            });
            continue;
        }
        // A missing note file keeps today's behavior: skip the save and
        // origin stages for that thread but still append its enrollment.
        let absolute = vault.join(PathBuf::from(&thread.note));
        let missing = fs::symlink_metadata(&absolute).is_err();
        let staged = StagedSaveNote {
            account: thread.account.clone(),
            thread_id: thread.thread_id.clone(),
            path: absolute,
            link: String::new(),
            title: String::new(),
            who: String::new(),
            message_count: 0,
            added_count: thread.added,
            created: thread.created,
            changed: false,
            status: "filed",
            original: None,
            updated: String::new(),
            preview: Vec::new(),
            preview_noop: true,
        };
        works.push(JournalWork {
            key,
            thread: RecoveryThread {
                pending_save: thread.pending_save && !missing,
                pending_origins: if missing {
                    Vec::new()
                } else {
                    thread.pending_origins.clone()
                },
                ..thread.clone()
            },
            staged: Some(staged),
            save_facts: SaveEventFacts {
                created: thread.created,
                added_count: thread.added,
                origins: thread.origins.clone(),
            },
            enrollment,
            from_recovery: true,
        });
    }

    let persist_journal =
        |outcome: &mut FilingOutcome, works: &[JournalWork]| -> bool {
            let threads: Vec<RecoveryThread> = works
                .iter()
                .filter(|work| {
                    work.thread.pending_save
                        || !work.thread.pending_origins.is_empty()
                        || work.thread.pending_enrollment
                })
                .map(|work| work.thread.clone())
                .collect();
            // Preserve both prior watch evidence and this run's deferred
            // turns so a filing journal failure never drops watch work.
            let watch = dedup_watch_pending(
                recovery
                    .watch
                    .iter()
                    .cloned()
                    .chain(plan.watch_pending.iter().cloned()),
            );
            persist_recovery(
                outcome,
                vault,
                account,
                &RecoveryData {
                    threads,
                    commit_paths: Vec::new(),
                    watch,
                },
            )
        };

    // Save events first. A failure keeps the save and origin stages pending
    // and blocks that thread's enrollment until a replay succeeds. A
    // checkpoint failure after a successful append stops later stages so
    // replay can skip the already-present event instead of duplicating it.
    let mut checkpoint_ok = true;
    for index in 0..works.len() {
        if !checkpoint_ok {
            break;
        }
        if !works[index].thread.pending_save || works[index].staged.is_none() {
            continue;
        }
        let staged = works[index].staged.as_ref().expect("checked above");
        if works[index].from_recovery
            && save_event_present(
                vault,
                staged.account.as_str(),
                staged.thread_id.as_str(),
                &save::path_to_slashes(
                    staged.path.strip_prefix(vault).unwrap_or(&staged.path),
                ),
            )
        {
            works[index].thread.pending_save = false;
            checkpoint_ok = persist_journal(&mut plan.outcome, &works);
            continue;
        }
        let result = save::append_save_event(
            vault,
            &host,
            staged,
            &works[index].save_facts,
        );
        if let Err(error) = result {
            plan.outcome.failed = true;
            plan.outcome
                .warnings
                .push(format!("could not append Gmail save journal: {error}"));
            continue;
        }
        works[index].thread.pending_save = false;
        checkpoint_ok = persist_journal(&mut plan.outcome, &works);
    }
    // Origin events next, only for threads whose save stage is done.
    // Checkpoint each successful origin separately so a later interruption
    // cannot re-append the ones that already landed.
    if checkpoint_ok {
        for index in 0..works.len() {
            if !checkpoint_ok {
                break;
            }
            if works[index].thread.pending_save {
                continue;
            }
            let mut pending =
                std::mem::take(&mut works[index].thread.pending_origins);
            let mut pos = 0usize;
            while pos < pending.len() {
                let (saved, source, sha256) = pending[pos].clone();
                if works[index].from_recovery
                    && origin_event_present(vault, &saved, &sha256)
                {
                    pending.remove(pos);
                    works[index].thread.pending_origins = pending.clone();
                    checkpoint_ok = persist_journal(&mut plan.outcome, &works);
                    if !checkpoint_ok {
                        break;
                    }
                    continue;
                }
                let display = attachment_journal::display_for_source(&source);
                let ts = attachment_journal::event_timestamp();
                let origin_host = attachment_journal::host_label();
                match attachment_journal::append_event(
                    vault,
                    &origin_host,
                    &saved,
                    &source,
                    &display,
                    &sha256,
                    "gmail",
                    ts,
                ) {
                    Ok(_) => {
                        pending.remove(pos);
                        works[index].thread.pending_origins = pending.clone();
                        checkpoint_ok =
                            persist_journal(&mut plan.outcome, &works);
                        if !checkpoint_ok {
                            break;
                        }
                    }
                    Err(error) => {
                        plan.outcome.warnings.push(format!(
                            "could not record imported image {saved}: append attachment journal in {}: {error}",
                            vault.display()
                        ));
                        plan.outcome.failed = true;
                        pos += 1;
                    }
                }
            }
            works[index].thread.pending_origins = pending;
        }
    }

    // Enrollment last, after the vault writes it describes are durable.
    // A thread's enrollment is appended only after its save and origin
    // stages are done, so a current-run save or origin failure blocks the
    // enrollment until a replay succeeds. A failed enrollment is reported
    // as a failure and deferred, so a failed adoption never counts as
    // newly enrolled watch; the next run repairs it by adoption, as today.
    if checkpoint_ok {
        for index in 0..works.len() {
            if !checkpoint_ok {
                break;
            }
            if !works[index].thread.pending_enrollment {
                continue;
            }
            if works[index].thread.pending_save
                || !works[index].thread.pending_origins.is_empty()
            {
                continue;
            }
            // Avoid duplicating a recovery enrollment the vault already records
            // (e.g. a prior replay partially succeeded).
            if works[index].from_recovery
                && let Ok(fold) = EnrollmentFold::load(vault)
                && fold.is_enrolled(&works[index].key)
            {
                works[index].thread.pending_enrollment = false;
                checkpoint_ok = persist_journal(&mut plan.outcome, &works);
                continue;
            }
            if let Err(error) =
                enroll::append(vault, &host, &works[index].enrollment)
            {
                plan.outcome.failed = true;
                plan.outcome.warnings.push(format!(
                    "could not append Gmail enrollment journal: {error}"
                ));
                plan.outcome.deferred.push(works[index].key.clone());
                plan.outcome.deferred.sort();
                plan.outcome.deferred.dedup();
                continue;
            }
            works[index].thread.pending_enrollment = false;
            checkpoint_ok = persist_journal(&mut plan.outcome, &works);
        }
    }
    // Surface recovered threads in this run's outcome so counts, watch
    // membership, and human rows reflect the finished transaction once.
    for work in &works {
        if !work.from_recovery {
            continue;
        }
        if plan
            .outcome
            .results
            .iter()
            .any(|result| result.key == work.key)
        {
            continue;
        }
        plan.outcome.results.push(recovery_row(
            &work.thread,
            &work.key,
            index,
            &plan.outcome.watch_snapshot,
            false,
        ));
    }
    // Pending save/origin/enrollment never counts as completed enrollment
    // or as newly enrolled watch.
    for work in &works {
        if work.thread.pending_save
            || !work.thread.pending_origins.is_empty()
            || work.thread.pending_enrollment
        {
            if let Some(result) = plan
                .outcome
                .results
                .iter_mut()
                .find(|result| result.key == work.key)
            {
                result.watch_member = false;
            }
            plan.outcome.deferred.push(work.key.clone());
        }
    }
    plan.outcome.deferred.sort();
    plan.outcome.deferred.dedup();
    plan.outcome
        .results
        .sort_by(|left, right| left.key.cmp(&right.key));

    // Watch markers follow durable note/task writes and the scoped commit.
    // A running-task hold has no task write and never advances its marker;
    // those threads stay in plan.watch_pending via the planner. Here we
    // append applied turns and one-time gone reports with the same receipt
    // approach as filing journals: checkpoint after each append so a retry
    // never duplicates what already landed.
    let mut watch_pending: Vec<RecoveryWatch> = plan.watch_pending.clone();
    let mut watch_checkpoint_ok = checkpoint_ok;
    // Re-check markers against the current journal so a concurrent host that
    // appended the same turn between planning and persisting does not cause
    // a duplicate append.
    let fresh_journal = WatchJournal::load(vault).unwrap_or_default();
    for turn in std::mem::take(&mut plan.watch_turns) {
        if !watch_checkpoint_ok {
            // Journal persistence already failed; retain the rest without
            // attempting more appends.
            watch_pending.push(recovery_watch_for_turn(&turn, true));
            continue;
        }
        if let Some(marker) = fresh_journal.marker(&turn.key)
            && marker.message_id.eq_ignore_ascii_case(&turn.message_id)
            && marker.internal_date == turn.internal_date
        {
            continue;
        }
        // Skip already-applied turns recorded by another host file that the
        // fresh load above may have missed due to a concurrent write that
        // landed after the load. The append itself is idempotent per key
        // only through the marker check; a duplicate append would still
        // fold to the same marker, but we avoid it when the pre-check hits.
        if let Err(error) = WatchJournal::append_turn(vault, &host, &turn) {
            plan.outcome.failed = true;
            plan.outcome
                .warnings
                .push(format!("could not append Gmail watch journal: {error}"));
            // Tasks are already durable; only the marker remains.
            watch_pending.push(recovery_watch_for_turn(&turn, false));
            // Persist the pending evidence immediately so a retry finishes
            // the marker without reapplying already-applied targets.
            let threads: Vec<RecoveryThread> = works
                .iter()
                .filter(|work| {
                    work.thread.pending_save
                        || !work.thread.pending_origins.is_empty()
                        || work.thread.pending_enrollment
                })
                .map(|work| work.thread.clone())
                .collect();
            watch_checkpoint_ok = persist_recovery(
                &mut plan.outcome,
                vault,
                account,
                &RecoveryData {
                    threads,
                    commit_paths: Vec::new(),
                    watch: dedup_watch_pending(
                        recovery
                            .watch
                            .iter()
                            .cloned()
                            .chain(watch_pending.iter().cloned()),
                    ),
                },
            );
            continue;
        }
        // Checkpoint the remaining watch evidence after each successful
        // marker so a later interruption does not re-append it.
        {
            let threads: Vec<RecoveryThread> = works
                .iter()
                .filter(|work| {
                    work.thread.pending_save
                        || !work.thread.pending_origins.is_empty()
                        || work.thread.pending_enrollment
                })
                .map(|work| work.thread.clone())
                .collect();
            watch_checkpoint_ok = persist_recovery(
                &mut plan.outcome,
                vault,
                account,
                &RecoveryData {
                    threads,
                    commit_paths: Vec::new(),
                    watch: dedup_watch_pending(
                        recovery
                            .watch
                            .iter()
                            .cloned()
                            .chain(watch_pending.iter().cloned()),
                    ),
                },
            );
            if !watch_checkpoint_ok {
                // The marker landed but its receipt did not; retain it as
                // marker-pending without reapplying tasks.
                watch_pending.push(recovery_watch_for_turn(&turn, false));
            }
        }
    }
    for key in std::mem::take(&mut plan.watch_gone) {
        if !watch_checkpoint_ok {
            continue;
        }
        let fresh = WatchJournal::load(vault).unwrap_or_default();
        if fresh.was_reported_gone(&key) {
            continue;
        }
        if let Err(error) = WatchJournal::append_gone(vault, &host, &key) {
            plan.outcome.failed = true;
            plan.outcome
                .warnings
                .push(format!("could not append Gmail watch journal: {error}"));
            let threads: Vec<RecoveryThread> = works
                .iter()
                .filter(|work| {
                    work.thread.pending_save
                        || !work.thread.pending_origins.is_empty()
                        || work.thread.pending_enrollment
                })
                .map(|work| work.thread.clone())
                .collect();
            watch_checkpoint_ok = persist_recovery(
                &mut plan.outcome,
                vault,
                account,
                &RecoveryData {
                    threads,
                    commit_paths: Vec::new(),
                    watch: dedup_watch_pending(
                        recovery
                            .watch
                            .iter()
                            .cloned()
                            .chain(watch_pending.iter().cloned()),
                    ),
                },
            );
            let _ = error;
            continue;
        }
        {
            let threads: Vec<RecoveryThread> = works
                .iter()
                .filter(|work| {
                    work.thread.pending_save
                        || !work.thread.pending_origins.is_empty()
                        || work.thread.pending_enrollment
                })
                .map(|work| work.thread.clone())
                .collect();
            watch_checkpoint_ok = persist_recovery(
                &mut plan.outcome,
                vault,
                account,
                &RecoveryData {
                    threads,
                    commit_paths: Vec::new(),
                    watch: dedup_watch_pending(
                        recovery
                            .watch
                            .iter()
                            .cloned()
                            .chain(watch_pending.iter().cloned()),
                    ),
                },
            );
        }
    }
    // Deferred watch threads (running holds, unknown siblings) never advance
    // a marker and stay pending with their body-free turn evidence.
    for pending in &watch_pending {
        if let Some(key) = pending.key()
            && !plan.outcome.deferred.contains(&key)
        {
            plan.outcome.deferred.push(key);
        }
    }
    // Final receipt: filing and watch evidence share one persistence so a
    // single clear failure keeps the same retry semantics as before. The
    // watch markers above already checkpointed after each append; this
    // retains deferred turns alongside any remaining filing stages.
    // Note: `persist_journal` already includes `plan.watch_pending` plus
    // prior watch evidence, but marker-only failures appended above added
    // new `watch_pending` entries after those checkpoints. Persist once
    // more only when watch work remains; otherwise leave the single filing
    // receipt (and its clear-failure accounting) untouched.
    if !watch_pending.is_empty() || !recovery.watch.is_empty() {
        let threads: Vec<RecoveryThread> = works
            .iter()
            .filter(|work| {
                work.thread.pending_save
                    || !work.thread.pending_origins.is_empty()
                    || work.thread.pending_enrollment
            })
            .map(|work| work.thread.clone())
            .collect();
        let watch = dedup_watch_pending(
            recovery
                .watch
                .iter()
                .cloned()
                .chain(watch_pending.iter().cloned()),
        );
        let _ = persist_recovery(
            &mut plan.outcome,
            vault,
            account,
            &RecoveryData {
                threads,
                commit_paths: Vec::new(),
                watch,
            },
        );
    } else {
        let _ = persist_journal(&mut plan.outcome, &works);
    }
    let _ = checkpoint_ok;
    let _ = watch_checkpoint_ok;
    // Restore current enrollment events for any caller that inspects the
    // plan after a journal failure (the outcome already records failure).
    plan.enrollment_events = current_events;
    // Surface deferred watch threads as failed so polling retries them.
    if plan
        .outcome
        .deferred
        .iter()
        .any(|key| watch_keys_for_outcome(&plan.outcome, key))
    {
        plan.outcome.failed = true;
    }
    Ok(plan.outcome)
}

fn recovery_watch_for_turn(
    turn: &crate::native::gmail::sync::watch_state::AppliedTurn,
    tasks_pending: bool,
) -> RecoveryWatch {
    let (turn_text, effect_text) = match (&turn.turn, &turn.effect) {
        (crate::native::gmail::turn::Turn::Theirs, _) => ("theirs", "hold"),
        (
            crate::native::gmail::turn::Turn::Yours,
            crate::native::gmail::sync::watch_state::AppliedEffect::Bounced,
        ) => ("yours", "bounced"),
        (crate::native::gmail::turn::Turn::Yours, _) => ("yours", "lift"),
    };
    RecoveryWatch {
        account: turn.key.account().to_string(),
        thread_id: turn.key.thread_id().to_string(),
        message_id: turn.message_id.clone(),
        internal_date: turn.internal_date,
        turn: turn_text.to_string(),
        effect: effect_text.to_string(),
        nudge_on: None,
        unblocked_on: None,
        pending_tasks: tasks_pending,
        pending_marker: true,
    }
}

fn dedup_watch_pending(
    pending: impl IntoIterator<Item = RecoveryWatch>,
) -> Vec<RecoveryWatch> {
    let mut by_key: BTreeMap<(String, String), RecoveryWatch> = BTreeMap::new();
    for item in pending {
        let key = (item.account.clone(), item.thread_id.clone());
        // Keep the newest turn per thread: larger Gmail date wins, then
        // larger message id. A newer turn supersedes obsolete pending holds.
        let replace = by_key.get(&key).is_none_or(|existing| {
            (item.internal_date, item.message_id.clone())
                > (existing.internal_date, existing.message_id.clone())
        });
        if replace {
            by_key.insert(key, item);
        }
    }
    by_key.into_values().collect()
}

fn watch_keys_for_outcome(outcome: &FilingOutcome, key: &ThreadKey) -> bool {
    outcome
        .watch_threads
        .iter()
        .any(|thread| thread.key == *key)
}

/// Threads with stored recovery evidence: in-flight filings the current
/// run must neither file nor adopt again.
fn in_flight_keys(recovery: &RecoveryData) -> BTreeSet<ThreadKey> {
    recovery
        .threads
        .iter()
        .filter_map(|thread| ThreadKey::new(&thread.account, &thread.thread_id))
        .collect()
}

/// The recorded enrollment event for one recovery thread: the `gmail.filed`
/// or `gmail.adopted` the replay appends. `None` when the stored identity
/// is invalid; callers warn and fail instead of panicking.
fn recovery_enrollment_event(
    thread: &RecoveryThread,
) -> Option<enroll::EnrollmentEvent> {
    let key = ThreadKey::new(&thread.account, &thread.thread_id)?;
    if thread.adopted {
        return Some(enroll::EnrollmentEvent::Adopted {
            key,
            note: thread.note.clone(),
            task_file: thread.task_file.clone(),
        });
    }
    let via = match thread.via.as_str() {
        "route" => enroll::FiledVia::Route,
        "problem" => enroll::FiledVia::Problem,
        _ => enroll::FiledVia::Inbox,
    };
    Some(enroll::EnrollmentEvent::Filed {
        key,
        note: thread.note.clone(),
        task_file: thread.task_file.clone(),
        via,
        route: thread.route.clone(),
    })
}

fn recovery_problem(thread: &RecoveryThread) -> Option<RouteProblem> {
    let label = thread.problem_label.as_ref()?;
    let kind = thread.problem_kind.as_ref()?;
    let message = thread.problem_message.as_ref()?;
    let kind = match kind.as_str() {
        "unknown_note" => {
            crate::native::gmail::route::RouteProblemKind::UnknownNote
        }
        "not_area_or_project" => {
            crate::native::gmail::route::RouteProblemKind::NotAreaOrProject
        }
        "closed_project" => {
            crate::native::gmail::route::RouteProblemKind::ClosedProject
        }
        "ambiguous_alias" => {
            crate::native::gmail::route::RouteProblemKind::AmbiguousAlias
        }
        "nested_route" => {
            crate::native::gmail::route::RouteProblemKind::NestedRoute
        }
        _ => crate::native::gmail::route::RouteProblemKind::AmbiguousRoute,
    };
    Some(RouteProblem {
        kind,
        label: label.clone(),
        message: message.clone(),
        hint: thread.problem_hint.clone().unwrap_or_default(),
    })
}

/// One recovered thread's result row. The link renders the mail note's
/// inert title and who from the index; dry-run projects the filing with
/// `would_file` instead of `filed`. Watch membership follows the current
/// open-task snapshot: a recovered closed or deleted task does not count.
fn recovery_row(
    thread: &RecoveryThread,
    key: &ThreadKey,
    index: &MailIndex,
    watchers: &WatchScan,
    dry_run: bool,
) -> FilingResult {
    let link = index
        .by_identity
        .get(&key.index_key())
        .map(|entry| entry.label.clone());
    let watch_member = watchers.is_watched(key);
    if thread.adopted {
        return FilingResult {
            key: key.clone(),
            note: Some(thread.note.clone()),
            link,
            task_file: Some(thread.task_file.clone()),
            status: "adopted",
            route: thread.route.clone(),
            via: None,
            problem: None,
            first_seen: thread.first_seen,
            watch_member,
        };
    }
    let via = match thread.via.as_str() {
        "route" => Some(enroll::FiledVia::Route),
        "problem" => Some(enroll::FiledVia::Problem),
        _ => Some(enroll::FiledVia::Inbox),
    };
    FilingResult {
        key: key.clone(),
        note: Some(thread.note.clone()),
        link,
        task_file: Some(thread.task_file.clone()),
        status: if dry_run { "would_file" } else { "filed" },
        route: thread.route.clone(),
        via,
        problem: recovery_problem(thread),
        first_seen: thread.first_seen,
        watch_member,
    }
}

/// Snapshot the SHA-256 of each owned path's bytes as sync wrote them. A
/// path that cannot be read keeps an empty hash, which never matches at
/// replay, so it is excluded with a warning instead of committed blind.
fn snapshot_commit_paths(
    vault: &Path,
    paths: &[PathBuf],
) -> Vec<StoredCommitPath> {
    paths
        .iter()
        .map(|path| {
            let sha256 = fs::read(vault.join(path))
                .map(|bytes| save::sha256(&bytes))
                .unwrap_or_default();
            StoredCommitPath {
                path: save::path_to_slashes(path),
                sha256,
            }
        })
        .collect()
}

/// Stored recovery paths that rejoin this run's scoped commit: owned,
/// still-dirty paths whose bytes still match what sync wrote. A changed
/// path is left out with a body-free warning naming it and dropped from
/// the evidence, while its thread's events still finish. Ownership and
/// read failures warn and fail the run instead of committing blind.
fn replay_commit_paths(
    outcome: &mut FilingOutcome,
    vault: &Path,
    child_env: &ob::ChildEnv,
    is_worktree: bool,
    stored: &[StoredCommitPath],
    commit_paths: &[PathBuf],
) -> Vec<PathBuf> {
    let mut replayed = Vec::new();
    for stored in stored {
        let relative = match save::safe_relative_path(&stored.path) {
            Ok(relative) => relative,
            Err(_) => {
                outcome.failed = true;
                outcome.warnings.push(format!(
                    "skipping stored Gmail sync recovery path with an invalid identity: {}",
                    stored.path
                ));
                continue;
            }
        };
        if save::validate_target_path(vault, &relative).is_err() {
            outcome.failed = true;
            outcome.warnings.push(format!(
                "skipping unsafe stored Gmail sync recovery path: {}",
                stored.path
            ));
            continue;
        }
        if relative.as_path() == Path::new("AGENTS.md") {
            continue;
        }
        if !is_worktree {
            continue;
        }
        if commit_paths.contains(&relative) || replayed.contains(&relative) {
            continue;
        }
        let dirty = match save::git_path_is_dirty(vault, child_env, &relative) {
            Ok(dirty) => dirty,
            Err(error) => {
                outcome.failed = true;
                outcome.warnings.push(format!(
                    "could not inspect stored Gmail sync recovery path {}: {error}",
                    stored.path
                ));
                continue;
            }
        };
        if !dirty {
            continue;
        }
        let matches = !stored.sha256.is_empty()
            && fs::read(vault.join(&relative))
                .map(|bytes| save::sha256(&bytes) == stored.sha256)
                .unwrap_or(false);
        if !matches {
            outcome.warnings.push(format!(
                "leaving edited file out of the Gmail sync commit: {}",
                stored.path
            ));
            continue;
        }
        replayed.push(relative);
    }
    replayed
}

/// Evidence for one current-run filing after a scoped-commit failure:
/// every stage stays pending. Adoptions keep only enrollment — they never
/// produce save receipts.
fn new_filed_evidence(
    result: &FilingResult,
    facts: &BTreeMap<(String, String), SaveEventFacts>,
) -> RecoveryThread {
    let adopted = result.status == "adopted";
    let event_facts = facts.get(&result.key.index_key());
    let (created, added, origins) = event_facts
        .map(|event_facts| {
            (
                event_facts.created,
                event_facts.added_count,
                event_facts.origins.clone(),
            )
        })
        .unwrap_or_default();
    RecoveryThread {
        account: result.key.account().to_string(),
        thread_id: result.key.thread_id().to_string(),
        note: result.note.clone().unwrap_or_default(),
        task_file: result.task_file.clone().unwrap_or_default(),
        via: result
            .via
            .map(|via| via.as_str().to_string())
            .unwrap_or_else(|| {
                if adopted {
                    "adopted".to_string()
                } else {
                    "inbox".to_string()
                }
            }),
        route: result.route.clone(),
        created,
        added,
        origins: origins.clone(),
        adopted,
        problem_label: result
            .problem
            .as_ref()
            .map(|problem| problem.label.clone()),
        problem_kind: result
            .problem
            .as_ref()
            .map(|problem| problem.kind.as_str().to_string()),
        problem_message: result
            .problem
            .as_ref()
            .map(|problem| problem.message.clone()),
        problem_hint: result
            .problem
            .as_ref()
            .map(|problem| problem.hint.clone()),
        first_seen: result.first_seen,
        pending_save: !adopted,
        pending_origins: if adopted { Vec::new() } else { origins },
        pending_enrollment: true,
    }
}

/// Evidence for one enrollment event without a result row after a
/// scoped-commit failure.
fn new_event_evidence(
    event: &enroll::EnrollmentEvent,
    facts: &BTreeMap<(String, String), SaveEventFacts>,
) -> RecoveryThread {
    let key = event.key();
    match event {
        enroll::EnrollmentEvent::Adopted {
            note, task_file, ..
        } => RecoveryThread {
            account: key.account().to_string(),
            thread_id: key.thread_id().to_string(),
            note: note.clone(),
            task_file: task_file.clone(),
            via: "adopted".to_string(),
            route: None,
            created: false,
            added: 0,
            origins: Vec::new(),
            adopted: true,
            problem_label: None,
            problem_kind: None,
            problem_message: None,
            problem_hint: None,
            first_seen: false,
            pending_save: false,
            pending_origins: Vec::new(),
            pending_enrollment: true,
        },
        enroll::EnrollmentEvent::Filed {
            note,
            task_file,
            via,
            route,
            ..
        } => {
            let event_facts = facts.get(&key.index_key());
            let (created, added, origins) = event_facts
                .map(|event_facts| {
                    (
                        event_facts.created,
                        event_facts.added_count,
                        event_facts.origins.clone(),
                    )
                })
                .unwrap_or_default();
            RecoveryThread {
                account: key.account().to_string(),
                thread_id: key.thread_id().to_string(),
                note: note.clone(),
                task_file: task_file.clone(),
                via: via.as_str().to_string(),
                route: route.clone(),
                created,
                added,
                origins: origins.clone(),
                adopted: false,
                problem_label: None,
                problem_kind: None,
                problem_message: None,
                problem_hint: None,
                first_seen: false,
                pending_save: true,
                pending_origins: origins,
                pending_enrollment: true,
            }
        }
    }
}

fn destination_for_route(
    vault: &Path,
    inbox: &str,
    verdict: &RouteVerdict,
    context: &RouteContext,
) -> Result<Destination, GmailError> {
    match verdict {
        RouteVerdict::Route(parent) => {
            let Some(current) = context.resolve_route(&parent.route) else {
                return Err(GmailError::runtime(
                    "route_changed",
                    "the routed area or project changed during Gmail preparation".to_string(),
                ));
            };
            if current.route != parent.route {
                return Err(GmailError::runtime(
                    "route_changed",
                    "the routed destination no longer matches the resolved label".to_string(),
                ));
            }
            let relative =
                save::safe_relative_path(&format!("{}.md", parent.route))?;
            save::validate_target_path(vault, &relative)?;
            let metadata = fs::symlink_metadata(vault.join(&relative))
                .map_err(|error| {
                    GmailError::runtime(
                        "route_changed",
                        format!(
                            "the routed destination is unavailable: {error}"
                        ),
                    )
                })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(GmailError::runtime(
                    "refused",
                    "the routed destination is not a regular in-vault note"
                        .to_string(),
                ));
            }
            Ok(Destination {
                relative,
                via: FiledVia::Route,
                route: Some(parent.route.clone()),
                problem: None,
                verdict: RouteVerdict::Route(parent.clone()),
            })
        }
        RouteVerdict::Unrouted => inbox_destination(
            vault,
            inbox,
            FiledVia::Inbox,
            None,
            RouteVerdict::Unrouted,
        ),
        RouteVerdict::Problem(problem) => inbox_destination(
            vault,
            inbox,
            FiledVia::Problem,
            Some(problem.clone()),
            RouteVerdict::Problem(problem.clone()),
        ),
    }
}

fn inbox_destination(
    vault: &Path,
    inbox: &str,
    via: FiledVia,
    problem: Option<RouteProblem>,
    verdict: RouteVerdict,
) -> Result<Destination, GmailError> {
    let relative = save::safe_relative_path(inbox)?;
    save::validate_target_path(vault, &relative)?;
    Ok(Destination {
        relative,
        via,
        route: None,
        problem,
        verdict,
    })
}

fn load_destinations(
    vault: &Path,
    inbox: &str,
    destinations: &BTreeMap<ThreadKey, Destination>,
) -> Result<BTreeMap<PathBuf, DestinationSnapshot>, GmailError> {
    let mut snapshots = BTreeMap::new();
    for destination in destinations.values() {
        if snapshots.contains_key(&destination.relative) {
            continue;
        }
        let target = vault.join(&destination.relative);
        let (original, updated) = match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(GmailError::runtime(
                        "refused",
                        "a task destination is not a regular in-vault note"
                            .to_string(),
                    ));
                }
                let text = fs::read_to_string(&target).map_err(|error| {
                    GmailError::runtime(
                        "write",
                        format!("could not read task destination: {error}"),
                    )
                })?;
                (Some(text.clone()), text)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if destination.relative.as_path() == Path::new(inbox) {
                    (None, inbox_template())
                } else {
                    return Err(GmailError::runtime(
                        "route_changed",
                        "the routed task destination disappeared during filing"
                            .to_string(),
                    ));
                }
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "path",
                    format!("could not inspect a task destination: {error}"),
                ));
            }
        };
        snapshots.insert(
            destination.relative.clone(),
            DestinationSnapshot {
                target,
                original,
                updated,
            },
        );
    }
    Ok(snapshots)
}

fn record_adoption(
    vault: &Path,
    intent: &AdoptionIntent,
    index: &MailIndex,
    outcome: &mut FilingOutcome,
    events: &mut Vec<EnrollmentEvent>,
    recorded: &mut BTreeSet<ThreadKey>,
) {
    let note_path = match save::safe_relative_path(&intent.note) {
        Ok(path) => path,
        Err(error) => {
            outcome.failures.push(FilingFailure {
                key: intent.key.clone(),
                kind: error.kind().to_string(),
                message: error.message().to_string(),
            });
            return;
        }
    };
    let task_path = match save::safe_relative_path(&intent.task_file) {
        Ok(path) => path,
        Err(error) => {
            outcome.failures.push(FilingFailure {
                key: intent.key.clone(),
                kind: error.kind().to_string(),
                message: error.message().to_string(),
            });
            return;
        }
    };
    if let Err(error) = save::validate_target_path(vault, &note_path)
        .and_then(|()| save::validate_target_path(vault, &task_path))
    {
        outcome.failures.push(FilingFailure {
            key: intent.key.clone(),
            kind: error.kind().to_string(),
            message: error.message().to_string(),
        });
        return;
    }
    for relative in [&note_path, &task_path] {
        let exists_as_file = fs::symlink_metadata(vault.join(relative))
            .is_ok_and(|metadata| {
                metadata.is_file() && !metadata.file_type().is_symlink()
            });
        if !exists_as_file {
            outcome.failures.push(FilingFailure {
                key: intent.key.clone(),
                kind: "note_changed".to_string(),
                message:
                    "the adopted mail note or task disappeared during filing"
                        .to_string(),
            });
            return;
        }
    }
    let entry = index.by_identity.get(&intent.key.index_key());
    outcome.results.push(FilingResult {
        key: intent.key.clone(),
        note: Some(intent.note.clone()),
        link: entry.map(|entry| entry.label.clone()),
        task_file: Some(intent.task_file.clone()),
        status: "adopted",
        route: None,
        via: None,
        problem: None,
        first_seen: false,
        watch_member: true,
    });
    if recorded.insert(intent.key.clone()) {
        events.push(intent.event());
    }
}

fn already_enrolled(
    key: &ThreadKey,
    origin: EnrollmentOrigin,
    enrollment: &EnrollmentFold,
    index: &MailIndex,
) -> FilingResult {
    let latest = enrollment.latest(key);
    let entry = index.by_identity.get(&key.index_key());
    FilingResult {
        key: key.clone(),
        note: entry
            .map(|entry| entry.path.clone())
            .or_else(|| latest.and_then(|record| record.note.clone())),
        link: entry.map(|entry| entry.label.clone()),
        task_file: latest.and_then(|record| record.task_file.clone()),
        status: match origin {
            EnrollmentOrigin::Legacy => "already_pulled",
            EnrollmentOrigin::Sync => "already_filed",
        },
        route: latest.and_then(|record| record.route.clone()),
        via: latest.and_then(|record| record.via),
        problem: None,
        first_seen: false,
        watch_member: false,
    }
}

fn path_relative(vault: &Path, absolute: &Path) -> Result<String, GmailError> {
    let relative = absolute.strip_prefix(vault).map_err(|_| {
        GmailError::runtime(
            "refused",
            "a planned mail note escaped the vault".to_string(),
        )
    })?;
    Ok(save::path_to_slashes(relative))
}

pub(crate) fn inbox_template() -> String {
    "---\nparent: \"[[inbox]]\"\ntype: \"[[area]]\"\nready_cap: off\n---\n\n# Gmail inbox\n\n## Tasks\n".to_string()
}

fn task_block(
    link: &str,
    created: &str,
    problem: Option<&RouteProblem>,
) -> String {
    let mut block = capture::format_task_line(link, created, None, None);
    if let Some(problem) = problem {
        block.push_str("\n\t- ⚠️ **GMAIL ROUTE:** `");
        block.push_str(&problem.label.replace('`', "\\`"));
        block.push_str("` ");
        block.push_str(&problem.message);
        if !problem.hint.is_empty() {
            block.push_str(" — ");
            block.push_str(&problem.hint);
        }
    }
    block
}

fn commit_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    is_worktree: bool,
    plan: &AttemptPlan,
    facts: &BTreeMap<(String, String), SaveEventFacts>,
) -> Result<Vec<PathBuf>, GmailError> {
    let mut paths = plan.auxiliary_commit_paths.clone();
    paths.extend(plan.task_commit_paths.iter().cloned());
    for note in &plan.plans {
        let relative = note
            .path
            .strip_prefix(vault)
            .map_err(|_| {
                GmailError::runtime(
                    "refused",
                    "a planned mail note escaped the vault".to_string(),
                )
            })?
            .to_path_buf();
        let dirty = if is_worktree {
            save::git_path_is_dirty(vault, child_env, &relative)
                .map_err(|error| GmailError::runtime("git", error))?
        } else {
            false
        };
        if note.changed || dirty {
            paths.push(relative);
        }
        if let Some(event_facts) =
            facts.get(&(note.account.clone(), note.thread_id.clone()))
        {
            for (saved, _, _) in &event_facts.origins {
                let relative = save::safe_relative_path(saved)?;
                if is_worktree
                    && save::git_path_is_dirty(vault, child_env, &relative)
                        .map_err(|error| GmailError::runtime("git", error))?
                {
                    paths.push(relative);
                }
            }
        }
    }

    // A previous commit may have failed after durable writes. Include only
    // this transaction's adoption targets when they remain dirty.
    if is_worktree {
        for result in &plan.outcome.results {
            if result.status != "adopted" {
                continue;
            }
            for relative in
                [result.note.as_deref(), result.task_file.as_deref()]
                    .into_iter()
                    .flatten()
            {
                let relative = save::safe_relative_path(relative)?;
                if save::git_path_is_dirty(vault, child_env, &relative)
                    .map_err(|error| GmailError::runtime("git", error))?
                {
                    paths.push(relative);
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn acquire_vault_lock<S: Sleeper, C: Clock>(
    budget: &SharedBudget,
    sleeper: &S,
    clock: &C,
) -> Result<Option<std::fs::File>, GmailError> {
    loop {
        match ob::try_acquire_lock() {
            Ok(file) => return Ok(Some(file)),
            Err(ob::LockAcquireError::Contended) => {
                if super::retry::sleep_within_budget(
                    sleeper,
                    budget,
                    clock,
                    Duration::from_millis(100),
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Err(ob::LockAcquireError::Open { path, error }) => {
                return Err(GmailError::runtime(
                    "lock",
                    format!(
                        "could not open the vault lock {}: {error}",
                        path.display()
                    ),
                ));
            }
            Err(ob::LockAcquireError::Acquire { path, error }) => {
                return Err(GmailError::runtime(
                    "lock",
                    format!(
                        "could not acquire the vault lock {}: {error}",
                        path.display()
                    ),
                ));
            }
        }
    }
}

fn candidate_key(candidate: &FilingCandidate) -> Result<ThreadKey, GmailError> {
    ThreadKey::new(
        &candidate.save.confirmed.identity.account,
        &candidate.save.confirmed.identity.thread_id,
    )
    .ok_or_else(|| {
        GmailError::runtime(
            "unresolvable",
            "the prepared Gmail candidate has an invalid identity".to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, process::Command, time::Duration};

    use crate::native::{
        config::GmailBodyMode,
        env::TestEnvGuard,
        gmail::{
            model::GmailThread,
            prepare,
            resolve::{ConfirmedThread, GmailRead, ResolvedIdentity},
        },
        parent_notes,
    };

    struct NoRemote;

    impl GmailRead for NoRemote {
        fn attachment_bytes(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, GmailError> {
            Err(unexpected_remote())
        }

        fn thread_get(
            &self,
            _: &str,
            _: &str,
        ) -> Result<GmailThread, GmailError> {
            Err(unexpected_remote())
        }

        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            Err(unexpected_remote())
        }

        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            Err(unexpected_remote())
        }

        fn labels_list(
            &self,
            _: &str,
        ) -> Result<Vec<super::super::super::gog::GmailLabel>, GmailError>
        {
            Err(unexpected_remote())
        }
    }

    fn unexpected_remote() -> GmailError {
        GmailError::runtime(
            "test",
            "filing attempted an unexpected Gmail request".to_string(),
        )
    }

    struct NoSleep;

    impl Sleeper for NoSleep {
        fn sleep(&self, _: Duration) {}
    }

    /// Advances on every sleep so lock-wait accounting is deterministic.
    struct TestClock {
        now: std::cell::RefCell<std::time::Instant>,
        sleeps: std::cell::RefCell<Vec<Duration>>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                now: std::cell::RefCell::new(std::time::Instant::now()),
                sleeps: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> std::time::Instant {
            *self.now.borrow()
        }
    }

    impl Sleeper for TestClock {
        fn sleep(&self, duration: Duration) {
            self.sleeps.borrow_mut().push(duration);
            *self.now.borrow_mut() += duration;
        }
    }

    fn test_env(root: &Path) -> TestEnvGuard {
        let lock = root.join("maintenance.lock");
        TestEnvGuard::set(&[
            ("BOB_NOW", Some(OsStr::new("2026-11-07 12:00:00"))),
            ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
        ])
    }

    /// Isolated `$XDG_STATE_HOME` so recovery-evidence tests never touch
    /// real host state and never collide across vaults.
    fn isolated_state() -> (tempfile::TempDir, TestEnvGuard) {
        let temp = tempfile::tempdir().expect("temporary state home");
        let guard = TestEnvGuard::set(&[(
            "XDG_STATE_HOME",
            Some(temp.path().as_os_str()),
        )]);
        (temp, guard)
    }

    const TEST_ACCOUNT: &str = "me@example.com";

    fn evidence_path(vault: &Path) -> PathBuf {
        recovery_path(vault, TEST_ACCOUNT)
    }

    fn read_evidence(vault: &Path) -> serde_json::Value {
        let bytes = fs::read(evidence_path(vault)).expect("evidence exists");
        serde_json::from_slice(&bytes).expect("evidence parses")
    }

    fn journal_lines(vault: &Path) -> Vec<String> {
        let path = vault.join(format!(
            ".bob/gmail/{}.jsonl",
            crate::native::vault_sync::hostname_slug()
        ));
        let Ok(text) = fs::read_to_string(&path) else {
            return Vec::new();
        };
        text.lines().map(str::to_string).collect()
    }

    fn count_journal(vault: &Path, kind: &str, thread: &str) -> usize {
        let needle = format!("\"kind\":\"{kind}\"");
        journal_lines(vault)
            .iter()
            .filter(|line| line.contains(&needle) && line.contains(thread))
            .count()
    }

    fn attachment_event_count(vault: &Path) -> usize {
        let path = vault.join(format!(
            ".bob/attachments/{}.jsonl",
            crate::native::vault_sync::hostname_slug()
        ));
        let Ok(text) = fs::read_to_string(&path) else {
            return 0;
        };
        text.lines()
            .filter(|line| line.contains("attachment.import"))
            .count()
    }

    fn count_attachment(vault: &Path, saved: &str) -> usize {
        let path = vault.join(format!(
            ".bob/attachments/{}.jsonl",
            crate::native::vault_sync::hostname_slug()
        ));
        let Ok(text) = fs::read_to_string(&path) else {
            return 0;
        };
        text.lines().filter(|line| line.contains(saved)).count()
    }

    fn reset_recovery_io() {
        super::RECOVERY_IO
            .with(|io| *io.borrow_mut() = super::RecoveryIo::default());
    }

    fn fail_next_recovery_writes(count: usize) {
        super::RECOVERY_IO.with(|io| io.borrow_mut().fail_writes = count);
    }

    fn succeed_then_fail_recovery_writes(succeed: usize, fail: usize) {
        super::RECOVERY_IO.with(|io| {
            let mut io = io.borrow_mut();
            io.succeed_writes = succeed;
            io.fail_writes = fail;
        });
    }

    fn fail_next_recovery_clears(count: usize) {
        super::RECOVERY_IO.with(|io| io.borrow_mut().fail_clears = count);
    }

    fn public_report(
        vault: &Path,
        outcome: FilingOutcome,
    ) -> crate::native::gmail::sync::report::SyncReport {
        let fold = EnrollmentFold::load(vault).unwrap();
        crate::native::gmail::sync::report::from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            TEST_ACCOUNT,
            &fold,
        )
    }

    fn try_apply(
        vault: &Path,
        candidates: Vec<FilingCandidate>,
        adoptions: &[AdoptionIntent],
        dry_run: bool,
    ) -> Result<FilingOutcome, GmailError> {
        let fold = EnrollmentFold::load(vault).unwrap();
        let index = resolve::scan_index(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
        )
        .unwrap();
        let watchers =
            scan::scan_watching_tasks(vault, Path::new("mail"), &index)
                .unwrap();
        // Production retries lock contention within the wait budget
        // instead of single-attempting it: the per-test vault lock is
        // never legitimately contended, so a transient denial waits out
        // a short poll and retries instead of failing the test.
        let budget = SharedBudget::new(Duration::from_secs(10));
        apply(
            FilingInput {
                vault,
                account: TEST_ACCOUNT,
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates,
                adoptions,
                enrollment: &fold,
                watchers: &watchers,
                dry_run,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
                watches: Vec::new(),
                watch_failures: Vec::new(),
                watch_deferred: Vec::new(),
                reply_wait_days: 3,
            },
            &crate::native::gmail::sync::retry::SystemSleeper,
            &crate::native::gmail::sync::retry::SystemClock,
        )
    }

    fn init_git_vault(vault: &Path) {
        git(vault, &["init", "-q"]);
        git(vault, &["config", "user.name", "Sync Test"]);
        git(vault, &["config", "user.email", "sync@example.test"]);
    }

    fn install_failing_hook(vault: &Path) {
        let hook = vault.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = fs::metadata(&hook).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&hook, permissions).unwrap();
        }
    }

    /// A filing candidate whose thread carries an inline image, so the
    /// shared save planner installs a real import and origin event for it.
    /// Small attachments ride inline in the MIME payload and need no fetch.
    fn candidate_with_image(
        thread_id: &str,
        route: RouteVerdict,
    ) -> FilingCandidate {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let bytes = vec![0x41u8; 100];
        let html = r#"<p>See <img src="cid:a" alt="A"></p>"#;
        let envelope = serde_json::json!({
            "thread": {
                "id": thread_id,
                "historyId": "1",
                "messages": [{
                    "id": format!("e{thread_id}"),
                    "threadId": thread_id,
                    "labelIds": ["INBOX"],
                    "internalDate": "1794070800000",
                    "snippet": "A short preview",
                    "payload": {
                        "mimeType": "multipart/related",
                        "headers": [
                            {"name": "From", "value": "Ada Example <ada@example.com>"},
                            {"name": "Subject", "value": "Lease renewal"},
                        ],
                        "parts": [
                            {
                                "partId": "body",
                                "mimeType": "text/html",
                                "headers": [{"name": "Content-Type", "value": "text/html; charset=utf-8"}],
                                "body": {"size": html.len(), "data": URL_SAFE_NO_PAD.encode(html.as_bytes())},
                            },
                            {
                                "partId": "img-<a>",
                                "mimeType": "image/png",
                                "headers": [{"name": "Content-ID", "value": "<a>"}],
                                "filename": "photo.png",
                                "body": {"size": bytes.len(), "data": URL_SAFE_NO_PAD.encode(&bytes)},
                            },
                        ],
                    },
                }],
            },
        });
        let thread =
            GmailThread::from_gog_json(envelope.to_string().as_bytes())
                .expect("valid test thread");
        let confirmed = ConfirmedThread {
            identity: ResolvedIdentity {
                account: TEST_ACCOUNT.to_string(),
                thread_id: thread_id.to_string(),
                known_path: None,
            },
            thread,
        };
        let prepared = prepare::prepare_thread(
            &NoRemote,
            TEST_ACCOUNT,
            &confirmed.thread,
            GmailBodyMode::Full,
        )
        .expect("inline image attachments need no fetch");
        assert!(
            !prepared.imports.is_empty() && !prepared.origins.is_empty(),
            "the image fixture must produce an import and an origin"
        );
        FilingCandidate {
            save: PreparedSave {
                confirmed,
                prepared,
                body_mode: GmailBodyMode::Full,
            },
            route,
        }
    }

    fn candidate_with_two_images(
        thread_id: &str,
        route: RouteVerdict,
    ) -> FilingCandidate {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let bytes_a = vec![0x41u8; 100];
        let bytes_b = vec![0x42u8; 100];
        let html = r#"<p>See <img src="cid:a" alt="A"> and <img src="cid:b" alt="B"></p>"#;
        let envelope = serde_json::json!({
            "thread": {
                "id": thread_id,
                "historyId": "1",
                "messages": [{
                    "id": format!("e{thread_id}"),
                    "threadId": thread_id,
                    "labelIds": ["INBOX"],
                    "internalDate": "1794070800000",
                    "snippet": "A short preview",
                    "payload": {
                        "mimeType": "multipart/related",
                        "headers": [
                            {"name": "From", "value": "Ada Example <ada@example.com>"},
                            {"name": "Subject", "value": "Lease renewal"},
                        ],
                        "parts": [
                            {
                                "partId": "body",
                                "mimeType": "text/html",
                                "headers": [{"name": "Content-Type", "value": "text/html; charset=utf-8"}],
                                "body": {"size": html.len(), "data": URL_SAFE_NO_PAD.encode(html.as_bytes())},
                            },
                            {
                                "partId": "img-<a>",
                                "mimeType": "image/png",
                                "headers": [{"name": "Content-ID", "value": "<a>"}],
                                "filename": "photo-a.png",
                                "body": {"size": bytes_a.len(), "data": URL_SAFE_NO_PAD.encode(&bytes_a)},
                            },
                            {
                                "partId": "img-<b>",
                                "mimeType": "image/png",
                                "headers": [{"name": "Content-ID", "value": "<b>"}],
                                "filename": "photo-b.png",
                                "body": {"size": bytes_b.len(), "data": URL_SAFE_NO_PAD.encode(&bytes_b)},
                            },
                        ],
                    },
                }],
            },
        });
        let thread =
            GmailThread::from_gog_json(envelope.to_string().as_bytes())
                .expect("valid test thread");
        let confirmed = ConfirmedThread {
            identity: ResolvedIdentity {
                account: TEST_ACCOUNT.to_string(),
                thread_id: thread_id.to_string(),
                known_path: None,
            },
            thread,
        };
        let prepared = prepare::prepare_thread(
            &NoRemote,
            TEST_ACCOUNT,
            &confirmed.thread,
            GmailBodyMode::Full,
        )
        .expect("inline image attachments need no fetch");
        assert_eq!(
            prepared.origins.len(),
            2,
            "the two-image fixture must produce two origins"
        );
        FilingCandidate {
            save: PreparedSave {
                confirmed,
                prepared,
                body_mode: GmailBodyMode::Full,
            },
            route,
        }
    }

    /// Block one journal namespace by replacing its directory with a
    /// regular file, so every append fails deterministically (even as
    /// root, where read-only directories still allow writes).
    fn block_journal(vault: &Path, namespace: &str) {
        let dir = vault.join(".bob").join(namespace);
        fs::create_dir_all(vault.join(".bob")).unwrap();
        // A prior phase may have created the namespace directory already.
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_file(&dir);
        fs::write(&dir, "blocked").unwrap();
    }

    /// Every non-Git file under the vault with its bytes, so dry-run
    /// tests can prove the tree is untouched.
    fn snapshot_tree(vault: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![vault.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.file_name().is_some_and(|name| name == ".git") {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(vault) {
                    out.insert(
                        rel.display().to_string(),
                        fs::read(&path).unwrap_or_default(),
                    );
                }
            }
        }
        out
    }

    fn count_mail_notes(vault: &Path) -> usize {
        fn walk(dir: &Path, count: &mut usize) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, count);
                } else if path.extension().is_some_and(|ext| ext == "md") {
                    *count += 1;
                }
            }
        }
        let mut count = 0;
        walk(&vault.join("mail"), &mut count);
        count
    }

    fn candidate(thread_id: &str, route: RouteVerdict) -> FilingCandidate {
        let thread = GmailThread::from_gog_json(
            format!(
                r#"{{"thread":{{"id":"{thread_id}","messages":[{{"id":"e{thread_id}","threadId":"{thread_id}","internalDate":"1794070800000","snippet":"A short preview","payload":{{"mimeType":"text/plain","headers":[{{"name":"From","value":"Ada Example <ada@example.com>"}},{{"name":"Subject","value":"Lease renewal"}}],"body":{{"size":5,"data":"SGVsbG8="}}}}}}]}}}}"#
            )
            .as_bytes(),
        )
        .expect("valid test thread");
        let confirmed = ConfirmedThread {
            identity: ResolvedIdentity {
                account: "me@example.com".to_string(),
                thread_id: thread_id.to_string(),
                known_path: None,
            },
            thread,
        };
        let prepared = prepare::prepare_thread(
            &NoRemote,
            "me@example.com",
            &confirmed.thread,
            GmailBodyMode::Headers,
        )
        .expect("headers mode does not fetch bodies");
        FilingCandidate {
            save: PreparedSave {
                confirmed,
                prepared,
                body_mode: GmailBodyMode::Headers,
            },
            route,
        }
    }

    fn area(vault: &Path, route: &str, headings: &str) {
        fs::write(
            vault.join(format!("{route}.md")),
            format!("---\ntype: \"[[area]]\"\n---\n\n# {route}\n\n{headings}"),
        )
        .expect("write area note");
    }

    fn git(vault: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn route(vault: &Path, name: &str) -> RouteVerdict {
        RouteVerdict::Route(
            RouteContext::load(vault)
                .resolve_route(name)
                .expect("area route resolves"),
        )
    }

    fn apply_one(
        vault: &Path,
        candidate: FilingCandidate,
        fold: &EnrollmentFold,
        watchers: &WatchScan,
        dry_run: bool,
    ) -> FilingOutcome {
        let budget = SharedBudget::new(Duration::ZERO);
        apply(
            FilingInput {
                vault,
                account: "me@example.com",
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates: vec![candidate],
                adoptions: &[],
                enrollment: fold,
                watchers,
                dry_run,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
                watches: Vec::new(),
                watch_failures: Vec::new(),
                watch_deferred: Vec::new(),
                reply_wait_days: 3,
            },
            &NoSleep,
            &crate::native::gmail::sync::retry::SystemClock,
        )
        .expect("filing succeeds")
    }

    #[test]
    fn routed_task_lands_in_ready_block_and_mail_note_uses_route_parent() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(
            vault,
            "job",
            "## Tasks\n\n### Next & In Progress\n\n### Done & Canceled\n",
        );
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            false,
        );

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].status, "filed");
        assert_eq!(outcome.results[0].task_file.as_deref(), Some("job.md"));
        let task_note = fs::read_to_string(vault.join("job.md")).unwrap();
        let task_position = task_note.find("- [ ] #task").unwrap();
        let next_position = task_note.find("### Next & In Progress").unwrap();
        assert!(task_position < next_position, "{task_note}");
        assert!(task_note.contains("[created::2026-11-07]"), "{task_note}");
        assert!(!task_note.contains("[fresh::"));

        let mail_path = outcome.results[0].note.as_ref().unwrap();
        let mail_note = fs::read_to_string(vault.join(mail_path)).unwrap();
        assert!(
            mail_note.starts_with("---\nparent: '[[job]]'\n"),
            "{mail_note}"
        );
        let journal = fs::read_to_string(vault.join(format!(
            ".bob/gmail/{}.jsonl",
            crate::native::vault_sync::hostname_slug()
        )))
        .unwrap();
        assert!(journal.contains("gmail.filed"));
    }

    #[test]
    fn problem_filing_creates_inbox_and_managed_first_child() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let problem = RouteProblem {
            kind: crate::native::gmail::route::RouteProblemKind::UnknownNote,
            label: "#/missing".to_string(),
            message: "unknown note".to_string(),
            hint: "create the note or change the label".to_string(),
        };
        let outcome = apply_one(
            vault,
            candidate("ccdd", RouteVerdict::Problem(problem)),
            &fold,
            &watchers,
            false,
        );

        let result = &outcome.results[0];
        assert_eq!(result.via, Some(FiledVia::Problem));
        assert!(result.first_seen);
        let inbox = fs::read_to_string(vault.join("gmail_inbox.md")).unwrap();
        assert!(inbox.contains("parent: \"[[inbox]]\""));
        assert!(inbox.contains("type: \"[[area]]\""));
        assert!(inbox.contains("ready_cap: off"));
        assert!(inbox.contains(
            "\n\t- ⚠️ **GMAIL ROUTE:** `#/missing` unknown note — create the note or change the label"
        ));
    }

    #[test]
    fn one_batch_routes_threads_to_different_parents_and_allocates_collisions()
    {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        area(vault, "body", "## Tasks\n");
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let budget = SharedBudget::new(Duration::ZERO);
        let candidates = vec![
            candidate("aabb", route(vault, "job")),
            candidate("ccdd", route(vault, "body")),
        ];
        let outcome = apply(
            FilingInput {
                vault,
                account: "me@example.com",
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates,
                adoptions: &[],
                enrollment: &fold,
                watchers: &watchers,
                dry_run: false,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
                watches: Vec::new(),
                watch_failures: Vec::new(),
                watch_deferred: Vec::new(),
                reply_wait_days: 3,
            },
            &NoSleep,
            &crate::native::gmail::sync::retry::SystemClock,
        )
        .expect("batch files");

        assert_eq!(outcome.results.len(), 2);
        let note_paths: BTreeSet<_> = outcome
            .results
            .iter()
            .map(|result| result.note.as_ref().unwrap().clone())
            .collect();
        assert_eq!(note_paths.len(), 2, "same-title notes need distinct paths");
        let parents: BTreeSet<_> = note_paths
            .iter()
            .map(|relative| {
                let note = fs::read_to_string(vault.join(relative)).unwrap();
                assert!(note.contains("parent:"), "{note}");
                if note.starts_with("---\nparent: '[[job]]'") {
                    "job"
                } else if note.starts_with("---\nparent: '[[body]]'") {
                    "body"
                } else {
                    panic!("unexpected note parent:\n{note}")
                }
            })
            .collect();
        assert_eq!(
            parents,
            BTreeSet::from(["body", "job"]),
            "notes: {note_paths:?}"
        );
    }

    #[test]
    fn refreshing_an_existing_note_preserves_its_parent_notes_and_reply() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        area(vault, "body", "## Tasks\n");
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            false,
        );
        let note_path = outcome.results[0].note.as_ref().unwrap();
        let absolute = vault.join(note_path);
        let mut contents = fs::read_to_string(&absolute).unwrap();
        contents = contents
            .replace("parent: '[[job]]'", "parent: '[[custom_parent]]'");
        contents = contents.replace(
            "## Notes\n\n## Conversation",
            "## Notes\n\nKeep this note.\n\n## Conversation",
        );
        contents.push_str("\n## Reply\n\nDraft reply stays here.\n");
        fs::write(&absolute, contents).unwrap();

        let index = resolve::scan_index(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
        )
        .unwrap();
        let mut candidate = candidate("aabb", route(vault, "body"));
        candidate.save.confirmed.identity.known_path = Some(note_path.clone());
        let key = ThreadKey::new("me@example.com", "aabb").unwrap();
        let parent_links =
            BTreeMap::from([(key.index_key(), "[[body]]".to_string())]);
        let mut works = BTreeMap::from([(key.index_key(), candidate.save)]);
        let plans = save::plan_prepared_saves_with_parent_links(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
            &mut works,
            &index,
            None,
            &parent_links,
            false,
            Utc::now(),
            chrono_tz::UTC,
        )
        .unwrap();
        let updated = &plans[0].updated;
        assert!(updated.starts_with("---\nparent: '[[custom_parent]]'\n"));
        assert!(updated.contains("Keep this note."));
        assert!(updated.contains("## Reply\n\nDraft reply stays here."));
    }

    #[test]
    fn git_apply_uses_one_sync_commit_and_leaves_unrelated_dirt_alone() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        git(vault, &["init", "-q"]);
        git(vault, &["config", "user.name", "Sync Test"]);
        git(vault, &["config", "user.email", "sync@example.test"]);
        git(vault, &["add", "job.md"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        fs::write(
            vault.join("unrelated.md"),
            "leave this outside the sync commit\n",
        )
        .unwrap();

        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            false,
        );
        assert_eq!(outcome.results[0].status, "filed");
        assert_eq!(
            git(vault, &["log", "-1", "--pretty=%s"]),
            "gmail: sync 1 threads"
        );
        assert_eq!(
            git(vault, &["status", "--porcelain", "--", "unrelated.md"]),
            "?? unrelated.md"
        );
    }

    #[test]
    fn dry_run_plans_filing_without_creating_files_or_directories() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            true,
        );
        assert_eq!(outcome.results[0].status, "would_file");
        assert!(!vault.join("mail").exists());
        assert!(!vault.join("gmail_inbox.md").exists());
        assert!(!vault.join(".bob").exists());
    }

    #[test]
    fn an_adoption_closed_during_fetch_is_deferred_without_filing() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        let key = ThreadKey::new("me@example.com", "aabb").unwrap();
        let stale = AdoptionIntent {
            key: key.clone(),
            note: "mail/2026/lease.md".to_string(),
            task_file: "job.md".to_string(),
            task_line: 1,
            watchers: 1,
        };
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let budget = SharedBudget::new(Duration::ZERO);
        let outcome = apply(
            FilingInput {
                vault,
                account: "me@example.com",
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates: Vec::new(),
                adoptions: std::slice::from_ref(&stale),
                enrollment: &fold,
                watchers: &watchers,
                dry_run: false,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
                watches: Vec::new(),
                watch_failures: Vec::new(),
                watch_deferred: Vec::new(),
                reply_wait_days: 3,
            },
            &NoSleep,
            &crate::native::gmail::sync::retry::SystemClock,
        )
        .expect("stale adoption is deferred");

        assert_eq!(outcome.deferred, vec![key]);
        assert!(outcome.results.is_empty());
        assert!(!vault.join(".bob").exists());
    }

    #[test]
    fn route_problem_child_and_ready_placement_are_stable() {
        let problem = RouteProblem {
            kind: crate::native::gmail::route::RouteProblemKind::UnknownNote,
            label: "#/unknown".to_string(),
            message: "unknown route".to_string(),
            hint: "use one label per note".to_string(),
        };
        let block = task_block(
            "[[mail/lease|✉️ Lease · Ada]]",
            "2026-11-07",
            Some(&problem),
        );
        let original = "## Tasks\n\n### Next & In Progress\n- [ ] #task Active\n\n### Done & Canceled\n- [x] #task Finished\n";
        let updated = capture::insert_task_line(original, &block).0;
        let row = updated.find("- [ ] #task [[mail/lease").unwrap();
        let child = updated.find("\n\t- ⚠️ **GMAIL ROUTE:**").unwrap();
        let next = updated.find("### Next & In Progress").unwrap();
        let done = updated.find("### Done & Canceled").unwrap();
        assert!(row < child && child < next && next < done, "{updated}");
        assert!(updated.contains("[created::2026-11-07]"));
        assert!(!updated.contains("[fresh::"));
    }

    #[test]
    fn missing_inbox_template_keeps_the_capture_parent_and_ready_cap() {
        let template = inbox_template();
        assert!(template.contains("parent: \"[[inbox]]\""));
        assert!(template.contains("type: \"[[area]]\""));
        assert!(template.contains("ready_cap: off"));
        assert!(template.ends_with("## Tasks\n"));
    }

    #[test]
    fn route_context_still_uses_parent_note_eligibility() {
        let temp = tempfile::tempdir().expect("temporary vault");
        area(temp.path(), "job", "## Tasks\n");
        let resolved = parent_notes::resolve_parent(temp.path(), "job")
            .expect("area resolves as a capture parent");
        assert_eq!(resolved.route, "job");
    }

    #[test]
    fn vault_lock_wait_consumes_the_shared_wait_budget() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        let _held =
            crate::native::ob::try_acquire_lock().expect("hold vault lock");
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let budget = SharedBudget::new(Duration::from_millis(250));
        let original = budget.remaining();
        let clock = TestClock::new();
        let outcome = apply(
            FilingInput {
                vault,
                account: "me@example.com",
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates: Vec::new(),
                adoptions: &[],
                enrollment: &fold,
                watchers: &watchers,
                dry_run: false,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
                watches: Vec::new(),
                watch_failures: Vec::new(),
                watch_deferred: Vec::new(),
                reply_wait_days: 3,
            },
            &clock,
            &clock,
        )
        .expect("busy skip");
        assert!(outcome.busy);
        assert!(!outcome.applied);
        assert!(
            budget.remaining() < original,
            "lock wait draws from the wait budget: {:?} vs {:?}",
            budget.remaining(),
            original
        );
        assert!(
            budget.is_exhausted(),
            "250ms budget is consumed by 100ms lock-wait polls"
        );
        assert!(
            !clock.sleeps.borrow().is_empty(),
            "the vault-lock wait slept against the test clock"
        );
    }

    #[test]
    fn recovery_evidence_path_is_host_local_and_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let (_state, _env) = isolated_state();
        assert_eq!(
            recovery_presence(vault, TEST_ACCOUNT),
            RecoveryPresence::Absent
        );
        let first = recovery_path(vault, "Me@Example.com ");
        assert_eq!(first, recovery_path(vault, "me@example.com"));
        assert_ne!(first, recovery_path(vault, "other@example.com"));
        let other_vault = tempfile::tempdir().unwrap();
        assert_ne!(first, recovery_path(other_vault.path(), TEST_ACCOUNT));
        let name = first.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("sync-recovery-") && name.ends_with(".json"),
            "{name}"
        );
        assert!(
            !first.to_string_lossy().contains('@'),
            "never a raw email path: {}",
            first.display()
        );
        assert_eq!(
            first.parent().unwrap(),
            bob_env::state_home().join("bob/gmail").as_path(),
        );
    }

    #[test]
    fn commit_failure_then_replay_files_once() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(
            vault,
            "job",
            "## Tasks\n\n### Next & In Progress\n\n### Done & Canceled\n",
        );
        // A customized hub the scoped commit must never touch.
        fs::write(vault.join("mail.md"), "customized hub\n").unwrap();
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        fs::write(vault.join("unrelated.md"), "leave this alone\n").unwrap();
        install_failing_hook(vault);

        let error = try_apply(
            vault,
            vec![candidate_with_image("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("a failing pre-commit hook fails the run");
        assert_eq!(error.kind(), "uncommitted");
        // No enrollment is recorded on a commit failure.
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 0);
        assert_eq!(count_journal(vault, "gmail.adopted", "aabb"), 0);
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 0);
        // The evidence keeps every stage pending with hashed owned paths.
        let evidence = read_evidence(vault);
        let threads = evidence["threads"].as_array().unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0]["pending_save"], true);
        assert_eq!(threads[0]["pending_enrollment"], true);
        assert!(!threads[0]["pending_origins"].as_array().unwrap().is_empty());
        let stored = evidence["commit_paths"].as_array().unwrap();
        assert!(!stored.is_empty(), "{evidence}");
        for path in stored {
            assert!(!path["sha256"].as_str().unwrap().is_empty(), "{path}");
        }

        // Replay after removing the hook: one task, one card set, one
        // commit, and each event exactly once.
        fs::remove_file(vault.join(".git/hooks/pre-commit")).unwrap();
        let outcome = try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("replay succeeds");
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].status, "filed");
        let link = outcome.results[0].link.as_ref().unwrap();
        assert!(link.contains("Lease renewal"), "{link}");
        assert_eq!(git(vault, &["log", "--oneline"]).lines().count(), 2);
        assert_eq!(
            git(vault, &["log", "-1", "--pretty=%s"]),
            "gmail: sync 1 threads"
        );
        let committed =
            git(vault, &["show", "--name-only", "--pretty=format:", "HEAD"]);
        let note = outcome.results[0].note.as_ref().unwrap();
        assert!(committed.contains(note), "{committed}");
        assert!(committed.contains("job.md"), "{committed}");
        assert!(committed.contains("img/mail/aabb/photo.png"), "{committed}");
        assert!(committed.contains("mail.base"), "{committed}");
        assert!(!committed.contains("unrelated.md"), "{committed}");
        assert!(!committed.contains("mail.md"), "{committed}");
        assert!(!committed.contains("AGENTS.md"), "{committed}");
        let tasks = fs::read_to_string(vault.join("job.md")).unwrap();
        assert_eq!(tasks.matches("✉️").count(), 1, "{tasks}");
        assert_eq!(
            git(vault, &["status", "--porcelain", "--", "unrelated.md"]),
            "?? unrelated.md"
        );
        assert_eq!(
            fs::read_to_string(vault.join("mail.md")).unwrap(),
            "customized hub\n"
        );
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert_eq!(count_attachment(vault, "img/mail/aabb/photo.png"), 1);
        assert!(
            !evidence_path(vault).exists(),
            "empty evidence is deleted, never written"
        );

        // A further replay files nothing and appends nothing.
        let outcome = try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("idempotent replay succeeds");
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].status, "already_filed");
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert_eq!(count_attachment(vault, "img/mail/aabb/photo.png"), 1);
        assert_eq!(git(vault, &["log", "--oneline"]).lines().count(), 2);
    }

    #[test]
    fn replay_without_candidates_still_finishes_filing() {
        // The label removed from search, or `-l 0`: no candidates reach
        // apply, but the pending evidence still replays.
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        try_apply(
            vault,
            vec![candidate("ccdd", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit must fail");
        fs::remove_file(vault.join(".git/hooks/pre-commit")).unwrap();

        let outcome =
            try_apply(vault, Vec::new(), &[], false).expect("replay succeeds");
        assert_eq!(outcome.results.len(), 1);
        assert!(!evidence_path(vault).exists());
    }

    #[test]
    fn dry_run_projects_recovery_without_touching_state() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        try_apply(
            vault,
            vec![candidate("eeff", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit must fail");
        let before = fs::read(evidence_path(vault)).unwrap();
        let tree = snapshot_tree(vault);

        let outcome =
            try_apply(vault, Vec::new(), &[], true).expect("dry run succeeds");
        assert!(!outcome.failed);
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].status, "would_file");
        assert!(outcome.results[0]
            .link
            .as_ref()
            .unwrap()
            .contains("Lease renewal"));
        // Dry-run never writes or deletes the evidence, the vault tree, or
        // the journals.
        assert_eq!(fs::read(evidence_path(vault)).unwrap(), before);
        assert_eq!(snapshot_tree(vault), tree);
        assert!(!vault.join(".bob").exists());
        assert_eq!(count_journal(vault, "gmail.filed", "eeff"), 0);
    }

    #[test]
    fn closed_task_before_replay_is_not_filed_twice() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit must fail");
        // Close the recovered task before replay while the label remains.
        let tasks = fs::read_to_string(vault.join("job.md")).unwrap();
        assert!(tasks.contains("- [ ] #task"), "{tasks}");
        fs::write(
            vault.join("job.md"),
            tasks.replacen("- [ ] #task", "- [x] #task", 1),
        )
        .unwrap();
        fs::remove_file(vault.join(".git/hooks/pre-commit")).unwrap();

        let outcome = try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("replay succeeds");
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].status, "filed");
        let tasks = fs::read_to_string(vault.join("job.md")).unwrap();
        assert_eq!(tasks.matches("✉️").count(), 1, "{tasks}");
        assert_eq!(count_mail_notes(vault), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        let report = public_report(vault, outcome);
        assert_eq!(
            report.watching, 0,
            "a recovered closed task is not a watcher"
        );
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
    }

    #[test]
    fn deleted_task_before_replay_is_not_watching() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit must fail");
        fs::write(vault.join("job.md"), "## Tasks\n").unwrap();
        fs::remove_file(vault.join(".git/hooks/pre-commit")).unwrap();
        let outcome = try_apply(vault, Vec::new(), &[], false).expect("replay");
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert_eq!(count_mail_notes(vault), 1);
        let report = public_report(vault, outcome);
        assert_eq!(
            report.watching, 0,
            "a recovered deleted task is not a watcher"
        );
    }

    #[test]
    fn edited_owned_path_is_excluded_with_warning_but_events_finish() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit must fail");
        // The user edits the owned mail note after the failure.
        let note = read_evidence(vault)["threads"][0]["note"]
            .as_str()
            .unwrap()
            .to_string();
        fs::write(vault.join(&note), "user-edited after failure\n").unwrap();
        fs::remove_file(vault.join(".git/hooks/pre-commit")).unwrap();

        let outcome = try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("replay succeeds");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains(&note)),
            "{:?}",
            outcome.warnings
        );
        let committed =
            git(vault, &["show", "--name-only", "--pretty=format:", "HEAD"]);
        assert!(!committed.contains(&note), "{committed}");
        assert!(committed.contains("job.md"), "{committed}");
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert!(!evidence_path(vault).exists());
    }

    #[test]
    fn journal_failure_then_replay_appends_each_event_once() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        // Block the Gmail journal: saves, and the enrollment below them,
        // fail while the vault writes still land.
        block_journal(vault, "gmail");

        let outcome = try_apply(
            vault,
            vec![candidate_with_image("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("journal failure fails the run, not the call");
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("journal")),
            "{:?}",
            outcome.warnings
        );
        let evidence = read_evidence(vault);
        assert_eq!(evidence["threads"].as_array().unwrap().len(), 1);
        assert_eq!(evidence["threads"][0]["pending_save"], true);
        assert_eq!(evidence["threads"][0]["pending_enrollment"], true);
        fs::remove_file(vault.join(".bob/gmail")).unwrap();
        let outcome = try_apply(
            vault,
            vec![candidate_with_image("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("replay succeeds");
        assert!(!outcome.failed, "{:?}", outcome.warnings);
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert_eq!(count_attachment(vault, "img/mail/aabb/photo.png"), 1);
        assert!(!evidence_path(vault).exists());

        // A save that succeeded while only origins failed is never
        // re-appended: its replay runs origins and enrollment only.
        block_journal(vault, "attachments");
        let outcome = try_apply(
            vault,
            vec![candidate_with_image("ccdd", route(vault, "job"))],
            &[],
            false,
        )
        .expect("origin failure fails the run, not the call");
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning
                    .contains("could not record imported image")),
            "{:?}",
            outcome.warnings
        );
        assert_eq!(count_journal(vault, "gmail.save", "ccdd"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "ccdd"), 0);
        let evidence = read_evidence(vault);
        assert_eq!(evidence["threads"][0]["pending_save"], false);
        assert!(!evidence["threads"][0]["pending_origins"]
            .as_array()
            .unwrap()
            .is_empty());

        fs::remove_file(vault.join(".bob/attachments")).unwrap();
        let outcome = try_apply(
            vault,
            vec![candidate_with_image("ccdd", route(vault, "job"))],
            &[],
            false,
        )
        .expect("replay succeeds");
        assert!(!outcome.failed, "{:?}", outcome.warnings);
        assert_eq!(count_journal(vault, "gmail.save", "ccdd"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "ccdd"), 1);
        assert_eq!(count_attachment(vault, "img/mail/ccdd/photo.png"), 1);
        assert!(!evidence_path(vault).exists());
    }

    #[test]
    fn adoption_only_journal_failure_replays_without_save_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        fs::create_dir_all(vault.join("mail/2026")).unwrap();
        fs::write(
            vault.join("mail/2026/lease.md"),
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: aabb\ntitle: Lease renewal\nwho: Ada\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            vault.join("job.md"),
            "- [ ] #task Lease\n\t- [[mail/2026/lease]]\n",
        )
        .unwrap();
        // Adoption intents come from the local scan, exactly as the
        // command builds them: no label, no search, just the open task.
        let fold = EnrollmentFold::load(vault).unwrap();
        let index = resolve::scan_index(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
        )
        .unwrap();
        let watchers =
            scan::scan_watching_tasks(vault, Path::new("mail"), &index)
                .unwrap();
        let adoptions =
            Membership::new(&fold, &watchers).adoptions(TEST_ACCOUNT);
        assert_eq!(adoptions.len(), 1);
        block_journal(vault, "gmail");

        let outcome = try_apply(vault, Vec::new(), &adoptions, false)
            .expect("journal failure fails the run, not the call");
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("journal")),
            "{:?}",
            outcome.warnings
        );
        let evidence = read_evidence(vault);
        let threads = evidence["threads"].as_array().unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0]["adopted"], true);
        assert_eq!(threads[0]["pending_save"], false);
        assert_eq!(threads[0]["pending_enrollment"], true);

        fs::remove_file(vault.join(".bob/gmail")).unwrap();
        let outcome = try_apply(vault, Vec::new(), &adoptions, false)
            .expect("replay succeeds");
        assert!(!outcome.failed, "{:?}", outcome.warnings);
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].status, "adopted");
        assert_eq!(count_journal(vault, "gmail.adopted", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 0);
        assert!(!evidence_path(vault).exists());
    }

    #[test]
    fn corrupt_and_invalid_evidence_warn_and_fail_without_panic() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");

        // Corrupt evidence: a warning plus failure, never a panic, and no
        // enrollment is written.
        fs::create_dir_all(evidence_path(vault).parent().unwrap()).unwrap();
        fs::write(evidence_path(vault), b"{not json").unwrap();
        let outcome =
            try_apply(vault, Vec::new(), &[], false).expect("no panic");
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("recovery")),
            "{:?}",
            outcome.warnings
        );
        assert!(journal_lines(vault).is_empty());

        // An invalid stored identity warns and fails the same way.
        let invalid = serde_json::json!({
            "threads": [{
                "account": TEST_ACCOUNT,
                "thread_id": "xyz",
                "note": "mail/2026/x.md",
                "task_file": "job.md",
                "via": "inbox",
                "route": null,
                "created": false,
                "added": 0,
                "origins": [],
                "adopted": false,
                "problem_label": null,
                "problem_kind": null,
                "problem_message": null,
                "problem_hint": null,
                "first_seen": false,
                "pending_save": true,
                "pending_origins": [],
                "pending_enrollment": true,
            }],
            "commit_paths": [],
        });
        fs::write(evidence_path(vault), serde_json::to_vec(&invalid).unwrap())
            .unwrap();
        let outcome =
            try_apply(vault, Vec::new(), &[], false).expect("no panic");
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("invalid identity")),
            "{:?}",
            outcome.warnings
        );
        assert!(journal_lines(vault).is_empty());

        // A clean run writes no evidence file at all. (A corrupt file
        // from the earlier step was already deleted when the failed run
        // persisted its empty pending set.)
        let _ = fs::remove_file(evidence_path(vault));
        let outcome = try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("filing succeeds");
        assert!(!outcome.failed);
        assert!(!evidence_path(vault).exists());
    }

    #[test]
    fn unreadable_evidence_directory_fails_idle_apply() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        fs::create_dir_all(evidence_path(vault)).unwrap();
        assert_eq!(
            recovery_presence(vault, TEST_ACCOUNT),
            RecoveryPresence::Unreadable
        );
        let outcome =
            try_apply(vault, Vec::new(), &[], false).expect("no panic");
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("recovery")),
            "{:?}",
            outcome.warnings
        );
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 0);
    }

    #[test]
    fn commit_failure_includes_injected_evidence_write_error() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        reset_recovery_io();
        fail_next_recovery_writes(1);
        let error = try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit and evidence write both fail");
        reset_recovery_io();
        assert_eq!(error.kind(), "uncommitted");
        assert!(
            error.message().contains("could not persist"),
            "{}",
            error.message()
        );
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 0);
    }

    #[test]
    fn save_checkpoint_failure_skips_the_save_on_replay() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        try_apply(
            vault,
            vec![candidate_with_image("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit must fail");
        fs::remove_file(vault.join(".git/hooks/pre-commit")).unwrap();
        reset_recovery_io();
        fail_next_recovery_writes(8);
        let outcome = try_apply(vault, Vec::new(), &[], false)
            .expect("persist failure fails the run, not the call");
        reset_recovery_io();
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("persist")),
            "{:?}",
            outcome.warnings
        );
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 0);
        assert_eq!(count_attachment(vault, "img/mail/aabb/photo.png"), 0);
        let report = public_report(vault, outcome);
        assert_eq!(
            report.watching, 0,
            "pending save/origin/enrollment is not enrolled watch"
        );
        assert!(evidence_path(vault).exists());

        let outcome =
            try_apply(vault, Vec::new(), &[], false).expect("replay succeeds");
        assert!(!outcome.failed, "{:?}", outcome.warnings);
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert_eq!(count_attachment(vault, "img/mail/aabb/photo.png"), 1);
        assert!(!evidence_path(vault).exists());
        let report = public_report(vault, outcome);
        assert_eq!(report.watching, 1);
        assert_eq!(
            fs::read_to_string(vault.join("job.md"))
                .unwrap()
                .matches("✉️")
                .count(),
            1
        );
    }

    #[test]
    fn two_origins_checkpoint_after_the_first_then_replay() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        init_git_vault(vault);
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        install_failing_hook(vault);
        try_apply(
            vault,
            vec![candidate_with_two_images("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect_err("commit must fail");
        fs::remove_file(vault.join(".git/hooks/pre-commit")).unwrap();
        reset_recovery_io();
        // Save checkpoint succeeds; the first origin checkpoint fails.
        succeed_then_fail_recovery_writes(1, 8);
        let outcome = try_apply(vault, Vec::new(), &[], false)
            .expect("origin checkpoint failure fails the run");
        reset_recovery_io();
        assert!(outcome.failed);
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 0);
        let origin_count = attachment_event_count(vault);
        assert_eq!(
            origin_count, 1,
            "exactly one origin landed before the interrupt: {:?}",
            outcome.warnings
        );
        let evidence = read_evidence(vault);
        assert!(!evidence["threads"][0]["pending_origins"]
            .as_array()
            .unwrap()
            .is_empty());
        let report = public_report(vault, outcome);
        assert_eq!(report.watching, 0);

        let outcome =
            try_apply(vault, Vec::new(), &[], false).expect("replay succeeds");
        assert!(!outcome.failed, "{:?}", outcome.warnings);
        assert_eq!(count_journal(vault, "gmail.save", "aabb"), 1);
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert_eq!(attachment_event_count(vault), 2);
        assert!(!evidence_path(vault).exists());
    }

    #[test]
    fn failed_final_evidence_deletion_is_retried() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        reset_recovery_io();
        fail_next_recovery_clears(2);
        let outcome = try_apply(
            vault,
            vec![candidate("aabb", route(vault, "job"))],
            &[],
            false,
        )
        .expect("clear failure fails the run, not the call");
        reset_recovery_io();
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("persist")),
            "{:?}",
            outcome.warnings
        );
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert!(evidence_path(vault).exists());
        let outcome =
            try_apply(vault, Vec::new(), &[], false).expect("cleanup replay");
        assert!(!outcome.failed, "{:?}", outcome.warnings);
        assert!(!evidence_path(vault).exists());
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 1);
        assert_eq!(
            fs::read_to_string(vault.join("job.md"))
                .unwrap()
                .matches("✉️")
                .count(),
            1
        );
    }

    #[test]
    fn image_rollback_on_failed_text_batch() {
        // `save::write_prepared_batch` removes only the images this
        // attempt created when the text batch fails, preserving the save
        // transaction's rollback rule.
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        fs::write(vault.join("job.md"), "actual bytes\n").unwrap();
        let mut filed = candidate("aabb", RouteVerdict::Unrouted);
        filed.save.prepared.imports.push(prepare::BinaryImport {
            relative_path: "mail/img/aabb.png".into(),
            bytes: vec![1, 2, 3],
        });
        let mut works = BTreeMap::new();
        works
            .insert((TEST_ACCOUNT.to_string(), "aabb".to_string()), filed.save);
        let plans = vec![StagedSaveNote {
            account: TEST_ACCOUNT.to_string(),
            thread_id: "aabb".to_string(),
            path: vault.join("job.md"),
            link: String::new(),
            title: String::new(),
            who: String::new(),
            message_count: 0,
            added_count: 0,
            created: false,
            changed: true,
            status: "filed",
            original: Some("actual bytes\n".to_string()),
            updated: "new bytes\n".to_string(),
            preview: Vec::new(),
            preview_noop: true,
        }];
        // The disk preimage changed after planning: refuse the batch.
        let staged = vec![StagedTextFile {
            target: vault.join("job.md"),
            target_existed: true,
            original_target: "stale bytes\n".to_string(),
            updated_target: "new bytes\n".to_string(),
        }];
        let error = save::write_prepared_batch(vault, &works, &plans, &staged)
            .expect_err("stale preimage refuses the batch");
        assert_eq!(error.kind(), "note_changed");
        assert!(
            !vault.join("mail/img/aabb.png").exists(),
            "images created by the failed attempt are removed"
        );
        assert_eq!(
            fs::read_to_string(vault.join("job.md")).unwrap(),
            "actual bytes\n"
        );
    }

    #[test]
    fn repeated_snapshot_conflicts_defer_without_filing_twice() {
        // The bounded second snapshot-conflict deferral: two conflicting
        // attempts defer the thread instead of filing it twice.
        use std::sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc::channel,
            Arc,
        };
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        area(vault, "job", "## Tasks\n");
        // Resolve the route before churning starts: the churner rewrites
        // the destination note, which stops being a valid area.
        let filed = candidate("aabb", route(vault, "job"));
        // Mutate the task destination continuously so both filing attempts
        // observe a changed disk preimage. Every write carries a counter so
        // the bytes always differ from any earlier snapshot; the first
        // write is rendezvoused before filing starts so the churner cannot
        // lose the startup race. A drop guard stops the churner on every
        // exit path, including a test panic.
        struct StopOnDrop(Arc<AtomicBool>);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let churn = stop.clone();
        let _stopper = StopOnDrop(stop.clone());
        let (started, first_write) = channel::<()>();
        let tasks = vault.join("job.md");
        let churner = std::thread::spawn(move || {
            use std::io::Write as _;
            let counter = AtomicU64::new(0);
            let mut signaled = false;
            while !churn.load(Ordering::Relaxed) {
                let tick = counter.fetch_add(1, Ordering::Relaxed);
                // Append only: the area frontmatter stays valid for route
                // resolution while the bytes always differ from any earlier
                // snapshot.
                if let Ok(mut file) =
                    std::fs::OpenOptions::new().append(true).open(&tasks)
                {
                    let _ = writeln!(file, "churn {tick}");
                }
                if !signaled {
                    signaled = true;
                    let _ = started.send(());
                }
            }
        });
        first_write.recv().unwrap();
        let outcome = try_apply(vault, vec![filed], &[], false)
            .expect("conflicts defer, they do not error");
        stop.store(true, Ordering::Relaxed);
        churner.join().unwrap();
        let key = ThreadKey::new(TEST_ACCOUNT, "aabb").unwrap();
        assert_eq!(outcome.deferred, vec![key]);
        assert!(outcome.failed);
        assert!(outcome.results.is_empty());
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("deferred")),
            "{:?}",
            outcome.warnings
        );
        assert_eq!(count_journal(vault, "gmail.filed", "aabb"), 0);
    }

    fn try_apply_with_watches(
        vault: &Path,
        watches: Vec<crate::native::gmail::sync::watch::PreparedWatch>,
    ) -> Result<FilingOutcome, GmailError> {
        let fold = EnrollmentFold::load(vault).unwrap();
        let index = resolve::scan_index(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
        )
        .unwrap();
        let watchers =
            scan::scan_watching_tasks(vault, Path::new("mail"), &index)
                .unwrap();
        let budget = SharedBudget::new(Duration::from_secs(10));
        apply(
            FilingInput {
                vault,
                account: TEST_ACCOUNT,
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates: Vec::new(),
                adoptions: &[],
                enrollment: &fold,
                watchers: &watchers,
                dry_run: false,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
                watches,
                watch_failures: Vec::new(),
                watch_deferred: Vec::new(),
                reply_wait_days: 3,
            },
            &crate::native::gmail::sync::retry::SystemSleeper,
            &crate::native::gmail::sync::retry::SystemClock,
        )
    }

    fn hold_watch_for_test(
        key: &ThreadKey,
    ) -> crate::native::gmail::sync::watch::PreparedWatch {
        use crate::native::gmail::turn::{Net, ThreadTurn, Turn};
        use chrono::TimeZone;
        let send = chrono::Utc.timestamp_millis_opt(10).unwrap();
        crate::native::gmail::sync::watch::PreparedWatch {
            observation: crate::native::gmail::sync::watch::ThreadObservation {
                key: key.clone(),
                folded: ThreadTurn {
                    net: Net::Hold,
                    turn: Some(Turn::Theirs),
                    turn_since: Some(send),
                    newest_counted_id: Some("aa02".to_string()),
                    newest_internal_date: Some(20),
                    last_send_at: Some(send),
                },
            },
            note_path: None,
            title: None,
            who: None,
            refresh: None,
            turn_update: None,
            report_gone: false,
            refresh_failure: None,
        }
    }

    #[test]
    fn watch_hold_applies_task_and_marker_in_one_scoped_write() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let _env = test_env(vault);
        let (_state, _state_env) = isolated_state();
        fs::create_dir_all(vault.join("mail")).unwrap();
        fs::write(
            vault.join("mail/aa01.md"),
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: aa01\ntitle: Lease\nwho: Dana\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            vault.join("job.md"),
            "---\nparent: \"[[inbox]]\"\ntype: \"[[area]]\"\n---\n\n# Job\n\n## Tasks\n- [ ] #task [[mail/aa01|✉️ Lease · Dana]] [created:: 2026-11-07]\n",
        )
        .unwrap();
        let key = ThreadKey::new(TEST_ACCOUNT, "aa01").unwrap();
        let watches = vec![hold_watch_for_test(&key)];
        let outcome =
            try_apply_with_watches(vault, watches).expect("watch applies");
        assert_eq!(outcome.watch_threads.len(), 1);
        assert_eq!(outcome.watch_threads[0].action, "held");
        let task = fs::read_to_string(vault.join("job.md")).unwrap();
        assert!(task.contains("#awaiting_reply"), "{task}");
        assert!(task.contains("[scheduled::"), "{task}");
        // Marker follows the durable task write.
        let journal =
            crate::native::gmail::sync::watch_state::WatchJournal::load(vault)
                .unwrap();
        let marker = journal.marker(&key).expect("marker advanced");
        assert_eq!(marker.message_id, "aa02");
        // A hand edit between polls survives: the same turn replays as a
        // byte-identical noop instead of reasserting.
        let handed = task
            .replace("[created:: 2026-11-07]", "[created:: 2026-11-07] extra");
        fs::write(vault.join("job.md"), &handed).unwrap();
        let key2 = ThreadKey::new(TEST_ACCOUNT, "aa01").unwrap();
        let watches2 = vec![hold_watch_for_test(&key2)];
        let second =
            try_apply_with_watches(vault, watches2).expect("replay is noop");
        let after = fs::read_to_string(vault.join("job.md")).unwrap();
        assert!(after.contains("extra"), "hand edit survives: {after}");
        assert!(after.contains("#awaiting_reply"), "{after}");
        let _ = second;
    }
}

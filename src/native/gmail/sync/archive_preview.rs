//! Read-only archive planning for `bob gmail sync`.
//!
//! The preview phase never writes: no vault locks, no directories, no
//! journal entries, and no label creation. It folds local evidence
//! (enrollment, the archive scan, the archive journal, and this run's watch
//! outcomes) with paced remote thread metadata, computes the frozen `A`/`U`
//! message sets, binds the actionable set in a canonical digest, and applies
//! the historical-backlog / burst confirmation gate. Live application (the
//! following phase) re-reads the same state and requires the same digest
//! before any archive write.
//!
//! Digest stability rules: the digest binds the account, the enrollment
//! label name and resolved id, the receipt label name (never the remote id,
//! so first-use label creation between preview and confirm cannot
//! invalidate it), the archive action and limit, and per thread the
//! close-generation facts plus task/note evidence and the frozen/archive
//! message sets. Planning timestamps and display text never enter the
//! digest, and thread rows sort by thread id so fetch order cannot change it.

use std::collections::BTreeSet;

use serde::Serialize;
use sha2::{Digest as ShaDigest, Sha256};

use super::{
    archive_scan::{ArchiveScan, ArchiveThreadEvidence},
    archive_state::{
        ArchiveCloseGeneration, ArchiveDisposition, ArchiveJournal,
        ArchiveLabelIdentity, ArchiveTaskLocator,
    },
    discover,
    enroll::EnrollmentFold,
    retry::{is_auth_abort, should_retry},
    ThreadKey,
};
use crate::native::gmail::{
    config::GmailConfig,
    gog::GmailLabel,
    model::GmailThread,
    policy::{self, NeverSavePolicy},
    resolve::GmailRead,
    GmailError,
};

/// The archive action bound into every preview digest.
const DIGEST_ACTION: &str = "archive";
/// Digest document version; bump when the bound fields change.
const DIGEST_VERSION: u32 = 1;

/// A thread is still watched: at least one linking task is open.
pub(crate) const HOLD_WATCHING: &str = "watching";
/// A thread carries an unknown checkbox status, vetoing all-closed inference.
pub(crate) const HOLD_UNKNOWN: &str = "unknown";
/// A thread links a merge-retired source and holds for review.
pub(crate) const HOLD_MERGE: &str = "merge_hold";
/// Mail-note evidence cannot prove completion (missing, unreadable,
/// damaged, mismatched, duplicated, or ambiguous identity).
pub(crate) const HOLD_IDENTITY: &str = "identity";
/// The vault scan did not read every note, so any thread may hide a linker.
pub(crate) const HOLD_INCOMPLETE: &str = "incomplete";
/// Malformed archive journal state holds affected work.
pub(crate) const HOLD_UNSAFE: &str = "unsafe";
/// The thread is linked but was never enrolled.
pub(crate) const HOLD_NOT_ENROLLED: &str = "not_enrolled";
/// This run applied an incoming watch move to the thread (C18).
pub(crate) const HOLD_INCOMING_MOVE: &str = "incoming_move";
/// A remote metadata read failed; the thread retries next run.
pub(crate) const HOLD_FETCH_FAILED: &str = "fetch_failed";
/// Gmail returned a different thread identity than requested.
pub(crate) const HOLD_UNRESOLVABLE: &str = "unresolvable";
/// Saved messages carry the receipt but no terminal event reconciles them;
/// the apply phase must reconcile before any replay.
pub(crate) const HOLD_UNRECONCILED: &str = "unreconciled";

/// An unsaved Inbox message keeps the whole thread in place.
pub(crate) const KEPT_UNSAVED: &str = "unsaved";
/// The enrollment label was peeled off in Gmail: per-thread opt-out.
pub(crate) const KEPT_CONSENT_REMOVED: &str = "consent_removed";
/// A policy exclusion hit: Spam/Trash, an Inbox draft, or a never_save rule.
pub(crate) const KEPT_POLICY: &str = "policy";
/// Gmail no longer has the thread.
pub(crate) const KEPT_GONE: &str = "gone";
/// The thread already left the Inbox by hand with no receipt to reconcile.
pub(crate) const KEPT_MANUAL: &str = "manual";

/// A historical-backlog member needs an explicit matching `-c` digest.
pub(crate) const GATE_BACKLOG: &str = "backlog_unconfirmed";
/// More archive-ready threads than `archive_max` with no matching `-c`.
pub(crate) const GATE_OVER_LIMIT: &str = "over_limit";
/// A supplied `-c` digest does not match the current preview.
pub(crate) const GATE_MISMATCH: &str = "confirm_mismatch";

/// A preview row that would archive on confirmation.
pub(crate) const ACTION_WOULD_ARCHIVE: &str = "would_archive";
/// A preview row that would record a one-shot keep on confirmation.
pub(crate) const ACTION_KEPT: &str = "kept";
/// A gated row: no mutation until the preview is confirmed.
pub(crate) const ACTION_PAUSED: &str = "paused";
/// A fresh close, archivable without confirmation when under the limit.
pub(crate) const REASON_ELIGIBLE: &str = "eligible";
/// A confirmed historical-backlog row.
pub(crate) const REASON_BACKLOG: &str = "backlog";

/// Local (vault and journal only) verdict for one thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalVerdict {
    /// The journal disposition allows planning; remote evidence still decides.
    Ready {
        evidence: Box<ArchiveThreadEvidence>,
        close: Option<Box<ArchiveCloseGeneration>>,
        historical: bool,
    },
    Held {
        key: ThreadKey,
        reason: &'static str,
    },
    Skip,
}

/// The actionable decision for one preview row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreviewDecision {
    Archive { archive_ids: Vec<String> },
    Kept { reason: &'static str },
}

/// One actionable preview row: an archive or keep the confirmed run applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchivePreviewRow {
    pub key: ThreadKey,
    pub note: String,
    pub tasks: Vec<ArchiveTaskLocator>,
    pub saved_ids: Vec<String>,
    pub note_fingerprint: String,
    pub enable_epoch: String,
    /// The recorded close generation; `None` for unconfirmed backlog rows,
    /// whose generation is derived from the confirmation digest at apply.
    pub generation: Option<String>,
    pub historical: bool,
    pub decision: PreviewDecision,
}

impl ArchivePreviewRow {
    /// The exact sorted message IDs the row's mutation would touch: the
    /// frozen archive set for archives, the frozen saved set for keeps.
    pub(crate) fn messages(&self) -> Vec<String> {
        match &self.decision {
            PreviewDecision::Archive { archive_ids } => archive_ids.clone(),
            PreviewDecision::Kept { .. } => self.saved_ids.clone(),
        }
    }
}

/// Whether the previewed set may proceed without further confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveGate {
    Open { confirmed: bool },
    Paused { reason: &'static str },
}

impl ArchiveGate {
    pub(crate) fn is_paused(&self) -> bool {
        matches!(self, Self::Paused { .. })
    }
}

/// The complete read-only plan: actionable rows, their canonical digest
/// (`None` when no row is actionable), and the confirmation gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchivePlan {
    pub rows: Vec<ArchivePreviewRow>,
    pub digest: Option<String>,
    pub gate: ArchiveGate,
}

/// Everything one `preview` call folds.
pub(crate) struct PreviewInput<'a> {
    pub account: &'a str,
    pub enrollment: &'a EnrollmentFold,
    pub scan: &'a ArchiveScan,
    pub journal: &'a ArchiveJournal,
    /// Threads with a newly applied incoming watch move this run.
    pub suppressed: &'a BTreeSet<ThreadKey>,
    pub policy: &'a NeverSavePolicy,
    pub labels: &'a [GmailLabel],
    pub receipt_label: ArchiveLabelIdentity,
}

/// A finished preview plus the body-free warnings it produced.
pub(crate) struct PreviewReport {
    pub plan: ArchivePlan,
    pub warnings: Vec<String>,
    /// True when a per-thread fetch failed or deferred: the previewed set
    /// is incomplete, so the run reports unsuccessful without writing.
    pub failed: bool,
    pub deferred: Vec<ThreadKey>,
}

/// Pure local pass: enrollment, journal disposition, and same-run incoming
/// suppression. No network, no clock, no writes.
pub(crate) fn local_verdict(
    account: &str,
    enrollment: &EnrollmentFold,
    scan: &ArchiveScan,
    journal: &ArchiveJournal,
    suppressed: &BTreeSet<ThreadKey>,
    evidence: &ArchiveThreadEvidence,
) -> LocalVerdict {
    let key = &evidence.key;
    if !key.is_account(account) {
        return LocalVerdict::Skip;
    }
    if !enrollment.is_enrolled(key) {
        return LocalVerdict::Held {
            key: key.clone(),
            reason: HOLD_NOT_ENROLLED,
        };
    }
    match journal.project(scan, evidence) {
        ArchiveDisposition::Disabled | ArchiveDisposition::Consumed { .. } => {
            LocalVerdict::Skip
        }
        ArchiveDisposition::Watching => LocalVerdict::Held {
            key: key.clone(),
            reason: HOLD_WATCHING,
        },
        ArchiveDisposition::Held => LocalVerdict::Held {
            key: key.clone(),
            reason: held_reason(scan, journal, evidence),
        },
        ArchiveDisposition::HistoricalBacklog => {
            if suppressed.contains(key) {
                LocalVerdict::Held {
                    key: key.clone(),
                    reason: HOLD_INCOMING_MOVE,
                }
            } else {
                LocalVerdict::Ready {
                    evidence: Box::new(evidence.clone()),
                    close: None,
                    historical: true,
                }
            }
        }
        ArchiveDisposition::Ready { close, .. } => {
            if suppressed.contains(key) {
                LocalVerdict::Held {
                    key: key.clone(),
                    reason: HOLD_INCOMING_MOVE,
                }
            } else {
                LocalVerdict::Ready {
                    evidence: Box::new(evidence.clone()),
                    close: Some(Box::new(close)),
                    historical: false,
                }
            }
        }
    }
}

/// Map a journal `Held` disposition to its ordered first-failure reason.
/// Mirrors `ArchiveJournal::project` without duplicating its fold: unsafe
/// journal state first, then scan completeness, then open tasks, then
/// unknown boxes, then merge holds, then mail-note identity.
fn held_reason(
    scan: &ArchiveScan,
    journal: &ArchiveJournal,
    evidence: &ArchiveThreadEvidence,
) -> &'static str {
    if !journal.thread_is_safe(&evidence.key) {
        HOLD_UNSAFE
    } else if !scan.scan_complete() {
        HOLD_INCOMPLETE
    } else if evidence.has_open() {
        HOLD_WATCHING
    } else if evidence.has_unknown() {
        HOLD_UNKNOWN
    } else if evidence.merge_hold {
        HOLD_MERGE
    } else if evidence.identity_uncertain
        || !evidence.issues.is_empty()
        || !evidence.mail_complete()
    {
        HOLD_IDENTITY
    } else {
        // Reachable only when the evidence names no linking task at all:
        // without positive closed proof the thread stays watched.
        HOLD_WATCHING
    }
}

/// Split live Inbox message IDs into the frozen archive set `A` (Inbox IDs
/// already saved to the mail note) and the unsaved remainder `U`. Both are
/// sorted; only a nonempty `A` with an empty `U` may archive.
pub(crate) fn frozen_sets(
    saved_ids: &[String],
    thread: &GmailThread,
) -> (Vec<String>, Vec<String>) {
    let mut archive = Vec::new();
    let mut unsaved = Vec::new();
    for id in inbox_message_ids(thread) {
        if saved_ids.binary_search(&id).is_ok() {
            archive.push(id);
        } else {
            unsaved.push(id);
        }
    }
    (archive, unsaved)
}

/// Sorted unique IDs of messages currently carrying the Inbox label.
fn inbox_message_ids(thread: &GmailThread) -> Vec<String> {
    let mut ids: Vec<String> = thread
        .messages
        .iter()
        .filter(|message| {
            message
                .label_ids
                .iter()
                .any(|label| label.eq_ignore_ascii_case("INBOX"))
        })
        .map(|message| message.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Whether any message (Inbox or not) carries a Spam or Trash label.
fn has_spam_or_trash(thread: &GmailThread) -> bool {
    thread
        .messages
        .iter()
        .flat_map(|message| &message.label_ids)
        .any(|label| {
            label.eq_ignore_ascii_case("SPAM")
                || label.eq_ignore_ascii_case("TRASH")
        })
}

/// Whether an Inbox message is still an unsent draft. Drafts never join the
/// archive set: removing Inbox from a draft would strand it.
fn has_inbox_draft(thread: &GmailThread) -> bool {
    thread.messages.iter().any(|message| {
        message
            .label_ids
            .iter()
            .any(|label| label.eq_ignore_ascii_case("DRAFT"))
            && message
                .label_ids
                .iter()
                .any(|label| label.eq_ignore_ascii_case("INBOX"))
    })
}

/// Whether a saved message still carries the receipt label. Gmail stores
/// user-label IDs on messages, so names must be resolved through the live
/// label list before reaching this check.
fn has_receipt(
    thread: &GmailThread,
    saved_ids: &[String],
    receipt: &ArchiveLabelIdentity,
) -> bool {
    thread.messages.iter().any(|message| {
        saved_ids.binary_search(&message.id).is_ok()
            && receipt.id.as_ref().is_some_and(|id| {
                message.label_ids.iter().any(|label| label == id)
            })
    })
}

/// Resolve the enrollment label name to its Gmail id. `None` (missing or
/// ambiguous) holds every remaining thread with `no_label`: consent is
/// unverifiable and never assumed.
fn enrollment_label_id(labels: &[GmailLabel], name: &str) -> Option<String> {
    let mut matches = labels
        .iter()
        .filter(|label| label.name == name)
        .map(|label| label.id.clone());
    let id = matches.next()?;
    matches.next().is_none().then_some(id)
}

/// The remote half of one thread's verdict: identity exactness, live label
/// consent, policy exclusions, and the frozen `A`/`U` calculation. Held
/// threads carry an optional body-free warning for the run report.
#[derive(Debug)]
pub(crate) enum RemoteVerdict {
    Archive {
        archive_ids: Vec<String>,
    },
    Kept {
        reason: &'static str,
    },
    Held {
        reason: &'static str,
        warning: Option<String>,
    },
}

/// Evaluate one locally-ready thread against fresh remote metadata.
/// `receipt` is the row's effective receipt identity: the recorded close
/// label for journaled closes, the configured label for backlog rows.
#[cfg(test)]
pub(crate) fn remote_verdict<R: GmailRead>(
    reader: &R,
    account: &str,
    evidence: &ArchiveThreadEvidence,
    thread: &GmailThread,
    enrollment_id: &str,
    receipt: &ArchiveLabelIdentity,
    policy: &NeverSavePolicy,
) -> Result<RemoteVerdict, GmailError> {
    remote_verdict_for_saved_ids(
        reader,
        account,
        evidence,
        &evidence.saved_ids,
        thread,
        enrollment_id,
        receipt,
        policy,
    )
}

/// Evaluate remote metadata using the close's frozen saved IDs when one
/// exists. Backlog callers pass the current scan's IDs instead.
///
/// Recovery ordering: a crash can leave frozen messages carrying the
/// receipt with no archived event. That evidence is evaluated before
/// consent, policy, and unsaved checks, so a reply or a removed enrollment
/// label after the crash still records the recovered ownership once. An
/// `invalid_grant` from the never-save policy read aborts like every other
/// call site.
pub(crate) fn remote_verdict_for_saved_ids<R: GmailRead>(
    reader: &R,
    account: &str,
    evidence: &ArchiveThreadEvidence,
    saved_ids: &[String],
    thread: &GmailThread,
    enrollment_id: &str,
    receipt: &ArchiveLabelIdentity,
    policy: &NeverSavePolicy,
) -> Result<RemoteVerdict, GmailError> {
    if thread.id != evidence.key.thread_id()
        || thread.messages.is_empty()
        || thread
            .messages
            .iter()
            .any(|message| message.thread_id != evidence.key.thread_id())
    {
        return Ok(RemoteVerdict::Held {
            reason: HOLD_UNRESOLVABLE,
            warning: Some(format!(
                "skipped Gmail thread {}: unresolvable: Gmail returned a different thread identity",
                evidence.key.thread_id(),
            )),
        });
    }
    // Crash recovery first: frozen receipt-bearing IDs with no archived
    // event reconcile once, even past a removed enrollment label, a policy
    // exclusion, or a new unsaved reply. It covers only the frozen set.
    if has_receipt(thread, saved_ids, receipt) {
        return Ok(RemoteVerdict::Held {
            reason: HOLD_UNRECONCILED,
            warning: None,
        });
    }
    if !thread
        .messages
        .iter()
        .flat_map(|message| &message.label_ids)
        .any(|label| label == enrollment_id)
    {
        return Ok(RemoteVerdict::Kept {
            reason: KEPT_CONSENT_REMOVED,
        });
    }
    if has_spam_or_trash(thread) || has_inbox_draft(thread) {
        return Ok(RemoteVerdict::Kept {
            reason: KEPT_POLICY,
        });
    }
    match policy::check(reader, account, thread, policy) {
        Ok(()) => {}
        Err(error) if error.kind() == "never_save" => {
            return Ok(RemoteVerdict::Kept {
                reason: KEPT_POLICY,
            });
        }
        Err(error) if is_auth_abort(&error) => {
            return Err(error);
        }
        Err(error) => {
            return Ok(RemoteVerdict::Held {
                reason: HOLD_FETCH_FAILED,
                warning: Some(format!(
                    "skipped Gmail thread {}: {}: {}",
                    evidence.key.thread_id(),
                    error.kind(),
                    error.message(),
                )),
            });
        }
    }
    let (archive, unsaved) = frozen_sets(saved_ids, thread);
    if !unsaved.is_empty() {
        return Ok(RemoteVerdict::Kept {
            reason: KEPT_UNSAVED,
        });
    }
    if archive.is_empty() {
        return Ok(RemoteVerdict::Kept {
            reason: KEPT_MANUAL,
        });
    }
    Ok(RemoteVerdict::Archive {
        archive_ids: archive,
    })
}

/// One thread row of the canonical digest document.
#[derive(Serialize)]
struct DigestThread<'a> {
    thread_id: &'a str,
    generation: Option<&'a str>,
    enable_epoch: &'a str,
    historical: bool,
    note: &'a str,
    note_fingerprint: &'a str,
    tasks: Vec<DigestTask<'a>>,
    saved_ids: &'a [String],
    archive_ids: &'a [String],
    decision: String,
}

/// One task locator of the canonical digest document. Task prose never
/// enters the digest: path, line, block id, and box status bind the row.
#[derive(Serialize)]
struct DigestTask<'a> {
    path: &'a str,
    line: usize,
    block_id: Option<&'a str>,
    status: String,
}

/// The canonical digest document. Struct field order is the canonical
/// order; every list is pre-sorted so fetch and scan order cannot change it.
#[derive(Serialize)]
struct DigestDoc<'a> {
    canonical_version: u32,
    account: &'a str,
    action: &'a str,
    archive_max: u32,
    enrollment_label: DigestLabel<'a>,
    receipt_label: &'a str,
    threads: Vec<DigestThread<'a>>,
}

#[derive(Serialize)]
struct DigestLabel<'a> {
    name: &'a str,
    id: Option<&'a str>,
}

/// Canonical SHA-256 over the actionable set, as 64 lowercase hex digits.
/// Order-independent: rows sort by thread id before encoding, and planning
/// timestamps and display text never enter the document.
pub(crate) fn plan_digest(
    account: &str,
    enrollment_label: &str,
    enrollment_id: Option<&str>,
    receipt_label: &str,
    archive_max: u32,
    rows: &[ArchivePreviewRow],
) -> String {
    let mut sorted: Vec<&ArchivePreviewRow> = rows.iter().collect();
    sorted.sort_by(|left, right| left.key.cmp(&right.key));
    let threads: Vec<DigestThread<'_>> = sorted
        .iter()
        .map(|row| {
            let mut tasks: Vec<DigestTask<'_>> = row
                .tasks
                .iter()
                .map(|task| DigestTask {
                    path: task.path.as_str(),
                    line: task.line,
                    block_id: task.block_id.as_deref(),
                    status: task.status.to_string(),
                })
                .collect();
            tasks.sort_by(|left, right| {
                (left.path, left.line, left.block_id).cmp(&(
                    right.path,
                    right.line,
                    right.block_id,
                ))
            });
            let (archive_ids, decision) = match &row.decision {
                PreviewDecision::Archive { archive_ids } => {
                    (archive_ids.as_slice(), "archive".to_string())
                }
                PreviewDecision::Kept { reason } => {
                    (&[][..], format!("kept:{reason}"))
                }
            };
            DigestThread {
                thread_id: row.key.thread_id(),
                generation: row.generation.as_deref(),
                enable_epoch: row.enable_epoch.as_str(),
                historical: row.historical,
                note: row.note.as_str(),
                note_fingerprint: row.note_fingerprint.as_str(),
                tasks,
                saved_ids: row.saved_ids.as_slice(),
                archive_ids,
                decision,
            }
        })
        .collect();
    let doc = DigestDoc {
        canonical_version: DIGEST_VERSION,
        account,
        action: DIGEST_ACTION,
        archive_max,
        enrollment_label: DigestLabel {
            name: enrollment_label,
            id: enrollment_id,
        },
        receipt_label,
        threads,
    };
    let bytes = serde_json::to_vec(&doc).expect("digest document serializes");
    hex::encode(Sha256::digest(bytes))
}

/// The confirmation gate: any historical-backlog member requires an
/// explicit matching `-c` digest regardless of count, and so does an
/// archive batch larger than `archive_max`. An empty plan stays open and
/// ignores a supplied digest.
pub(crate) fn apply_gate(
    rows: &[ArchivePreviewRow],
    archive_max: u32,
    confirm: Option<&str>,
    digest: &str,
) -> ArchiveGate {
    if rows.is_empty() {
        return ArchiveGate::Open { confirmed: false };
    }
    let archive_count =
        rows.iter().filter(|row| row.is_archive()).count() as u32;
    let needs =
        rows.iter().any(|row| row.historical) || archive_count > archive_max;
    if !needs {
        return ArchiveGate::Open { confirmed: false };
    }
    match confirm {
        Some(supplied) if supplied == digest => {
            ArchiveGate::Open { confirmed: true }
        }
        Some(_) => ArchiveGate::Paused {
            reason: GATE_MISMATCH,
        },
        None => ArchiveGate::Paused {
            reason: if rows.iter().any(|row| row.historical) {
                GATE_BACKLOG
            } else {
                GATE_OVER_LIMIT
            },
        },
    }
}

impl ArchivePreviewRow {
    fn is_archive(&self) -> bool {
        matches!(self.decision, PreviewDecision::Archive { .. })
    }
}

/// Fold every enrolled, evidenced thread into a read-only plan. Fetches
/// fresh paced metadata only for locally-ready threads; every other thread
/// resolves from vault and journal state alone. Returns the plan plus
/// body-free warnings; a revoked grant aborts with an error.
pub(crate) fn preview<R: GmailRead>(
    reader: &R,
    input: &PreviewInput<'_>,
    config: &GmailConfig,
    confirm: Option<&str>,
) -> Result<PreviewReport, GmailError> {
    let mut warnings = Vec::new();
    let mut failed = false;
    let mut deferred = Vec::new();
    let mut rows = Vec::new();

    let mut local: Vec<(ThreadKey, LocalVerdict)> = Vec::new();
    let mut scan_keys: Vec<&ThreadKey> = input.scan.threads().collect();
    scan_keys.sort();
    for key in scan_keys {
        let Some(evidence) = input.scan.evidence(key) else {
            continue;
        };
        local.push((
            key.clone(),
            local_verdict(
                input.account,
                input.enrollment,
                input.scan,
                input.journal,
                input.suppressed,
                evidence,
            ),
        ));
    }

    let enrollment_id = config.sync_label().and_then(|name| {
        let resolved = enrollment_label_id(input.labels, name);
        if resolved.is_none() {
            warnings.push(format!(
                "Gmail enrollment label {name:?} is missing or ambiguous: archive consent is unverifiable"
            ));
        }
        resolved
    });
    let receipt_name = config.archive_label();

    // Locally-ready threads still need an enrollment label id before any
    // remote read: without one, consent is unverifiable and every thread
    // holds (the warning above already names the label).
    let mut ready: Vec<(
        ArchiveThreadEvidence,
        Option<ArchiveCloseGeneration>,
        bool,
    )> = Vec::new();
    if enrollment_id.is_some() {
        for verdict in local.iter().map(|(_, verdict)| verdict) {
            if let LocalVerdict::Ready {
                evidence,
                close,
                historical,
            } = verdict
            {
                ready.push((
                    evidence.as_ref().clone(),
                    close.clone().map(|boxed| *boxed),
                    *historical,
                ));
            }
        }
    } else if local
        .iter()
        .any(|(_, verdict)| matches!(verdict, LocalVerdict::Ready { .. }))
    {
        failed = true;
    }

    let keys: Vec<ThreadKey> = ready
        .iter()
        .map(|(evidence, _, _)| evidence.key.clone())
        .collect();
    if !keys.is_empty() {
        let enrollment_id =
            enrollment_id.expect("ready threads need an enrollment id");
        let fetched = discover::fetch_candidate_metadata(reader, &keys);
        warnings.extend(fetched.warnings);
        if let Some(aborted) = fetched.aborted {
            if is_auth_abort(&aborted) {
                return Err(aborted);
            }
            warnings.push(format!(
                "stopped Gmail archive reads: {}: {}",
                aborted.kind(),
                aborted.message(),
            ));
            failed = true;
            deferred.extend(keys.iter().cloned());
        } else {
            for key in &fetched.deferred {
                deferred.push(key.clone());
            }
            if !fetched.deferred.is_empty() {
                failed = true;
            }
            let mut by_key: std::collections::BTreeMap<ThreadKey, GmailThread> =
                fetched.successes.into_iter().collect();
            // Gmail has no such thread: the mail is gone, so the close is
            // kept terminally instead of retried.
            let mut gone_keys = BTreeSet::new();
            for (key, error) in &fetched.failures {
                if error.kind() == "not_found" {
                    gone_keys.insert(key.clone());
                    continue;
                }
                failed =
                    failed || error.kind() == "deferred" || should_retry(error);
                warnings.push(format!(
                    "skipped Gmail thread {}: {}: {}",
                    key.thread_id(),
                    error.kind(),
                    error.message(),
                ));
            }
            for (evidence, close, historical) in &ready {
                let key = &evidence.key;
                if gone_keys.contains(key) {
                    rows.push(row_for(
                        evidence,
                        close,
                        *historical,
                        PreviewDecision::Kept { reason: KEPT_GONE },
                    ));
                    continue;
                }
                if deferred.contains(key)
                    || fetched.failures.iter().any(|(failed, _)| failed == key)
                {
                    continue;
                }
                let Some(thread) = by_key.remove(key) else {
                    continue;
                };
                let receipt = close
                    .as_ref()
                    .map(|close| {
                        if close.label.id.is_some() {
                            close.label.clone()
                        } else {
                            input.receipt_label.clone()
                        }
                    })
                    .unwrap_or_else(|| input.receipt_label.clone());
                let saved_ids = close
                    .as_ref()
                    .map(|close| close.saved_ids.as_slice())
                    .unwrap_or(&evidence.saved_ids);
                match remote_verdict_for_saved_ids(
                    reader,
                    input.account,
                    evidence,
                    saved_ids,
                    &thread,
                    &enrollment_id,
                    &receipt,
                    input.policy,
                )? {
                    RemoteVerdict::Archive { archive_ids } => {
                        rows.push(row_for(
                            evidence,
                            close,
                            *historical,
                            PreviewDecision::Archive { archive_ids },
                        ));
                    }
                    RemoteVerdict::Kept { reason } => {
                        rows.push(row_for(
                            evidence,
                            close,
                            *historical,
                            PreviewDecision::Kept { reason },
                        ));
                    }
                    RemoteVerdict::Held { reason, warning } => {
                        let _ = reason;
                        if let Some(warning) = warning {
                            warnings.push(warning);
                        }
                        if reason == HOLD_FETCH_FAILED {
                            failed = true;
                        }
                    }
                }
            }
        }
    }
    rows.sort_by(|left: &ArchivePreviewRow, right: &ArchivePreviewRow| {
        left.key.cmp(&right.key)
    });

    let digest = if rows.is_empty() {
        None
    } else {
        Some(plan_digest(
            input.account,
            config.sync_label().unwrap_or_default(),
            enrollment_label_id(
                input.labels,
                config.sync_label().unwrap_or_default(),
            )
            .as_deref(),
            receipt_name,
            config.archive_max(),
            &rows,
        ))
    };
    let gate = match &digest {
        Some(digest) => {
            apply_gate(&rows, config.archive_max(), confirm, digest)
        }
        None => ArchiveGate::Open { confirmed: false },
    };
    Ok(PreviewReport {
        plan: ArchivePlan { rows, digest, gate },
        warnings,
        failed,
        deferred,
    })
}

/// Build a preview row from local evidence and its remote decision.
fn row_for(
    evidence: &ArchiveThreadEvidence,
    close: &Option<ArchiveCloseGeneration>,
    historical: bool,
    decision: PreviewDecision,
) -> ArchivePreviewRow {
    let mut tasks: Vec<ArchiveTaskLocator> = evidence
        .positive_closed()
        .iter()
        .map(|task| ArchiveTaskLocator::from(*task))
        .collect();
    tasks.sort();
    tasks.dedup();
    ArchivePreviewRow {
        key: evidence.key.clone(),
        note: evidence.note.clone(),
        tasks,
        saved_ids: close
            .as_ref()
            .map(|close| close.saved_ids.clone())
            .unwrap_or_else(|| evidence.saved_ids.clone()),
        note_fingerprint: evidence.note_fingerprint.clone().unwrap_or_default(),
        enable_epoch: close
            .as_ref()
            .map(|close| close.enable_epoch.clone())
            .unwrap_or_default(),
        generation: close.as_ref().map(|close| close.generation.clone()),
        historical,
        decision,
    }
}

#[cfg(test)]
mod tests {
    use super::super::archive_scan::{
        classify_archive_status, ArchiveLinkTask,
    };
    use super::super::scan::LinkPlacement;
    use super::*;
    use crate::native::config as bob_config;
    use std::collections::BTreeMap;
    use std::fs;

    const ACCOUNT: &str = "me@example.com";
    const ENROLLMENT_ID: &str = "Label_1";

    fn key(thread: &str) -> ThreadKey {
        ThreadKey::new(ACCOUNT, thread).unwrap()
    }

    fn label(name: &str, id: Option<&str>) -> ArchiveLabelIdentity {
        ArchiveLabelIdentity::new(name, id.map(str::to_string)).unwrap()
    }

    fn receipt() -> ArchiveLabelIdentity {
        label("bob/closed", Some("Label_9"))
    }

    fn task(status: char) -> ArchiveLinkTask {
        ArchiveLinkTask {
            path: "Projects/Job.md".to_string(),
            line: 7,
            text: format!("- [{status}] #task Done [[mail/thread]]"),
            block_id: Some("task-7".to_string()),
            status,
            class: classify_archive_status(status),
            placement: LinkPlacement::TaskLine,
            in_done: false,
            merge_retired: false,
            from_duplicate_note: false,
        }
    }

    fn evidence_for(
        key: ThreadKey,
        tasks: Vec<ArchiveLinkTask>,
    ) -> ArchiveThreadEvidence {
        ArchiveThreadEvidence {
            key,
            note: "mail/thread.md".to_string(),
            tasks,
            saved_ids: vec!["aa01".to_string(), "bb02".to_string()],
            note_fingerprint: Some(format!("sha256:{}", "a".repeat(64))),
            merge_hold: false,
            identity_uncertain: false,
            issues: Vec::new(),
        }
    }

    fn closed_evidence(thread: &str) -> ArchiveThreadEvidence {
        evidence_for(key(thread), vec![task('x')])
    }

    fn config_on() -> GmailConfig {
        GmailConfig::from_settings(bob_config::GmailSettings {
            sync: Some(bob_config::GmailSyncSettings {
                archive: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("archive config resolves")
    }

    fn labels() -> Vec<GmailLabel> {
        vec![
            GmailLabel {
                id: ENROLLMENT_ID.to_string(),
                name: "bob".to_string(),
                kind: Some("user".to_string()),
            },
            GmailLabel {
                id: "Label_9".to_string(),
                name: "bob/closed".to_string(),
                kind: Some("user".to_string()),
            },
        ]
    }

    fn enroll(vault: &std::path::Path, threads: &[&str]) {
        let dir = vault.join(".bob/gmail");
        fs::create_dir_all(&dir).unwrap();
        let mut lines = Vec::new();
        for (index, thread) in threads.iter().enumerate() {
            lines.push(
                serde_json::to_string(&serde_json::json!({
                    "v": 1,
                    "ts": format!("2026-11-01T09:00:{:02}-05:00", index),
                    "host": "athena",
                    "kind": "gmail.filed",
                    "value": {
                        "account": ACCOUNT,
                        "thread_id": thread,
                        "note": "mail/thread.md",
                        "task_file": "Projects/Job.md",
                    },
                }))
                .unwrap(),
            );
        }
        lines.push(String::new());
        fs::write(dir.join("athena.jsonl"), lines.join("\n")).unwrap();
    }

    fn journal_with(
        vault: &std::path::Path,
        events: &[(&ArchiveThreadEvidence, char)],
    ) -> ArchiveJournal {
        let enable_label = label("bob/closed", None);
        let mut state = ArchiveJournal::load(vault).unwrap();
        if !state.is_enabled(ACCOUNT) {
            let enable = state
                .event_for_enablement(ACCOUNT, true, enable_label)
                .expect("enablement plans");
            ArchiveJournal::append(vault, "athena", &enable).unwrap();
            state = ArchiveJournal::load(vault).unwrap();
        }
        for (evidence, status) in events {
            let open_evidence =
                evidence_for(evidence.key.clone(), vec![task(' ')]);
            let open_scan = ArchiveScan::test_scan(open_evidence.clone(), true);
            if *status == 'x' {
                let open = state
                    .event_for_open(&open_scan, &open_evidence)
                    .expect("open observes");
                ArchiveJournal::append(vault, "athena", &open).unwrap();
                state = ArchiveJournal::load(vault).unwrap();
            }
            let closed_scan = ArchiveScan::test_scan((*evidence).clone(), true);
            let close = state
                .event_for_close(
                    &closed_scan,
                    evidence,
                    label("bob/closed", None),
                )
                .expect("close records");
            ArchiveJournal::append(vault, "athena", &close).unwrap();
            state = ArchiveJournal::load(vault).unwrap();
        }
        state
    }

    struct StubRead {
        threads: BTreeMap<String, Result<GmailThread, (String, String)>>,
        labels: Vec<GmailLabel>,
    }

    impl StubRead {
        fn ok(thread: GmailThread) -> Self {
            let mut threads = BTreeMap::new();
            threads.insert(thread.id.clone(), Ok(thread));
            Self {
                threads,
                labels: labels(),
            }
        }
    }

    impl GmailRead for StubRead {
        fn attachment_bytes(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, GmailError> {
            unreachable!()
        }

        fn thread_get(
            &self,
            account: &str,
            thread_id: &str,
        ) -> Result<GmailThread, GmailError> {
            self.thread_metadata(account, thread_id)
        }

        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }

        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }

        fn labels_list(&self, _: &str) -> Result<Vec<GmailLabel>, GmailError> {
            Ok(self.labels.clone())
        }

        fn thread_metadata(
            &self,
            _: &str,
            thread_id: &str,
        ) -> Result<GmailThread, GmailError> {
            match self.threads.get(thread_id) {
                Some(Ok(thread)) => Ok(thread.clone()),
                Some(Err((kind, message))) => {
                    Err(GmailError::runtime(kind, message.clone()))
                }
                None => Err(GmailError::runtime(
                    "not_found",
                    "Google has no matching Gmail item".to_string(),
                )),
            }
        }
    }

    fn thread_json(messages: &str) -> GmailThread {
        GmailThread::from_gog_json(
            format!(
                "{{\"thread\":{{\"id\":\"aa11\",\"messages\":[{messages}]}}}}"
            )
            .as_bytes(),
        )
        .expect("synthetic thread parses")
    }

    fn message(id: &str, labels: &[&str], from: Option<&str>) -> String {
        let label_list = labels
            .iter()
            .map(|label| format!("\"{label}\""))
            .collect::<Vec<_>>()
            .join(",");
        let headers = from.map_or(String::new(), |from| {
            format!(
                ",\"payload\":{{\"headers\":[{{\"name\":\"From\",\"value\":\"{from}\"}}]}}"
            )
        });
        format!("{{\"id\":\"{id}\",\"threadId\":\"aa11\",\"labelIds\":[{label_list}]{headers}}}")
    }

    fn archived_thread() -> GmailThread {
        thread_json(
            &[
                message("aa01", &["INBOX", ENROLLMENT_ID], None),
                message("bb02", &["INBOX", ENROLLMENT_ID], None),
            ]
            .join(","),
        )
    }

    fn input<'a>(
        enrollment: &'a EnrollmentFold,
        scan: &'a ArchiveScan,
        journal: &'a ArchiveJournal,
        suppressed: &'a BTreeSet<ThreadKey>,
        policy: &'a NeverSavePolicy,
        labels: &'a [GmailLabel],
    ) -> PreviewInput<'a> {
        PreviewInput {
            account: ACCOUNT,
            enrollment,
            scan,
            journal,
            suppressed,
            policy,
            labels,
            receipt_label: receipt(),
        }
    }

    #[test]
    fn frozen_sets_split_inbox_ids_from_saved_evidence() {
        let thread = thread_json(
            &[
                message("aa01", &["INBOX", ENROLLMENT_ID], None),
                message("bb02", &["INBOX", ENROLLMENT_ID], None),
                message("cc03", &["INBOX", ENROLLMENT_ID], None),
                message("dd04", &[ENROLLMENT_ID], None),
            ]
            .join(","),
        );
        let (archive, unsaved) =
            frozen_sets(&["aa01".to_string(), "bb02".to_string()], &thread);
        assert_eq!(archive, vec!["aa01".to_string(), "bb02".to_string()]);
        assert_eq!(unsaved, vec!["cc03".to_string()]);
    }

    #[test]
    fn local_verdict_orders_account_enrollment_and_evidence_first() {
        let temp = tempfile::tempdir().unwrap();
        enroll(temp.path(), &["aa11"]);
        let enrollment = EnrollmentFold::load(temp.path()).unwrap();
        let journal = journal_with(temp.path(), &[]);
        let scan = ArchiveScan::test_empty_scan(true);
        let suppressed = BTreeSet::new();

        let foreign = evidence_for(
            ThreadKey::new("other@example.com", "aa11").unwrap(),
            vec![task('x')],
        );
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &foreign
            ),
            LocalVerdict::Skip
        );

        let unenrolled = closed_evidence("bb22");
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &unenrolled
            ),
            LocalVerdict::Held {
                key: key("bb22"),
                reason: HOLD_NOT_ENROLLED,
            }
        );

        let open = evidence_for(key("aa11"), vec![task(' ')]);
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &open
            ),
            LocalVerdict::Held {
                key: key("aa11"),
                reason: HOLD_WATCHING,
            }
        );

        let unknown = evidence_for(key("aa11"), vec![task('~')]);
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &unknown
            ),
            LocalVerdict::Held {
                key: key("aa11"),
                reason: HOLD_UNKNOWN,
            }
        );
    }

    #[test]
    fn local_verdict_holds_merge_and_identity_evidence() {
        let temp = tempfile::tempdir().unwrap();
        enroll(temp.path(), &["aa11"]);
        let enrollment = EnrollmentFold::load(temp.path()).unwrap();
        let journal = journal_with(temp.path(), &[]);
        let scan = ArchiveScan::test_empty_scan(true);
        let suppressed = BTreeSet::new();

        let mut merged = closed_evidence("aa11");
        merged.merge_hold = true;
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &merged
            ),
            LocalVerdict::Held {
                key: key("aa11"),
                reason: HOLD_MERGE,
            }
        );

        let mut uncertain = closed_evidence("aa11");
        uncertain.identity_uncertain = true;
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &uncertain
            ),
            LocalVerdict::Held {
                key: key("aa11"),
                reason: HOLD_IDENTITY,
            }
        );
    }

    #[test]
    fn local_verdict_suppresses_same_run_incoming_moves() {
        let temp = tempfile::tempdir().unwrap();
        enroll(temp.path(), &["aa11"]);
        let enrollment = EnrollmentFold::load(temp.path()).unwrap();
        let evidence = closed_evidence("aa11");
        let scan = ArchiveScan::test_scan(evidence.clone(), true);
        let journal = journal_with(temp.path(), &[(&evidence, 'x')]);
        let suppressed = BTreeSet::from([key("aa11")]);
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &evidence
            ),
            LocalVerdict::Held {
                key: key("aa11"),
                reason: HOLD_INCOMING_MOVE,
            }
        );
        assert!(matches!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &scan,
                &journal,
                &BTreeSet::new(),
                &evidence
            ),
            LocalVerdict::Ready { .. }
        ));
    }

    #[test]
    fn local_verdict_maps_backlog_ready_and_consumed() {
        let temp = tempfile::tempdir().unwrap();
        enroll(temp.path(), &["aa11", "bb22", "cc33"]);
        let enrollment = EnrollmentFold::load(temp.path()).unwrap();
        let suppressed = BTreeSet::new();

        let backlog = closed_evidence("aa11");
        let backlog_scan = ArchiveScan::test_scan(backlog.clone(), true);
        let journal = journal_with(temp.path(), &[]);
        assert!(matches!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &backlog_scan,
                &journal,
                &suppressed,
                &backlog
            ),
            LocalVerdict::Ready {
                historical: true,
                ..
            }
        ));

        let fresh = closed_evidence("bb22");
        let fresh_scan = ArchiveScan::test_scan(fresh.clone(), true);
        let journal = journal_with(temp.path(), &[(&fresh, 'x')]);
        assert!(matches!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &fresh_scan,
                &journal,
                &suppressed,
                &fresh
            ),
            LocalVerdict::Ready {
                historical: false,
                ..
            }
        ));

        let done_evidence = closed_evidence("cc33");
        let done_scan = ArchiveScan::test_scan(done_evidence.clone(), true);
        let mut journal = journal_with(temp.path(), &[(&done_evidence, 'x')]);
        let ArchiveDisposition::Ready { close, .. } =
            journal.project(&done_scan, &done_evidence)
        else {
            panic!("close should be ready")
        };
        let terminal = journal
            .event_for_terminal(
                &close.key,
                &close.generation,
                crate::native::gmail::sync::archive_state::ArchiveTerminalKind::Kept {
                    reason: KEPT_UNSAVED.to_string(),
                },
            )
            .expect("kept plans");
        ArchiveJournal::append(temp.path(), "athena", &terminal).unwrap();
        journal = ArchiveJournal::load(temp.path()).unwrap();
        assert_eq!(
            local_verdict(
                ACCOUNT,
                &enrollment,
                &done_scan,
                &journal,
                &suppressed,
                &done_evidence
            ),
            LocalVerdict::Skip
        );
    }

    #[test]
    fn remote_verdict_archives_only_frozen_inbox_ids() {
        let reader = StubRead::ok(archived_thread());
        let evidence = closed_evidence("aa11");
        let policy = NeverSavePolicy::for_test(&[], &[]);
        match remote_verdict(
            &reader,
            ACCOUNT,
            &evidence,
            &archived_thread(),
            ENROLLMENT_ID,
            &receipt(),
            &policy,
        )
        .expect("verdict")
        {
            RemoteVerdict::Archive { archive_ids } => {
                assert_eq!(
                    archive_ids,
                    vec!["aa01".to_string(), "bb02".to_string()]
                );
            }
            other => panic!("expected archive, got {other:?}"),
        }
    }

    #[test]
    fn close_saved_ids_keep_new_inbox_saves_unsaved_and_receipt_checks_use_ids()
    {
        let policy = NeverSavePolicy::for_test(&[], &[]);
        let mut evidence = closed_evidence("aa11");
        evidence.saved_ids = vec!["aa01".to_string(), "bb02".to_string()];
        let thread = thread_json(
            &[
                message("aa01", &["INBOX", ENROLLMENT_ID], None),
                message("bb02", &["INBOX", ENROLLMENT_ID], None),
            ]
            .join(","),
        );
        let reader = StubRead::ok(thread.clone());
        assert!(matches!(
            remote_verdict_for_saved_ids(
                &reader,
                ACCOUNT,
                &evidence,
                &["aa01".to_string()],
                &thread,
                ENROLLMENT_ID,
                &receipt(),
                &policy,
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_UNSAVED
            }
        ));

        let receipt_name_only =
            thread_json(&message("aa01", &[ENROLLMENT_ID, "bob/closed"], None));
        assert!(matches!(
            remote_verdict_for_saved_ids(
                &reader,
                ACCOUNT,
                &evidence,
                &["aa01".to_string()],
                &receipt_name_only,
                ENROLLMENT_ID,
                &receipt(),
                &policy,
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_MANUAL
            }
        ));
    }

    #[test]
    fn remote_verdict_keeps_unsaved_consent_and_policy() {
        let policy = NeverSavePolicy::for_test(&[], &[]);
        let evidence = closed_evidence("aa11");

        let unsaved = thread_json(
            &[
                message("aa01", &["INBOX", ENROLLMENT_ID], None),
                message("zz99", &["INBOX", ENROLLMENT_ID], None),
            ]
            .join(","),
        );
        let reader = StubRead::ok(unsaved.clone());
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &unsaved,
                ENROLLMENT_ID,
                &receipt(),
                &policy
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_UNSAVED
            }
        ));

        let peeled = thread_json(&message("aa01", &["INBOX"], None));
        let reader = StubRead::ok(peeled.clone());
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &peeled,
                ENROLLMENT_ID,
                &receipt(),
                &policy
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_CONSENT_REMOVED
            }
        ));

        let trashed = thread_json(
            &[
                message("aa01", &["INBOX", ENROLLMENT_ID], None),
                message("bb02", &["INBOX", ENROLLMENT_ID, "TRASH"], None),
            ]
            .join(","),
        );
        let reader = StubRead::ok(trashed.clone());
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &trashed,
                ENROLLMENT_ID,
                &receipt(),
                &policy
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_POLICY
            }
        ));

        let drafted = thread_json(
            &[
                message("aa01", &["INBOX", ENROLLMENT_ID, "DRAFT"], None),
                message("bb02", &["INBOX", ENROLLMENT_ID], None),
            ]
            .join(","),
        );
        let reader = StubRead::ok(drafted.clone());
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &drafted,
                ENROLLMENT_ID,
                &receipt(),
                &policy
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_POLICY
            }
        ));

        let blocked = NeverSavePolicy::for_test(&["spam@example.com"], &[]);
        let listed = thread_json(
            &[
                message(
                    "aa01",
                    &["INBOX", ENROLLMENT_ID],
                    Some("spam@example.com"),
                ),
                message("bb02", &["INBOX", ENROLLMENT_ID], None),
            ]
            .join(","),
        );
        let reader = StubRead::ok(listed.clone());
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &listed,
                ENROLLMENT_ID,
                &receipt(),
                &blocked
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_POLICY
            }
        ));
    }

    #[test]
    fn remote_verdict_holds_unresolvable_and_manual_or_reconcile() {
        let reader = StubRead::ok(archived_thread());
        let evidence = closed_evidence("aa11");
        let policy = NeverSavePolicy::for_test(&[], &[]);

        let wrong = thread_json(&message("aa01", &["INBOX"], None));
        let mut wrong = wrong;
        wrong.id = "bb22".to_string();
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &wrong,
                ENROLLMENT_ID,
                &receipt(),
                &policy
            )
            .expect("verdict"),
            RemoteVerdict::Held {
                reason: HOLD_UNRESOLVABLE,
                ..
            }
        ));

        let empty_inbox = thread_json(&message("aa01", &[ENROLLMENT_ID], None));
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &empty_inbox,
                ENROLLMENT_ID,
                &receipt(),
                &policy
            )
            .expect("verdict"),
            RemoteVerdict::Kept {
                reason: KEPT_MANUAL
            }
        ));

        let receipted = thread_json(
            &[
                message("aa01", &[ENROLLMENT_ID, "Label_9"], None),
                message("bb02", &[ENROLLMENT_ID], None),
            ]
            .join(","),
        );
        assert!(matches!(
            remote_verdict(
                &reader,
                ACCOUNT,
                &evidence,
                &receipted,
                ENROLLMENT_ID,
                &receipt(),
                &policy
            )
            .expect("verdict"),
            RemoteVerdict::Held {
                reason: HOLD_UNRECONCILED,
                ..
            }
        ));
    }

    #[test]
    fn digest_is_order_independent_and_binds_every_input() {
        let left = ArchivePreviewRow {
            key: key("bb22"),
            note: "mail/b.md".to_string(),
            tasks: vec![ArchiveTaskLocator {
                path: "Projects/Job.md".to_string(),
                line: 7,
                block_id: Some("task-7".to_string()),
                status: 'x',
            }],
            saved_ids: vec!["cc03".to_string()],
            note_fingerprint: format!("sha256:{}", "b".repeat(64)),
            enable_epoch: "epoch-1".to_string(),
            generation: Some("sha256:gen".to_string()),
            historical: false,
            decision: PreviewDecision::Archive {
                archive_ids: vec!["cc03".to_string()],
            },
        };
        let right = ArchivePreviewRow {
            key: key("aa11"),
            note: "mail/a.md".to_string(),
            tasks: vec![ArchiveTaskLocator {
                path: "Projects/Job.md".to_string(),
                line: 3,
                block_id: None,
                status: 'x',
            }],
            saved_ids: vec!["aa01".to_string()],
            note_fingerprint: format!("sha256:{}", "a".repeat(64)),
            enable_epoch: "epoch-1".to_string(),
            generation: None,
            historical: true,
            decision: PreviewDecision::Kept {
                reason: KEPT_UNSAVED,
            },
        };
        let digest = |rows: &[ArchivePreviewRow]| {
            plan_digest(
                "me@example.com",
                "bob",
                Some("Label_1"),
                "bob/closed",
                10,
                rows,
            )
        };
        let forward = digest(&[left.clone(), right.clone()]);
        assert_eq!(forward.len(), 64);
        assert_eq!(digest(&[right.clone(), left.clone()]), forward);

        let mut changed = left.clone();
        changed.saved_ids = vec!["zz99".to_string()];
        assert_ne!(digest(&[changed, right.clone()]), forward);

        let mut changed = right.clone();
        changed.historical = false;
        assert_ne!(digest(&[left.clone(), changed]), forward);

        assert_ne!(
            plan_digest(
                "other@example.com",
                "bob",
                Some("Label_1"),
                "bob/closed",
                10,
                &[left.clone(), right.clone()]
            ),
            forward
        );
        let mut changed = left.clone();
        changed.decision = PreviewDecision::Archive {
            archive_ids: vec!["cc03".to_string(), "dd04".to_string()],
        };
        assert_ne!(digest(&[changed, right.clone()]), forward);
    }

    #[test]
    fn gate_requires_backlog_and_burst_confirmation() {
        let row = |historical: bool| ArchivePreviewRow {
            key: key("aa11"),
            note: "mail/a.md".to_string(),
            tasks: Vec::new(),
            saved_ids: vec!["aa01".to_string()],
            note_fingerprint: String::new(),
            enable_epoch: String::new(),
            generation: None,
            historical,
            decision: PreviewDecision::Archive {
                archive_ids: vec!["aa01".to_string()],
            },
        };
        let digest = "current-digest";
        assert_eq!(
            apply_gate(&[row(false)], 10, None, digest),
            ArchiveGate::Open { confirmed: false }
        );
        assert_eq!(
            apply_gate(&[row(true)], 10, None, digest),
            ArchiveGate::Paused {
                reason: GATE_BACKLOG
            }
        );
        assert_eq!(
            apply_gate(&[row(true)], 10, Some(digest), digest),
            ArchiveGate::Open { confirmed: true }
        );
        assert_eq!(
            apply_gate(&[row(true)], 10, Some("stale"), digest),
            ArchiveGate::Paused {
                reason: GATE_MISMATCH
            }
        );

        let many: Vec<ArchivePreviewRow> =
            (0..11).map(|_| row(false)).collect();
        assert_eq!(
            apply_gate(&many, 10, None, digest),
            ArchiveGate::Paused {
                reason: GATE_OVER_LIMIT
            }
        );
        let ten: Vec<ArchivePreviewRow> = many.into_iter().take(10).collect();
        assert_eq!(
            apply_gate(&ten, 10, None, digest),
            ArchiveGate::Open { confirmed: false }
        );
        let fourteen: Vec<ArchivePreviewRow> =
            (0..14).map(|_| row(false)).collect();
        assert_eq!(
            apply_gate(&fourteen, 10, None, digest),
            ArchiveGate::Paused {
                reason: GATE_OVER_LIMIT
            }
        );

        let kept: Vec<ArchivePreviewRow> = (0..11)
            .map(|_| ArchivePreviewRow {
                decision: PreviewDecision::Kept {
                    reason: KEPT_UNSAVED,
                },
                ..row(false)
            })
            .collect();
        assert_eq!(
            apply_gate(&kept, 10, None, digest),
            ArchiveGate::Open { confirmed: false }
        );

        assert_eq!(
            apply_gate(&[], 10, Some(digest), digest),
            ArchiveGate::Open { confirmed: false }
        );
    }

    #[test]
    fn preview_folds_a_fresh_close_without_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        enroll(temp.path(), &["aa11"]);
        let enrollment = EnrollmentFold::load(temp.path()).unwrap();
        let evidence = closed_evidence("aa11");
        let scan = ArchiveScan::test_scan(evidence.clone(), true);
        let journal = journal_with(temp.path(), &[(&evidence, 'x')]);
        let suppressed = BTreeSet::new();
        let policy = NeverSavePolicy::for_test(&[], &[]);
        let labels = labels();
        let reader = StubRead::ok(archived_thread());
        let report = preview(
            &reader,
            &input(&enrollment, &scan, &journal, &suppressed, &policy, &labels),
            &config_on(),
            None,
        )
        .expect("preview reads");
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(!report.failed);
        assert_eq!(report.plan.rows.len(), 1);
        let row = &report.plan.rows[0];
        assert_eq!(row.key, key("aa11"));
        assert!(!row.historical);
        assert!(matches!(row.decision, PreviewDecision::Archive { .. }));
        assert_eq!(report.plan.gate, ArchiveGate::Open { confirmed: false });
        let digest = report.plan.digest.clone().expect("digest binds rows");
        assert_eq!(digest.len(), 64);
        // A re-read of the same semantic state confirms.
        let again = preview(
            &reader,
            &input(&enrollment, &scan, &journal, &suppressed, &policy, &labels),
            &config_on(),
            Some(&digest),
        )
        .expect("preview re-reads");
        assert_eq!(again.plan.digest.as_deref(), Some(digest.as_str()));
        assert_eq!(again.plan.gate, ArchiveGate::Open { confirmed: false });
    }

    #[test]
    fn preview_pauses_backlog_until_the_digest_confirms() {
        let temp = tempfile::tempdir().unwrap();
        enroll(temp.path(), &["aa11"]);
        let enrollment = EnrollmentFold::load(temp.path()).unwrap();
        let evidence = closed_evidence("aa11");
        let scan = ArchiveScan::test_scan(evidence.clone(), true);
        let journal = journal_with(temp.path(), &[]);
        let suppressed = BTreeSet::new();
        let policy = NeverSavePolicy::for_test(&[], &[]);
        let labels = labels();
        let reader = StubRead::ok(archived_thread());
        let report = preview(
            &reader,
            &input(&enrollment, &scan, &journal, &suppressed, &policy, &labels),
            &config_on(),
            None,
        )
        .expect("preview reads");
        assert!(report.plan.rows.iter().all(|row| row.historical));
        assert_eq!(
            report.plan.gate,
            ArchiveGate::Paused {
                reason: GATE_BACKLOG
            }
        );
        let digest = report.plan.digest.clone().expect("digest binds rows");
        let confirmed = preview(
            &reader,
            &input(&enrollment, &scan, &journal, &suppressed, &policy, &labels),
            &config_on(),
            Some(&digest),
        )
        .expect("confirmed preview reads");
        assert_eq!(confirmed.plan.gate, ArchiveGate::Open { confirmed: true });
        assert_eq!(confirmed.plan.rows.len(), 1);
    }

    #[test]
    fn preview_holds_without_the_enrollment_label_and_keeps_gone() {
        let temp = tempfile::tempdir().unwrap();
        enroll(temp.path(), &["aa11"]);
        let enrollment = EnrollmentFold::load(temp.path()).unwrap();
        let evidence = closed_evidence("aa11");
        let scan = ArchiveScan::test_scan(evidence.clone(), true);
        let journal = journal_with(temp.path(), &[(&evidence, 'x')]);
        let suppressed = BTreeSet::new();
        let policy = NeverSavePolicy::for_test(&[], &[]);

        let no_enrollment: Vec<GmailLabel> = labels()
            .into_iter()
            .filter(|label| label.name != "bob")
            .collect();
        let reader = StubRead::ok(archived_thread());
        let report = preview(
            &reader,
            &input(
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &policy,
                &no_enrollment,
            ),
            &config_on(),
            None,
        )
        .expect("preview holds");
        assert!(report.plan.rows.is_empty());
        assert!(report.plan.digest.is_none());
        assert!(report.failed);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("bob")),
            "{:?}",
            report.warnings
        );

        let mut gone_threads = BTreeMap::new();
        gone_threads.insert(
            "aa11".to_string(),
            Err((
                "not_found".to_string(),
                "Google has no matching Gmail item".to_string(),
            )),
        );
        let gone_reader = StubRead {
            threads: gone_threads,
            labels: labels(),
        };
        let gone_labels = gone_reader.labels.clone();
        let report = preview(
            &gone_reader,
            &input(
                &enrollment,
                &scan,
                &journal,
                &suppressed,
                &policy,
                &gone_labels,
            ),
            &config_on(),
            None,
        )
        .expect("gone preview reads");
        assert_eq!(report.plan.rows.len(), 1);
        assert!(matches!(
            report.plan.rows[0].decision,
            PreviewDecision::Kept { reason: KEPT_GONE }
        ));
    }
}

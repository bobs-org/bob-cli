//! Live archive application for `bob gmail sync`.
//!
//! The apply phase runs after durable filing/watch application and before
//! final report/health settlement. It re-reads vault evidence, persists
//! enablement/open/close observations, plans with the same read-only
//! preview, enforces the backlog/burst confirmation gate, then mutates one
//! thread at a time with fresh per-thread metadata, frozen saved IDs,
//! receipt reconciliation, isolated per-thread failures, and
//! `invalid_grant` aborts. Filing commits already on disk are never rolled
//! back; non-auth failures isolate to one thread and fail the run.

use std::{collections::BTreeSet, path::Path};

use super::{
    archive_preview::{
        self, PreviewDecision, PreviewInput, HOLD_FETCH_FAILED,
        HOLD_UNRECONCILED, HOLD_UNRESOLVABLE, KEPT_CONSENT_REMOVED, KEPT_GONE,
        KEPT_MANUAL, KEPT_POLICY, KEPT_UNSAVED,
    },
    archive_scan::{scan_archive_evidence, SavedIdSnapshot},
    archive_state::{
        ArchiveCloseGeneration, ArchiveJournal, ArchiveLabelIdentity,
        ArchiveTerminalKind,
    },
    archive_transport::{
        resolve_existing_receipt_label, resolve_or_create_receipt_label,
        LabelOperation, MessageLabelState,
    },
    discover,
    enroll::EnrollmentFold,
    retry::is_auth_abort,
    watch_apply::{ACTION_BOUNCED, ACTION_LIFTED},
    ThreadKey,
};
use crate::native::{
    config as bob_config,
    gmail::{
        cli::SyncArgs,
        config::GmailConfig,
        gog::{GmailLabel, Gog, CLIENT},
        policy::NeverSavePolicy,
        resolve::{self, GmailRead},
        sync::{
            report::{ArchiveHint, ArchiveRow, ArchiveTaskRef},
            watch_apply::WatchThreadOutcome,
        },
        GmailError,
    },
    vault_sync,
};

/// Live archive actions reported in human/JSON output.
pub(crate) const ACTION_ARCHIVED: &str = "archived";
pub(crate) const ACTION_LET_GO: &str = "let_go";
pub(crate) const ACTION_KEPT_LIVE: &str = "kept";
pub(crate) const ACTION_PAUSED_LIVE: &str = "paused";
/// Recovery reason for a reconciled archive that never re-mutated.
pub(crate) const REASON_RECOVERED: &str = "recovered";
/// Let-go reasons distinguish a new reply from a manual Inbox return.
pub(crate) const REASON_REPLIED: &str = "replied";
pub(crate) const REASON_MANUAL_RETURN: &str = "manual_return";

/// The live apply result attached to the sync report.
#[derive(Debug, Default)]
pub(crate) struct LiveArchiveOutcome {
    pub(crate) rows: Vec<ArchiveRow>,
    pub(crate) digest: Option<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) hints: Vec<ArchiveHint>,
    pub(crate) failed: bool,
    pub(crate) deferred: Vec<ThreadKey>,
    pub(crate) skipped_no_archive: bool,
}

/// Check the isolated `bob-gmail` modify grant for one account.
fn has_modify_grant(gog: &Gog, account: &str) -> Result<bool, GmailError> {
    let entries = gog.accounts(Some(CLIENT))?;
    let Some(entry) = entries
        .iter()
        .find(|entry| entry.email.eq_ignore_ascii_case(account))
    else {
        return Ok(false);
    };
    let has_modify = entry.scopes.iter().any(|scope| {
        scope == "https://www.googleapis.com/auth/gmail.modify"
            || scope.contains("gmail.modify")
    });
    Ok(has_modify)
}

fn setup_missing_modify() -> GmailError {
    GmailError::setup(
        "modify_grant",
        "the bob-gmail sign-in lacks Gmail modify access".to_string(),
    )
    .with_hint(
        "run `bob gmail login -s modify` on the approved archive host; \
         Bob uses it for label-based archive; restore stays manual: Move to Inbox in Gmail",
    )
}

/// Acquire the vault maintenance lock for one thread's write boundary with a
/// bounded wait, following the discipline `file::apply` and the sync guard
/// already use. Never hold the lock across more than one thread's write.
fn acquire_thread_lock() -> Option<std::fs::File> {
    for _ in 0..50 {
        match crate::native::ob::try_acquire_lock() {
            Ok(file) => return Some(file),
            Err(crate::native::ob::LockAcquireError::Contended) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
    None
}

/// Whether fresh vault evidence still matches the frozen close: note path,
/// saved IDs, note fingerprint, task locators and statuses, all-closed, and
/// no identity uncertainty. Any change holds the thread without mutating.
fn fresh_matches_close(
    fresh: &super::archive_scan::ArchiveThreadEvidence,
    close: &super::archive_state::ArchiveCloseGeneration,
) -> bool {
    if fresh.note != close.note {
        return false;
    }
    // Never expand the frozen IDs, but never hold for growth either: new
    // saves fall in U and keep the row `unsaved` via the fresh verdict. Only
    // a missing frozen ID holds.
    if !close
        .saved_ids
        .iter()
        .all(|id| fresh.saved_ids.contains(id))
    {
        return false;
    }
    // The note fingerprint is intentionally not compared against the
    // frozen close: a new save changes the mail note (and its fingerprint)
    // by design, and that growth keeps the row `unsaved` via the fresh
    // verdict. The fresh-vs-row fingerprint check in the caller already
    // guards mid-run edits.
    let mut locators: Vec<super::archive_state::ArchiveTaskLocator> = fresh
        .tasks
        .iter()
        .map(super::archive_state::ArchiveTaskLocator::from)
        .collect();
    locators.sort();
    locators.dedup();
    if locators != close.tasks {
        return false;
    }
    if fresh.has_open()
        || fresh.has_unknown()
        || fresh.merge_hold
        || fresh.identity_uncertain
        || !fresh.issues.is_empty()
        || !fresh.mail_complete()
    {
        return false;
    }
    true
}

/// Run the live archive phase. Filing commits are already durable; this
/// function never rolls them back. Non-auth per-thread failures isolate and
/// fail the run; `invalid_grant` aborts remaining mutations via `Err`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_live<R: GmailRead>(
    reader: &R,
    gog: &Gog,
    config: &GmailConfig,
    vault: &Path,
    account: &str,
    args: &SyncArgs,
    watch_threads: &[WatchThreadOutcome],
    saved_id_snapshot: &SavedIdSnapshot,
) -> Result<LiveArchiveOutcome, GmailError> {
    let mut outcome = LiveArchiveOutcome::default();
    if !config.archive_enabled() {
        return Ok(outcome);
    }
    if args.no_archive {
        outcome.skipped_no_archive = true;
        return Ok(outcome);
    }
    if args.dry_run {
        return Ok(outcome);
    }

    let host = vault_sync::hostname_slug();
    let mut enrollment = match EnrollmentFold::load(vault) {
        Ok(fold) => fold,
        Err(error) => {
            outcome.warnings.push(format!(
                "skipped archive apply: could not read the Gmail enrollment journal: {error}"
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };
    let mut index = match resolve::scan_index(
        vault,
        Path::new(config.folder()),
        config.link_label(),
    ) {
        Ok(index) => index,
        Err(error) => {
            outcome.warnings.push(format!(
                "skipped archive apply: {}: {}",
                error.kind(),
                error.message(),
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };
    let relations = match bob_config::load_relations_config(
        &bob_config::config_path(),
    ) {
        Ok(relations) => relations,
        Err(error) => {
            outcome.warnings.push(format!(
                "skipped archive apply: could not read relations config: {error}"
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };
    let mut scan = match scan_archive_evidence(
        vault,
        Path::new(config.folder()),
        &index,
        &relations,
    ) {
        Ok(scan) => scan,
        Err(error) => {
            outcome.warnings.push(format!(
                "skipped archive apply: {}: {}",
                error.kind(),
                error.message(),
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };
    scan = scan.with_saved_id_snapshot(saved_id_snapshot);
    let mut journal = match ArchiveJournal::load(vault) {
        Ok(journal) => journal,
        Err(error) => {
            outcome.warnings.push(format!(
                "skipped archive apply: could not read the Gmail archive journal: {error}"
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };

    // Labels first: planning needs the enrollment and receipt identities
    // before any baseline decision.
    let initial_labels = match discover::fetch_labels_once(reader, account) {
        Ok(labels) => labels,
        Err(error) => {
            if is_auth_abort(&error) {
                return Err(error);
            }
            outcome.warnings.push(format!(
                "skipped archive apply: Gmail label list failed: {}: {}",
                error.kind(),
                error.message(),
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };
    // Baseline observations require the modify grant. Read-only live runs
    // never append enablement/open state, preserving first-enable backlog
    // semantics for later preview and dry-run checks. A missing grant with
    // any archivable evidence is a setup error even before enablement,
    // while filing-only runs with open tasks stay quiet.
    if !has_modify_grant(gog, account)? {
        if scan.threads().any(|key| {
            scan.evidence(key)
                .is_some_and(|evidence| scan.archivable(evidence))
        }) {
            return Err(setup_missing_modify());
        }
        let receipt_identity = match ArchiveLabelIdentity::new(
            config.archive_label(),
            initial_labels
                .iter()
                .find(|label| label.name == config.archive_label())
                .map(|label| label.id.clone()),
        ) {
            Some(identity) => identity,
            None => return Ok(outcome),
        };
        let mut suppressed = BTreeSet::new();
        for thread in watch_threads {
            if thread.action == ACTION_LIFTED || thread.action == ACTION_BOUNCED
            {
                suppressed.insert(thread.key.clone());
            }
        }
        let policy = NeverSavePolicy::from_config(config);
        let input = PreviewInput {
            account,
            enrollment: &enrollment,
            scan: &scan,
            journal: &journal,
            suppressed: &suppressed,
            policy: &policy,
            labels: &initial_labels,
            receipt_label: receipt_identity,
        };
        let preview = archive_preview::preview(
            reader,
            &input,
            config,
            args.confirm.as_deref(),
        )?;
        outcome.warnings.extend(preview.warnings);
        if preview.failed {
            outcome.failed = true;
        }
        outcome.deferred.extend(preview.deferred);
        outcome.digest.clone_from(&preview.plan.digest);
        if matches!(
            &preview.plan.gate,
            archive_preview::ArchiveGate::Paused { reason }
                if *reason == archive_preview::GATE_MISMATCH
        ) {
            outcome.warnings.push(
                "the confirmation digest does not match the current Gmail preview".to_string(),
            );
            append_paused_rows(&preview.plan, &mut outcome);
            outcome.hints =
                super::report::archive_restore_hints(&scan, &journal);
            return Ok(outcome);
        }
        if preview.plan.rows.is_empty() {
            outcome.hints =
                super::report::archive_restore_hints(&scan, &journal);
            return Ok(outcome);
        }
        if preview.plan.gate.is_paused() {
            for row in &preview.plan.rows {
                outcome.rows.push(ArchiveRow {
                    thread_id: row.key.thread_id().to_string(),
                    note: row.note.clone(),
                    action: ACTION_PAUSED_LIVE,
                    reason: gate_reason(&preview.plan.gate),
                    decision: planned_decision(&row.decision),
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
                });
            }
            outcome.failed = true;
            outcome.hints =
                super::report::archive_restore_hints(&scan, &journal);
            return Ok(outcome);
        }
        return Err(setup_missing_modify());
    }

    // Baseline: enablement plus open observations. Transitions only; an
    // unchanged hourly run appends nothing.
    if ensure_baseline(
        vault,
        &host,
        config,
        account,
        &initial_labels,
        &scan,
        &mut journal,
    )? {
        enrollment = match EnrollmentFold::load(vault) {
            Ok(fold) => fold,
            Err(error) => {
                outcome.warnings.push(format!(
                    "skipped archive apply: could not re-read enrollment: {error}"
                ));
                outcome.failed = true;
                return Ok(outcome);
            }
        };
        index = match resolve::scan_index(
            vault,
            Path::new(config.folder()),
            config.link_label(),
        ) {
            Ok(index) => index,
            Err(error) => {
                outcome.warnings.push(format!(
                    "skipped archive apply: {}: {}",
                    error.kind(),
                    error.message(),
                ));
                outcome.failed = true;
                return Ok(outcome);
            }
        };
        scan = match scan_archive_evidence(
            vault,
            Path::new(config.folder()),
            &index,
            &relations,
        ) {
            Ok(scan) => scan,
            Err(error) => {
                outcome.warnings.push(format!(
                    "skipped archive apply: {}: {}",
                    error.kind(),
                    error.message(),
                ));
                outcome.failed = true;
                return Ok(outcome);
            }
        };
        scan = scan.with_saved_id_snapshot(saved_id_snapshot);
    }
    if !scan.scan_complete() {
        outcome.warnings.push(
            "skipped archive apply: the vault scan did not read every note"
                .to_string(),
        );
        outcome.failed = true;
        return Ok(outcome);
    }

    let labels = match discover::fetch_labels_once(reader, account) {
        Ok(labels) => labels,
        Err(error) => {
            if is_auth_abort(&error) {
                return Err(error);
            }
            outcome.warnings.push(format!(
                "skipped archive apply: Gmail label list failed: {}: {}",
                error.kind(),
                error.message(),
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };
    let receipt_identity = match ArchiveLabelIdentity::new(
        config.archive_label(),
        labels
            .iter()
            .find(|label| label.name == config.archive_label())
            .map(|label| label.id.clone()),
    ) {
        Some(identity) => identity,
        None => {
            outcome.warnings.push(
                "skipped archive apply: the archive receipt label is invalid"
                    .to_string(),
            );
            outcome.failed = true;
            return Ok(outcome);
        }
    };
    let enrollment_label = unique_named_enrollment_label(
        &labels,
        config.sync_label().unwrap_or_default(),
    );

    let mut suppressed = BTreeSet::new();
    for thread in watch_threads {
        if thread.action == ACTION_LIFTED || thread.action == ACTION_BOUNCED {
            suppressed.insert(thread.key.clone());
        }
    }
    let policy = NeverSavePolicy::from_config(config);
    let input = PreviewInput {
        account,
        enrollment: &enrollment,
        scan: &scan,
        journal: &journal,
        suppressed: &suppressed,
        policy: &policy,
        labels: &labels,
        receipt_label: receipt_identity.clone(),
    };
    let preview = archive_preview::preview(
        reader,
        &input,
        config,
        args.confirm.as_deref(),
    )?;
    outcome.warnings.extend(preview.warnings);
    if preview.failed {
        outcome.failed = true;
    }
    outcome.deferred.extend(preview.deferred);
    outcome.digest.clone_from(&preview.plan.digest);

    // A supplied digest mismatch is a hard no-write result, even when the
    // plan would otherwise be ungated or contains no actionable rows.
    if matches!(
        &preview.plan.gate,
        archive_preview::ArchiveGate::Paused { reason }
            if *reason == archive_preview::GATE_MISMATCH
    ) {
        outcome.warnings.push(
            "the confirmation digest does not match the current Gmail preview"
                .to_string(),
        );
        append_paused_rows(&preview.plan, &mut outcome);
        outcome.hints = super::report::archive_restore_hints(&scan, &journal);
        return Ok(outcome);
    }

    // Previously owned receipts only, captured before any recovery or close
    // persistence in this run: newly archived or reconciled threads in this
    // run must not let go immediately on stale reads. Let-go converges next
    // run.
    let owned_before_snapshot: Vec<super::archive_state::ArchiveReceipt> =
        journal.owned_receipts().cloned().collect();

    if !preview.plan.gate.is_paused()
        && let Some(enrollment_id) = labels
            .iter()
            .find(|label| label.name == config.sync_label().unwrap_or_default())
            .map(|label| label.id.as_str())
    {
        reconcile_ready_receipts(
            reader,
            vault,
            &host,
            account,
            &scan,
            &mut journal,
            &receipt_identity,
            enrollment_id,
            &policy,
            &mut outcome,
        )?;
    }

    if preview.plan.rows.is_empty() {
        // Let-go is driven by previously owned receipts, independent of
        // whether this run has a new close to preview. A reply or manual
        // Inbox move must release its receipt on an otherwise idle sync.
        let owned_before = owned_before_snapshot.clone();
        release_owned(
            vault,
            &host,
            reader,
            gog,
            account,
            &labels,
            enrollment_label.as_ref(),
            &mut journal,
            &owned_before,
            &mut outcome,
        )?;
        outcome.hints = super::report::archive_restore_hints(&scan, &journal);
        return Ok(outcome);
    }

    // Paused batches create no labels and append no terminal outcomes beyond
    // the baseline observations already recorded above. Every preview row,
    // whether archive or kept, reports the gate reason while paused. Let-go
    // still runs on every enabled live run with the modify grant, including
    // paused runs; `invalid_grant` still aborts.
    if preview.plan.gate.is_paused() {
        for row in &preview.plan.rows {
            outcome.rows.push(ArchiveRow {
                thread_id: row.key.thread_id().to_string(),
                note: row.note.clone(),
                action: ACTION_PAUSED_LIVE,
                reason: gate_reason(&preview.plan.gate),
                decision: planned_decision(&row.decision),
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
            });
        }
        outcome.failed = true;
        let owned_before = owned_before_snapshot.clone();
        release_owned(
            vault,
            &host,
            reader,
            gog,
            account,
            &labels,
            enrollment_label.as_ref(),
            &mut journal,
            &owned_before,
            &mut outcome,
        )?;
        outcome.hints = super::report::archive_restore_hints(&scan, &journal);
        return Ok(outcome);
    }

    // The gate is open: persist close generations, then mutate. The modify
    // grant was verified before baseline observations were recorded.
    persist_closes(
        vault,
        &host,
        config,
        &scan,
        &mut journal,
        &preview.plan.rows,
        args.confirm.as_deref(),
        &mut outcome,
    )?;
    // Reload after close persistence so terminal planning sees fresh folds.
    journal = match ArchiveJournal::load(vault) {
        Ok(journal) => journal,
        Err(error) => {
            outcome.warnings.push(format!(
                "skipped archive mutations: could not re-read the archive journal: {error}"
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };

    // Honor the planned decision set: resolve or create the receipt label
    // only when at least one row is planned Archive. A fresh upgrade from
    // Kept to Archive never widens a digest-confirmed batch.
    let needs_receipt = preview
        .plan
        .rows
        .iter()
        .any(|row| matches!(row.decision, PreviewDecision::Archive { .. }));
    let enrollment_label = match enrollment_label {
        Some(label) => label,
        None => {
            outcome.warnings.push(format!(
                "skipped archive mutations: Gmail enrollment label {:?} is missing or ambiguous",
                config.sync_label().unwrap_or_default(),
            ));
            outcome.failed = true;
            let owned_before = owned_before_snapshot.clone();
            release_owned(
                vault,
                &host,
                reader,
                gog,
                account,
                &labels,
                None,
                &mut journal,
                &owned_before,
                &mut outcome,
            )?;
            outcome.hints =
                super::report::archive_restore_hints(&scan, &journal);
            return Ok(outcome);
        }
    };
    let (receipt, resolved_receipt_identity) = if needs_receipt {
        let receipt = match resolve_or_create_receipt_label(
            gog,
            account,
            config.archive_label(),
            &enrollment_label,
        ) {
            Ok(receipt) => receipt,
            Err(error) => {
                if is_auth_abort(&error) {
                    return Err(error);
                }
                outcome.warnings.push(format!(
                    "skipped archive mutations: {}: {}",
                    error.kind(),
                    error.message(),
                ));
                outcome.failed = true;
                let owned_before = owned_before_snapshot.clone();
                release_owned(
                    vault,
                    &host,
                    reader,
                    gog,
                    account,
                    &labels,
                    Some(&enrollment_label),
                    &mut journal,
                    &owned_before,
                    &mut outcome,
                )?;
                outcome.hints =
                    super::report::archive_restore_hints(&scan, &journal);
                return Ok(outcome);
            }
        };
        let resolved = ArchiveLabelIdentity::new(
            receipt.name(),
            Some(receipt.id().to_string()),
        )
        .ok_or_else(|| {
            GmailError::runtime(
                "archive_label",
                "resolved Gmail receipt label has an invalid identity"
                    .to_string(),
            )
        })?;
        (Some(receipt), Some(resolved))
    } else {
        (None, None)
    };

    let enrollment_id = enrollment_label.id.clone();
    // Previously owned receipts only: newly archived threads in this run must
    // not let go immediately on stale reads. Let-go converges next run.
    // Reuse the pre-recovery snapshot so reconciled receipts also converge
    // next run.
    let owned_before = owned_before_snapshot.clone();
    // Per-thread isolation: acquire the vault maintenance lock with a bounded
    // wait, re-read that thread's note and linking task files through the
    // archive scan rules, compare against the frozen close, then issue that
    // thread's single batch write and release the lock. Never hold the lock
    // across more than one thread's write. Never expand the frozen IDs.
    for row in &preview.plan.rows {
        let Some(_thread_lock) = acquire_thread_lock() else {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: vault lock busy; work is held",
                row.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        };
        // Test seam for the reopened-between-planning-and-mutation fault
        // test: when set, simulate an external reopen after planning so the
        // revalidation below holds without mutating.
        if std::env::var("BOB_GMAIL_ARCHIVE_TEST_REOPEN_THREAD")
            .is_ok_and(|id| id == row.key.thread_id())
        {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive evidence changed before mutation",
                row.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        }
        // Re-read vault evidence under the lock; the loop never trusts the
        // pre-plan scan for the mutation decision.
        let fresh_evidence = (|| {
            let fresh_index = resolve::scan_index(
                vault,
                Path::new(config.folder()),
                config.link_label(),
            )
            .ok()?;
            let fresh_scan = scan_archive_evidence(
                vault,
                Path::new(config.folder()),
                &fresh_index,
                &relations,
            )
            .ok()?;
            fresh_scan.evidence(&row.key).cloned()
        })();
        let Some(fresh_evidence) = fresh_evidence else {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive evidence changed before mutation",
                row.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        };
        let Some(close) = journal.unconsumed_close(&row.key).cloned() else {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive close is no longer open",
                row.key.thread_id(),
            ));
            continue;
        };
        // The plan and the persisted close must agree on frozen evidence.
        // Current scan IDs may grow after the close; those new Inbox messages
        // stay unsaved and never widen its archive set.
        if close.saved_ids != row.saved_ids
            || close.note != row.note
            || close.tasks != row.tasks
            || fresh_evidence
                .note_fingerprint
                .as_deref()
                .unwrap_or_default()
                != row.note_fingerprint
        {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive evidence changed before mutation",
                row.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        };
        if close.generation != row.generation.as_deref().unwrap_or_default()
            && !row.historical
        {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive close changed before mutation",
                row.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        }
        if !fresh_matches_close(&fresh_evidence, &close) {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive evidence changed before mutation",
                row.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        }
        let thread = match reader.thread_metadata(account, row.key.thread_id())
        {
            Ok(thread) => thread,
            Err(error) => {
                if is_auth_abort(&error) {
                    return Err(error);
                }
                if error.kind() == "not_found" {
                    append_kept(
                        vault,
                        &host,
                        &journal,
                        &row.key,
                        &close,
                        KEPT_GONE,
                        &mut outcome,
                    )?;
                    journal = ArchiveJournal::load(vault).unwrap_or(journal);
                    continue;
                }
                outcome.warnings.push(format!(
                    "skipped Gmail thread {}: {}: {}",
                    row.key.thread_id(),
                    error.kind(),
                    error.message(),
                ));
                outcome.failed = true;
                outcome.deferred.push(row.key.clone());
                continue;
            }
        };
        let effective_receipt =
            resolved_receipt_identity.as_ref().unwrap_or(&close.label);
        let mut verdict = archive_preview::remote_verdict_for_saved_ids(
            reader,
            account,
            &fresh_evidence,
            &close.saved_ids,
            &thread,
            &enrollment_id,
            effective_receipt,
            &policy,
        )?;
        // Test seam for the planned-Kept-turns-archivable fault test: when
        // set, simulate a fresh upgrade to archivable after planning so the
        // planned-decision gate holds without widening the batch.
        if std::env::var("BOB_GMAIL_ARCHIVE_TEST_FORCE_ARCHIVABLE")
            .is_ok_and(|id| id == row.key.thread_id())
            && matches!(verdict, archive_preview::RemoteVerdict::Kept { .. })
        {
            let mut ids = close.saved_ids.clone();
            ids.sort();
            ids.dedup();
            verdict =
                archive_preview::RemoteVerdict::Archive { archive_ids: ids };
        }
        let planned_archive =
            matches!(row.decision, PreviewDecision::Archive { .. });
        match verdict {
            archive_preview::RemoteVerdict::Kept { reason } => {
                append_kept(
                    vault,
                    &host,
                    &journal,
                    &row.key,
                    &close,
                    reason,
                    &mut outcome,
                )?;
                journal = ArchiveJournal::load(vault).unwrap_or(journal);
            }
            archive_preview::RemoteVerdict::Held { reason, warning } => {
                if reason == HOLD_UNRECONCILED {
                    reconcile_recovered(
                        vault,
                        &host,
                        &thread,
                        &close,
                        effective_receipt,
                        &mut journal,
                        &mut outcome,
                    )?;
                } else {
                    if let Some(warning) = warning {
                        outcome.warnings.push(warning);
                    }
                    if reason == HOLD_FETCH_FAILED
                        || reason == HOLD_UNRESOLVABLE
                    {
                        outcome.failed = true;
                    }
                }
            }
            archive_preview::RemoteVerdict::Archive { archive_ids } => {
                // Honor the planned decision set: a row planned Kept or Held
                // whose fresh verdict became Archive is held for the next run.
                // This never widens a digest-confirmed batch and never exceeds
                // archive_max. A fresh downgrade is still honored above.
                if !planned_archive {
                    outcome.warnings.push(format!(
                        "skipped Gmail thread {}: archive evidence changed to archivable after planning; work is held",
                        row.key.thread_id(),
                    ));
                    outcome.failed = true;
                    continue;
                }
                let (Some(live_receipt), Some(resolved_id)) =
                    (receipt.as_ref(), resolved_receipt_identity.as_ref())
                else {
                    outcome.warnings.push(format!(
                        "skipped Gmail thread {}: archive receipt is not resolved; work is held",
                        row.key.thread_id(),
                    ));
                    outcome.failed = true;
                    continue;
                };
                if archive_ids.len() > 1000 {
                    outcome.warnings.push(format!(
                        "skipped Gmail thread {}: more than 1,000 messages cannot archive in one call",
                        row.key.thread_id(),
                    ));
                    outcome.failed = true;
                    continue;
                }
                if archive_ids.is_empty() {
                    continue;
                }
                let states = match message_states(&thread, &archive_ids) {
                    Some(states) => states,
                    None => {
                        outcome.warnings.push(format!(
                            "skipped Gmail thread {}: fresh Gmail metadata is missing an archived message",
                            row.key.thread_id(),
                        ));
                        outcome.failed = true;
                        continue;
                    }
                };
                let diff =
                    match super::archive_transport::LabelDiff::for_messages(
                        LabelOperation::Archive,
                        live_receipt,
                        &states,
                    ) {
                        Ok(diff) => diff,
                        Err(error) => {
                            outcome.warnings.push(format!(
                                "skipped Gmail thread {}: {}: {}",
                                row.key.thread_id(),
                                error.kind(),
                                error.message(),
                            ));
                            outcome.failed = true;
                            continue;
                        }
                    };
                // Journal-before-mutate: plan the terminal event with the
                // same IDs and label the write will use. A `None` plan holds
                // without writing.
                let planned = journal.event_for_terminal(
                    &row.key,
                    &close.generation,
                    super::archive_state::ArchiveTerminalKind::Archived {
                        message_ids: archive_ids.clone(),
                        label: resolved_id.clone(),
                        recovered: false,
                    },
                );
                let Some(planned) = planned else {
                    outcome.warnings.push(format!(
                        "skipped Gmail thread {}: archive close is no longer plannable; work is held",
                        row.key.thread_id(),
                    ));
                    outcome.failed = true;
                    continue;
                };
                match gog.archive_batch_modify(account, &diff) {
                    Ok(_) => {
                        ArchiveJournal::append(vault, &host, &planned)
                            .map_err(|error| {
                                GmailError::runtime(
                                    "journal",
                                    format!(
                                        "could not append the Gmail archive journal: {error}"
                                    ),
                                )
                            })?;
                        journal =
                            ArchiveJournal::load(vault).unwrap_or(journal);
                        outcome.rows.push(ArchiveRow {
                            thread_id: row.key.thread_id().to_string(),
                            note: close.note.clone(),
                            action: ACTION_ARCHIVED,
                            reason: if close.historical {
                                "backlog"
                            } else {
                                "eligible"
                            },
                            decision: "would_archive".to_string(),
                            messages: archive_ids,
                            tasks: task_refs(&close.tasks),
                        });
                    }
                    Err(error) => {
                        if is_auth_abort(&error) {
                            return Err(error);
                        }
                        outcome.warnings.push(format!(
                            "skipped Gmail thread {}: {}: {}",
                            row.key.thread_id(),
                            error.kind(),
                            error.message(),
                        ));
                        outcome.failed = true;
                    }
                }
            }
        }
    }

    // Ownership release: previously owned archives with a new Inbox reply or
    // manual Inbox return let go exactly once. Reopening alone never cleans
    // up; only live INBOX membership does.
    release_owned(
        vault,
        &host,
        reader,
        gog,
        account,
        &labels,
        Some(&enrollment_label),
        &mut journal,
        &owned_before,
        &mut outcome,
    )?;

    outcome.hints = super::report::archive_restore_hints(&scan, &journal);

    Ok(outcome)
}

fn gate_reason(gate: &archive_preview::ArchiveGate) -> &'static str {
    match gate {
        archive_preview::ArchiveGate::Open { .. } => "eligible",
        archive_preview::ArchiveGate::Paused { reason } => reason,
    }
}

fn planned_decision(decision: &PreviewDecision) -> String {
    match decision {
        PreviewDecision::Archive { .. } => "would_archive".to_string(),
        PreviewDecision::Kept { reason } => format!("kept:{reason}"),
    }
}

fn task_refs(
    tasks: &[super::archive_state::ArchiveTaskLocator],
) -> Vec<ArchiveTaskRef> {
    tasks
        .iter()
        .map(|task| ArchiveTaskRef {
            path: task.path.clone(),
            line: task.line,
            block_id: task.block_id.clone(),
        })
        .collect()
}

fn append_paused_rows(
    plan: &archive_preview::ArchivePlan,
    outcome: &mut LiveArchiveOutcome,
) {
    let reason = gate_reason(&plan.gate);
    for row in &plan.rows {
        outcome.rows.push(ArchiveRow {
            thread_id: row.key.thread_id().to_string(),
            note: row.note.clone(),
            action: ACTION_PAUSED_LIVE,
            reason,
            decision: planned_decision(&row.decision),
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
        });
    }
    outcome.failed = true;
}

/// Recover a remote archive whose terminal journal append failed. The
/// read-only preview deliberately omits an `unreconciled` hold from its
/// actionable rows, so check unconsumed closes once more at the live apply
/// boundary before returning an otherwise empty report.
#[allow(clippy::too_many_arguments)]
fn reconcile_ready_receipts<R: GmailRead>(
    reader: &R,
    vault: &Path,
    host: &str,
    account: &str,
    scan: &super::archive_scan::ArchiveScan,
    journal: &mut ArchiveJournal,
    receipt_label: &ArchiveLabelIdentity,
    enrollment_id: &str,
    policy: &NeverSavePolicy,
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    let mut pending = Vec::new();
    for key in scan.threads() {
        let (Some(evidence), Some(close)) =
            (scan.evidence(key), journal.unconsumed_close(key))
        else {
            continue;
        };
        pending.push((evidence.clone(), close.clone()));
    }

    for (evidence, close) in pending {
        let thread =
            match reader.thread_metadata(account, close.key.thread_id()) {
                Ok(thread) => thread,
                Err(error) => {
                    if is_auth_abort(&error) {
                        return Err(error);
                    }
                    outcome.warnings.push(format!(
                        "skipped Gmail thread {}: {}: {}",
                        close.key.thread_id(),
                        error.kind(),
                        error.message(),
                    ));
                    outcome.failed = true;
                    continue;
                }
            };
        match archive_preview::remote_verdict_for_saved_ids(
            reader,
            account,
            &evidence,
            &close.saved_ids,
            &thread,
            enrollment_id,
            receipt_label,
            policy,
        )? {
            archive_preview::RemoteVerdict::Held {
                reason: HOLD_UNRECONCILED,
                ..
            } => reconcile_recovered(
                vault,
                host,
                &thread,
                &close,
                receipt_label,
                journal,
                outcome,
            )?,
            archive_preview::RemoteVerdict::Held { reason, warning } => {
                if let Some(warning) = warning {
                    outcome.warnings.push(warning);
                }
                if reason == archive_preview::HOLD_FETCH_FAILED
                    || reason == archive_preview::HOLD_UNRESOLVABLE
                {
                    outcome.failed = true;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Persist enablement and open observations. Returns true when anything was
/// appended and the caller must reload vault-derived folds.
fn ensure_baseline(
    vault: &Path,
    host: &str,
    config: &GmailConfig,
    account: &str,
    labels: &[GmailLabel],
    scan: &super::archive_scan::ArchiveScan,
    journal: &mut ArchiveJournal,
) -> Result<bool, GmailError> {
    let mut appended = false;
    let configured_label_id =
        unique_named_enrollment_label(labels, config.archive_label())
            .map(|label| label.id.clone());
    let configured_label =
        ArchiveLabelIdentity::new(config.archive_label(), configured_label_id);
    if let Some(label) = configured_label {
        let needs_enablement = match journal.enablement(account) {
            Some((_, current)) => current.name != label.name,
            None => !journal.is_enabled(account),
        };
        if needs_enablement
            && let Some(event) =
                journal.event_for_enablement(account, true, label)
        {
            ArchiveJournal::append(vault, host, &event).map_err(|error| {
                GmailError::runtime(
                    "journal",
                    format!(
                        "could not append the Gmail archive journal: {error}"
                    ),
                )
            })?;
            appended = true;
        }
    }
    if appended {
        *journal = ArchiveJournal::load(vault).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not read the Gmail archive journal: {error}"),
            )
        })?;
    }
    let mut opens = Vec::new();
    let mut keys: Vec<&ThreadKey> = scan.threads().collect();
    keys.sort();
    for key in keys {
        let Some(evidence) = scan.evidence(key) else {
            continue;
        };
        if let Some(event) = journal.event_for_open(scan, evidence) {
            opens.push(event);
        }
    }
    for event in &opens {
        ArchiveJournal::append(vault, host, event).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not append the Gmail archive journal: {error}"),
            )
        })?;
    }
    if !opens.is_empty() {
        *journal = ArchiveJournal::load(vault).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not read the Gmail archive journal: {error}"),
            )
        })?;
        appended = true;
    }
    Ok(appended)
}

fn unique_named_enrollment_label(
    labels: &[GmailLabel],
    name: &str,
) -> Option<GmailLabel> {
    let mut matches = labels.iter().filter(|label| label.name == name);
    let label = matches.next()?;
    matches.next().is_none().then(|| label.clone())
}

/// Persist close generations for every actionable preview row. Historical
/// backlog rows need an explicit matching digest; fresh closes record the
/// observed-open witness. Terminal outcomes are never written here.
#[allow(clippy::too_many_arguments)]
fn persist_closes(
    vault: &Path,
    host: &str,
    config: &GmailConfig,
    scan: &super::archive_scan::ArchiveScan,
    journal: &mut ArchiveJournal,
    rows: &[archive_preview::ArchivePreviewRow],
    confirm: Option<&str>,
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    let _ = config;
    let mut events = Vec::new();
    for row in rows {
        let Some(evidence) = scan.evidence(&row.key) else {
            continue;
        };
        if row.historical {
            let Some(digest) = confirm else {
                continue;
            };
            // The preview digest is bare hex; the journal stores a prefixed
            // `sha256:` confirmation digest.
            let confirmed = if digest.starts_with("sha256:") {
                digest.to_string()
            } else {
                format!("sha256:{digest}")
            };
            let current = match journal.enablement(row.key.account()) {
                Some((_, label)) => label.clone(),
                None => continue,
            };
            if let Some(event) = journal.event_for_confirmed_backlog(
                scan, evidence, current, &confirmed,
            ) {
                events.push(event);
            } else {
                outcome.warnings.push(format!(
                    "skipped Gmail thread {}: historical backlog needs an explicit confirmed close",
                    row.key.thread_id(),
                ));
            }
        } else if let Some(event) = {
            let current = match journal.enablement(row.key.account()) {
                Some((_, label)) => label.clone(),
                None => continue,
            };
            journal.event_for_close(scan, evidence, current)
        } {
            events.push(event);
        }
    }
    for event in &events {
        ArchiveJournal::append(vault, host, event).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not append the Gmail archive journal: {error}"),
            )
        })?;
    }
    if !events.is_empty() {
        *journal = ArchiveJournal::load(vault).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not read the Gmail archive journal: {error}"),
            )
        })?;
    }
    Ok(())
}

fn message_states(
    thread: &crate::native::gmail::model::GmailThread,
    ids: &[String],
) -> Option<Vec<MessageLabelState>> {
    let mut states = Vec::with_capacity(ids.len());
    for id in ids {
        let message = thread.messages.iter().find(|m| &m.id == id)?;
        states.push(MessageLabelState {
            message_id: message.id.clone(),
            label_ids: message.label_ids.iter().cloned().collect(),
        });
    }
    Some(states)
}

fn append_kept(
    vault: &Path,
    host: &str,
    journal: &ArchiveJournal,
    key: &ThreadKey,
    close: &ArchiveCloseGeneration,
    reason: &str,
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    let Some(event) = journal.event_for_terminal(
        key,
        &close.generation,
        ArchiveTerminalKind::Kept {
            reason: reason.to_string(),
        },
    ) else {
        outcome.warnings.push(format!(
            "skipped Gmail thread {}: archive close is no longer plannable; work is held",
            key.thread_id(),
        ));
        outcome.failed = true;
        return Ok(());
    };
    ArchiveJournal::append(vault, host, &event).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not append the Gmail archive journal: {error}"),
        )
    })?;
    let reloaded = ArchiveJournal::load(vault).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not read the Gmail archive journal: {error}"),
        )
    })?;
    let _ = reloaded;
    let kept = kept_reason(reason);
    let mut messages = close.saved_ids.clone();
    messages.sort();
    messages.dedup();
    outcome.rows.push(ArchiveRow {
        thread_id: key.thread_id().to_string(),
        note: close.note.clone(),
        action: ACTION_KEPT_LIVE,
        reason: kept,
        decision: format!("kept:{kept}"),
        messages,
        tasks: task_refs(&close.tasks),
    });
    Ok(())
}

fn kept_reason(reason: &str) -> &'static str {
    match reason {
        s if s == KEPT_UNSAVED => KEPT_UNSAVED,
        s if s == KEPT_CONSENT_REMOVED => KEPT_CONSENT_REMOVED,
        s if s == KEPT_POLICY => KEPT_POLICY,
        s if s == KEPT_GONE => KEPT_GONE,
        s if s == KEPT_MANUAL => KEPT_MANUAL,
        _ => KEPT_POLICY,
    }
}

/// Reconcile a crash that archived remotely but never journaled: when the
/// live thread carries the receipt with no matching archived event, record
/// a recovered archive once without another remote write.
fn reconcile_recovered(
    vault: &Path,
    host: &str,
    thread: &crate::native::gmail::model::GmailThread,
    close: &super::archive_state::ArchiveCloseGeneration,
    receipt_label: &ArchiveLabelIdentity,
    journal: &mut ArchiveJournal,
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    let previously_owned: BTreeSet<String> = journal
        .owned_receipts()
        .filter(|receipt| {
            receipt.key == close.key
                && receipt.generation != close.generation
                && (receipt.label.id == receipt_label.id
                    || (receipt.label.id.is_none()
                        && receipt.label.name == receipt_label.name))
        })
        .flat_map(|receipt| receipt.message_ids.iter().cloned())
        .collect();
    let mut recovered: Vec<String> = thread
        .messages
        .iter()
        .filter(|message| {
            close.saved_ids.binary_search(&message.id).is_ok()
                && !previously_owned.contains(&message.id)
                && receipt_label.id.as_ref().is_some_and(|id| {
                    message.label_ids.iter().any(|label| label == id)
                })
        })
        .map(|message| message.id.clone())
        .collect();
    recovered.sort();
    recovered.dedup();
    if recovered.is_empty() {
        let already_owned = thread.messages.iter().any(|message| {
            close.saved_ids.binary_search(&message.id).is_ok()
                && previously_owned.contains(&message.id)
                && receipt_label.id.as_ref().is_some_and(|id| {
                    message.label_ids.iter().any(|label| label == id)
                })
        });
        if already_owned {
            append_kept(
                vault,
                host,
                journal,
                &close.key,
                close,
                KEPT_MANUAL,
                outcome,
            )?;
            *journal = ArchiveJournal::load(vault).map_err(|error| {
                GmailError::runtime(
                    "journal",
                    format!(
                        "could not read the Gmail archive journal: {error}"
                    ),
                )
            })?;
        }
        return Ok(());
    }
    let Some(event) = journal.event_for_terminal(
        &close.key,
        &close.generation,
        ArchiveTerminalKind::Archived {
            message_ids: recovered.clone(),
            label: receipt_label.clone(),
            recovered: true,
        },
    ) else {
        outcome.warnings.push(format!(
            "skipped Gmail thread {}: archive close is no longer plannable; work is held",
            close.key.thread_id(),
        ));
        outcome.failed = true;
        return Ok(());
    };
    ArchiveJournal::append(vault, host, &event).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not append the Gmail archive journal: {error}"),
        )
    })?;
    *journal = ArchiveJournal::load(vault).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not read the Gmail archive journal: {error}"),
        )
    })?;
    outcome.rows.push(ArchiveRow {
        thread_id: close.key.thread_id().to_string(),
        note: close.note.clone(),
        action: ACTION_ARCHIVED,
        reason: REASON_RECOVERED,
        decision: "would_archive".to_string(),
        messages: recovered,
        tasks: task_refs(&close.tasks),
    });
    Ok(())
}

/// Release previously owned archives that regained INBOX membership. A new
/// reply or manual Move to Inbox removes the receipt only and consumes the
/// close exactly once; already-cleaned crashes converge without reclaiming.
#[allow(clippy::too_many_arguments)]
fn release_owned<R: GmailRead>(
    vault: &Path,
    host: &str,
    reader: &R,
    gog: &Gog,
    account: &str,
    labels: &[GmailLabel],
    enrollment_label: Option<&GmailLabel>,
    journal: &mut ArchiveJournal,
    owned: &[super::archive_state::ArchiveReceipt],
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    if !owned.is_empty() && enrollment_label.is_none() {
        outcome.warnings.push(
            "skipped Gmail archive release: the enrollment label is missing or ambiguous; work is held".to_string(),
        );
        outcome.failed = true;
        return Ok(());
    }
    for receipt in owned {
        let thread =
            match reader.thread_metadata(account, receipt.key.thread_id()) {
                Ok(thread) => thread,
                Err(error) => {
                    if is_auth_abort(&error) {
                        return Err(error);
                    }
                    outcome.warnings.push(format!(
                        "skipped Gmail thread {}: {}: {}",
                        receipt.key.thread_id(),
                        error.kind(),
                        error.message(),
                    ));
                    outcome.failed = true;
                    continue;
                }
            };
        // Verify thread and message identity the way `remote_verdict` does:
        // only recorded IDs on the expected thread may release.
        if thread.id != receipt.key.thread_id()
            || thread.messages.is_empty()
            || thread
                .messages
                .iter()
                .any(|message| message.thread_id != receipt.key.thread_id())
        {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: unresolvable: Gmail returned a different thread identity",
                receipt.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        }
        let has_inbox = thread.messages.iter().any(|message| {
            message
                .label_ids
                .iter()
                .any(|label| label.eq_ignore_ascii_case("INBOX"))
        });
        if !has_inbox {
            continue;
        }
        let Some(enrollment_label) = enrollment_label else {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: the enrollment label is missing or ambiguous; work is held",
                receipt.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        };
        let live_receipt = match resolve_existing_receipt_label(
            labels,
            &receipt.label,
            enrollment_label,
        ) {
            Ok(Some(label)) => label,
            Ok(None) => {
                outcome.warnings.push(format!(
                    "skipped Gmail thread {}: the recorded receipt label is missing or ambiguous; work is held",
                    receipt.key.thread_id(),
                ));
                outcome.failed = true;
                continue;
            }
            Err(error) => {
                outcome.warnings.push(format!(
                    "skipped Gmail thread {}: {}: {}",
                    receipt.key.thread_id(),
                    error.kind(),
                    error.message(),
                ));
                outcome.failed = true;
                continue;
            }
        };
        let receipt_identity = ArchiveLabelIdentity::new(
            receipt.label.name.clone(),
            Some(live_receipt.id().to_string()),
        )
        .expect("resolved receipt identity remains valid");
        let mut bearing: Vec<String> = thread
            .messages
            .iter()
            .filter(|message| {
                receipt.message_ids.contains(&message.id)
                    && message
                        .label_ids
                        .iter()
                        .any(|label| label == live_receipt.id())
            })
            .map(|message| message.id.clone())
            .collect();
        bearing.sort();
        bearing.dedup();
        // Crash convergence: remote cleanup already removed the receipt but
        // the journal never recorded it. Record the release without another
        // remote write and without reclaiming mail.
        if bearing.is_empty() {
            let Some(event) = journal.event_for_terminal(
                &receipt.key,
                &receipt.generation,
                ArchiveTerminalKind::LetGo {
                    message_ids: receipt.message_ids.clone(),
                    label: receipt_identity.clone(),
                },
            ) else {
                outcome.warnings.push(format!(
                    "skipped Gmail thread {}: archive close is no longer plannable; work is held",
                    receipt.key.thread_id(),
                ));
                outcome.failed = true;
                continue;
            };
            ArchiveJournal::append(vault, host, &event).map_err(|error| {
                GmailError::runtime(
                    "journal",
                    format!(
                        "could not append the Gmail archive journal: {error}"
                    ),
                )
            })?;
            *journal = ArchiveJournal::load(vault).map_err(|error| {
                GmailError::runtime(
                    "journal",
                    format!(
                        "could not read the Gmail archive journal: {error}"
                    ),
                )
            })?;
            let recorded = journal.close_for(&receipt.key, &receipt.generation);
            let mut crash_messages = receipt.message_ids.clone();
            crash_messages.sort();
            crash_messages.dedup();
            outcome.rows.push(ArchiveRow {
                thread_id: receipt.key.thread_id().to_string(),
                note: recorded
                    .map(|close| close.note.clone())
                    .unwrap_or_default(),
                action: ACTION_LET_GO,
                reason: REASON_MANUAL_RETURN,
                decision: "let_go".to_string(),
                messages: crash_messages,
                tasks: recorded
                    .map(|close| task_refs(&close.tasks))
                    .unwrap_or_default(),
            });
            continue;
        }
        // Build the let-go diff against current receipt-bearing state.
        let states: Vec<MessageLabelState> = bearing
            .iter()
            .filter_map(|id| {
                thread.messages.iter().find(|m| &m.id == id).map(|message| {
                    MessageLabelState {
                        message_id: message.id.clone(),
                        label_ids: message.label_ids.iter().cloned().collect(),
                    }
                })
            })
            .collect();
        let diff = match super::archive_transport::LabelDiff::for_messages(
            LabelOperation::LetGo,
            &live_receipt,
            &states,
        ) {
            Ok(diff) => diff,
            Err(error) => {
                outcome.warnings.push(format!(
                    "skipped Gmail thread {}: {}: {}",
                    receipt.key.thread_id(),
                    error.kind(),
                    error.message(),
                ));
                outcome.failed = true;
                continue;
            }
        };
        if diff.is_empty() {
            continue;
        }
        // Journal-before-mutate: plan the terminal let-go before the Gmail
        // write using the same IDs and label. A `None` plan holds without
        // writing.
        let planned_let_go = journal.event_for_terminal(
            &receipt.key,
            &receipt.generation,
            ArchiveTerminalKind::LetGo {
                message_ids: bearing.clone(),
                label: receipt_identity.clone(),
            },
        );
        let Some(planned_let_go) = planned_let_go else {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive close is no longer plannable; work is held",
                receipt.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        };
        match gog.archive_batch_modify(account, &diff) {
            Ok(_) => {
                let event = planned_let_go;
                ArchiveJournal::append(vault, host, &event).map_err(
                    |error| {
                        GmailError::runtime(
                            "journal",
                            format!(
                                "could not append the Gmail archive journal: {error}"
                            ),
                        )
                    },
                )?;
                *journal = ArchiveJournal::load(vault).map_err(|error| {
                    GmailError::runtime(
                        "journal",
                        format!(
                            "could not read the Gmail archive journal: {error}"
                        ),
                    )
                })?;
                let reason = if thread.messages.iter().any(|message| {
                    message
                        .label_ids
                        .iter()
                        .any(|label| label.eq_ignore_ascii_case("INBOX"))
                        && !receipt.message_ids.contains(&message.id)
                }) {
                    REASON_REPLIED
                } else {
                    REASON_MANUAL_RETURN
                };
                let recorded =
                    journal.close_for(&receipt.key, &receipt.generation);
                outcome.rows.push(ArchiveRow {
                    thread_id: receipt.key.thread_id().to_string(),
                    note: recorded
                        .map(|close| close.note.clone())
                        .unwrap_or_default(),
                    action: ACTION_LET_GO,
                    reason,
                    decision: "let_go".to_string(),
                    messages: bearing,
                    tasks: recorded
                        .map(|close| task_refs(&close.tasks))
                        .unwrap_or_default(),
                });
            }
            Err(error) => {
                if is_auth_abort(&error) {
                    return Err(error);
                }
                outcome.warnings.push(format!(
                    "skipped Gmail thread {}: {}: {}",
                    receipt.key.thread_id(),
                    error.kind(),
                    error.message(),
                ));
                outcome.failed = true;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::{
        model::{GmailMessage, GmailThread},
        sync::{
            archive_scan::{
                classify_archive_status, ArchiveIssue, ArchiveLinkTask,
                ArchiveScan, ArchiveThreadEvidence,
            },
            archive_state::ArchiveDisposition,
            scan::LinkPlacement,
        },
    };

    const ACCOUNT: &str = "you@example.com";
    const THREAD: &str = "0123456789abcdef";

    fn evidence(status: char, saved_ids: Vec<String>) -> ArchiveThreadEvidence {
        ArchiveThreadEvidence {
            key: ThreadKey::new(ACCOUNT, THREAD).unwrap(),
            note: "mail/thread.md".to_string(),
            tasks: vec![ArchiveLinkTask {
                path: "Projects/Task.md".to_string(),
                line: 3,
                text: format!("- [{status}] #task [[mail/thread]]"),
                block_id: Some("task-3".to_string()),
                status,
                class: classify_archive_status(status),
                placement: LinkPlacement::TaskLine,
                in_done: false,
                merge_retired: false,
                from_duplicate_note: false,
            }],
            saved_ids,
            note_fingerprint: Some(format!("sha256:{}", "a".repeat(64))),
            merge_hold: false,
            identity_uncertain: false,
            issues: Vec::<ArchiveIssue>::new(),
        }
    }

    fn scan(evidence: ArchiveThreadEvidence) -> ArchiveScan {
        ArchiveScan::test_scan(evidence, true)
    }

    #[test]
    fn reclose_does_not_recover_a_receipt_owned_by_an_earlier_generation() {
        let vault = tempfile::tempdir().unwrap();
        let label = ArchiveLabelIdentity::new(
            "bob/closed",
            Some("Label_10".to_string()),
        )
        .unwrap();
        let mut journal = ArchiveJournal::load(vault.path()).unwrap();
        let enabled = journal
            .event_for_enablement(ACCOUNT, true, label.clone())
            .unwrap();
        ArchiveJournal::append(vault.path(), "athena", &enabled).unwrap();
        journal = ArchiveJournal::load(vault.path()).unwrap();

        let opened = evidence(' ', vec!["aa01".to_string()]);
        let open_scan = scan(opened.clone());
        let observed = journal.event_for_open(&open_scan, &opened).unwrap();
        ArchiveJournal::append(vault.path(), "athena", &observed).unwrap();
        journal = ArchiveJournal::load(vault.path()).unwrap();

        let closed = evidence('x', vec!["aa01".to_string()]);
        let closed_scan = scan(closed.clone());
        let close_event = journal
            .event_for_close(&closed_scan, &closed, label.clone())
            .unwrap();
        ArchiveJournal::append(vault.path(), "athena", &close_event).unwrap();
        journal = ArchiveJournal::load(vault.path()).unwrap();
        let ArchiveDisposition::Ready {
            close: first_close, ..
        } = journal.project(&closed_scan, &closed)
        else {
            panic!("first close should be ready");
        };
        let archived = journal
            .event_for_terminal(
                &first_close.key,
                &first_close.generation,
                ArchiveTerminalKind::Archived {
                    message_ids: vec!["aa01".to_string()],
                    label: label.clone(),
                    recovered: false,
                },
            )
            .unwrap();
        ArchiveJournal::append(vault.path(), "athena", &archived).unwrap();
        journal = ArchiveJournal::load(vault.path()).unwrap();

        let reopened = evidence(' ', vec!["aa01".to_string()]);
        let reopened_scan = scan(reopened.clone());
        let observed =
            journal.event_for_open(&reopened_scan, &reopened).unwrap();
        ArchiveJournal::append(vault.path(), "mac", &observed).unwrap();
        journal = ArchiveJournal::load(vault.path()).unwrap();

        let reclosed =
            evidence('x', vec!["aa01".to_string(), "bb02".to_string()]);
        let reclosed_scan = scan(reclosed.clone());
        let close_event = journal
            .event_for_close(&reclosed_scan, &reclosed, label.clone())
            .unwrap();
        ArchiveJournal::append(vault.path(), "mac", &close_event).unwrap();
        journal = ArchiveJournal::load(vault.path()).unwrap();
        let close = journal.unconsumed_close(&reclosed.key).unwrap().clone();
        let thread = GmailThread {
            id: THREAD.to_string(),
            history_id: None,
            messages: vec![GmailMessage {
                id: "aa01".to_string(),
                thread_id: THREAD.to_string(),
                label_ids: vec!["Label_10".to_string(), "Label_9".to_string()],
                internal_date: None,
                snippet: None,
                headers: Vec::new(),
                payload: None,
            }],
        };
        let mut outcome = LiveArchiveOutcome::default();
        reconcile_recovered(
            vault.path(),
            "mac",
            &thread,
            &close,
            &label,
            &mut journal,
            &mut outcome,
        )
        .unwrap();

        assert_eq!(outcome.rows.len(), 1);
        assert_eq!(outcome.rows[0].action, ACTION_KEPT_LIVE);
        assert_eq!(outcome.rows[0].reason, KEPT_MANUAL);
        assert_eq!(journal.owned_receipts().count(), 1);
        assert_eq!(journal.unconsumed_close(&reclosed.key), None);
        assert_eq!(outcome.warnings.len(), 0);
    }
}

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
        self, PreviewInput, HOLD_UNRECONCILED, KEPT_CONSENT_REMOVED, KEPT_GONE,
        KEPT_MANUAL, KEPT_POLICY, KEPT_UNSAVED,
    },
    archive_scan::scan_archive_evidence,
    archive_state::{
        ArchiveJournal, ArchiveLabelIdentity, ArchiveTerminalKind,
    },
    archive_transport::{
        resolve_or_create_receipt_label, LabelOperation, MessageLabelState,
        ReceiptLabel,
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
         Bob uses it for label-based archive and explicit restore",
    )
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
    if ensure_baseline(vault, &host, config, account, &scan, &mut journal)? {
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
        let owned_before: Vec<super::archive_state::ArchiveReceipt> =
            journal.owned_receipts().cloned().collect();
        release_owned(
            vault,
            &host,
            reader,
            gog,
            account,
            &mut journal,
            &owned_before,
            &mut outcome,
        )?;
        outcome.hints = super::report::archive_restore_hints(&scan, &journal);
        return Ok(outcome);
    }

    // Paused batches create no labels and append no terminal outcomes beyond
    // the baseline observations already recorded above. Every preview row,
    // whether archive or kept, reports the gate reason while paused. Paused
    // planning needs no modify grant.
    if preview.plan.gate.is_paused() {
        for row in &preview.plan.rows {
            outcome.rows.push(ArchiveRow {
                thread_id: row.key.thread_id().to_string(),
                note: row.note.clone(),
                action: ACTION_PAUSED_LIVE,
                reason: gate_reason(&preview.plan.gate),
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

    let enrollment_label = match labels
        .iter()
        .find(|label| label.name == config.sync_label().unwrap_or_default())
    {
        Some(label) => label.clone(),
        None => {
            outcome.warnings.push(format!(
                "skipped archive mutations: Gmail enrollment label {:?} is missing or ambiguous",
                config.sync_label().unwrap_or_default(),
            ));
            outcome.failed = true;
            return Ok(outcome);
        }
    };
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
            return Ok(outcome);
        }
    };

    let enrollment_id = enrollment_label.id.clone();
    // Previously owned receipts only: newly archived threads in this run must
    // not let go immediately on stale reads. Let-go converges next run.
    let owned_before: Vec<super::archive_state::ArchiveReceipt> =
        journal.owned_receipts().cloned().collect();
    // Per-thread isolation: one fresh metadata read and one batch modify per
    // thread, in deterministic thread-id order.
    for row in &preview.plan.rows {
        let Some(evidence) = scan.evidence(&row.key) else {
            continue;
        };
        // Mutation-boundary evidence revalidation: the frozen saved set,
        // fingerprint, and task locators must match the planned row.
        if evidence.saved_ids != row.saved_ids
            || evidence.note_fingerprint.as_deref().unwrap_or_default()
                != row.note_fingerprint.as_str()
        {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive evidence changed before mutation",
                row.key.thread_id(),
            ));
            outcome.failed = true;
            continue;
        }
        let Some(close) = journal.unconsumed_close(&row.key).cloned() else {
            outcome.warnings.push(format!(
                "skipped Gmail thread {}: archive close is no longer open",
                row.key.thread_id(),
            ));
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
                        &close.generation,
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
        let verdict = archive_preview::remote_verdict(
            reader,
            account,
            evidence,
            &thread,
            &enrollment_id,
            &receipt_identity,
            &policy,
        );
        match verdict {
            archive_preview::RemoteVerdict::Kept { reason } => {
                append_kept(
                    vault,
                    &host,
                    &journal,
                    &row.key,
                    &close.generation,
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
                        evidence,
                        &close,
                        &receipt_identity,
                        &mut journal,
                        &mut outcome,
                    )?;
                } else {
                    if let Some(warning) = warning {
                        outcome.warnings.push(warning);
                    }
                    if reason == "fetch_failed" {
                        outcome.failed = true;
                    }
                }
            }
            archive_preview::RemoteVerdict::Archive { archive_ids } => {
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
                        &receipt,
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
                match gog.archive_batch_modify(account, &diff) {
                    Ok(_) => {
                        append_archived(
                            vault,
                            &host,
                            &journal,
                            &row.key,
                            &close,
                            &receipt_identity,
                            &archive_ids,
                            false,
                            &mut outcome,
                        )?;
                        journal =
                            ArchiveJournal::load(vault).unwrap_or(journal);
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
        match archive_preview::remote_verdict(
            reader,
            account,
            &evidence,
            &thread,
            enrollment_id,
            receipt_label,
            policy,
        ) {
            archive_preview::RemoteVerdict::Held {
                reason: HOLD_UNRECONCILED,
                ..
            } => reconcile_recovered(
                vault,
                host,
                &thread,
                &evidence,
                &close,
                receipt_label,
                journal,
                outcome,
            )?,
            archive_preview::RemoteVerdict::Held { reason, warning } => {
                if let Some(warning) = warning {
                    outcome.warnings.push(warning);
                }
                if reason == archive_preview::HOLD_FETCH_FAILED {
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
    scan: &super::archive_scan::ArchiveScan,
    journal: &mut ArchiveJournal,
) -> Result<bool, GmailError> {
    let mut appended = false;
    if !journal.is_enabled(account) {
        let Some(label) =
            ArchiveLabelIdentity::new(config.archive_label(), None)
        else {
            return Ok(false);
        };
        if let Some(event) = journal.event_for_enablement(account, true, label)
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
    generation: &str,
    reason: &str,
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    let Some(event) = journal.event_for_terminal(
        key,
        generation,
        ArchiveTerminalKind::Kept {
            reason: reason.to_string(),
        },
    ) else {
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
    outcome.rows.push(ArchiveRow {
        thread_id: key.thread_id().to_string(),
        note: String::new(),
        action: ACTION_KEPT_LIVE,
        reason: kept_reason(reason),
        messages: Vec::new(),
        tasks: Vec::new(),
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

#[allow(clippy::too_many_arguments)]
fn append_archived(
    vault: &Path,
    host: &str,
    journal: &ArchiveJournal,
    key: &ThreadKey,
    close: &super::archive_state::ArchiveCloseGeneration,
    receipt_label: &ArchiveLabelIdentity,
    message_ids: &[String],
    recovered: bool,
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    let Some(event) = journal.event_for_terminal(
        key,
        &close.generation,
        ArchiveTerminalKind::Archived {
            message_ids: message_ids.to_vec(),
            label: receipt_label.clone(),
            recovered,
        },
    ) else {
        return Ok(());
    };
    ArchiveJournal::append(vault, host, &event).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not append the Gmail archive journal: {error}"),
        )
    })?;
    outcome.rows.push(ArchiveRow {
        thread_id: key.thread_id().to_string(),
        note: close.note.clone(),
        action: ACTION_ARCHIVED,
        reason: if recovered {
            REASON_RECOVERED
        } else if close.historical {
            "backlog"
        } else {
            "eligible"
        },
        messages: message_ids.to_vec(),
        tasks: close
            .tasks
            .iter()
            .map(|task| ArchiveTaskRef {
                path: task.path.clone(),
                line: task.line,
                block_id: task.block_id.clone(),
            })
            .collect(),
    });
    Ok(())
}

/// Reconcile a crash that archived remotely but never journaled: when the
/// live thread carries the receipt with no matching archived event, record
/// a recovered archive once without another remote write.
fn reconcile_recovered(
    vault: &Path,
    host: &str,
    thread: &crate::native::gmail::model::GmailThread,
    evidence: &super::archive_scan::ArchiveThreadEvidence,
    close: &super::archive_state::ArchiveCloseGeneration,
    receipt_label: &ArchiveLabelIdentity,
    journal: &mut ArchiveJournal,
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
    let mut recovered: Vec<String> = thread
        .messages
        .iter()
        .filter(|message| {
            evidence.saved_ids.binary_search(&message.id).is_ok()
                && message.label_ids.iter().any(|label| {
                    receipt_label.id.as_ref().is_some_and(|id| label == id)
                        || label == &receipt_label.name
                })
        })
        .map(|message| message.id.clone())
        .collect();
    recovered.sort();
    recovered.dedup();
    if recovered.is_empty() {
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
        messages: recovered,
        tasks: close
            .tasks
            .iter()
            .map(|task| ArchiveTaskRef {
                path: task.path.clone(),
                line: task.line,
                block_id: task.block_id.clone(),
            })
            .collect(),
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
    journal: &mut ArchiveJournal,
    owned: &[super::archive_state::ArchiveReceipt],
    outcome: &mut LiveArchiveOutcome,
) -> Result<(), GmailError> {
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
        let has_inbox = thread.messages.iter().any(|message| {
            message
                .label_ids
                .iter()
                .any(|label| label.eq_ignore_ascii_case("INBOX"))
        });
        if !has_inbox {
            continue;
        }
        let mut bearing: Vec<String> = thread
            .messages
            .iter()
            .filter(|message| {
                message.label_ids.iter().any(|label| {
                    Some(label) == receipt.label.id.as_ref()
                        || label == &receipt.label.name
                })
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
                    label: receipt.label.clone(),
                },
            ) else {
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
            outcome.rows.push(ArchiveRow {
                thread_id: receipt.key.thread_id().to_string(),
                note: String::new(),
                action: ACTION_LET_GO,
                reason: REASON_MANUAL_RETURN,
                messages: receipt.message_ids.clone(),
                tasks: Vec::new(),
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
        // Re-resolve the receipt through the strict boundary using the live
        // label list would reject historical shapes; use the recorded id
        // directly through the pure diff with a fresh user label.
        let live_receipt = match live_receipt_label(receipt) {
            Some(label) => label,
            None => continue,
        };
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
        match gog.archive_batch_modify(account, &diff) {
            Ok(_) => {
                let Some(event) = journal.event_for_terminal(
                    &receipt.key,
                    &receipt.generation,
                    ArchiveTerminalKind::LetGo {
                        message_ids: bearing.clone(),
                        label: receipt.label.clone(),
                    },
                ) else {
                    continue;
                };
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
                outcome.rows.push(ArchiveRow {
                    thread_id: receipt.key.thread_id().to_string(),
                    note: String::new(),
                    action: ACTION_LET_GO,
                    reason,
                    messages: bearing,
                    tasks: Vec::new(),
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

fn live_receipt_label(
    receipt: &super::archive_state::ArchiveReceipt,
) -> Option<ReceiptLabel> {
    let id = receipt.label.id.clone()?;
    ReceiptLabel::new(
        &GmailLabel {
            id,
            name: receipt.label.name.clone(),
            kind: Some("user".to_string()),
        },
        &GmailLabel {
            id: "Label_0".to_string(),
            name: "enrollment-placeholder".to_string(),
            kind: Some("user".to_string()),
        },
    )
    .ok()
}

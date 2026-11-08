//! Confirmed draft-id-only dispatch, crash recovery, and send reporting.
//!
//! Tests inject a finalization callback to exercise state transitions without
//! a live Gmail account. Journals and errors stay body-free.

use std::{
    fs,
    io::{self, IsTerminal as _, Write as _},
    path::Path,
};

use super::super::{
    cli::{DraftArgs, HumanFormat, SendArgs},
    config::{GmailConfig, COMPOSE_SCOPE},
    gog::{Gog, SentReceipt},
    model, resolve, save, GmailError,
};
use super::{
    source::{self, ComposeSource},
    state::{
        self, append_send_event, attempt_timestamp, load_send_state,
        operation_lock, persist_send_state, send_operation_id, sha256_hex,
        subject_hash, SendStage, SendState,
    },
};
use crate::native::{google, vault_sync};

enum SendResponse {
    Preview(super::draft::PreparedDraft),
    State {
        status: &'static str,
        note: String,
        draft_id: Option<String>,
        thread_id: Option<String>,
        sent_message_id: Option<String>,
    },
}

/// Run the public confirmed-send command.
pub(crate) fn run(args: &SendArgs) -> i32 {
    match run_inner(args) {
        Ok(response) => {
            report_response(response, args.format);
            0
        }
        Err(error) => crate::native::gmail::ui::report_error(
            "send",
            &error,
            args.format.as_str(),
        ),
    }
}

fn run_inner(args: &SendArgs) -> Result<SendResponse, GmailError> {
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    super::validate_vault(&vault)?;
    super::require_writer(&config)?;
    let folder = Path::new(config.folder());
    super::validate_mail_folder(&vault, folder)?;
    let relative = {
        let _maintenance = super::maintenance_lock()?;
        super::validate_mail_folder(&vault, folder)?;
        let relative = super::resolve_note_path(&vault, folder, &args.note)?;
        super::validate_target(&vault, &relative, true)?;
        relative
    };
    let note = save::path_to_slashes(&relative);
    let _operation = operation_lock(&relative)?;

    let current =
        fs::read_to_string(vault.join(&relative)).map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not read compose note: {error}"),
            )
        })?;
    let existing = load_send_state(&current).map_err(|error| {
        GmailError::runtime("send_state_invalid", error.to_string())
    })?;

    if let Some(state) = existing.as_ref()
        && state.stage == SendStage::Complete
    {
        super::finalize::reconcile_complete(&vault, &relative, state)?;
        return Ok(state_response(
            "already_sent",
            note,
            Some(state.draft_id.clone()),
            state.sent_thread_id.clone(),
            state.sent_message_id.clone(),
        ));
    }

    let gog = Gog::resolve(google::GMAIL, config.gog(), config.timeout())?;
    let account = if let Some(state) = existing.as_ref() {
        state.account.clone()
    } else {
        resolve::resolve_account(&gog, None, config.account())?
    };
    match recover(&vault, &relative, &gog, &account)? {
        RecoveryDecision::NoState => {}
        RecoveryDecision::DraftPresent => {
            return Ok(state_response(
                "recovered_to_draft",
                note,
                existing.map(|state| state.draft_id),
                None,
                None,
            ));
        }
        RecoveryDecision::Unknown { reason } => {
            return Err(GmailError::runtime(
                reason,
                "the previous Gmail send is unresolved; Bob retained its state and did not send again".to_string(),
            )
            .with_hint("inspect the draft and SENT folder, then retry `bob gmail send NOTE` for recovery"));
        }
        RecoveryDecision::Acknowledged {
            message_id,
            thread_id,
        } => {
            let state = state::load_send_state(
                &fs::read_to_string(vault.join(&relative)).map_err(
                    |error| {
                        GmailError::runtime(
                            "note",
                            format!("could not reread compose note: {error}"),
                        )
                    },
                )?,
            )
            .map_err(|error| {
                GmailError::runtime("send_state_invalid", error.to_string())
            })?
            .ok_or_else(|| {
                GmailError::runtime(
                    "send_ack_unpersisted",
                    "the acknowledged send state is missing".to_string(),
                )
            })?;
            super::finalize::finalize(
                &vault, &relative, &state, &config, &gog,
            )?;
            return Ok(state_response(
                "sent",
                note,
                Some(state.draft_id),
                Some(thread_id),
                Some(message_id),
            ));
        }
        RecoveryDecision::Complete => {
            let state = existing.ok_or_else(|| {
                GmailError::runtime(
                    "send_state_invalid",
                    "the completed send state is missing".to_string(),
                )
            })?;
            super::finalize::reconcile_complete(&vault, &relative, &state)?;
            return Ok(state_response(
                "already_sent",
                note,
                Some(state.draft_id),
                state.sent_thread_id,
                state.sent_message_id,
            ));
        }
    }

    let draft_args = DraftArgs {
        bob_dir: Some(vault.clone()),
        format: args.format,
        note: args.note.clone(),
    };
    let prepared = super::draft::prepare_for_send(&draft_args, true)?;

    // Dry-run deliberately wins over a supplied confirmation digest. It
    // updates the stored draft and returns the intentional preview payload.
    if args.dry_run {
        return Ok(SendResponse::Preview(prepared));
    }

    let tty = super::super::config::assume_tty() || io::stdin().is_terminal();
    if tty {
        print_preview_for_confirmation(&prepared, args.format);
    }
    match args.confirm.as_deref() {
        Some(confirm) if confirm != prepared.digest => {
            return Err(GmailError::setup(
                "confirmation_mismatch",
                "the confirmation digest does not match the current Gmail preview".to_string(),
            )
            .with_hint("run `bob gmail send NOTE --dry-run` to obtain the current digest"));
        }
        Some(_) => {}
        None if !tty => {
            let command = confirmation_command(args, &prepared.digest);
            return Err(GmailError::setup(
                "confirmation_required",
                "sending requires a TTY confirmation or the exact preview digest".to_string(),
            )
            .with_hint(&format!("confirm this exact preview with: {command}")));
        }
        None => {
            if !confirm_on_tty()? {
                return Ok(state_response(
                    "declined",
                    prepared.note,
                    Some(prepared.draft_id),
                    None,
                    None,
                ));
            }
        }
    }

    let outcome = dispatch_once_locked(
        &vault,
        &relative,
        &gog,
        &prepared.approved,
        &config,
        |state| {
            super::finalize::finalize(&vault, &relative, state, &config, &gog)
        },
    )?;
    match outcome {
        DispatchOutcome::Sent {
            message_id,
            thread_id,
        }
        | DispatchOutcome::AlreadyAcknowledged {
            message_id,
            thread_id,
        } => Ok(state_response(
            "sent",
            prepared.note,
            Some(prepared.draft_id),
            Some(thread_id),
            Some(message_id),
        )),
        DispatchOutcome::CompleteNoop => Ok(state_response(
            "already_sent",
            prepared.note,
            Some(prepared.draft_id),
            None,
            None,
        )),
        DispatchOutcome::RecoveredToDraft => Ok(state_response(
            "recovered_to_draft",
            prepared.note,
            Some(prepared.draft_id),
            None,
            None,
        )),
        DispatchOutcome::UnknownRetained { reason } => Err(GmailError::runtime(
            reason,
            "the Gmail send outcome is unresolved; Bob retained its state and did not send again".to_string(),
        )),
    }
}

fn state_response(
    status: &'static str,
    note: String,
    draft_id: Option<String>,
    thread_id: Option<String>,
    sent_message_id: Option<String>,
) -> SendResponse {
    SendResponse::State {
        status,
        note,
        draft_id,
        thread_id,
        sent_message_id,
    }
}

fn report_response(response: SendResponse, format: HumanFormat) {
    match response {
        SendResponse::Preview(prepared) => super::draft::report_success(
            &prepared.note,
            &prepared.draft_id,
            &prepared.digest,
            &prepared.preview,
            format,
        ),
        SendResponse::State {
            status,
            note,
            draft_id,
            thread_id,
            sent_message_id,
        } => {
            if format.is_json() {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema_version": 1,
                        "ok": true,
                        "status": status,
                        "note": note,
                        "draft_id": draft_id,
                        "thread_id": thread_id,
                        "sent_message_id": sent_message_id,
                    })
                );
            } else {
                match status {
                    "sent" => println!(
                        "Sent {} in thread {}",
                        sent_message_id.as_deref().unwrap_or("(unknown)"),
                        thread_id.as_deref().unwrap_or("(unknown)")
                    ),
                    "already_sent" => println!("Already sent; local state reconciled."),
                    "recovered_to_draft" => println!("The prior send was not accepted; review a fresh preview before sending again."),
                    "declined" => println!("Not sent. The Gmail draft remains saved."),
                    _ => println!("Send status: {status}"),
                }
            }
        }
    }
}

fn print_preview_for_confirmation(
    prepared: &super::draft::PreparedDraft,
    format: HumanFormat,
) {
    let output_to_stderr = format.is_json();
    let mut output = String::new();
    use std::fmt::Write as _;
    let preview = &prepared.preview;
    let _ = writeln!(output, "Gmail send preview · {}", prepared.digest);
    let _ = writeln!(output, "From: {}", preview.from);
    let _ = writeln!(output, "To: {}", preview.to.join(", "));
    if !preview.cc.is_empty() {
        let _ = writeln!(output, "Cc: {}", preview.cc.join(", "));
    }
    if !preview.bcc.is_empty() {
        let _ = writeln!(output, "Bcc: {}", preview.bcc.join(", "));
    }
    let _ = writeln!(output, "Subject: {}\n", preview.subject);
    let _ = writeln!(output, "{}", preview.text_body);
    if output_to_stderr {
        eprint!("{output}");
    } else {
        print!("{output}");
    }
}

fn confirm_on_tty() -> Result<bool, GmailError> {
    eprint!("Send this exact Gmail draft? [y/N] ");
    io::stderr().flush().ok();
    let mut answer = String::new();
    let read = io::stdin().read_line(&mut answer).map_err(|error| {
        GmailError::runtime(
            "confirmation",
            format!("could not read confirmation: {error}"),
        )
    })?;
    Ok(read > 0
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn confirmation_command(args: &SendArgs, digest: &str) -> String {
    let mut parts = vec![
        "bob gmail send".to_string(),
        shell_quote(&args.note.to_string_lossy()),
    ];
    if let Some(vault) = args.bob_dir.as_ref() {
        parts.push(format!(
            "--bob-dir {}",
            shell_quote(&vault.to_string_lossy())
        ));
    }
    parts.push(format!("--confirm {}", shell_quote(digest)));
    if args.format.is_json() {
        parts.push("--format json".to_string());
    }
    parts.join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Minimal Gmail surface the send engine needs. `Gog` implements it; tests
/// inject an in-memory fake to exercise every recovery outcome without a
/// live account.
pub(crate) trait SendTransport {
    fn verify_draft_snapshot(
        &self,
        account: &str,
        draft_id: &str,
        source: &ComposeSource,
        expected_fingerprint: &str,
    ) -> Result<(), GmailError> {
        let _ = (account, draft_id, source, expected_fingerprint);
        Ok(())
    }

    fn draft_get(
        &self,
        account: &str,
        draft_id: &str,
    ) -> Result<model::GmailDraft, GmailError>;
    fn draft_send(
        &self,
        account: &str,
        draft_id: &str,
    ) -> Result<SentReceipt, GmailError>;
    fn search(
        &self,
        account: &str,
        query: &str,
        max: u32,
    ) -> Result<serde_json::Value, GmailError>;
    fn thread_metadata(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<model::GmailThread, GmailError>;
    fn accounts(
        &self,
        client: Option<&str>,
    ) -> Result<Vec<google::gog::GogAccount>, GmailError>;
}

impl SendTransport for Gog {
    fn verify_draft_snapshot(
        &self,
        account: &str,
        draft_id: &str,
        source: &ComposeSource,
        expected_fingerprint: &str,
    ) -> Result<(), GmailError> {
        super::draft::verify_remote_snapshot(
            self,
            account,
            draft_id,
            source,
            expected_fingerprint,
        )
    }

    fn draft_get(
        &self,
        account: &str,
        draft_id: &str,
    ) -> Result<model::GmailDraft, GmailError> {
        Gog::draft_get(self, account, draft_id)
    }

    fn draft_send(
        &self,
        account: &str,
        draft_id: &str,
    ) -> Result<SentReceipt, GmailError> {
        Gog::draft_send(self, account, draft_id)
    }

    fn search(
        &self,
        account: &str,
        query: &str,
        max: u32,
    ) -> Result<serde_json::Value, GmailError> {
        Gog::search(self, account, query, max)
    }

    fn thread_metadata(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<model::GmailThread, GmailError> {
        Gog::thread_metadata(self, account, thread_id)
    }

    fn accounts(
        &self,
        client: Option<&str>,
    ) -> Result<Vec<google::gog::GogAccount>, GmailError> {
        Gog::accounts(self, client)
    }
}

/// Approved snapshot the confirmed-send command validates before dispatch.
/// It binds the exact preview digest to the durable draft identity;
/// the engine rechecks it immediately before sending.
///
/// No `Debug` impl: callers retain the approved subject, so accidental log
/// formatting must not disclose it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ApprovedSend {
    pub(crate) note: String,
    pub(crate) account: String,
    pub(crate) draft_id: String,
    pub(crate) source_digest: String,
    pub(crate) preview_digest: String,
    pub(crate) draft_message_id: Option<String>,
    pub(crate) draft_rfc_id: Option<String>,
    pub(crate) thread_id: String,
    pub(crate) subject: String,
    pub(crate) recipient_count: u32,
    pub(crate) remote_fingerprint: String,
}

impl ApprovedSend {
    pub(crate) fn subject_hash(&self) -> String {
        subject_hash(&self.subject)
    }
}

/// Recovery classification for unresolved send state. Recovery never sends,
/// creates, or updates a draft; it only observes and persists state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryDecision {
    /// No durable send state; the caller may begin a fresh send.
    NoState,
    /// The recorded draft still exists. State returned to draft; the caller
    /// must require a fresh preview/confirmation before another attempt.
    DraftPresent,
    /// Exactly one matching SENT message was proven and acknowledged.
    Acknowledged {
        message_id: String,
        thread_id: String,
    },
    /// Missing, ambiguous, or failed-lookup outcome. State retained; the
    /// caller must not create, update, or send a replacement.
    Unknown { reason: &'static str },
    /// The operation already completed; repeated send is a no-op.
    Complete,
}

/// Result of one engine dispatch attempt. The finalizer runs only after the
/// acknowledged identity is durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DispatchOutcome {
    Sent {
        message_id: String,
        thread_id: String,
    },
    AlreadyAcknowledged {
        message_id: String,
        thread_id: String,
    },
    CompleteNoop,
    RecoveredToDraft,
    UnknownRetained {
        reason: &'static str,
    },
}

/// Begin a send operation by durably persisting `sending`. Fails without
/// sending when persistence, writer, scope, or preimage checks fail.
pub(crate) fn begin_send(
    vault: &Path,
    relative: &Path,
    approved: &ApprovedSend,
    config: &GmailConfig,
) -> Result<SendState, GmailError> {
    require_writer(config)?;
    let current = read_source(vault, relative)?;
    if current.fingerprint != approved.source_digest {
        return Err(GmailError::runtime(
            "confirmation_mismatch",
            "the compose source changed since the approved preview".to_string(),
        )
        .with_hint(
            "run `bob gmail draft NOTE` again to preview the newer text",
        ));
    }
    if current.draft_id.as_deref() != Some(approved.draft_id.as_str()) {
        return Err(GmailError::runtime(
            "confirmation_mismatch",
            "the recorded Gmail draft changed since the approved preview"
                .to_string(),
        )
        .with_hint(
            "run `bob gmail draft NOTE` again to preview the newer text",
        ));
    }
    if current.remote_fingerprint.as_deref()
        != Some(approved.remote_fingerprint.as_str())
    {
        return Err(GmailError::runtime(
            "confirmation_mismatch",
            "the stored draft changed since the approved preview".to_string(),
        )
        .with_hint(
            "run `bob gmail draft NOTE` again to preview the newer text",
        ));
    }
    if let Some(pinned) = current.pinned_account.as_deref()
        && !pinned.eq_ignore_ascii_case(&approved.account)
    {
        return Err(GmailError::runtime(
            "other_account",
            "this compose note is pinned to a different Gmail account"
                .to_string(),
        ));
    }
    let operation_id = send_operation_id(
        &approved.note,
        &approved.preview_digest,
        &approved.account,
    );
    let state = SendState {
        version: state::SEND_VERSION,
        operation_id: operation_id.clone(),
        stage: SendStage::Sending,
        account: approved.account.clone(),
        draft_id: approved.draft_id.clone(),
        source_digest: approved.source_digest.clone(),
        preview_digest: approved.preview_digest.clone(),
        draft_message_id: approved.draft_message_id.clone(),
        draft_rfc_id: approved.draft_rfc_id.clone(),
        thread_id: approved.thread_id.clone(),
        subject_hash: approved.subject_hash(),
        recipient_count: approved.recipient_count,
        attempt_time: attempt_timestamp(),
        sent_message_id: None,
        sent_thread_id: None,
        remote_fingerprint: Some(approved.remote_fingerprint.clone()),
        completed_note_digest: None,
        completed_commit_paths: Vec::new(),
        completed_path_receipts: Vec::new(),
        watch_turns: Vec::new(),
    };
    persist_send_state(
        vault,
        relative,
        &approved.source_digest,
        Some(state.clone()),
    )
    .map_err(|error| {
        if error.kind() == "compose_changed" {
            return error;
        }
        GmailError::runtime(
            "send_intent_unpersisted",
            "could not durably record the send intent; nothing was sent"
                .to_string(),
        )
        .with_hint("retry the confirmed send after the vault is writable")
    })?;
    let _ = append_send_event(
        vault,
        "gmail.send.sending",
        serde_json::json!({
            "account": state.account,
            "note": approved.note,
            "draft_id": state.draft_id,
            "operation_id": state.operation_id,
            "source_digest": state.source_digest,
            "preview_digest": state.preview_digest,
            "thread_id": state.thread_id,
            "subject_hash": state.subject_hash,
        }),
    );
    Ok(state)
}

/// Classify unresolved send state without ever sending. The caller must hold
/// no vault maintenance lock across Gmail waits; this function acquires it
/// only for short durable writes.
pub(crate) fn recover(
    vault: &Path,
    relative: &Path,
    gog: &impl SendTransport,
    account: &str,
) -> Result<RecoveryDecision, GmailError> {
    let state = read_state(vault, relative)?;
    let Some(state) = state else {
        return Ok(RecoveryDecision::NoState);
    };
    if state.stage == SendStage::Complete {
        return Ok(RecoveryDecision::Complete);
    }
    if !state.account.eq_ignore_ascii_case(account) {
        return Err(GmailError::runtime(
            "other_account",
            "the pending send belongs to a different Gmail account".to_string(),
        ));
    }
    if state.stage == SendStage::Acknowledged {
        let message_id = state.sent_message_id.clone().unwrap_or_default();
        let thread_id = state.sent_thread_id.clone().unwrap_or_default();
        return Ok(RecoveryDecision::Acknowledged {
            message_id,
            thread_id,
        });
    }
    let source = read_source(vault, relative)?;
    // Stage is Sending: the draft may still exist, or it may have been
    // consumed by an accepted send.
    match gog.draft_get(account, &state.draft_id) {
        Ok(draft) => {
            if draft.id != state.draft_id {
                return Ok(RecoveryDecision::Unknown {
                    reason: "draft_identity_mismatch",
                });
            }
            // The recorded draft still exists: return to draft state and
            // require a fresh preview/confirmation. This invocation does not
            // send.
            persist_send_state(vault, relative, &source.fingerprint, None)
                .map_err(|error| {
                    if error.kind() == "compose_changed" {
                        return error;
                    }
                    GmailError::runtime(
                        "send_state_unpersisted",
                        "could not return the recovered send to draft state"
                            .to_string(),
                    )
                })?;
            let _ = append_send_event(
                vault,
                "gmail.send.recovery",
                serde_json::json!({
                    "account": state.account,
                    "note": path_to_slashes(relative),
                    "draft_id": state.draft_id,
                    "operation_id": state.operation_id,
                    "outcome": "draft_present",
                }),
            );
            Ok(RecoveryDecision::DraftPresent)
        }
        Err(error) if error.kind() == "not_found" => {
            recover_from_sent(vault, relative, gog, &source, &state)
        }
        Err(error) => {
            // Failed lookup (network, timeout, auth): retain state and
            // report the typed failure without sending.
            Err(error)
        }
    }
}

fn recover_from_sent(
    vault: &Path,
    relative: &Path,
    gog: &impl SendTransport,
    source: &ComposeSource,
    state: &SendState,
) -> Result<RecoveryDecision, GmailError> {
    let candidate = find_unique_sent(gog, state)?;
    let Some(candidate) = candidate else {
        let _ = append_send_event(
            vault,
            "gmail.send.recovery",
            serde_json::json!({
                "account": state.account,
                "note": path_to_slashes(relative),
                "draft_id": state.draft_id,
                "operation_id": state.operation_id,
                "outcome": "outcome_unknown",
            }),
        );
        return Ok(RecoveryDecision::Unknown {
            reason: "outcome_unknown",
        });
    };
    let acknowledged = SendState {
        stage: SendStage::Acknowledged,
        sent_message_id: Some(candidate.message_id.clone()),
        sent_thread_id: Some(candidate.thread_id.clone()),
        ..state.clone()
    };
    persist_send_state(
        vault,
        relative,
        &source.fingerprint,
        Some(acknowledged.clone()),
    )
    .map_err(|_| {
        GmailError::runtime(
            "send_ack_unpersisted",
            "a sent message was proven but its acknowledgement could not be recorded"
                .to_string(),
        )
        .with_hint("recovery found exactly one SENT match; retry recovery to record it")
    })?;
    let _ = append_send_event(
        vault,
        "gmail.send.acknowledged",
        serde_json::json!({
            "account": state.account,
            "note": path_to_slashes(relative),
            "draft_id": state.draft_id,
            "operation_id": state.operation_id,
            "sent_message_id": candidate.message_id,
            "sent_thread_id": candidate.thread_id,
        }),
    );
    Ok(RecoveryDecision::Acknowledged {
        message_id: candidate.message_id,
        thread_id: candidate.thread_id,
    })
}

/// One matching SENT message proven by RFC identity or by thread, subject
/// hash, SENT label, and attempt-time evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SentMatch {
    message_id: String,
    thread_id: String,
}

fn find_unique_sent(
    gog: &impl SendTransport,
    state: &SendState,
) -> Result<Option<SentMatch>, GmailError> {
    let query = match state.draft_rfc_id.as_deref() {
        Some(rfc) => format!("rfc822msgid:{rfc} in:sent"),
        None => "in:sent".to_string(),
    };
    let response = gog.search(&state.account, &query, 20)?;
    let threads = response
        .get("threads")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut matches = Vec::new();
    for thread in threads.iter().take(20) {
        let Some(thread_id) = thread.get("id").and_then(|id| id.as_str())
        else {
            continue;
        };
        // Bound the fan-out: only inspect threads that could match. When no
        // RFC identity is recorded, only the recorded thread can prove the
        // send.
        if state.draft_rfc_id.is_none() && thread_id != state.thread_id {
            continue;
        }
        let fetched = gog.thread_metadata(&state.account, thread_id)?;
        for message in &fetched.messages {
            if !is_sent(message) {
                continue;
            }
            if message.thread_id != state.thread_id {
                continue;
            }
            if let Some(rfc) = state.draft_rfc_id.as_deref() {
                let headers: Vec<&str> = message
                    .header_values("Message-ID")
                    .chain(message.header_values("Message-Id"))
                    .collect();
                if !headers.iter().any(|header| header.trim() == rfc.trim()) {
                    continue;
                }
            } else {
                let subject = message
                    .first_header(model::HeaderKind::Subject)
                    .unwrap_or_default();
                if sha256_hex(subject.as_bytes()) != state.subject_hash {
                    continue;
                }
                if !after_attempt(message, &state.attempt_time) {
                    continue;
                }
            }
            matches.push(SentMatch {
                message_id: message.id.clone(),
                thread_id: message.thread_id.clone(),
            });
        }
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.remove(0))),
        _ => Ok(None),
    }
}

fn is_sent(message: &model::GmailMessage) -> bool {
    message
        .label_ids
        .iter()
        .any(|label| label.eq_ignore_ascii_case("SENT"))
}

fn after_attempt(message: &model::GmailMessage, attempt_time: &str) -> bool {
    let Ok(attempt) = chrono::DateTime::parse_from_rfc3339(attempt_time) else {
        return false;
    };
    let Some(internal) = message.internal_date.as_deref() else {
        return false;
    };
    let Ok(millis) = internal.parse::<i64>() else {
        return false;
    };
    let attempt_millis = attempt.timestamp_millis();
    // Tolerate up to five minutes of clock skew so a just-sent message is
    // never rejected as too old; anything older stays rejected.
    millis >= attempt_millis.saturating_sub(300_000)
}

/// Dispatch exactly the recorded draft ID once, holding the host-local
/// operation lock across the whole sequence so two processes race at most
/// one send. The finalizer runs only after the acknowledgement is durable.
/// Production holds the lock across preview and confirmation and calls
/// [`dispatch_once_locked`] directly; fault tests use this wrapper.
#[cfg(test)]
pub(crate) fn dispatch_once<F>(
    vault: &Path,
    relative: &Path,
    gog: &impl SendTransport,
    approved: &ApprovedSend,
    config: &GmailConfig,
    finalize: F,
) -> Result<DispatchOutcome, GmailError>
where
    F: FnOnce(&SendState) -> Result<(), GmailError>,
{
    let _operation = operation_lock(relative)?;
    dispatch_once_locked(vault, relative, gog, approved, config, finalize)
}

/// Dispatch while the caller already holds the shared compose-operation
/// lock across recovery, preview, confirmation, and send.
pub(crate) fn dispatch_once_locked<F>(
    vault: &Path,
    relative: &Path,
    gog: &impl SendTransport,
    approved: &ApprovedSend,
    config: &GmailConfig,
    finalize: F,
) -> Result<DispatchOutcome, GmailError>
where
    F: FnOnce(&SendState) -> Result<(), GmailError>,
{
    let existing = read_state(vault, relative)?;
    if let Some(state) = existing {
        match state.stage {
            SendStage::Complete => {
                finalize(&state)?;
                return Ok(DispatchOutcome::CompleteNoop);
            }
            SendStage::Acknowledged => {
                let message_id =
                    state.sent_message_id.clone().unwrap_or_default();
                let thread_id =
                    state.sent_thread_id.clone().unwrap_or_default();
                finalize(&state).map_err(finalization_error)?;
                return Ok(DispatchOutcome::AlreadyAcknowledged {
                    message_id,
                    thread_id,
                });
            }
            SendStage::Sending => {
                return recover_and_report(
                    vault, relative, gog, &state, finalize,
                );
            }
        }
    }
    let source = read_source(vault, relative)?;
    // Fresh send: validate the approved snapshot against current inputs.
    validate_approved(&source, approved)?;
    require_writer(config)?;
    require_compose_grant(gog, &approved.account)?;

    // Persist `sending` before any network send. A persistence failure must
    // not send.
    let sending = begin_send(vault, relative, approved, config)?;

    // Re-read the live draft immediately before dispatch. A remote edit
    // between preview and confirmation refuses instead of overwriting.
    match gog.draft_get(&sending.account, &sending.draft_id) {
        Ok(draft) => {
            if draft.message.thread_id != sending.thread_id {
                let _ = clear_to_draft(vault, relative, &source.fingerprint);
                return Err(GmailError::runtime(
                    "draft_remote_conflict",
                    "the stored Gmail draft changed outside Bob".to_string(),
                )
                .with_hint("review the Gmail draft, then run `bob gmail draft NOTE` again"));
            }
            if let Some(expected) = sending.draft_message_id.as_deref()
                && draft.message.id != expected
            {
                let _ = clear_to_draft(vault, relative, &source.fingerprint);
                return Err(GmailError::runtime(
                    "draft_remote_conflict",
                    "the stored Gmail draft changed outside Bob".to_string(),
                )
                .with_hint("review the Gmail draft, then run `bob gmail draft NOTE` again"));
            }
        }
        Err(error) => {
            // The draft vanished between preview and dispatch: leave the
            // durable `sending` state for recovery to classify rather than
            // assuming a send outcome here.
            if error.kind() == "not_found" {
                return Ok(DispatchOutcome::UnknownRetained {
                    reason: "outcome_unknown",
                });
            }
            return Err(error);
        }
    }
    if let Err(error) = gog.verify_draft_snapshot(
        &sending.account,
        &sending.draft_id,
        &source,
        &approved.remote_fingerprint,
    ) {
        if error.kind() == "draft_remote_conflict" {
            let _ = clear_to_draft(vault, relative, &source.fingerprint);
        }
        return Err(error);
    }

    let receipt = match gog.draft_send(&sending.account, &sending.draft_id) {
        Ok(receipt) => receipt,
        Err(error) => {
            // Timeout, network, or lost response after Gmail may have
            // accepted: retain `sending` and never auto-resend. Recovery
            // classifies the draft/SENT evidence on the next attempt.
            return Err(GmailError::runtime(
                error.kind(),
                "the Gmail send outcome is unknown; state was retained"
                    .to_string(),
            ));
        }
    };
    acknowledge_and_finalize(
        vault, relative, &source, &sending, &receipt, finalize,
    )
}

fn recover_and_report<F>(
    vault: &Path,
    relative: &Path,
    gog: &impl SendTransport,
    state: &SendState,
    finalize: F,
) -> Result<DispatchOutcome, GmailError>
where
    F: FnOnce(&SendState) -> Result<(), GmailError>,
{
    let account = state.account.clone();
    match recover(vault, relative, gog, &account)? {
        RecoveryDecision::DraftPresent => Ok(DispatchOutcome::RecoveredToDraft),
        RecoveryDecision::Unknown { reason } => {
            Ok(DispatchOutcome::UnknownRetained { reason })
        }
        RecoveryDecision::NoState => Ok(DispatchOutcome::UnknownRetained {
            reason: "outcome_unknown",
        }),
        RecoveryDecision::Complete => Ok(DispatchOutcome::CompleteNoop),
        RecoveryDecision::Acknowledged {
            message_id,
            thread_id,
        } => {
            let (_, current) = read_source_and_state(vault, relative)?;
            let state = current.ok_or_else(|| {
                GmailError::runtime(
                    "send_ack_unpersisted",
                    "the acknowledged send state is missing".to_string(),
                )
            })?;
            finalize(&state).map_err(finalization_error)?;
            Ok(DispatchOutcome::AlreadyAcknowledged {
                message_id,
                thread_id,
            })
        }
    }
}

fn acknowledge_and_finalize<F>(
    vault: &Path,
    relative: &Path,
    source: &ComposeSource,
    sending: &SendState,
    receipt: &SentReceipt,
    finalize: F,
) -> Result<DispatchOutcome, GmailError>
where
    F: FnOnce(&SendState) -> Result<(), GmailError>,
{
    let acknowledged = SendState {
        stage: SendStage::Acknowledged,
        sent_message_id: Some(receipt.message_id.clone()),
        sent_thread_id: Some(receipt.thread_id.clone()),
        ..sending.clone()
    };
    persist_send_state(
        vault,
        relative,
        &source.fingerprint,
        Some(acknowledged.clone()),
    )
    .map_err(|_| {
        GmailError::runtime(
            "send_ack_unpersisted",
            "Gmail accepted the send but its acknowledgement could not be recorded"
                .to_string(),
        )
        .with_hint("recovery will find the SENT message; do not resend")
    })?;
    let _ = append_send_event(
        vault,
        "gmail.send.acknowledged",
        serde_json::json!({
            "account": acknowledged.account,
            "note": path_to_slashes(relative),
            "draft_id": acknowledged.draft_id,
            "operation_id": acknowledged.operation_id,
            "sent_message_id": receipt.message_id,
            "sent_thread_id": receipt.thread_id,
        }),
    );
    // A failed local refresh never erases the acknowledged send; it stays
    // acknowledged and is retried only for local finalization.
    finalize(&acknowledged).map_err(finalization_error)?;
    Ok(DispatchOutcome::Sent {
        message_id: receipt.message_id.clone(),
        thread_id: receipt.thread_id.clone(),
    })
}

fn validate_approved(
    source: &ComposeSource,
    approved: &ApprovedSend,
) -> Result<(), GmailError> {
    if source.fingerprint != approved.source_digest {
        return Err(GmailError::runtime(
            "confirmation_mismatch",
            "the compose source changed since the approved preview".to_string(),
        )
        .with_hint(
            "run `bob gmail draft NOTE` again to preview the newer text",
        ));
    }
    if source.draft_id.as_deref() != Some(approved.draft_id.as_str()) {
        return Err(GmailError::runtime(
            "confirmation_mismatch",
            "the recorded Gmail draft changed since the approved preview"
                .to_string(),
        )
        .with_hint(
            "run `bob gmail draft NOTE` again to preview the newer text",
        ));
    }
    if source.remote_fingerprint.as_deref()
        != Some(approved.remote_fingerprint.as_str())
    {
        return Err(GmailError::runtime(
            "confirmation_mismatch",
            "the stored draft changed since the approved preview".to_string(),
        )
        .with_hint(
            "run `bob gmail draft NOTE` again to preview the newer text",
        ));
    }
    Ok(())
}

fn read_source(
    vault: &Path,
    relative: &Path,
) -> Result<ComposeSource, GmailError> {
    let path = vault.join(relative);
    let contents = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read compose note: {error}"),
        )
    })?;
    source::parse(&contents)
        .map_err(|error| GmailError::setup("note", error.to_string()))
}

fn read_source_and_state(
    vault: &Path,
    relative: &Path,
) -> Result<(ComposeSource, Option<SendState>), GmailError> {
    let path = vault.join(relative);
    let contents = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read compose note: {error}"),
        )
    })?;
    let source = source::parse(&contents)
        .map_err(|error| GmailError::setup("note", error.to_string()))?;
    let state = load_send_state(&contents).map_err(|error| {
        GmailError::runtime("send_state_invalid", error.to_string())
    })?;
    Ok((source, state))
}

fn read_state(
    vault: &Path,
    relative: &Path,
) -> Result<Option<SendState>, GmailError> {
    let path = vault.join(relative);
    let contents = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read compose note: {error}"),
        )
    })?;
    load_send_state(&contents).map_err(|error| {
        GmailError::runtime("send_state_invalid", error.to_string())
    })
}

fn finalization_error(error: GmailError) -> GmailError {
    if error.kind() == "sent_needs_review" {
        return error;
    }
    GmailError::runtime(
        "send_finalization_pending",
        format!(
            "the send is acknowledged but local finalization failed ({})",
            error.kind()
        ),
    )
}

fn clear_to_draft(
    vault: &Path,
    relative: &Path,
    expected_fingerprint: &str,
) -> Result<(), GmailError> {
    let _ = persist_send_state(vault, relative, expected_fingerprint, None);
    Ok(())
}

fn require_writer(config: &GmailConfig) -> Result<(), GmailError> {
    let host = vault_sync::hostname_slug();
    if let Some(writer) = config.writer()
        && writer != host
    {
        return Err(GmailError::setup(
            "writer",
            format!("this host ({host}) is not the configured Gmail writer"),
        )
        .with_hint(&format!(
            "run this command on the configured writer {writer}"
        )));
    }
    Ok(())
}

fn require_compose_grant(
    gog: &impl SendTransport,
    account: &str,
) -> Result<(), GmailError> {
    let accounts = gog.accounts(Some(google::GMAIL.client_bucket))?;
    let grant = accounts
        .iter()
        .find(|candidate| candidate.email.eq_ignore_ascii_case(account));
    if grant.is_some_and(|entry| {
        entry.scopes.iter().any(|scope| scope == COMPOSE_SCOPE)
    }) {
        return Ok(());
    }
    Err(GmailError::setup(
        "insufficient_scope",
        "the isolated bob-gmail grant lacks the compose scope".to_string(),
    )
    .with_hint("run `bob gmail login -s compose` on the approved compose host"))
}

fn path_to_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::native::config::GmailSettings;

    const BODY_SENTINEL: &str = "PRIVATE_BODY_SENTINEL_7f3a";

    fn test_config() -> GmailConfig {
        GmailConfig::from_settings(GmailSettings {
            account: Some("you@example.com".to_string()),
            writer: None,
            ..Default::default()
        })
        .expect("test gmail config")
    }

    fn writer_config(writer: &str) -> GmailConfig {
        GmailConfig::from_settings(GmailSettings {
            account: Some("you@example.com".to_string()),
            writer: Some(writer.to_string()),
            ..Default::default()
        })
        .expect("writer gmail config")
    }

    fn write_compose_note(
        vault: &Path,
        relative: &str,
        subject: &str,
        body: &str,
    ) {
        let path = vault.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("note parent");
        }
        let contents = format!(
            "---\nparent: \"[[mail]]\"\ntype: \"[[mail]]\"\ncssclasses: [bob-mail]\nstatus: draft\nto: ada@example.com\ncc: []\nbcc: []\nsubject: {subject}\nattachments: []\n---\n\n# {subject}\n\n{body}"
        );
        std::fs::write(&path, contents).expect("write compose note");
    }

    fn add_draft_metadata(
        vault: &Path,
        relative: &str,
        account: &str,
        draft_id: &str,
        remote_fingerprint: &str,
    ) -> String {
        let path = vault.join(relative);
        let contents = std::fs::read_to_string(&path).expect("read note");
        let updated = source::persist_draft_metadata(
            &contents,
            account,
            Some(draft_id),
            None,
            Some(remote_fingerprint),
        )
        .expect("persist draft metadata");
        std::fs::write(&path, updated).expect("write draft metadata");
        let current =
            std::fs::read_to_string(vault.join(relative)).expect("reread");
        source::parse(&current).expect("parse").fingerprint
    }

    fn approved_for(
        note: &str,
        fingerprint: &str,
        remote_fingerprint: &str,
    ) -> ApprovedSend {
        ApprovedSend {
            note: note.to_string(),
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.to_string(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject: "Lease renewal".to_string(),
            recipient_count: 1,
            remote_fingerprint: remote_fingerprint.to_string(),
        }
    }

    fn draft_message(id: &str, thread_id: &str) -> model::GmailMessage {
        model::GmailMessage {
            id: id.to_string(),
            thread_id: thread_id.to_string(),
            label_ids: vec!["DRAFT".to_string()],
            internal_date: None,
            snippet: None,
            headers: Vec::new(),
            payload: None,
        }
    }

    fn sent_message(
        id: &str,
        thread_id: &str,
        subject: &str,
        rfc: Option<&str>,
        internal_ms: i64,
    ) -> model::GmailMessage {
        let mut headers = vec![
            model::Header {
                name: "Subject".to_string(),
                value: subject.to_string(),
            },
            model::Header {
                name: "From".to_string(),
                value: "you@example.com".to_string(),
            },
        ];
        if let Some(rfc) = rfc {
            headers.push(model::Header {
                name: "Message-ID".to_string(),
                value: rfc.to_string(),
            });
        }
        model::GmailMessage {
            id: id.to_string(),
            thread_id: thread_id.to_string(),
            label_ids: vec!["SENT".to_string()],
            internal_date: Some(internal_ms.to_string()),
            snippet: None,
            headers,
            payload: None,
        }
    }

    fn sent_thread(
        thread_id: &str,
        messages: Vec<model::GmailMessage>,
    ) -> model::GmailThread {
        model::GmailThread {
            id: thread_id.to_string(),
            history_id: None,
            messages,
        }
    }

    struct FakeTransport {
        draft: Mutex<Option<(String, model::GmailMessage)>>,
        draft_error: Mutex<Option<String>>,
        sent: Mutex<Vec<model::GmailThread>>,
        search_error: Mutex<Option<String>>,
        thread_error: Mutex<Option<String>>,
        send_receipt: Mutex<Option<SentReceipt>>,
        send_error: Mutex<Option<String>>,
        calls: Mutex<Vec<String>>,
        send_calls: Mutex<u32>,
        compose_scope: bool,
        on_send: Mutex<Option<Box<dyn Fn() + Send>>>,
    }

    impl FakeTransport {
        fn new() -> Self {
            Self {
                draft: Mutex::new(None),
                draft_error: Mutex::new(None),
                sent: Mutex::new(Vec::new()),
                search_error: Mutex::new(None),
                thread_error: Mutex::new(None),
                send_receipt: Mutex::new(None),
                send_error: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
                send_calls: Mutex::new(0),
                compose_scope: true,
                on_send: Mutex::new(None),
            }
        }

        fn with_draft(self, id: &str, message: model::GmailMessage) -> Self {
            *self.draft.lock().unwrap() = Some((id.to_string(), message));
            self
        }

        fn record(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_string());
        }

        fn call_count(&self, call: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.as_str() == call)
                .count()
        }

        fn send_call_count(&self) -> u32 {
            *self.send_calls.lock().unwrap()
        }
    }

    impl SendTransport for FakeTransport {
        fn draft_get(
            &self,
            _account: &str,
            draft_id: &str,
        ) -> Result<model::GmailDraft, GmailError> {
            self.record("draft_get");
            if let Some(kind) = self.draft_error.lock().unwrap().clone() {
                return Err(GmailError::runtime(
                    Box::leak(kind.into_boxed_str()),
                    "fake draft lookup failure".to_string(),
                ));
            }
            let guard = self.draft.lock().unwrap();
            match guard.as_ref() {
                Some((id, message)) if id == draft_id => {
                    Ok(model::GmailDraft {
                        id: id.clone(),
                        message: message.clone(),
                    })
                }
                _ => Err(GmailError::runtime(
                    "not_found",
                    "fake synthetic draft missing".to_string(),
                )),
            }
        }

        fn draft_send(
            &self,
            _account: &str,
            draft_id: &str,
        ) -> Result<SentReceipt, GmailError> {
            self.record("draft_send");
            *self.send_calls.lock().unwrap() += 1;
            if let Some(hook) = self.on_send.lock().unwrap().take() {
                hook();
            }
            if let Some(kind) = self.send_error.lock().unwrap().clone() {
                return Err(GmailError::runtime(
                    Box::leak(kind.into_boxed_str()),
                    "fake send failure".to_string(),
                ));
            }
            if let Some(receipt) = self.send_receipt.lock().unwrap().clone() {
                return Ok(receipt);
            }
            Ok(SentReceipt {
                message_id: format!("sent-for-{draft_id}"),
                thread_id: "thread-1".to_string(),
            })
        }

        fn search(
            &self,
            _account: &str,
            query: &str,
            _max: u32,
        ) -> Result<serde_json::Value, GmailError> {
            self.record("search");
            if let Some(kind) = self.search_error.lock().unwrap().clone() {
                return Err(GmailError::runtime(
                    Box::leak(kind.into_boxed_str()),
                    "fake search failure".to_string(),
                ));
            }
            let sent = self.sent.lock().unwrap();
            let mut ids = Vec::new();
            if let Some(rfc) = query
                .split("rfc822msgid:")
                .nth(1)
                .map(|rest| rest.split_whitespace().next().unwrap_or_default())
            {
                for thread in sent.iter() {
                    if thread.messages.iter().any(|message| {
                        message.headers.iter().any(|header| {
                            header.name.eq_ignore_ascii_case("Message-ID")
                                && header.value.trim() == rfc.trim()
                        })
                    }) {
                        ids.push(thread.id.clone());
                    }
                }
            } else {
                for thread in sent.iter() {
                    ids.push(thread.id.clone());
                }
            }
            Ok(serde_json::json!({
                "threads": ids.into_iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>(),
            }))
        }

        fn thread_metadata(
            &self,
            _account: &str,
            thread_id: &str,
        ) -> Result<model::GmailThread, GmailError> {
            self.record("thread_metadata");
            if let Some(kind) = self.thread_error.lock().unwrap().clone() {
                return Err(GmailError::runtime(
                    Box::leak(kind.into_boxed_str()),
                    "fake thread failure".to_string(),
                ));
            }
            let sent = self.sent.lock().unwrap();
            sent.iter()
                .find(|thread| thread.id == thread_id)
                .cloned()
                .ok_or_else(|| {
                    GmailError::runtime(
                        "not_found",
                        "fake thread missing".to_string(),
                    )
                })
        }

        fn accounts(
            &self,
            _client: Option<&str>,
        ) -> Result<Vec<google::gog::GogAccount>, GmailError> {
            self.record("accounts");
            if self.compose_scope {
                Ok(vec![google::gog::GogAccount {
                    email: "you@example.com".to_string(),
                    client: Some("bob-gmail".to_string()),
                    services: vec!["gmail".to_string()],
                    scopes: vec![
                        "https://www.googleapis.com/auth/gmail.readonly"
                            .to_string(),
                        "https://www.googleapis.com/auth/gmail.compose"
                            .to_string(),
                    ],
                }])
            } else {
                Ok(vec![google::gog::GogAccount {
                    email: "you@example.com".to_string(),
                    client: Some("bob-gmail".to_string()),
                    services: vec!["gmail".to_string()],
                    scopes: vec![
                        "https://www.googleapis.com/auth/gmail.readonly"
                            .to_string(),
                    ],
                }])
            }
        }
    }

    fn sent_now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn journal_text(vault: &Path) -> String {
        let dir = vault.join(".bob/gmail");
        let mut out = String::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                    out.push_str(
                        &std::fs::read_to_string(entry.path())
                            .unwrap_or_default(),
                    );
                }
            }
        }
        out
    }

    #[test]
    fn approved_subject_hash_is_body_free() {
        let approved = ApprovedSend {
            note: "mail/2026/note.md".to_string(),
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: "a".repeat(64),
            preview_digest: "b".repeat(64),
            draft_message_id: None,
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject: "Lease renewal".to_string(),
            recipient_count: 1,
            remote_fingerprint: "c".repeat(64),
        };
        let hash = approved.subject_hash();
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains("Lease"));
    }

    #[test]
    fn send_operation_id_is_stable() {
        let first =
            send_operation_id("note", &"a".repeat(64), "you@example.com");
        let second =
            send_operation_id("note", &"a".repeat(64), "you@example.com");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn crash_before_dispatch_restart_returns_to_draft() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/crash-before-dispatch.md");
        let body = format!("Hello {BODY_SENTINEL}.\n");
        write_compose_note(
            &vault,
            "mail/2026/crash-before-dispatch.md",
            "Lease renewal",
            &body,
        );
        let remote = "d".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/crash-before-dispatch.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let config = test_config();
        let approved = ApprovedSend {
            note: "mail/2026/crash-before-dispatch.md".to_string(),
            source_digest: fingerprint.clone(),
            remote_fingerprint: remote.clone(),
            ..approved_for(
                "mail/2026/crash-before-dispatch.md",
                &fingerprint,
                &remote,
            )
        };
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        // Crash before dispatch: persist `sending`, then never call send.
        let sending = begin_send(&vault, relative, &approved, &config)
            .expect("begin send");
        assert_eq!(sending.stage, SendStage::Sending);
        assert_eq!(transport.send_call_count(), 0);
        // Restart from disk: state survives, draft still exists.
        let reread =
            std::fs::read_to_string(vault.join(relative)).expect("reread");
        let loaded = state::load_send_state(&reread)
            .expect("load")
            .expect("sending");
        assert_eq!(loaded.operation_id, sending.operation_id);
        let decision = recover(&vault, relative, &transport, "you@example.com")
            .expect("recover");
        assert_eq!(decision, RecoveryDecision::DraftPresent);
        assert_eq!(transport.send_call_count(), 0);
        assert_eq!(transport.call_count("draft_send"), 0);
        // Returned to draft: no durable send state remains.
        let cleared =
            std::fs::read_to_string(vault.join(relative)).expect("cleared");
        assert!(state::load_send_state(&cleared)
            .expect("load cleared")
            .is_none());
        assert!(!journal_text(&vault).contains(BODY_SENTINEL));
    }

    #[test]
    fn timeout_before_gmail_accepts_retains_sending() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/timeout-before-accept.md");
        write_compose_note(
            &vault,
            "mail/2026/timeout-before-accept.md",
            "Lease renewal",
            &format!("Hello {BODY_SENTINEL}.\n"),
        );
        let remote = "e".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/timeout-before-accept.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let config = test_config();
        let approved = ApprovedSend {
            note: "mail/2026/timeout-before-accept.md".to_string(),
            source_digest: fingerprint.clone(),
            remote_fingerprint: remote.clone(),
            ..approved_for(
                "mail/2026/timeout-before-accept.md",
                &fingerprint,
                &remote,
            )
        };
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        *transport.send_error.lock().unwrap() = Some("timeout".to_string());
        let result = dispatch_once(
            &vault,
            relative,
            &transport,
            &approved,
            &config,
            |_| panic!("finalizer must not run on unknown send"),
        );
        assert!(result.is_err(), "timeout stays unknown: {result:?}");
        assert_eq!(transport.send_call_count(), 1);
        // Sending state retained for recovery.
        let contents =
            std::fs::read_to_string(vault.join(relative)).expect("read");
        let state = state::load_send_state(&contents)
            .expect("load")
            .expect("sending");
        assert_eq!(state.stage, SendStage::Sending);
        // Recovery still sees the draft and returns to draft without another send.
        let decision = recover(&vault, relative, &transport, "you@example.com")
            .expect("recover");
        assert_eq!(decision, RecoveryDecision::DraftPresent);
        assert_eq!(transport.send_call_count(), 1);
    }

    #[test]
    fn accepted_send_with_lost_response_acknowledges_unique_sent() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/lost-response.md");
        write_compose_note(
            &vault,
            "mail/2026/lost-response.md",
            "Lease renewal",
            &format!("Hello {BODY_SENTINEL}.\n"),
        );
        let remote = "f".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/lost-response.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let config = test_config();
        let approved = ApprovedSend {
            note: "mail/2026/lost-response.md".to_string(),
            source_digest: fingerprint.clone(),
            remote_fingerprint: remote.clone(),
            ..approved_for("mail/2026/lost-response.md", &fingerprint, &remote)
        };
        let _ = &config;
        // Simulate Gmail accepting while the response was lost: the draft is
        // gone and exactly one SENT message matches thread + subject + time.
        let transport = FakeTransport::new();
        let sending = SendState {
            version: state::SEND_VERSION,
            operation_id: send_operation_id(
                "mail/2026/lost-response.md",
                &"a".repeat(64),
                "you@example.com",
            ),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.clone(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject_hash: subject_hash("Lease renewal"),
            recipient_count: 1,
            attempt_time: state::attempt_timestamp(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some(remote.clone()),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
            completed_path_receipts: Vec::new(),
            watch_turns: Vec::new(),
        };
        state::persist_send_state(
            &vault,
            relative,
            &fingerprint,
            Some(sending),
        )
        .expect("persist sending");
        *transport.sent.lock().unwrap() = vec![sent_thread(
            "thread-1",
            vec![sent_message(
                "sent-1",
                "thread-1",
                "Lease renewal",
                None,
                sent_now_ms(),
            )],
        )];
        let finalized = Arc::new(Mutex::new(0u32));
        let marker = Arc::clone(&finalized);
        // Recovery (not dispatch) proves the send; the finalizer runs once.
        match recover(&vault, relative, &transport, "you@example.com")
            .expect("recover")
        {
            RecoveryDecision::Acknowledged {
                message_id,
                thread_id,
            } => {
                assert_eq!(message_id, "sent-1");
                assert_eq!(thread_id, "thread-1");
                *marker.lock().unwrap() += 1;
            }
            other => panic!("expected acknowledged, got {other:?}"),
        }
        assert_eq!(*finalized.lock().unwrap(), 1);
        assert_eq!(transport.send_call_count(), 0);
        assert_eq!(transport.call_count("draft_send"), 0);
        let contents =
            std::fs::read_to_string(vault.join(relative)).expect("read");
        let acknowledged = state::load_send_state(&contents)
            .expect("load")
            .expect("acked");
        assert_eq!(acknowledged.stage, SendStage::Acknowledged);
        assert_eq!(acknowledged.sent_message_id.as_deref(), Some("sent-1"));
        assert!(!journal_text(&vault).contains(BODY_SENTINEL));
        let _ = approved;
    }

    #[test]
    fn draft_gone_with_unique_sent_acknowledges() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/unique-sent.md");
        write_compose_note(
            &vault,
            "mail/2026/unique-sent.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "b".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/unique-sent.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let sending = SendState {
            version: state::SEND_VERSION,
            operation_id: send_operation_id(
                "mail/2026/unique-sent.md",
                &"a".repeat(64),
                "you@example.com",
            ),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.clone(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject_hash: subject_hash("Lease renewal"),
            recipient_count: 1,
            attempt_time: state::attempt_timestamp(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some(remote),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
            completed_path_receipts: Vec::new(),
            watch_turns: Vec::new(),
        };
        state::persist_send_state(
            &vault,
            relative,
            &fingerprint,
            Some(sending),
        )
        .expect("persist sending");
        let transport = FakeTransport::new();
        *transport.sent.lock().unwrap() = vec![sent_thread(
            "thread-1",
            vec![sent_message(
                "sent-9",
                "thread-1",
                "Lease renewal",
                None,
                sent_now_ms(),
            )],
        )];
        match recover(&vault, relative, &transport, "you@example.com")
            .expect("recover")
        {
            RecoveryDecision::Acknowledged {
                message_id,
                thread_id,
            } => {
                assert_eq!(message_id, "sent-9");
                assert_eq!(thread_id, "thread-1");
            }
            other => panic!("expected acknowledged, got {other:?}"),
        }
        assert_eq!(transport.call_count("draft_send"), 0);
    }

    #[test]
    fn unrelated_old_sent_stays_unknown() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/old-sent.md");
        write_compose_note(
            &vault,
            "mail/2026/old-sent.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "c".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/old-sent.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let sending = SendState {
            version: state::SEND_VERSION,
            operation_id: send_operation_id(
                "mail/2026/old-sent.md",
                &"a".repeat(64),
                "you@example.com",
            ),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.clone(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject_hash: subject_hash("Lease renewal"),
            recipient_count: 1,
            attempt_time: state::attempt_timestamp(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some(remote),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
            completed_path_receipts: Vec::new(),
            watch_turns: Vec::new(),
        };
        state::persist_send_state(
            &vault,
            relative,
            &fingerprint,
            Some(sending),
        )
        .expect("persist sending");
        let transport = FakeTransport::new();
        // Old unrelated SENT: different subject and long before the attempt.
        let old_ms = chrono::Utc::now().timestamp_millis() - 86_400_000;
        *transport.sent.lock().unwrap() = vec![sent_thread(
            "thread-1",
            vec![sent_message(
                "sent-old",
                "thread-1",
                "Different subject",
                None,
                old_ms,
            )],
        )];
        let decision = recover(&vault, relative, &transport, "you@example.com")
            .expect("recover");
        assert_eq!(
            decision,
            RecoveryDecision::Unknown {
                reason: "outcome_unknown"
            }
        );
        assert_eq!(transport.call_count("draft_send"), 0);
        let contents =
            std::fs::read_to_string(vault.join(relative)).expect("read");
        assert!(state::load_send_state(&contents).expect("load").is_some());
    }

    #[test]
    fn ambiguous_sent_matches_stay_unknown() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/ambiguous-sent.md");
        write_compose_note(
            &vault,
            "mail/2026/ambiguous-sent.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "d".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/ambiguous-sent.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let sending = SendState {
            version: state::SEND_VERSION,
            operation_id: send_operation_id(
                "mail/2026/ambiguous-sent.md",
                &"a".repeat(64),
                "you@example.com",
            ),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.clone(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject_hash: subject_hash("Lease renewal"),
            recipient_count: 1,
            attempt_time: state::attempt_timestamp(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some(remote),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
            completed_path_receipts: Vec::new(),
            watch_turns: Vec::new(),
        };
        state::persist_send_state(
            &vault,
            relative,
            &fingerprint,
            Some(sending),
        )
        .expect("persist sending");
        let transport = FakeTransport::new();
        let now = sent_now_ms();
        *transport.sent.lock().unwrap() = vec![sent_thread(
            "thread-1",
            vec![
                sent_message("sent-a", "thread-1", "Lease renewal", None, now),
                sent_message("sent-b", "thread-1", "Lease renewal", None, now),
            ],
        )];
        let decision = recover(&vault, relative, &transport, "you@example.com")
            .expect("recover");
        assert_eq!(
            decision,
            RecoveryDecision::Unknown {
                reason: "outcome_unknown"
            }
        );
        assert_eq!(transport.call_count("draft_send"), 0);
    }

    #[test]
    fn draft_gone_without_evidence_stays_unknown() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/no-evidence.md");
        write_compose_note(
            &vault,
            "mail/2026/no-evidence.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "e".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/no-evidence.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let sending = SendState {
            version: state::SEND_VERSION,
            operation_id: send_operation_id(
                "mail/2026/no-evidence.md",
                &"a".repeat(64),
                "you@example.com",
            ),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.clone(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject_hash: subject_hash("Lease renewal"),
            recipient_count: 1,
            attempt_time: state::attempt_timestamp(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some(remote),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
            completed_path_receipts: Vec::new(),
            watch_turns: Vec::new(),
        };
        state::persist_send_state(
            &vault,
            relative,
            &fingerprint,
            Some(sending),
        )
        .expect("persist sending");
        let transport = FakeTransport::new();
        let decision = recover(&vault, relative, &transport, "you@example.com")
            .expect("recover");
        assert_eq!(
            decision,
            RecoveryDecision::Unknown {
                reason: "outcome_unknown"
            }
        );
        assert_eq!(transport.call_count("draft_send"), 0);
    }

    #[test]
    fn lookup_failure_retains_state_and_reports_typed_error() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/lookup-failure.md");
        write_compose_note(
            &vault,
            "mail/2026/lookup-failure.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "f".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/lookup-failure.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let sending = SendState {
            version: state::SEND_VERSION,
            operation_id: send_operation_id(
                "mail/2026/lookup-failure.md",
                &"a".repeat(64),
                "you@example.com",
            ),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.clone(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject_hash: subject_hash("Lease renewal"),
            recipient_count: 1,
            attempt_time: state::attempt_timestamp(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some(remote),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
            completed_path_receipts: Vec::new(),
            watch_turns: Vec::new(),
        };
        state::persist_send_state(
            &vault,
            relative,
            &fingerprint,
            Some(sending),
        )
        .expect("persist sending");
        let transport = FakeTransport::new();
        *transport.search_error.lock().unwrap() = Some("timeout".to_string());
        let error = recover(&vault, relative, &transport, "you@example.com")
            .expect_err("lookup failure is typed");
        assert_eq!(error.kind(), "timeout");
        assert_eq!(transport.call_count("draft_send"), 0);
        let contents =
            std::fs::read_to_string(vault.join(relative)).expect("read");
        assert!(state::load_send_state(&contents).expect("load").is_some());
    }

    #[test]
    fn persist_intent_failure_sends_nothing() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/intent-failure.md");
        write_compose_note(
            &vault,
            "mail/2026/intent-failure.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "a".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/intent-failure.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let config = test_config();
        let mut approved =
            approved_for("mail/2026/intent-failure.md", &fingerprint, &remote);
        approved.source_digest = "b".repeat(64);
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        let error = begin_send(&vault, relative, &approved, &config)
            .expect_err("stale preimage must not persist intent");
        assert_eq!(error.kind(), "confirmation_mismatch");
        assert_eq!(transport.send_call_count(), 0);
        let contents =
            std::fs::read_to_string(vault.join(relative)).expect("read");
        assert!(state::load_send_state(&contents).expect("load").is_none());
    }

    #[test]
    fn malformed_state_is_a_typed_refusal() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/malformed-state.md");
        write_compose_note(
            &vault,
            "mail/2026/malformed-state.md",
            "Lease renewal",
            "Hello.\n",
        );
        let path = vault.join(relative);
        let contents = std::fs::read_to_string(&path).expect("read");
        let broken = contents.replace(
            "subject: Lease renewal",
            "subject: Lease renewal\ngmail_send:\n  version: 99",
        );
        std::fs::write(&path, broken).expect("write broken");
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        let error = recover(&vault, relative, &transport, "you@example.com")
            .expect_err("malformed state refuses");
        assert_eq!(error.kind(), "send_state_invalid");
        assert_eq!(transport.call_count("draft_send"), 0);
    }

    #[test]
    fn acknowledge_persist_failure_stays_sending_without_finalize() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/ack-failure.md");
        write_compose_note(
            &vault,
            "mail/2026/ack-failure.md",
            "Lease renewal",
            "Source body.\n",
        );
        let remote = "b".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/ack-failure.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let config = test_config();
        let approved = ApprovedSend {
            note: "mail/2026/ack-failure.md".to_string(),
            source_digest: fingerprint.clone(),
            remote_fingerprint: remote.clone(),
            ..approved_for("mail/2026/ack-failure.md", &fingerprint, &remote)
        };
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        *transport.send_receipt.lock().unwrap() = Some(SentReceipt {
            message_id: "sent-ack-fail".to_string(),
            thread_id: "thread-1".to_string(),
        });
        // Concurrent source edit during dispatch makes the acknowledgement
        // unpersistable; the send stays `sending` and the finalizer never runs.
        let vault_path = vault.clone();
        let relative_path = relative.to_path_buf();
        *transport.on_send.lock().unwrap() = Some(Box::new(move || {
            let path = vault_path.join(&relative_path);
            let contents =
                std::fs::read_to_string(&path).expect("reread for race");
            let edited = contents.replacen(
                "Source body.",
                "Source body.\nConcurrent edit.",
                1,
            );
            std::fs::write(&path, edited).expect("concurrent edit");
        }));
        let finalized = Arc::new(Mutex::new(0u32));
        let result = dispatch_once(
            &vault,
            relative,
            &transport,
            &approved,
            &config,
            |_| {
                *finalized.lock().unwrap() += 1;
                Ok(())
            },
        );
        assert!(result.is_err(), "ack failure stays pending: {result:?}");
        assert_eq!(*finalized.lock().unwrap(), 0);
        assert_eq!(transport.send_call_count(), 1);
    }

    #[test]
    fn race_two_dispatches_sends_once() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/dispatch-race.md");
        write_compose_note(
            &vault,
            "mail/2026/dispatch-race.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "c".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/dispatch-race.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let approved = ApprovedSend {
            note: "mail/2026/dispatch-race.md".to_string(),
            source_digest: fingerprint.clone(),
            remote_fingerprint: remote.clone(),
            ..approved_for("mail/2026/dispatch-race.md", &fingerprint, &remote)
        };
        let transport = Arc::new(FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        ));
        *transport.send_receipt.lock().unwrap() = Some(SentReceipt {
            message_id: "sent-race".to_string(),
            thread_id: "thread-1".to_string(),
        });
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let vault_path = vault.clone();
                let transport = Arc::clone(&transport);
                let approved = approved.clone();
                let config = test_config();
                std::thread::spawn(move || {
                    dispatch_once(
                        &vault_path,
                        relative,
                        &*transport,
                        &approved,
                        &config,
                        |_| Ok(()),
                    )
                })
            })
            .collect();
        let mut outcomes = Vec::new();
        for handle in handles {
            outcomes.push(handle.join().expect("thread"));
        }
        assert_eq!(transport.send_call_count(), 1, "at most one dispatch");
        assert!(outcomes.iter().any(|result| matches!(
            result,
            Ok(DispatchOutcome::Sent { .. })
                | Ok(DispatchOutcome::AlreadyAcknowledged { .. })
        )));
    }

    #[test]
    fn writer_and_scope_refusals_send_nothing() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/refusals.md");
        write_compose_note(
            &vault,
            "mail/2026/refusals.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "d".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/refusals.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let approved =
            approved_for("mail/2026/refusals.md", &fingerprint, &remote);
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        let writer = writer_config("other-host");
        let error = begin_send(&vault, relative, &approved, &writer)
            .expect_err("writer refusal");
        assert_eq!(error.kind(), "writer");
        assert_eq!(transport.send_call_count(), 0);
        let mut no_scope = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        no_scope.compose_scope = false;
        let config = test_config();
        let result = dispatch_once(
            &vault,
            relative,
            &no_scope,
            &approved,
            &config,
            |_| panic!("no finalize without scope"),
        );
        assert!(result.is_err());
        assert_eq!(no_scope.send_call_count(), 0);
    }

    #[test]
    fn stale_digest_sends_nothing() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/stale-digest.md");
        write_compose_note(
            &vault,
            "mail/2026/stale-digest.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "e".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/stale-digest.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let config = test_config();
        let mut approved =
            approved_for("mail/2026/stale-digest.md", &fingerprint, &remote);
        approved.source_digest = "f".repeat(64);
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        let result = dispatch_once(
            &vault,
            relative,
            &transport,
            &approved,
            &config,
            |_| panic!("no finalize on stale digest"),
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), "confirmation_mismatch");
        assert_eq!(transport.call_count("draft_send"), 0);
        assert_eq!(transport.call_count("draft_get"), 0);
    }

    #[test]
    fn rfc_lookup_prefers_exact_message_id() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/rfc-lookup.md");
        write_compose_note(
            &vault,
            "mail/2026/rfc-lookup.md",
            "Lease renewal",
            "Hello.\n",
        );
        let remote = "f".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/rfc-lookup.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let rfc = "<exact@example.test>";
        let sending = SendState {
            version: state::SEND_VERSION,
            operation_id: send_operation_id(
                "mail/2026/rfc-lookup.md",
                &"a".repeat(64),
                "you@example.com",
            ),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: fingerprint.clone(),
            preview_digest: "a".repeat(64),
            draft_message_id: Some("draft-message-1".to_string()),
            draft_rfc_id: Some(rfc.to_string()),
            thread_id: "thread-1".to_string(),
            subject_hash: subject_hash("Lease renewal"),
            recipient_count: 1,
            attempt_time: state::attempt_timestamp(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some(remote),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
            completed_path_receipts: Vec::new(),
            watch_turns: Vec::new(),
        };
        state::persist_send_state(
            &vault,
            relative,
            &fingerprint,
            Some(sending),
        )
        .expect("persist sending");
        let transport = FakeTransport::new();
        let now = sent_now_ms();
        *transport.sent.lock().unwrap() = vec![
            sent_thread(
                "thread-1",
                vec![sent_message(
                    "sent-exact",
                    "thread-1",
                    "Lease renewal",
                    Some(rfc),
                    now,
                )],
            ),
            sent_thread(
                "thread-1",
                vec![sent_message(
                    "sent-other",
                    "thread-1",
                    "Lease renewal",
                    Some("<other@example.test>"),
                    now,
                )],
            ),
        ];
        match recover(&vault, relative, &transport, "you@example.com")
            .expect("recover")
        {
            RecoveryDecision::Acknowledged { message_id, .. } => {
                assert_eq!(message_id, "sent-exact");
            }
            other => panic!("expected exact RFC match, got {other:?}"),
        }
        assert_eq!(transport.call_count("draft_send"), 0);
    }

    #[test]
    fn journals_and_state_stay_body_free() {
        let vault = tempfile::tempdir().expect("vault").keep();
        let relative = Path::new("mail/2026/body-free.md");
        write_compose_note(
            &vault,
            "mail/2026/body-free.md",
            "Lease renewal",
            &format!("Hello {BODY_SENTINEL}.\n"),
        );
        let remote = "a".repeat(64);
        let fingerprint = add_draft_metadata(
            &vault,
            "mail/2026/body-free.md",
            "you@example.com",
            "draft-1",
            &remote,
        );
        let config = test_config();
        let approved =
            approved_for("mail/2026/body-free.md", &fingerprint, &remote);
        let transport = FakeTransport::new().with_draft(
            "draft-1",
            draft_message("draft-message-1", "thread-1"),
        );
        *transport.send_receipt.lock().unwrap() = Some(SentReceipt {
            message_id: "sent-body-free".to_string(),
            thread_id: "thread-1".to_string(),
        });
        let outcome = dispatch_once(
            &vault,
            relative,
            &transport,
            &approved,
            &config,
            |_| Ok(()),
        )
        .expect("dispatch");
        assert!(matches!(outcome, DispatchOutcome::Sent { .. }));
        let note = std::fs::read_to_string(vault.join(relative)).expect("note");
        // The body lives in the note's message region, but the durable send
        // block itself must never carry it. Only inspect the frontmatter
        // `gmail_send:` block, not the message body that follows it.
        let start = note.find("gmail_send:").expect("send state missing");
        let block_end = note[start..]
            .find("\n---")
            .map(|offset| start + offset)
            .unwrap_or(note.len());
        assert!(!note[start..block_end].contains(BODY_SENTINEL));
        assert!(!journal_text(&vault).contains(BODY_SENTINEL));
    }
}

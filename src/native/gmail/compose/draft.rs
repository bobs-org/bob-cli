//! Gmail draft synchronization and the exact readback preview.
//!
//! Request bodies and attachment bytes only pass through private temporary
//! files or memory. Durable note metadata and journal events contain draft
//! identity and digests, never message text.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use base64::Engine as _;
use chrono::SecondsFormat;
use fs2::FileExt as _;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::native::{
    capture::StagedTextFile,
    env as bob_env,
    gmail::{
        body::{self, CandidateKind, FetchedBodies},
        cli::{DraftArgs, HumanFormat},
        compose::{attachments, render, source},
        config::{GmailConfig, COMPOSE_SCOPE},
        gog::{DraftCompose, Gog},
        model::{GmailMessage, HeaderKind, MimePart},
        resolve, ui, GmailError,
    },
    google,
    pref_journal::{self, Namespace},
    vault_sync,
};

const DRAFT_JOURNAL: Namespace = Namespace {
    dir: ".bob/gmail",
    noun: "gmail",
};

#[derive(Serialize)]
struct PreviewDocument<'a> {
    schema_version: u8,
    ok: bool,
    status: &'static str,
    note: &'a str,
    draft_id: &'a str,
    digest: &'a str,
    preview: &'a Preview,
}

/// Field order is the canonical digest order. Do not reorder without changing
/// `canonical_version`.
#[derive(Serialize)]
pub(crate) struct Preview {
    pub(crate) account: String,
    pub(crate) from: String,
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    pub(crate) bcc: Vec<String>,
    pub(crate) subject: String,
    pub(crate) reply_target: Option<ReplyPreview>,
    pub(crate) text_body: String,
    pub(crate) html_sha256: String,
    pub(crate) signature: &'static str,
    pub(crate) attachments: Vec<AttachmentPreview>,
    pub(crate) flattened_links: Vec<FlattenedLinkPreview>,
}

#[derive(Serialize)]
pub(crate) struct ReplyPreview {
    pub(crate) thread_id: String,
    pub(crate) message_id: String,
    pub(crate) rfc_message_id: Option<String>,
    pub(crate) mode: &'static str,
}

#[derive(Serialize)]
pub(crate) struct AttachmentPreview {
    pub(crate) vault_path: String,
    pub(crate) filename: String,
    pub(crate) byte_length: u64,
    pub(crate) sha256: String,
}

#[derive(Serialize)]
pub(crate) struct FlattenedLinkPreview {
    pub(crate) original_target: String,
    pub(crate) text: String,
}

#[derive(Serialize)]
struct CanonicalDigest<'a> {
    canonical_version: u8,
    note: &'a str,
    preview: &'a Preview,
}

#[derive(Serialize)]
struct RemoteFingerprint<'a> {
    version: u8,
    account: &'a str,
    from: &'a str,
    to: &'a [String],
    cc: &'a [String],
    bcc: &'a [String],
    subject: &'a str,
    thread_id: &'a str,
    in_reply_to: &'a [String],
    text_body: &'a str,
    html_sha256: &'a str,
    attachments: &'a [RemoteAttachment],
}

#[derive(Serialize)]
struct RemoteAttachment {
    filename: String,
    byte_length: u64,
    sha256: String,
}

struct Readback {
    draft_id: String,
    message_id: String,
    rfc_message_id: Option<String>,
    thread_id: String,
    preview: Preview,
    remote_fingerprint: String,
}

/// Exact stored-draft preview plus the identities needed by the send engine.
/// This deliberately has no `Debug` implementation because `preview` carries
/// the outgoing body.
pub(crate) struct PreparedDraft {
    pub(crate) note: String,
    pub(crate) draft_id: String,
    pub(crate) digest: String,
    pub(crate) preview: Preview,
    pub(crate) approved: super::send::ApprovedSend,
}

struct DraftRequest {
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    attachments: Vec<PathBuf>,
    signature: bool,
    reply_to_message_id: Option<String>,
    reply_all: bool,
    clear_reply_context: bool,
}

impl DraftRequest {
    fn as_transport<'a>(&'a self, files: &'a BodyFiles) -> DraftCompose<'a> {
        DraftCompose {
            to: &self.to,
            cc: &self.cc,
            bcc: &self.bcc,
            subject: &self.subject,
            body_file: &files.plain,
            html_file: &files.html,
            attachments: &self.attachments,
            signature: self.signature,
            reply_to_message_id: self.reply_to_message_id.as_deref(),
            reply_all: self.reply_all,
            clear_reply_context: self.clear_reply_context,
        }
    }
}

struct BodyFiles {
    dir: tempfile::TempDir,
    plain: PathBuf,
    html: PathBuf,
}

impl BodyFiles {
    fn new(plain: &str, html: &str) -> Result<Self, GmailError> {
        let dir = tempfile::TempDir::new().map_err(|error| {
            GmailError::runtime(
                "staging",
                format!("could not create private draft staging: {error}"),
            )
        })?;
        restrict_dir(dir.path())?;
        let plain_path = dir.path().join("body.txt");
        let html_path = dir.path().join("body.html");
        write_private(&plain_path, plain.as_bytes())?;
        write_private(&html_path, html.as_bytes())?;
        Ok(Self {
            dir,
            plain: plain_path,
            html: html_path,
        })
    }
}

impl std::fmt::Debug for BodyFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodyFiles")
            .field("dir", &self.dir.path())
            .finish_non_exhaustive()
    }
}

pub(crate) fn run(args: &DraftArgs) -> i32 {
    match prepare_for_send(args, false) {
        Ok(prepared) => {
            report_success(
                &prepared.note,
                &prepared.draft_id,
                &prepared.digest,
                &prepared.preview,
                args.format,
            );
            0
        }
        Err(error) => ui::report_error("draft", &error, args.format.as_str()),
    }
}

pub(crate) fn prepare_for_send(
    args: &DraftArgs,
    operation_locked: bool,
) -> Result<PreparedDraft, GmailError> {
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
    let note_path = path_to_slashes(&relative);
    let _operation = if operation_locked {
        None
    } else {
        Some(operation_lock(&relative)?)
    };

    let (_original, parsed) = read_source(&vault, &relative)?;
    let gog = Gog::resolve(google::GMAIL, config.timeout())?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    require_compose_grant(&gog, &account)?;
    if parsed
        .pinned_account
        .as_deref()
        .is_some_and(|pinned| !pinned.eq_ignore_ascii_case(&account))
    {
        return Err(GmailError::runtime(
            "other_account",
            "this compose note is pinned to a different Gmail account"
                .to_string(),
        ));
    }
    if let Some(target) = parsed.reply_target.as_ref()
        && !target.account.eq_ignore_ascii_case(&account)
    {
        return Err(GmailError::runtime(
            "other_account",
            "the saved reply note belongs to a different Gmail account"
                .to_string(),
        ));
    }

    if let Some(operation) = parsed.create_operation.as_deref() {
        return recover_create(
            &vault,
            &relative,
            &note_path,
            &account,
            operation,
            parsed.draft_id.as_deref(),
            &parsed,
            &gog,
            &config,
        );
    }

    let signature = matches!(
        config.signature(),
        crate::native::config::GmailSignature::Gmail
    );
    let existing_readback = if let Some(draft_id) = parsed.draft_id.as_deref() {
        let remote = readback(
            &gog, &account, draft_id, &parsed, None, None, signature, false,
        )?;
        let Some(expected_fingerprint) = parsed.remote_fingerprint.as_deref()
        else {
            return Err(GmailError::runtime(
                "draft_baseline_missing",
                "the recorded draft has no trusted readback fingerprint".to_string(),
            )
            .with_hint("inspect the draft in Gmail before replacing its recorded identity"));
        };
        if remote.remote_fingerprint != expected_fingerprint {
            return Err(GmailError::runtime(
                "draft_remote_conflict",
                "the stored Gmail draft changed outside Bob".to_string(),
            )
            .with_hint("review the Gmail draft, then resolve the conflict before running `bob gmail draft` again"));
        }
        Some(remote)
    } else {
        None
    };

    let rendered = render::render(parsed.body(), &parsed.attachments).map_err(
        |error| {
            GmailError::setup("render", error.to_string())
                .with_hint(error.hint())
        },
    )?;
    let attachments =
        attachments::AttachmentStaging::prepare(&vault, &parsed.attachments)
            .map_err(|error| {
                GmailError::setup("attachment", error.to_string())
            })?;
    let body_files = BodyFiles::new(&rendered.plain_body, &rendered.html_body)?;
    let request = compose_request(
        &parsed,
        &account,
        &attachments,
        existing_readback.as_ref(),
        &gog,
        signature,
    )?;

    let mut create_operation = None;
    let draft_id = if let Some(draft_id) = parsed.draft_id.as_deref() {
        let compose = request.as_transport(&body_files);
        let receipt = gog.draft_update(&account, draft_id, &compose)?;
        if receipt.id != draft_id {
            return Err(GmailError::runtime(
                "draft_identity_changed",
                "Gmail returned a different draft id during update".to_string(),
            ));
        }
        draft_id.to_string()
    } else {
        let operation = operation_id(&note_path, &parsed.fingerprint, &account);
        create_operation = Some(operation.clone());
        persist_create_marker(
            &vault,
            &relative,
            &account,
            &operation,
            &parsed.fingerprint,
        )?;
        let compose = request.as_transport(&body_files);
        let receipt = match gog.draft_create(&account, &compose) {
            Ok(receipt) => receipt,
            Err(error) => {
                return Err(GmailError::runtime(
                    "draft_create_unknown",
                    format!("Gmail did not confirm draft creation ({})", error.kind()),
                )
                .with_hint("the note is marked as an unresolved create; inspect Gmail before retrying"));
            }
        };
        receipt.id
    };

    let mut fetched = match readback(
        &gog,
        &account,
        &draft_id,
        &parsed,
        Some(&rendered),
        Some(&attachments),
        signature,
        true,
    ) {
        Ok(readback) => readback,
        Err(error) if create_operation.is_some() => {
            let operation = create_operation.as_deref().unwrap_or_default();
            if persist_recovery_id(
                &vault, &relative, &account, &draft_id, operation,
            )
            .is_err()
            {
                append_recovery_event(
                    &vault,
                    &note_path,
                    &account,
                    &draft_id,
                    &parsed.fingerprint,
                    operation,
                )?;
            }
            return Err(GmailError::runtime(
                "draft_readback_unknown",
                format!("the Gmail draft id is known, but exact readback failed ({})", error.kind()),
            )
            .with_hint("the draft id was retained in Bob's recovery state; rerun `bob gmail draft NOTE` to read it back"));
        }
        Err(error) => return Err(error),
    };
    fetched.draft_id = draft_id.clone();
    if let Err(error) = verify_request_envelope(
        &fetched.preview,
        &request,
        parsed.reply_target.is_some(),
    ) {
        if let Some(operation) = create_operation.as_deref()
            && persist_recovery_id(
                &vault, &relative, &account, &draft_id, operation,
            )
            .is_err()
        {
            append_recovery_event(
                &vault,
                &note_path,
                &account,
                &draft_id,
                &parsed.fingerprint,
                operation,
            )?;
        }
        return Err(error);
    }
    let attachments_changed =
        attachments::AttachmentStaging::prepare(&vault, &parsed.attachments)
            .map(|current| !same_attachment_snapshots(&attachments, &current))
            .unwrap_or(true);
    let source_changed = persist_after_remote(
        &vault,
        &relative,
        &note_path,
        &account,
        &draft_id,
        &fetched.remote_fingerprint,
        &parsed.fingerprint,
        create_operation.as_deref(),
    )? || attachments_changed;
    if source_changed {
        return Err(GmailError::runtime(
            "compose_changed",
            "the compose source changed while Bob was updating its Gmail draft".to_string(),
        )
        .with_hint("the draft id was preserved; run `bob gmail draft NOTE` again to update the newer text"));
    }

    let digest = canonical_digest(&note_path, &fetched.preview)?;
    Ok(prepared_draft(
        note_path, draft_id, digest, &parsed, &account, fetched,
    ))
}

fn prepared_draft(
    note: String,
    draft_id: String,
    digest: String,
    source: &source::ComposeSource,
    account: &str,
    readback: Readback,
) -> PreparedDraft {
    let approved = super::send::ApprovedSend {
        note: note.clone(),
        account: account.to_string(),
        draft_id: draft_id.clone(),
        source_digest: source.fingerprint.clone(),
        preview_digest: digest.clone(),
        draft_message_id: Some(readback.message_id.clone()),
        draft_rfc_id: readback.rfc_message_id.clone(),
        thread_id: readback.thread_id.clone(),
        subject: readback.preview.subject.clone(),
        recipient_count: (readback.preview.to.len()
            + readback.preview.cc.len()
            + readback.preview.bcc.len()) as u32,
        remote_fingerprint: readback.remote_fingerprint.clone(),
    };
    PreparedDraft {
        note,
        draft_id,
        digest,
        preview: readback.preview,
        approved,
    }
}

fn same_attachment_snapshots(
    left: &attachments::AttachmentStaging,
    right: &attachments::AttachmentStaging,
) -> bool {
    left.files().len() == right.files().len()
        && left.files().iter().zip(right.files()).all(|(left, right)| {
            left.vault_path == right.vault_path
                && left.filename == right.filename
                && left.len == right.len
                && left.sha256 == right.sha256
        })
}

fn compose_request(
    source: &source::ComposeSource,
    account: &str,
    attachments: &attachments::AttachmentStaging,
    existing: Option<&Readback>,
    gog: &Gog,
    signature: bool,
) -> Result<DraftRequest, GmailError> {
    if source.subject.trim().is_empty() && source.reply_target.is_none() {
        return Err(GmailError::setup(
            "subject",
            "a Gmail draft needs a subject".to_string(),
        ));
    }
    let (to, cc, bcc, subject, reply_to_message_id, reply_all) =
        if let Some(existing) = existing {
            if source.reply_target.is_some() {
                (
                    existing.preview.to.clone(),
                    existing.preview.cc.clone(),
                    existing.preview.bcc.clone(),
                    existing.preview.subject.clone(),
                    None,
                    false,
                )
            } else {
                (
                    source.to.clone(),
                    source.cc.clone(),
                    source.bcc.clone(),
                    source.subject.clone(),
                    None,
                    false,
                )
            }
        } else if let Some(reply) = source.reply_target.as_ref() {
            let thread = gog.thread_get(account, &reply.thread_id)?;
            if thread.id != reply.thread_id {
                return Err(GmailError::runtime(
                "reply_target_mismatch",
                "Gmail returned a different thread for the selected reply note"
                    .to_string(),
            ));
            }
            let target_message = thread
                .messages
                .iter()
                .find(|message| message.id == reply.message_id)
                .filter(|message| !is_draft(message))
                .ok_or_else(|| {
                    GmailError::runtime(
                    "reply_target_missing",
                    "the selected reply message is missing or is still a draft"
                        .to_string(),
                )
                })?;
            let original_subject = target_message
                .first_header(HeaderKind::Subject)
                .unwrap_or_default();
            let subject = reply_subject(original_subject);
            let recipients = if matches!(
                source.kind,
                source::ComposeKind::Reply { all: false }
            ) {
                let from = target_message
                    .first_header(HeaderKind::From)
                    .ok_or_else(|| {
                        GmailError::runtime(
                            "reply_target_missing",
                            "the selected reply message has no sender address"
                                .to_string(),
                        )
                    })?;
                source::parse_recipient_list(from).map_err(|_| {
                    GmailError::runtime(
                    "reply_target_invalid",
                    "the selected reply message has an invalid sender address"
                        .to_string(),
                )
                })?
            } else {
                Vec::new()
            };
            (
                recipients,
                Vec::new(),
                Vec::new(),
                subject,
                Some(reply.message_id.clone()),
                matches!(source.kind, source::ComposeKind::Reply { all: true }),
            )
        } else {
            (
                source.to.clone(),
                source.cc.clone(),
                source.bcc.clone(),
                source.subject.clone(),
                None,
                false,
            )
        };
    let attachment_paths = attachments
        .files()
        .iter()
        .map(|attachment| attachment.snapshot.clone())
        .collect::<Vec<_>>();
    Ok(DraftRequest {
        to,
        cc,
        bcc,
        subject,
        attachments: attachment_paths,
        signature,
        reply_to_message_id,
        reply_all,
        clear_reply_context: source.reply_target.is_none(),
    })
}

fn is_draft(message: &GmailMessage) -> bool {
    message
        .label_ids
        .iter()
        .any(|label| label.eq_ignore_ascii_case("DRAFT"))
}

fn reply_subject(subject: &str) -> String {
    let subject = subject.trim();
    if subject.is_empty() {
        "Re: (no subject)".to_string()
    } else if subject
        .split_once(':')
        .is_some_and(|(prefix, _)| prefix.eq_ignore_ascii_case("re"))
    {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

fn require_compose_grant(gog: &Gog, account: &str) -> Result<(), GmailError> {
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

fn read_source(
    vault: &Path,
    relative: &Path,
) -> Result<(String, source::ComposeSource), GmailError> {
    let _maintenance = super::maintenance_lock()?;
    super::validate_target(vault, relative, true)?;
    let path = vault.join(relative);
    let contents = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read compose note: {error}"),
        )
    })?;
    let parsed = source::parse(&contents)
        .map_err(|error| GmailError::setup("note", error.to_string()))?;
    Ok((contents, parsed))
}

fn persist_create_marker(
    vault: &Path,
    relative: &Path,
    account: &str,
    operation: &str,
    expected_fingerprint: &str,
) -> Result<(), GmailError> {
    let _maintenance = super::maintenance_lock()?;
    let path = vault.join(relative);
    let original = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not re-read compose note: {error}"),
        )
    })?;
    let current = source::parse(&original)
        .map_err(|error| GmailError::setup("note", error.to_string()))?;
    if current.fingerprint != expected_fingerprint {
        return Err(GmailError::runtime(
            "compose_changed",
            "the compose source changed before Gmail draft creation"
                .to_string(),
        ));
    }
    let updated = source::persist_draft_metadata(
        &original,
        account,
        None,
        Some(operation),
        None,
    )
    .map_err(|error| GmailError::runtime("note", error.to_string()))?;
    let staged = vec![StagedTextFile {
        target: path,
        target_existed: true,
        original_target: original,
        updated_target: updated,
    }];
    super::write_and_commit(vault, staged, "gmail: draft operation")
}

fn persist_after_remote(
    vault: &Path,
    relative: &Path,
    note: &str,
    account: &str,
    draft_id: &str,
    remote_fingerprint: &str,
    expected_source_fingerprint: &str,
    recovery_operation: Option<&str>,
) -> Result<bool, GmailError> {
    let _maintenance = super::maintenance_lock()?;
    let path = vault.join(relative);
    let original = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not re-read compose note: {error}"),
        )
    })?;
    let current = source::parse(&original)
        .map_err(|error| GmailError::setup("note", error.to_string()))?;
    let changed = current.fingerprint != expected_source_fingerprint;
    let updated = match source::persist_draft_metadata(
        &original,
        account,
        Some(draft_id),
        None,
        Some(remote_fingerprint),
    ) {
        Ok(updated) => updated,
        Err(error) => {
            append_recovery_event(
                vault,
                note,
                account,
                draft_id,
                expected_source_fingerprint,
                recovery_operation.unwrap_or_default(),
            )
            .map_err(|journal_error| {
                GmailError::runtime(
                    "draft_identity_unpersisted",
                    format!("could not persist the known Gmail draft id ({error}) or its recovery record ({})", journal_error.kind()),
                )
            })?;
            return Err(GmailError::runtime(
                "draft_identity_unpersisted",
                "the Gmail draft exists, but its identity could not be added to the note".to_string(),
            )
            .with_hint("a body-free recovery journal entry was recorded; rerun `bob gmail draft NOTE`"));
        }
    };
    let staged = vec![StagedTextFile {
        target: path,
        target_existed: true,
        original_target: original,
        updated_target: updated,
    }];
    if let Err(error) = super::write_and_commit(vault, staged, "gmail: draft") {
        append_recovery_event(
            vault,
            note,
            account,
            draft_id,
            expected_source_fingerprint,
            recovery_operation.unwrap_or_default(),
        )
        .map_err(|journal_error| {
            GmailError::runtime(
                "draft_identity_unpersisted",
                format!("the remote draft is known but note persistence failed ({}) and recovery journaling failed ({})", error.kind(), journal_error.kind()),
            )
        })?;
        return Err(GmailError::runtime(
            "draft_identity_unpersisted",
            "the Gmail draft exists, but the note could not be updated".to_string(),
        )
        .with_hint("a body-free recovery journal entry was recorded; rerun `bob gmail draft NOTE`"));
    }
    append_event(
        vault,
        "gmail.draft",
        serde_json::json!({
            "account": account,
            "note": note,
            "draft_id": draft_id,
        }),
    )?;
    Ok(changed)
}

fn recover_create(
    vault: &Path,
    relative: &Path,
    note: &str,
    account: &str,
    operation: &str,
    stored_id: Option<&str>,
    parsed: &source::ComposeSource,
    gog: &Gog,
    config: &GmailConfig,
) -> Result<PreparedDraft, GmailError> {
    let recovered = stored_id
        .map(str::to_string)
        .or_else(|| known_recovery_draft(vault, note, operation));
    let Some(draft_id) = recovered else {
        return Err(GmailError::runtime(
            "draft_create_unresolved",
            "a previous Gmail draft creation has no confirmed draft id".to_string(),
        )
        .with_hint("inspect Gmail for a matching draft; Bob will not create another draft automatically"));
    };
    let rendered = render::render(parsed.body(), &parsed.attachments).map_err(
        |error| {
            GmailError::setup("render", error.to_string())
                .with_hint(error.hint())
        },
    )?;
    let attachments =
        attachments::AttachmentStaging::prepare(vault, &parsed.attachments)
            .map_err(|error| {
                GmailError::setup("attachment", error.to_string())
            })?;
    let operation_matches_current =
        operation_id(note, &parsed.fingerprint, account) == operation;
    let signature = matches!(
        config.signature(),
        crate::native::config::GmailSignature::Gmail
    );
    let readback = readback(
        gog,
        account,
        &draft_id,
        parsed,
        Some(&rendered),
        Some(&attachments),
        signature,
        operation_matches_current,
    )?;
    if operation_matches_current {
        let request = compose_request(
            parsed,
            account,
            &attachments,
            None,
            gog,
            signature,
        )?;
        verify_request_envelope(
            &readback.preview,
            &request,
            parsed.reply_target.is_some(),
        )?;
    }
    let write_changed = persist_after_remote(
        vault,
        relative,
        note,
        account,
        &draft_id,
        &readback.remote_fingerprint,
        &parsed.fingerprint,
        Some(operation),
    )?;
    let changed = !operation_matches_current || write_changed;
    if changed {
        return Err(GmailError::runtime(
            "compose_changed",
            "the compose source changed while the earlier Gmail draft was being created".to_string(),
        )
        .with_hint("the known draft id was restored; run `bob gmail draft NOTE` again to update the newer text"));
    }
    let digest = canonical_digest(note, &readback.preview)?;
    Ok(prepared_draft(
        note.to_string(),
        draft_id,
        digest,
        parsed,
        account,
        readback,
    ))
}

fn readback(
    gog: &Gog,
    account: &str,
    expected_draft_id: &str,
    source: &source::ComposeSource,
    rendered: Option<&render::RenderedMessage>,
    expected_attachments: Option<&attachments::AttachmentStaging>,
    signature: bool,
    verify_local: bool,
) -> Result<Readback, GmailError> {
    let draft = gog.draft_get(account, expected_draft_id)?;
    if draft.id != expected_draft_id {
        return Err(GmailError::runtime(
            "draft_identity_mismatch",
            "Gmail returned a different stored draft id".to_string(),
        ));
    }
    let message = &draft.message;
    if let Some(target) = source.reply_target.as_ref() {
        if message.thread_id != target.thread_id {
            return Err(GmailError::runtime(
                "draft_thread_mismatch",
                "the stored reply draft belongs to a different Gmail thread"
                    .to_string(),
            ));
        }
        if let Some(rfc_id) = target.rfc_message_id.as_deref() {
            let in_reply_to: Vec<_> =
                message.header_values("In-Reply-To").collect();
            if !in_reply_to.iter().any(|header| header.contains(rfc_id)) {
                return Err(GmailError::runtime(
                    "draft_reply_mismatch",
                    "the stored reply draft no longer targets the selected message".to_string(),
                ));
            }
        }
    } else if message
        .header_values("In-Reply-To")
        .chain(message.header_values("References"))
        .any(|header| !header.trim().is_empty())
    {
        return Err(GmailError::runtime(
            "draft_reply_context",
            "a standalone draft unexpectedly contains reply headers"
                .to_string(),
        ));
    }
    let mut fetched = FetchedBodies::default();
    let first = body::collect_message_bodies(message, &fetched)
        .map_err(|_| bad_readback("the stored draft MIME tree is invalid"))?;
    let mut pending = BTreeSet::new();
    for need in first.needs_fetch {
        if let Some(id) = need.attachment_id {
            pending.insert(id);
        }
    }
    for id in pending {
        let bytes = gog.attachment_bytes(account, &message.id, &id)?;
        fetched.insert(&message.id, id, bytes);
    }
    let inventory = body::collect_message_bodies(message, &fetched)
        .map_err(|_| bad_readback("the stored draft MIME tree is invalid"))?;
    let plain = unique_body(&inventory.plain_candidates, CandidateKind::Plain)?;
    let html = unique_body(&inventory.html_candidates, CandidateKind::Html)?;
    if plain.is_empty() && !source.body().trim().is_empty() {
        return Err(bad_readback("the stored draft has no plain-text body"));
    }
    if html.is_empty() && !source.body().trim().is_empty() {
        return Err(bad_readback("the stored draft has no HTML body"));
    }
    let html_sha256 = sha256(html.as_bytes());
    let remote_attachments =
        read_attachment_parts(gog, account, message, expected_attachments)?;
    if let (Some(rendered), Some(expected)) = (rendered, expected_attachments)
        && verify_local
    {
        verify_local_content(
            rendered,
            expected,
            signature,
            plain,
            html,
            &remote_attachments,
        )?;
    }
    let from = message
        .first_header(HeaderKind::From)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| bad_readback("the stored draft has no From header"))?
        .to_string();
    let to = recipient_headers(message, "To")?;
    let cc = recipient_headers(message, "Cc")?;
    let bcc = recipient_headers(message, "Bcc")?;
    let subject = message
        .first_header(HeaderKind::Subject)
        .unwrap_or_default()
        .to_string();
    let reply_target =
        source.reply_target.as_ref().map(|target| ReplyPreview {
            thread_id: target.thread_id.clone(),
            message_id: target.message_id.clone(),
            rfc_message_id: target.rfc_message_id.clone(),
            mode: if matches!(
                source.kind,
                source::ComposeKind::Reply { all: true }
            ) {
                "reply_all"
            } else {
                "reply"
            },
        });
    let attachment_previews = remote_attachments
        .iter()
        .zip(
            expected_attachments
                .map(|staging| staging.files())
                .unwrap_or(&[]),
        )
        .map(|(remote, expected)| AttachmentPreview {
            vault_path: path_to_slashes(&expected.vault_path),
            filename: remote.filename.clone(),
            byte_length: remote.byte_length,
            sha256: remote.sha256.clone(),
        })
        .collect();
    let flattened_links = rendered
        .map(|message| {
            message
                .flattened_links
                .iter()
                .map(|link| FlattenedLinkPreview {
                    original_target: link.original.clone(),
                    text: link.text.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let preview = Preview {
        account: account.to_string(),
        from,
        to,
        cc,
        bcc,
        subject,
        reply_target,
        text_body: plain.to_string(),
        html_sha256,
        signature: if signature { "gmail" } else { "none" },
        attachments: attachment_previews,
        flattened_links,
    };
    let in_reply_to: Vec<String> = message
        .header_values("In-Reply-To")
        .map(str::to_string)
        .collect();
    let remote_fingerprint = remote_fingerprint(
        &preview,
        &message.thread_id,
        &in_reply_to,
        &remote_attachments,
    )?;
    Ok(Readback {
        draft_id: draft.id,
        message_id: message.id.clone(),
        rfc_message_id: message
            .first_header(HeaderKind::MessageId)
            .filter(|value| {
                value.starts_with('<')
                    && value.ends_with('>')
                    && value.contains('@')
            })
            .map(str::to_string),
        thread_id: message.thread_id.clone(),
        preview,
        remote_fingerprint,
    })
}

/// Re-read every digest-bound part of the stored draft immediately before
/// dispatch. The send engine uses this after the durable `sending` intent and
/// refuses a remote edit without replacing it.
pub(crate) fn verify_remote_snapshot(
    gog: &Gog,
    account: &str,
    draft_id: &str,
    source: &source::ComposeSource,
    expected_fingerprint: &str,
) -> Result<(), GmailError> {
    let current =
        readback(gog, account, draft_id, source, None, None, false, false)?;
    if current.remote_fingerprint != expected_fingerprint {
        return Err(GmailError::runtime(
            "draft_remote_conflict",
            "the stored Gmail draft changed outside Bob".to_string(),
        )
        .with_hint(
            "review the Gmail draft, then run `bob gmail draft NOTE` again",
        ));
    }
    Ok(())
}

fn verify_request_envelope(
    preview: &Preview,
    request: &DraftRequest,
    is_reply: bool,
) -> Result<(), GmailError> {
    if preview.subject != request.subject {
        return Err(bad_readback(
            "the stored draft subject differs from the compose note",
        ));
    }
    if request.reply_all {
        if preview.to.is_empty() && preview.cc.is_empty() {
            return Err(bad_readback(
                "Gmail did not resolve any reply-all recipients",
            ));
        }
        return Ok(());
    }
    if preview.to != request.to
        || preview.cc != request.cc
        || preview.bcc != request.bcc
    {
        return Err(bad_readback(
            "the stored draft recipients differ from the compose note",
        ));
    }
    if !is_reply && preview.reply_target.is_some() {
        return Err(bad_readback(
            "a standalone compose note became a reply draft",
        ));
    }
    Ok(())
}

fn unique_body<'a>(
    candidates: &'a [body::BodyCandidate],
    kind: CandidateKind,
) -> Result<&'a str, GmailError> {
    let matches = candidates
        .iter()
        .filter(|candidate| candidate.kind == kind)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [candidate] => Ok(&candidate.text),
        [] => Err(bad_readback(
            "the stored draft is missing a required MIME body",
        )),
        _ => Err(bad_readback(
            "the stored draft has ambiguous MIME alternatives",
        )),
    }
}

fn recipient_headers(
    message: &GmailMessage,
    name: &str,
) -> Result<Vec<String>, GmailError> {
    let raw = message.header_values(name).collect::<Vec<_>>().join(", ");
    source::parse_recipient_list(&raw).map_err(|_| {
        bad_readback("the stored draft has an invalid recipient header")
    })
}

fn read_attachment_parts(
    gog: &Gog,
    account: &str,
    message: &GmailMessage,
    expected: Option<&attachments::AttachmentStaging>,
) -> Result<Vec<RemoteAttachment>, GmailError> {
    let Some(root) = message.payload.as_ref() else {
        if expected.is_some_and(|staging| !staging.files().is_empty()) {
            return Err(bad_readback(
                "the stored draft has no attachment MIME tree",
            ));
        }
        return Ok(Vec::new());
    };
    let mut parts = Vec::new();
    walk_attachments(root, 0, &mut 0, &mut parts)?;
    let mut output = Vec::with_capacity(parts.len());
    for part in parts {
        let filename = part
            .filename
            .as_deref()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                bad_readback("a transmitted attachment has no filename")
            })?
            .to_string();
        let body = part.body.as_ref().ok_or_else(|| {
            bad_readback("a transmitted attachment has no MIME body")
        })?;
        let bytes = if let Some(data) = body.data.as_deref() {
            decode_base64(data).ok_or_else(|| {
                bad_readback("a stored attachment has invalid encoded data")
            })?
        } else if let Some(id) = body.attachment_id.as_deref() {
            gog.attachment_bytes(account, &message.id, id)?
        } else {
            return Err(bad_readback(
                "a stored attachment is not fully available",
            ));
        };
        if body.size.is_some_and(|size| size != bytes.len() as u64) {
            return Err(bad_readback(
                "a stored attachment has a size mismatch",
            ));
        }
        output.push(RemoteAttachment {
            filename,
            byte_length: bytes.len() as u64,
            sha256: sha256(&bytes),
        });
    }
    if let Some(expected) = expected
        && output.len() != expected.files().len()
    {
        return Err(bad_readback(
            "the stored draft attachment count does not match the compose note",
        ));
    }
    Ok(output)
}

fn walk_attachments<'a>(
    part: &'a MimePart,
    depth: usize,
    nodes: &mut usize,
    output: &mut Vec<&'a MimePart>,
) -> Result<(), GmailError> {
    *nodes = nodes.saturating_add(1);
    if depth > 64 || *nodes > 4096 {
        return Err(bad_readback(
            "the stored draft MIME tree exceeds its limits",
        ));
    }
    let disposition = part
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("Content-Disposition"))
        .map(|header| header.value.to_ascii_lowercase())
        .unwrap_or_default();
    if part.filename.is_some() || disposition.starts_with("attachment") {
        output.push(part);
    } else {
        for child in &part.parts {
            walk_attachments(child, depth + 1, nodes, output)?;
        }
    }
    Ok(())
}

fn verify_local_content(
    rendered: &render::RenderedMessage,
    expected: &attachments::AttachmentStaging,
    signature: bool,
    plain: &str,
    html: &str,
    remote_attachments: &[RemoteAttachment],
) -> Result<(), GmailError> {
    let plain_actual = normalize_newlines(plain);
    let plain_expected = normalize_newlines(&rendered.plain_body);
    let html_actual = normalize_newlines(html);
    let html_expected = normalize_newlines(&rendered.html_body);
    if !signature {
        if plain_actual != plain_expected || html_actual != html_expected {
            return Err(bad_readback("the stored draft body differs from the rendered compose source"));
        }
    } else if !plain_actual.starts_with(&plain_expected)
        || !html_actual.starts_with(&html_expected)
    {
        return Err(bad_readback("the stored draft body does not preserve the rendered compose source and signature"));
    }
    for (remote, local) in remote_attachments.iter().zip(expected.files()) {
        if remote.filename != local.filename
            || remote.byte_length != local.len
            || remote.sha256 != local.sha256
        {
            return Err(bad_readback(
                "a stored attachment differs from its private snapshot",
            ));
        }
    }
    Ok(())
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn remote_fingerprint(
    preview: &Preview,
    thread_id: &str,
    in_reply_to: &[String],
    attachments: &[RemoteAttachment],
) -> Result<String, GmailError> {
    let bytes = serde_json::to_vec(&RemoteFingerprint {
        version: 1,
        account: &preview.account,
        from: &preview.from,
        to: &preview.to,
        cc: &preview.cc,
        bcc: &preview.bcc,
        subject: &preview.subject,
        thread_id,
        in_reply_to,
        text_body: &preview.text_body,
        html_sha256: &preview.html_sha256,
        attachments,
    })
    .map_err(|_| {
        GmailError::runtime(
            "digest",
            "could not encode draft fingerprint".to_string(),
        )
    })?;
    Ok(sha256(&bytes))
}

fn canonical_digest(
    note: &str,
    preview: &Preview,
) -> Result<String, GmailError> {
    let canonical = serde_json::to_vec(&CanonicalDigest {
        canonical_version: 1,
        note,
        preview,
    })
    .map_err(|_| {
        GmailError::runtime(
            "digest",
            "could not encode canonical draft preview".to_string(),
        )
    })?;
    Ok(sha256(&canonical))
}

pub(crate) fn report_success(
    note: &str,
    draft_id: &str,
    digest: &str,
    preview: &Preview,
    format: HumanFormat,
) {
    if format.is_json() {
        let document = PreviewDocument {
            schema_version: 1,
            ok: true,
            status: "preview",
            note,
            draft_id,
            digest,
            preview,
        };
        match serde_json::to_string(&document) {
            Ok(json) => println!("{json}"),
            Err(_) => eprintln!("bob gmail draft: could not encode preview"),
        }
        return;
    }
    println!("Draft {} · {}", draft_id, digest);
    println!("From: {}", preview.from);
    println!("To: {}", preview.to.join(", "));
    if !preview.cc.is_empty() {
        println!("Cc: {}", preview.cc.join(", "));
    }
    if !preview.bcc.is_empty() {
        println!("Bcc: {}", preview.bcc.join(", "));
    }
    println!("Subject: {}", preview.subject);
    println!("\n{}", preview.text_body);
}

fn operation_lock(relative: &Path) -> Result<File, GmailError> {
    let directory = bob_env::bob_cli_state_dir().join("gmail/compose-locks");
    let id = sha256(path_to_slashes(relative).as_bytes());
    let file =
        bob_env::create_state_lock_file(&directory, &format!("{id}.lock"))
            .map_err(|error| {
                GmailError::runtime(
                    "lock",
                    format!("could not open compose-operation lock: {error}"),
                )
            })?;
    let start = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= Duration::from_secs(30) {
                    return Err(GmailError::runtime(
                        "lock",
                        "another compose operation held the lock for 30 s"
                            .to_string(),
                    ));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "lock",
                    format!("could not lock compose operation: {error}"),
                ))
            }
        }
    }
}

fn operation_id(note: &str, fingerprint: &str, account: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"gmail-draft-create-v1\0");
    for value in [note, fingerprint, account] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    hex::encode(hash.finalize())
}

fn append_event(
    vault: &Path,
    kind: &str,
    value: serde_json::Value,
) -> Result<(), GmailError> {
    let host = vault_sync::hostname_slug();
    let mut line = serde_json::to_string(&serde_json::json!({
        "v": 1,
        "ts": pref_journal::event_timestamp().to_rfc3339_opts(SecondsFormat::Secs, false),
        "host": host,
        "kind": kind,
        "value": value,
    }))
    .unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    pref_journal::append_line(vault, DRAFT_JOURNAL, &host, &line).map_err(
        |error| {
            GmailError::runtime(
                "journal",
                format!(
                    "could not record the body-free Gmail draft event: {error}"
                ),
            )
        },
    )
}

fn append_recovery_event(
    vault: &Path,
    note: &str,
    account: &str,
    draft_id: &str,
    source_fingerprint: &str,
    operation_id: &str,
) -> Result<(), GmailError> {
    append_event(
        vault,
        "gmail.draft.recovery",
        serde_json::json!({
            "account": account,
            "note": note,
            "draft_id": draft_id,
            "source_fingerprint": source_fingerprint,
            "operation_id": operation_id,
        }),
    )
}

fn known_recovery_draft(
    vault: &Path,
    note: &str,
    operation_id: &str,
) -> Option<String> {
    let (events, _) = pref_journal::read_raw(vault, DRAFT_JOURNAL).ok()?;
    events.into_iter().rev().find_map(|event| {
        if event.kind != "gmail.draft.recovery" {
            return None;
        }
        let value = event.value.get("value")?;
        (value.get("note")?.as_str()? == note
            && value.get("operation_id")?.as_str()? == operation_id)
            .then(|| value.get("draft_id")?.as_str().map(str::to_string))?
    })
}

fn persist_recovery_id(
    vault: &Path,
    relative: &Path,
    account: &str,
    draft_id: &str,
    operation: &str,
) -> Result<(), GmailError> {
    let _maintenance = super::maintenance_lock()?;
    let path = vault.join(relative);
    let original = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read compose note during recovery: {error}"),
        )
    })?;
    let updated = source::persist_draft_metadata(
        &original,
        account,
        Some(draft_id),
        Some(operation),
        None,
    )
    .map_err(|error| GmailError::runtime("note", error.to_string()))?;
    super::write_and_commit(
        vault,
        vec![StagedTextFile {
            target: path,
            target_existed: true,
            original_target: original,
            updated_target: updated,
        }],
        "gmail: draft recovery",
    )
}

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{
        STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD,
    };
    let cleaned = value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    STANDARD
        .decode(&cleaned)
        .or_else(|_| URL_SAFE.decode(&cleaned))
        .or_else(|_| STANDARD_NO_PAD.decode(&cleaned))
        .or_else(|_| URL_SAFE_NO_PAD.decode(&cleaned))
        .ok()
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), GmailError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        GmailError::runtime(
            "staging",
            format!("could not create private draft body file: {error}"),
        )
    })?;
    file.write_all(bytes).map_err(|error| {
        GmailError::runtime(
            "staging",
            format!("could not write private draft body file: {error}"),
        )
    })?;
    file.flush().map_err(|error| {
        GmailError::runtime(
            "staging",
            format!("could not flush private draft body file: {error}"),
        )
    })
}

fn restrict_dir(path: &Path) -> Result<(), GmailError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                GmailError::runtime(
                    "staging",
                    format!(
                        "could not restrict private draft staging: {error}"
                    ),
                )
            },
        )?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn path_to_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn bad_readback(message: &str) -> GmailError {
    GmailError::runtime("draft_readback", message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Preview {
        Preview {
            account: "you@example.com".to_string(),
            from: "You <you@example.com>".to_string(),
            to: vec!["Ada <ada@example.com>".to_string()],
            cc: vec!["lee@example.test".to_string()],
            bcc: Vec::new(),
            subject: "Lease renewal".to_string(),
            reply_target: None,
            text_body: "Hello Ada.\n\n-- \nTest signature".to_string(),
            html_sha256: "a".repeat(64),
            signature: "gmail",
            attachments: vec![AttachmentPreview {
                vault_path: "files/lease.pdf".to_string(),
                filename: "lease.pdf".to_string(),
                byte_length: 5,
                sha256: "b".repeat(64),
            }],
            flattened_links: vec![FlattenedLinkPreview {
                original_target: "Lease|the lease".to_string(),
                text: "the lease".to_string(),
            }],
        }
    }

    #[test]
    fn canonical_digest_is_stable_and_binds_every_preview_field() {
        let note = "mail/2026/261106_lease_renewal.md";
        let baseline = canonical_digest(note, &preview()).unwrap();
        assert_eq!(baseline.len(), 64);
        assert_eq!(
            canonical_digest(note, &preview()).unwrap(),
            baseline,
            "unchanged content must confirm across invocations"
        );

        let mutations: Vec<(&str, fn(&mut Preview))> = vec![
            ("account", |p| p.account = "other@example.com".to_string()),
            ("from", |p| p.from = "Alias <you@example.com>".to_string()),
            ("to", |p| p.to.push("bo@example.com".to_string())),
            ("to order", |p| {
                p.to.insert(0, "bo@example.com".to_string());
            }),
            ("cc", |p| p.cc.clear()),
            ("bcc", |p| p.bcc.push("hidden@example.com".to_string())),
            ("subject", |p| p.subject = "Lease renewal!".to_string()),
            ("reply_target", |p| {
                p.reply_target = Some(ReplyPreview {
                    thread_id: "0123456789abcdef".to_string(),
                    message_id: "0123456789abcdef".to_string(),
                    rfc_message_id: Some("<lease@example.test>".to_string()),
                    mode: "reply",
                });
            }),
            ("text_body", |p| p.text_body.push('!')),
            ("html-only", |p| p.html_sha256 = "c".repeat(64)),
            ("signature", |p| p.signature = "none"),
            ("attachment bytes", |p| {
                p.attachments[0].sha256 = "d".repeat(64);
            }),
            ("attachment name", |p| {
                p.attachments[0].filename = "lease-v2.pdf".to_string();
            }),
            ("attachment removed", |p| p.attachments.clear()),
            ("flattened links", |p| {
                p.flattened_links[0].text = "Lease".to_string();
            }),
        ];
        for (field, mutate) in mutations {
            let mut changed = preview();
            mutate(&mut changed);
            assert_ne!(
                canonical_digest(note, &changed).unwrap(),
                baseline,
                "changing {field} must invalidate the confirmation digest"
            );
        }
        assert_ne!(
            canonical_digest("mail/2026/other.md", &preview()).unwrap(),
            baseline,
            "the note identity is digest-bound"
        );
    }
}

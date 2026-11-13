//! Explicit, receipt-scoped Gmail restore for `bob gmail unarchive`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde_json::{json, Value};

use super::{
    cli::{HumanFormat, UnarchiveArgs},
    config::GmailConfig,
    gog::{GmailLabel, Gog, CLIENT},
    note::index::MailIndex,
    resolve::{self, GmailRead},
    sync::{
        archive_state::{
            ArchiveHistoryState, ArchiveJournal, ArchiveReceipt,
            ArchiveTerminalKind,
        },
        archive_transport::{
            resolve_recorded_receipt_label, LabelDiff, LabelOperation,
            MessageLabelState, ReceiptLabel,
        },
        guard::{self, GuardAttempt},
        retry::{CoordinatedRead, SharedBudget, SystemClock, SystemSleeper},
        ThreadKey,
    },
    ui, GmailError,
};
use crate::native::vault_sync;

#[derive(Clone)]
struct Target {
    receipt: ArchiveReceipt,
    note: String,
}

#[derive(Clone)]
struct Row {
    input: String,
    thread_id: String,
    note: String,
    action: &'static str,
    reason: Option<&'static str>,
    message_ids: Vec<String>,
    label: Option<(String, Option<String>)>,
    error: Option<RowError>,
}

#[derive(Clone)]
struct RowError {
    kind: String,
    message: String,
    hint: Option<String>,
    setup: bool,
}

impl Row {
    fn no_receipt(
        input: &str,
        thread_id: Option<&str>,
        note: Option<&str>,
    ) -> Self {
        Self {
            input: input.to_string(),
            thread_id: thread_id.unwrap_or_default().to_string(),
            note: note.unwrap_or_default().to_string(),
            action: "unchanged",
            reason: Some("no_owned_receipt"),
            message_ids: Vec::new(),
            label: None,
            error: None,
        }
    }

    fn failed(input: &str, target: Option<&Target>, error: RowError) -> Self {
        Self {
            input: input.to_string(),
            thread_id: target
                .map(|target| target.receipt.key.thread_id().to_string())
                .unwrap_or_default(),
            note: target.map_or_else(String::new, |target| target.note.clone()),
            action: "failed",
            reason: None,
            message_ids: target
                .map(|target| target.receipt.message_ids.clone())
                .unwrap_or_default(),
            label: target.map(|target| {
                (
                    target.receipt.label.name.clone(),
                    target.receipt.label.id.clone(),
                )
            }),
            error: Some(error),
        }
    }

    fn from_target(
        input: &str,
        target: &Target,
        action: &'static str,
        reason: Option<&'static str>,
    ) -> Self {
        let mut message_ids = target.receipt.message_ids.clone();
        message_ids.sort();
        message_ids.dedup();
        Self {
            input: input.to_string(),
            thread_id: target.receipt.key.thread_id().to_string(),
            note: target.note.clone(),
            action,
            reason,
            message_ids,
            label: Some((
                target.receipt.label.name.clone(),
                target.receipt.label.id.clone(),
            )),
            error: None,
        }
    }

    fn error_json(error: &RowError) -> Value {
        let mut value = json!({"kind": error.kind, "message": error.message});
        if let Some(hint) = &error.hint {
            value["hint"] = json!(hint);
        }
        value
    }

    fn json(&self) -> Value {
        let (label, label_id) = self
            .label
            .as_ref()
            .map(|(name, id)| {
                (json!(name), id.as_ref().map_or(Value::Null, |id| json!(id)))
            })
            .unwrap_or((Value::Null, Value::Null));
        let mut value = json!({
            "input": self.input,
            "thread_id": self.thread_id,
            "note": self.note,
            "action": self.action,
            "message_ids": self.message_ids,
            "label": {"name": label, "id": label_id},
        });
        if let Some(reason) = self.reason {
            value["reason"] = json!(reason);
        }
        if let Some(error) = &self.error {
            value["error"] = Self::error_json(error);
        }
        value
    }
}

pub(crate) fn run(args: &UnarchiveArgs) -> i32 {
    match run_inner(args) {
        Ok((rows, setup_failure)) => {
            let ok = rows.iter().all(|row| row.action != "failed");
            print_report(args.format, &rows, ok);
            if ok {
                0
            } else if setup_failure
                && rows.iter().filter(|row| row.action == "failed").all(|row| {
                    row.error.as_ref().is_some_and(|error| error.setup)
                })
            {
                2
            } else {
                1
            }
        }
        Err(error) => print_top_error(args.format, error),
    }
}

fn run_inner(args: &UnarchiveArgs) -> Result<(Vec<Row>, bool), GmailError> {
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    if !args.dry_run {
        check_writer(&config)?;
    }
    let gog = Gog::resolve(
        crate::native::google::GMAIL,
        config.gog(),
        config.timeout(),
    )?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let journal = ArchiveJournal::load(&vault).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not read the Gmail archive journal: {error}"),
        )
    })?;
    let folder = Path::new(config.folder());
    let index = resolve::scan_index(&vault, folder, config.link_label())?;

    let unsafe_scope = !journal.scope_is_safe(Some(&account));
    if unsafe_scope {
        let rows = args.inputs.iter().map(|input| {
            Row::failed(input, None, RowError {
                kind: "unsafe_journal".to_string(),
                message: "the Gmail archive journal is unsafe for this account; no receipts were changed".to_string(),
                hint: None,
                setup: false,
            })
        }).collect();
        return Ok((rows, false));
    }

    let plans = resolve_inputs(&args.inputs, &account, &journal, &index);
    let needs_remote = plans
        .iter()
        .any(|plan| matches!(plan, InputPlan::Targets(_, _)));

    let _guard = if args.dry_run || !needs_remote {
        None
    } else {
        match guard::try_acquire(&account).map_err(|error| {
            GmailError::runtime("lock", format!("could not acquire the Gmail sync guard: {error}"))
        })? {
            GuardAttempt::Acquired(guard) => Some(guard),
            GuardAttempt::Busy => return Err(GmailError::runtime(
                "busy",
                "another Gmail sync or restore is already running for this account".to_string(),
            )),
        }
    };

    if !args.dry_run && needs_remote && !has_modify_grant(&gog, &account)? {
        return Err(GmailError::setup(
            "modify_grant",
            "the bob-gmail sign-in lacks Gmail modify access".to_string(),
        )
        .with_hint(
            "run `bob gmail login -s modify` on the approved Gmail writer",
        ));
    }

    let budget =
        SharedBudget::new(Duration::from_secs(config.timeout().as_secs()));
    let sleeper = SystemSleeper;
    let clock = SystemClock;
    let last_request = std::cell::RefCell::new(None);
    let reader =
        CoordinatedRead::new(&gog, &sleeper, &budget, &clock, &last_request);
    let labels = if needs_remote {
        reader.labels_list(&account)?
    } else {
        Vec::new()
    };
    let enrollment = current_enrollment_label(config.sync_label(), &labels)?;
    let mut rows = Vec::new();
    let mut setup_failure = false;
    let mut auth_failed = false;
    let mut seen = BTreeSet::new();
    let mut seen_inputs = BTreeSet::new();

    for (input_index, (input, plan)) in
        args.inputs.iter().zip(plans.iter()).enumerate()
    {
        if !seen_inputs.insert(plan_dedup_key(plan, input, &account)) {
            continue;
        }
        match plan {
            InputPlan::NoReceipt {
                thread_id,
                note,
                repair,
            } => {
                if !args.dry_run
                    && let Some((key, note_path)) = repair
                    && let Err(error) = project_note(&vault, &note_path, &key)
                {
                    rows.push(Row::failed(input, None, row_error(error)));
                } else {
                    rows.push(Row::no_receipt(
                        input,
                        thread_id.as_deref(),
                        note.as_deref(),
                    ));
                }
            }
            InputPlan::Failure(error) => {
                setup_failure |= error.setup;
                rows.push(Row::failed(input, None, error.clone()));
            }
            InputPlan::Targets(targets, identity_note) => {
                if targets.is_empty() {
                    rows.push(Row::no_receipt(
                        input,
                        None,
                        identity_note.as_deref(),
                    ));
                    continue;
                }
                for target in targets {
                    if !seen.insert(target.receipt.archive_event_id.clone()) {
                        continue;
                    }
                    match restore_one(
                        input,
                        &target,
                        &account,
                        &vault,
                        &reader,
                        &gog,
                        &labels,
                        enrollment.as_ref(),
                        args.dry_run,
                    ) {
                        Ok(row) => rows.push(row),
                        Err(error) => {
                            let auth =
                                super::sync::retry::is_auth_abort(&error);
                            auth_failed |= auth;
                            setup_failure |= error.exit_code() == 2;
                            rows.push(Row::failed(
                                input,
                                Some(&target),
                                row_error(error),
                            ));
                            if auth_failed {
                                break;
                            }
                        }
                    }
                }
                if auth_failed {
                    break;
                }
            }
        }
        if auth_failed {
            for (remaining, remaining_plan) in
                args.inputs.iter().zip(plans.iter()).skip(input_index + 1)
            {
                if seen_inputs.insert(plan_dedup_key(
                    remaining_plan,
                    remaining,
                    &account,
                )) {
                    rows.push(Row::failed(remaining, None, RowError {
                        kind: "aborted".to_string(),
                        message: "remaining restores were stopped after Gmail authorization failed".to_string(),
                        hint: Some("restore the Gmail grant, then retry the listed inputs".to_string()),
                        setup: true,
                    }));
                }
            }
            setup_failure = true;
            break;
        }
    }
    Ok((rows, setup_failure))
}

#[derive(Clone)]
enum InputPlan {
    NoReceipt {
        thread_id: Option<String>,
        note: Option<String>,
        repair: Option<(ThreadKey, String)>,
    },
    Targets(Vec<Target>, Option<String>),
    Failure(RowError),
}

fn plan_dedup_key(plan: &InputPlan, input: &str, account: &str) -> String {
    match plan {
        InputPlan::Targets(targets, _) => targets.first().map_or_else(
            || format!("input:{}", input.trim().to_ascii_lowercase()),
            |target| {
                format!(
                    "thread:{}:{}",
                    target.receipt.key.account(),
                    target.receipt.key.thread_id()
                )
            },
        ),
        InputPlan::NoReceipt {
            thread_id: Some(thread_id),
            ..
        } => format!("thread:{account}:{}", thread_id.to_ascii_lowercase()),
        InputPlan::NoReceipt {
            note: Some(note), ..
        } => format!("note:{}", note.to_ascii_lowercase()),
        InputPlan::NoReceipt { .. } | InputPlan::Failure(_) => {
            format!("input:{}", input.trim().to_ascii_lowercase())
        }
    }
}

fn resolve_inputs(
    inputs: &[String],
    account: &str,
    journal: &ArchiveJournal,
    index: &MailIndex,
) -> Vec<InputPlan> {
    let mut known = BTreeMap::<(String, String), String>::new();
    for record in journal.archive_history() {
        known.insert(
            (
                record.receipt.key.account().to_string(),
                record.receipt.key.thread_id().to_string(),
            ),
            record.note,
        );
    }
    for entry in index.by_identity.values() {
        known.insert(
            (
                entry.account.to_ascii_lowercase(),
                entry.thread_id.to_ascii_lowercase(),
            ),
            entry.path.clone(),
        );
    }
    for (other_account, thread_id, paths) in &index.duplicates {
        if let Some(path) = paths.first() {
            known.insert(
                (
                    other_account.to_ascii_lowercase(),
                    thread_id.to_ascii_lowercase(),
                ),
                path.clone(),
            );
        }
    }

    let active: Vec<Target> = journal
        .owned_receipts()
        .filter_map(|receipt| {
            let note = journal
                .close_for(&receipt.key, &receipt.generation)?
                .note
                .clone();
            Some(Target {
                receipt: receipt.clone(),
                note,
            })
        })
        .collect();

    inputs.iter().map(|input| {
        let input = input.trim();
        let thread_like = input.bytes().all(|byte| byte.is_ascii_hexdigit()) && !input.is_empty();
        let path_like = input.contains('/') || input.to_ascii_lowercase().ends_with(".md");
        let note_matches: Vec<_> = known.iter().filter(|((_, _), path)| note_identity_matches(input, path, path_like)).collect();
        let thread_matches: Vec<_> = if thread_like {
            known.iter().filter(|((_, id), _)| id.eq_ignore_ascii_case(input)).collect()
        } else { Vec::new() };
        let mut identities = BTreeMap::<(String, String), String>::new();
        for (key, path) in note_matches.into_iter().chain(thread_matches.into_iter()) {
            identities.insert(key.clone(), path.clone());
        }
        if identities.len() > 1 {
            let accounts: BTreeSet<_> = identities.keys().map(|(a, _)| a.as_str()).collect();
            let cross = accounts.len() > 1 || accounts.iter().any(|candidate| *candidate != account);
            return InputPlan::Failure(RowError {
                kind: if cross { "cross_account" } else { "ambiguous_identity" }.to_string(),
                message: if cross { "the selected note identity belongs to more than one Gmail account".to_string() } else { "the selected note identity matches more than one saved thread".to_string() },
                hint: Some("use the exact saved mail-note path or a thread ID for the configured account".to_string()),
                setup: true,
            });
        }
        if let Some(((matched_account, thread_id), path)) = identities.into_iter().next() {
            if matched_account != account {
                return InputPlan::Failure(RowError {
                    kind: "cross_account".to_string(),
                    message: "the selected mail note belongs to a different Gmail account".to_string(),
                    hint: Some("select a saved mail note for the configured Gmail account".to_string()),
                    setup: true,
                });
            }
            let targets: Vec<_> = active.iter().filter(|target| {
                target.receipt.key.account() == matched_account && target.receipt.key.thread_id() == thread_id
            }).cloned().collect();
            if targets.is_empty() {
                let repair = ThreadKey::new(&matched_account, &thread_id)
                    .map(|key| (key, path.clone()));
                return InputPlan::NoReceipt { thread_id: Some(thread_id), note: Some(path), repair };
            }
            return InputPlan::Targets(targets, Some(path));
        }
        if thread_like {
            let current: Vec<_> = active.iter().filter(|target| target.receipt.key.is_account(account) && target.receipt.key.thread_id().eq_ignore_ascii_case(input)).cloned().collect();
            if !current.is_empty() {
                let path = current.first().map(|target| target.note.clone());
                return InputPlan::Targets(current, path);
            }
            if active.iter().any(|target| target.receipt.key.thread_id().eq_ignore_ascii_case(input) && !target.receipt.key.is_account(account)) {
                return InputPlan::Failure(RowError {
                    kind: "cross_account".to_string(),
                    message: "the matching archive receipt belongs to a different Gmail account".to_string(),
                    hint: None,
                    setup: true,
                });
            }
            return InputPlan::NoReceipt { thread_id: Some(input.to_ascii_lowercase()), note: None, repair: None };
        }
        InputPlan::NoReceipt { thread_id: None, note: Some(input.to_string()), repair: None }
    }).collect()
}

fn note_identity_matches(input: &str, path: &str, path_like: bool) -> bool {
    let normalize = |value: &str| {
        value
            .replace('\\', "/")
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    let input = normalize(input);
    let path = normalize(path);
    fn stem(value: &str) -> &str {
        value.strip_suffix(".md").unwrap_or(value)
    }
    if path_like {
        stem(&input) == stem(&path)
    } else {
        Path::new(&path)
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == input)
    }
}

#[allow(clippy::too_many_arguments)]
fn restore_one<R: GmailRead>(
    input: &str,
    target: &Target,
    account: &str,
    vault: &Path,
    reader: &R,
    gog: &Gog,
    labels: &[GmailLabel],
    enrollment: Option<&GmailLabel>,
    dry_run: bool,
) -> Result<Row, GmailError> {
    let receipt = &target.receipt;
    let mut states = Vec::with_capacity(receipt.message_ids.len());
    for id in &receipt.message_ids {
        let value = reader.message_metadata(account, id)?;
        states.push(parse_message_state(&value, id, receipt.key.thread_id())?);
    }
    if states.iter().any(|state| {
        state.label_ids.iter().any(|label| {
            matches!(
                label.to_ascii_uppercase().as_str(),
                "SPAM" | "TRASH" | "DRAFT"
            )
        })
    }) {
        return Err(GmailError::runtime("policy_excluded", "a recorded Gmail message is in Spam, Trash, or Drafts; no message was changed".to_string()));
    }

    let live_label =
        resolve_recorded_receipt_label(labels, &receipt.label, enrollment)?;
    let label_id = live_label
        .as_ref()
        .map(ReceiptLabel::id)
        .or(receipt.label.id.as_deref());
    let all_inbox_without_receipt = states.iter().all(|state| {
        state.label_ids.contains("INBOX")
            && label_id.is_none_or(|label| !state.label_ids.contains(label))
    });
    if all_inbox_without_receipt {
        if dry_run {
            return Ok(Row::from_target(
                input,
                target,
                "unchanged",
                Some("already_in_inbox"),
            ));
        }
        return settle_release(
            input,
            target,
            account,
            vault,
            receipt.message_ids.clone(),
            "already_in_inbox",
        );
    }
    let Some(live_label) = live_label else {
        return Err(GmailError::runtime(
            "receipt_label_missing",
            "the recorded receipt label is missing and the messages are not already restored; no message was changed".to_string(),
        ));
    };
    if states.iter().any(|state| {
        !state.label_ids.contains(live_label.id())
            && !state.label_ids.contains("INBOX")
    }) {
        return Err(GmailError::runtime(
            "stale_ownership",
            "a recorded message has neither its receipt label nor INBOX; current ownership is uncertain".to_string(),
        ));
    }
    let diff =
        LabelDiff::for_messages(LabelOperation::Restore, &live_label, &states)?;
    if dry_run {
        let action = if diff.is_empty() {
            "unchanged"
        } else {
            "would_restore"
        };
        let reason = if diff.is_empty() {
            Some("already_in_inbox")
        } else {
            None
        };
        return Ok(Row::from_target(input, target, action, reason));
    }
    confirm_owned(vault, receipt)?;
    if !diff.is_empty() {
        gog.archive_batch_modify(account, &diff)?;
    }
    settle_release(
        input,
        target,
        account,
        vault,
        receipt.message_ids.clone(),
        if diff.is_empty() {
            "already_in_inbox"
        } else {
            "restored"
        },
    )
}

fn parse_message_state(
    value: &Value,
    expected_id: &str,
    expected_thread: &str,
) -> Result<MessageLabelState, GmailError> {
    let bad = || {
        GmailError::runtime("remote_evidence", "Gmail returned missing or malformed metadata for a recorded message".to_string())
    };
    let id = value.get("id").and_then(Value::as_str).ok_or_else(bad)?;
    let thread_id = value
        .get("threadId")
        .and_then(Value::as_str)
        .ok_or_else(bad)?;
    let labels = value
        .get("labelIds")
        .and_then(Value::as_array)
        .ok_or_else(bad)?;
    if !id.eq_ignore_ascii_case(expected_id)
        || !thread_id.eq_ignore_ascii_case(expected_thread)
        || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !thread_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(bad());
    }
    let mut label_ids = BTreeSet::new();
    for label in labels {
        let label = label.as_str().ok_or_else(bad)?;
        if label.is_empty()
            || label.chars().any(char::is_control)
            || !label_ids.insert(label.to_string())
        {
            return Err(bad());
        }
    }
    Ok(MessageLabelState {
        message_id: id.to_string(),
        label_ids,
    })
}

fn confirm_owned(
    vault: &Path,
    receipt: &ArchiveReceipt,
) -> Result<(), GmailError> {
    let _lock = acquire_vault_lock()?;
    let journal = ArchiveJournal::load(vault).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not re-read the Gmail archive journal: {error}"),
        )
    })?;
    if !journal.scope_is_safe(Some(receipt.key.account()))
        || !journal.owned_receipts().any(|current| {
            current.archive_event_id == receipt.archive_event_id
                && current.generation == receipt.generation
        })
    {
        return Err(GmailError::runtime(
            "stale_ownership",
            "the archive receipt is no longer owned; no message was changed"
                .to_string(),
        ));
    }
    Ok(())
}

fn settle_release(
    input: &str,
    target: &Target,
    account: &str,
    vault: &Path,
    message_ids: Vec<String>,
    reason: &'static str,
) -> Result<Row, GmailError> {
    let receipt = &target.receipt;
    let _lock = acquire_vault_lock()?;
    let current = ArchiveJournal::load(vault).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not re-read the Gmail archive journal: {error}"),
        )
    })?;
    if !current.scope_is_safe(Some(account)) {
        return Err(GmailError::runtime("unsafe_journal", "the Gmail archive journal became unsafe before the restore could be recorded".to_string()));
    }
    let terminal = ArchiveTerminalKind::Restored {
        message_ids,
        label: receipt.label.clone(),
    };
    let event = current.event_for_terminal(&receipt.key, &receipt.generation, terminal)
        .ok_or_else(|| GmailError::runtime("stale_ownership", "the archive receipt is no longer owned; Gmail is already restored and the journal was left unchanged".to_string()))?;
    ArchiveJournal::append(vault, &vault_sync::hostname_slug(), &event).map_err(|error| GmailError::runtime("journal", format!("Gmail was restored but the receipt release could not be recorded: {error}")))?;
    drop(_lock);
    project_note(vault, &target.note, &receipt.key)?;
    Ok(Row::from_target(input, target, "restored", Some(reason)))
}

fn project_note(
    vault: &Path,
    note: &str,
    key: &ThreadKey,
) -> Result<(), GmailError> {
    let relative = safe_note_path(note)?;
    let path = vault.join(&relative);
    let _lock = acquire_vault_lock()?;
    let fresh = ArchiveJournal::load(vault).map_err(|error| GmailError::runtime("journal", format!("could not read the Gmail archive journal for note repair: {error}")))?;
    if !fresh.scope_is_safe(Some(key.account())) {
        return Err(GmailError::runtime("unsafe_journal", "the Gmail archive journal is unsafe; the mail note receipt was left unchanged".to_string()));
    }
    let newest = fresh
        .archive_history()
        .into_iter()
        .filter(|record| {
            record.receipt.key == *key
                && matches!(record.state, ArchiveHistoryState::Archived)
        })
        .max_by(|left, right| {
            left.receipt
                .archived_at
                .cmp(&right.receipt.archived_at)
                .then_with(|| {
                    left.receipt
                        .archive_event_id
                        .cmp(&right.receipt.archive_event_id)
                })
        });
    let archived_at = newest.map(|record| {
        record
            .receipt
            .archived_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    });
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!(
                "could not inspect the saved mail note after restore: {error}"
            ),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(GmailError::runtime("note", "the saved mail note is not a regular in-vault file; its receipt projection was left unchanged".to_string()));
    }
    let contents = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!(
                "could not read the saved mail note after restore: {error}"
            ),
        )
    })?;
    if let Some(updated) = super::note::receipt::project(&contents, archived_at.as_deref()).map_err(|error| GmailError::runtime("note_projection", format!("the Gmail receipt was released but the note projection needs repair: {error}")))? {
        fs::write(&path, updated).map_err(|error| GmailError::runtime("note_projection", format!("the Gmail receipt was released but the note projection could not be written: {error}")))?;
    }
    Ok(())
}

fn safe_note_path(note: &str) -> Result<PathBuf, GmailError> {
    let path = Path::new(note);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.extension().is_none_or(|extension| extension != "md")
    {
        return Err(GmailError::runtime(
            "note_path",
            "the recorded mail note path is unsafe".to_string(),
        ));
    }
    Ok(path.to_path_buf())
}

fn acquire_vault_lock() -> Result<fs::File, GmailError> {
    for _ in 0..50 {
        match crate::native::ob::try_acquire_lock() {
            Ok(file) => return Ok(file),
            Err(crate::native::ob::LockAcquireError::Contended) => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "lock",
                    format!(
                    "could not acquire the vault maintenance lock: {error:?}"
                ),
                ))
            }
        }
    }
    Err(GmailError::runtime(
        "lock",
        "the vault maintenance lock stayed busy; retry the restore".to_string(),
    ))
}

fn current_enrollment_label(
    name: Option<&str>,
    labels: &[GmailLabel],
) -> Result<Option<GmailLabel>, GmailError> {
    let Some(name) = name else { return Ok(None) };
    let mut matches = labels.iter().filter(|label| label.name == name);
    let Some(label) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(GmailError::setup(
            "sync_label",
            "the configured Gmail enrollment label name is ambiguous"
                .to_string(),
        ));
    }
    Ok(Some(label.clone()))
}

fn has_modify_grant(gog: &Gog, account: &str) -> Result<bool, GmailError> {
    let entries = gog.accounts(Some(CLIENT))?;
    Ok(entries
        .iter()
        .find(|entry| entry.email.eq_ignore_ascii_case(account))
        .is_some_and(|entry| {
            entry.scopes.iter().any(|scope| {
                scope == "https://www.googleapis.com/auth/gmail.modify"
                    || scope.contains("gmail.modify")
            })
        }))
}

fn check_writer(config: &GmailConfig) -> Result<(), GmailError> {
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

fn row_error(error: GmailError) -> RowError {
    RowError {
        kind: error.kind().to_string(),
        message: error.message().to_string(),
        hint: error.hint().map(str::to_string),
        setup: error.exit_code() == 2,
    }
}

fn print_report(format: HumanFormat, rows: &[Row], ok: bool) {
    if format.is_json() {
        println!(
            "{}",
            json!({"schema_version": 1, "ok": ok, "results": rows.iter().map(Row::json).collect::<Vec<_>>()})
        );
        return;
    }
    for row in rows {
        let label = row
            .label
            .as_ref()
            .map(|(name, _)| format!(" · {name}"))
            .unwrap_or_default();
        let message = match &row.error {
            Some(error) => format!("{}: {}", error.kind, error.message),
            None => row
                .reason
                .map_or_else(String::new, |reason| format!(" · {reason}")),
        };
        println!(
            "{} {}{}{}",
            action_mark(row.action),
            row.action,
            if row.note.is_empty() {
                &row.input
            } else {
                &row.note
            },
            format!("{label}{message}")
        );
    }
}

fn action_mark(action: &str) -> &'static str {
    match action {
        "restored" => "↩",
        "would_restore" => "↩",
        "unchanged" => "·",
        _ => "✗",
    }
}

fn print_top_error(format: HumanFormat, error: GmailError) -> i32 {
    if format.is_json() {
        let mut payload = json!({"schema_version": 1, "ok": false, "results": [], "error": {"kind": error.kind(), "message": error.message()}});
        if let Some(hint) = error.hint() {
            payload["error"]["hint"] = json!(hint);
        }
        println!("{payload}");
    } else {
        ui::report_error("unarchive", &error, "human");
    }
    error.exit_code()
}

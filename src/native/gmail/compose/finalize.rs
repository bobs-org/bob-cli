//! In-place sent-note refresh and idempotent completion reconciliation.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::Datelike;
use serde_yaml::Value;

use super::super::{
    config::GmailConfig,
    gog::Gog,
    note,
    policy::{self, NeverSavePolicy},
    prepare,
    resolve::{self, ResolvedIdentity},
    save::{self, PreparedSave},
    sync::ThreadKey,
    GmailError,
};
use super::{
    send_watch,
    source::{self, ComposeKind},
    state::{self, SendStage, SendState},
};
use crate::native::{
    attachments::journal as attachment_journal,
    capture::{self, StagedTextFile},
    env as bob_env, ob,
};

/// Refresh Gmail and finish the acknowledged send without changing the note
/// path. The vault lock covers only local planning and writes; all Gmail work
/// finishes first.
pub(crate) fn finalize(
    vault: &Path,
    relative: &Path,
    state: &SendState,
    config: &GmailConfig,
    gog: &Gog,
) -> Result<(), GmailError> {
    if state.stage == SendStage::Complete {
        return reconcile_complete(vault, relative, state);
    }
    if state.stage != SendStage::Acknowledged {
        return Err(GmailError::runtime(
            "send_state_invalid",
            "only an acknowledged send can be finalized".to_string(),
        ));
    }

    let sent_message_id =
        state.sent_message_id.as_deref().ok_or_else(|| {
            GmailError::runtime(
                "send_state_invalid",
                "the acknowledged send has no sent message identity"
                    .to_string(),
            )
        })?;
    let sent_thread_id = state.sent_thread_id.as_deref().ok_or_else(|| {
        GmailError::runtime(
            "send_state_invalid",
            "the acknowledged send has no sent thread identity".to_string(),
        )
    })?;
    if !sent_thread_id.eq_ignore_ascii_case(&state.thread_id) {
        return Err(GmailError::runtime(
            "sent_thread_mismatch",
            "Gmail acknowledged a different thread than the compose note"
                .to_string(),
        ));
    }

    let confirmed = resolve::fetch_confirmed_thread(
        gog,
        ResolvedIdentity {
            account: state.account.clone(),
            thread_id: state.thread_id.clone(),
            known_path: None,
        },
    )?;
    let has_sent_message = confirmed.thread.messages.iter().any(|message| {
        message.id.eq_ignore_ascii_case(sent_message_id)
            && message
                .label_ids
                .iter()
                .any(|label| label.eq_ignore_ascii_case("SENT"))
    });
    if !has_sent_message {
        return Err(GmailError::runtime(
            "sent_refresh_pending",
            "Gmail accepted the send, but the sent message is not yet visible in the thread".to_string(),
        ));
    }
    policy::check(
        gog,
        &state.account,
        &confirmed.thread,
        &NeverSavePolicy::from_config(config),
    )?;
    let mut prepared = prepare::prepare_thread(
        gog,
        &state.account,
        &confirmed.thread,
        config.body(),
    )?;

    let folder = Path::new(config.folder());
    save::validate_mail_folder(vault, folder)?;
    super::validate_target(vault, relative, true)?;

    // Sibling metadata for multi-thread aggregation finishes before the
    // lock. Local watch planning below rechecks membership and never
    // fetches Gmail again.
    let siblings = ThreadKey::new(&state.account, &state.thread_id)
        .map(|key| send_watch::fetch_siblings(vault, config, gog, &key))
        .unwrap_or_default();

    // TTY/network work is complete. The maintenance lock protects the exact
    // source snapshot, image destinations, staged note write, and commit.
    let _maintenance = super::maintenance_lock()?;
    let note_path = vault.join(relative);
    let original = fs::read_to_string(&note_path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not re-read the compose note: {error}"),
        )
    })?;
    let compose = source::parse(&original).map_err(|error| {
        GmailError::runtime("sent_needs_review", error.to_string())
    })?;
    if compose.fingerprint != state.source_digest {
        return Err(sent_needs_review());
    }

    let index = resolve::scan_index(vault, folder, config.link_label())?;
    let identity_key = (
        state.account.to_ascii_lowercase(),
        state.thread_id.to_ascii_lowercase(),
    );
    if index.duplicates.iter().any(|(account, thread, _)| {
        account.eq_ignore_ascii_case(&identity_key.0)
            && thread.eq_ignore_ascii_case(&identity_key.1)
    }) {
        return Err(thread_identity_conflict());
    }
    let indexed = index.by_identity.get(&identity_key);
    match &compose.kind {
        ComposeKind::New if indexed.is_some() => {
            return Err(thread_identity_conflict());
        }
        ComposeKind::Reply { .. }
            if indexed.is_none_or(|entry| {
                entry.path != save::path_to_slashes(relative)
            }) =>
        {
            return Err(thread_identity_conflict());
        }
        _ => {}
    }

    let parent = frontmatter_parent(&original);
    save::adjust_image_collisions(
        vault,
        &mut prepared,
        Some(&original),
        &BTreeMap::new(),
    )?;
    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    let saved_at = save::current_utc();
    let rendered = note::layout::render(&note::layout::LayoutInput {
        account: &confirmed.identity.account,
        body_mode: config.body(),
        current_year: saved_at.with_timezone(&timezone).year(),
        parent: &parent,
        rendered: &prepared.rendered,
        saved_at,
        thread: &confirmed.thread,
        timezone,
    })
    .map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not render the sent thread: {error}"),
        )
    })?;

    let work = PreparedSave {
        confirmed,
        prepared,
        body_mode: config.body(),
    };
    let mut works = BTreeMap::new();
    works.insert(identity_key.clone(), work);
    let plan = save::plan_prepared_saves(
        vault,
        folder,
        config.link_label(),
        &mut works,
        &index,
        None,
        false,
        saved_at,
        timezone,
    )?
    .into_iter()
    .next()
    .ok_or_else(|| {
        GmailError::runtime(
            "note",
            "the sent thread produced no save plan".to_string(),
        )
    })?;

    let updated = match &compose.kind {
        ComposeKind::New => promote_new(&original, &compose, &rendered)?,
        ComposeKind::Reply { .. } => source::remove_sent_reply(
            &plan.updated,
            &compose,
        )
        .map_err(|error| {
            if error == source::SourceError::ChangedSinceSend {
                sent_needs_review()
            } else {
                GmailError::runtime("sent_needs_review", error.to_string())
            }
        })?,
    };

    super::validate_target(vault, relative, true)?;
    super::ensure_parent_dirs(vault, relative)?;
    let child_env = ob::child_env();
    let is_worktree =
        ob::detect_git_worktree(vault, &child_env).map_err(|error| {
            GmailError::runtime(
                "git",
                format!("could not inspect the vault repository: {error}"),
            )
        })?;

    let work = works.get(&identity_key).ok_or_else(|| {
        GmailError::runtime(
            "note",
            "the sent thread preparation was lost".to_string(),
        )
    })?;
    let imports = &work.prepared.imports;
    let origins =
        save::newly_referenced_origins(&work.prepared.origins, Some(&original));
    let created_images = save::install_imports(vault, imports)?;

    let watch_plan = send_watch::plan_under_lock(
        vault,
        config,
        &state.account,
        &state.thread_id,
        &work.confirmed.thread,
        &siblings,
        relative,
    );

    let commit_paths = (|| -> Result<BTreeSet<PathBuf>, GmailError> {
        let mut paths = BTreeSet::<PathBuf>::new();
        paths.insert(relative.to_path_buf());
        for (image, _) in &created_images {
            paths.insert(image.clone());
        }
        for import in imports {
            let image = save::safe_relative_path(&import.relative_path)?;
            if is_worktree
                && fs::read(vault.join(&image)).ok().is_some_and(|bytes| {
                    save::sha256(&bytes) == save::sha256(&import.bytes)
                })
                && save::git_path_is_dirty(vault, &child_env, &image)
                    .map_err(|error| {
                        GmailError::runtime(
                            "git",
                            format!(
                                "could not inspect an imported image path: {error}"
                            ),
                        )
                    })?
            {
                paths.insert(image);
            }
        }
        for extra in &watch_plan.extra_paths {
            super::validate_target(vault, extra, true)?;
            paths.insert(extra.clone());
        }
        Ok(paths)
    })();
    let commit_paths = match commit_paths {
        Ok(paths) => paths,
        Err(error) => {
            save::cleanup_created_images(vault, &created_images);
            return Err(error);
        }
    };

    let mut completed = state.clone();
    completed.stage = SendStage::Complete;
    completed.completed_note_digest =
        Some(state::content_digest(&updated).map_err(|error| {
            save::cleanup_created_images(vault, &created_images);
            GmailError::runtime("send_state_invalid", error.to_string())
        })?);
    completed.completed_commit_paths = commit_paths
        .iter()
        .map(|path| save::path_to_slashes(path))
        .collect();
    completed.completed_path_receipts = watch_plan.receipts.clone();
    completed.watch_turns = watch_plan.turns.clone();
    let updated = match state::patch_send_state(&updated, Some(&completed)) {
        Ok(updated) => updated,
        Err(error) => {
            save::cleanup_created_images(vault, &created_images);
            return Err(GmailError::runtime("note", error.to_string()));
        }
    };

    let mut staged = vec![StagedTextFile {
        target: note_path.clone(),
        target_existed: true,
        original_target: original.clone(),
        updated_target: updated,
    }];
    staged.extend(watch_plan.staged);
    let write_result = capture::write_staged_files(&staged);
    if let Err(error) = write_result {
        save::cleanup_created_images(vault, &created_images);
        return Err(GmailError::runtime(
            "write",
            format!(
                "could not safely finalize the sent note: {}",
                error.message
            ),
        ));
    }

    if is_worktree {
        let paths: Vec<_> = commit_paths.into_iter().collect();
        ob::commit_paths(
            vault,
            &child_env,
            &format!("gmail: send {}", save::commit_subject(&rendered.metadata.title)),
            &paths,
        )
        .map_err(|error| {
            GmailError::runtime(
                "uncommitted",
                format!("the sent note was written but could not be committed: {error}"),
            )
            .with_hint("rerun `bob gmail send NOTE` to finish local finalization")
        })?;
    }

    // Markers follow durable note/task writes and the scoped commit. A
    // running deferral leaves this list empty so the next sync holds.
    send_watch::append_turns(vault, &completed.watch_turns)?;

    // Attachment-origin logging is ancillary to the durable mail note. The
    // sent event below remains the authoritative idempotent completion record.
    let _ = attachment_journal::append_origin_events(vault, &origins, "gmail");
    state::ensure_sent_event(vault, &completed)?;
    Ok(())
}

/// Reconcile a committed or partly committed completion after restart. The
/// digest excludes only `gmail_send`, so a later edit to the same note is
/// never swept into this operation's retry commit.
pub(crate) fn reconcile_complete(
    vault: &Path,
    relative: &Path,
    state: &SendState,
) -> Result<(), GmailError> {
    let _maintenance = super::maintenance_lock()?;
    let note_path = vault.join(relative);
    let contents = fs::read_to_string(&note_path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read the sent note: {error}"),
        )
    })?;
    let current = state::load_send_state(&contents).map_err(|error| {
        GmailError::runtime("send_state_invalid", error.to_string())
    })?;
    let note_relative = save::path_to_slashes(relative);
    let same_operation = current.as_ref().is_some_and(|current| {
        current.stage == SendStage::Complete
            && current.operation_id == state.operation_id
            && current.sent_message_id == state.sent_message_id
            && current.completed_commit_paths.contains(&note_relative)
    });
    if !same_operation {
        return Err(GmailError::runtime(
            "send_state_invalid",
            "the completed send state no longer matches this note".to_string(),
        ));
    }
    let digest_matches = state
        .completed_note_digest
        .as_deref()
        .zip(state::content_digest(&contents).ok().as_deref())
        .is_some_and(|(expected, actual)| expected == actual);
    let completed = current.as_ref().ok_or_else(|| {
        GmailError::runtime(
            "send_state_invalid",
            "the completed send state is missing".to_string(),
        )
    })?;
    let child_env = ob::child_env();
    let is_worktree =
        ob::detect_git_worktree(vault, &child_env).map_err(|error| {
            GmailError::runtime(
                "git",
                format!("could not inspect the vault repository: {error}"),
            )
        })?;
    if is_worktree {
        let mut commit_paths = Vec::new();
        for path in &completed.completed_commit_paths {
            let relative = PathBuf::from(path);
            if !save::git_path_is_dirty(vault, &child_env, &relative).map_err(
                |error| {
                    GmailError::runtime(
                        "git",
                        format!(
                            "could not inspect a completed send path: {error}"
                        ),
                    )
                },
            )? {
                continue;
            }
            let is_note = path == &note_relative;
            let include = if is_note {
                digest_matches
            } else if send_watch::receipt_matches(
                vault,
                &completed.completed_path_receipts,
                path,
            ) {
                true
            } else if completed
                .completed_path_receipts
                .iter()
                .any(|receipt| receipt.path == *path)
            {
                // The task (or other extra path) changed after send wrote
                // it; leave the later edit uncommitted.
                false
            } else {
                digest_matches
            };
            if include {
                commit_paths.push(relative);
            }
        }
        if !commit_paths.is_empty() {
            ob::commit_paths(
                vault,
                &child_env,
                "gmail: complete sent note",
                &commit_paths,
            )
            .map_err(|error| {
                GmailError::runtime(
                    "uncommitted",
                    format!(
                        "the completed sent note could not be committed: {error}"
                    ),
                )
            })?;
        }
    }
    send_watch::append_turns(vault, &completed.watch_turns)?;
    state::ensure_sent_event(vault, state)
}

fn promote_new(
    original: &str,
    source: &source::ComposeSource,
    rendered: &note::layout::RenderedNote,
) -> Result<String, GmailError> {
    let mut frontmatter = note::parse::parse_frontmatter_only(original)
        .ok_or_else(|| {
            GmailError::runtime(
                "note",
                "compose frontmatter is invalid".to_string(),
            )
        })?;
    note::parse::set_owned_metadata(&mut frontmatter, &rendered.metadata)
        .map_err(|error| GmailError::runtime("note", error.to_string()))?;
    let mapping = frontmatter.as_mapping_mut().ok_or_else(|| {
        GmailError::runtime(
            "note",
            "compose frontmatter is not a mapping".to_string(),
        )
    })?;
    mapping.insert(
        Value::String("saved".to_string()),
        Value::String(rendered.metadata.saved.clone()),
    );
    for key in [
        "status",
        "to",
        "cc",
        "bcc",
        "subject",
        "attachments",
        "gmail_draft_id",
        "gmail_draft_account",
        "gmail_draft_remote_fingerprint",
        "gmail_draft_create_operation",
    ] {
        mapping.remove(Value::String(key.to_string()));
    }
    let yaml = serde_yaml::to_string(&frontmatter).map_err(|_| {
        GmailError::runtime(
            "note",
            "could not render promoted frontmatter".to_string(),
        )
    })?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let mut output = String::from("---\n");
    output.push_str(yaml);
    if !yaml.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("---\n");
    let body_start = frontmatter_body_start(original)?;
    let mut body = original[body_start..].to_string();
    let first_insert = source
        .regions
        .first()
        .map(|region| region.start)
        .ok_or_else(|| {
            GmailError::runtime(
                "note",
                "compose message region is missing".to_string(),
            )
        })?
        - body_start;
    for region in source.regions.iter().rev() {
        body.replace_range(
            region.start - body_start..region.end - body_start,
            "",
        );
    }
    let mut conversation = format!(
        "\n{}\n· {}\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n",
        rendered.open_gmail, rendered.summary
    );
    for (index, card) in rendered.cards.iter().enumerate() {
        if index > 0 {
            conversation.push('\n');
        }
        conversation.push_str(&card.markdown);
        conversation.push('\n');
    }
    conversation.push_str("\n<!-- bob:gmail:end -->\n");
    body.insert_str(first_insert, &conversation);
    output.push_str(&body);
    Ok(output)
}

fn frontmatter_parent(contents: &str) -> String {
    note::parse::parse_frontmatter_only(contents)
        .and_then(|value| {
            value
                .as_mapping()?
                .get(Value::String("parent".to_string()))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "[[mail]]".to_string())
}

fn frontmatter_body_start(contents: &str) -> Result<usize, GmailError> {
    let mut lines = contents.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Err(GmailError::runtime(
            "note",
            "compose note is empty".to_string(),
        ));
    };
    if first.trim_end_matches(['\r', '\n']).trim() != "---" {
        return Err(GmailError::runtime(
            "note",
            "compose frontmatter is invalid".to_string(),
        ));
    }
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            return Ok(offset);
        }
    }
    Err(GmailError::runtime(
        "note",
        "compose frontmatter is incomplete".to_string(),
    ))
}

fn thread_identity_conflict() -> GmailError {
    GmailError::runtime(
        "thread_identity_conflict",
        "another mail note already owns this Gmail thread identity".to_string(),
    )
    .with_hint("keep the acknowledged send state and resolve the duplicate mail note before retrying")
}

fn sent_needs_review() -> GmailError {
    GmailError::runtime(
        "sent_needs_review",
        "Gmail sent the message, but the compose text changed before local finalization".to_string(),
    )
    .with_hint("the sent message was not retransmitted; review the note and restore the submitted section if it should be removed")
}

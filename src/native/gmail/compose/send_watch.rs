//! Shared Gmail watch planning for acknowledged-send finalization.
//!
//! Sibling metadata is fetched before the maintenance lock. Local planning
//! runs under that lock without a nested lock, a network read, or a second
//! commit. Only an enrolled, currently watched thread changes tasks.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use super::super::{
    config::GmailConfig,
    gog::Gog,
    model::GmailThread,
    resolve, save,
    sync::{
        enroll::EnrollmentFold,
        file, hold, scan, watch, watch_apply,
        watch_apply::RecoveryWatch,
        watch_state::{AppliedEffect, AppliedTurn, WatchJournal},
        ThreadKey,
    },
    turn, GmailError,
};
use super::state::{PathReceipt, SendWatchTurn};
use crate::native::{capture::StagedTextFile, env as bob_env, vault_sync};

/// Observation-only sibling threads needed for multi-thread aggregation.
pub(super) fn fetch_siblings(
    vault: &Path,
    config: &GmailConfig,
    gog: &Gog,
    sent: &ThreadKey,
) -> BTreeMap<ThreadKey, GmailThread> {
    let mut siblings = BTreeMap::new();
    let folder = Path::new(config.folder());
    let Ok(index) = resolve::scan_index(vault, folder, config.link_label())
    else {
        return siblings;
    };
    let Ok(watchers) = scan::scan_watching_tasks(vault, folder, &index) else {
        return siblings;
    };
    let Ok(enrollment) = EnrollmentFold::load(vault) else {
        return siblings;
    };
    if !enrollment.is_enrolled(sent) || !watchers.is_watched(sent) {
        return siblings;
    }
    for task in watchers.watchers(sent) {
        for target in &task.targets {
            if &target.key == sent || siblings.contains_key(&target.key) {
                continue;
            }
            match gog.thread_metadata(sent.account(), target.key.thread_id()) {
                Ok(thread)
                    if thread
                        .id
                        .eq_ignore_ascii_case(target.key.thread_id())
                        && thread.messages.iter().all(|message| {
                            message
                                .thread_id
                                .eq_ignore_ascii_case(target.key.thread_id())
                        }) =>
                {
                    siblings.insert(target.key.clone(), thread);
                }
                _ => {}
            }
        }
    }
    siblings
}

/// Planned task edits and marker receipts for the acknowledged send.
#[derive(Debug, Default)]
pub(super) struct SendWatchPlan {
    pub(super) staged: Vec<StagedTextFile>,
    pub(super) extra_paths: Vec<PathBuf>,
    pub(super) receipts: Vec<PathReceipt>,
    pub(super) turns: Vec<SendWatchTurn>,
    pub(super) pending: Vec<RecoveryWatch>,
}

/// Plan watch edits while the send lock is already held. Never fetches
/// Gmail, never takes a nested lock, and never commits.
pub(super) fn plan_under_lock(
    vault: &Path,
    config: &GmailConfig,
    account: &str,
    thread_id: &str,
    sent_thread: &GmailThread,
    siblings: &BTreeMap<ThreadKey, GmailThread>,
    mail_relative: &Path,
) -> Result<SendWatchPlan, GmailError> {
    let Some(sent) = ThreadKey::new(account, thread_id) else {
        return Ok(SendWatchPlan::default());
    };
    let folder = Path::new(config.folder());
    let Ok(index) = resolve::scan_index(vault, folder, config.link_label())
    else {
        return Ok(SendWatchPlan::default());
    };
    let Ok(watchers) = scan::scan_watching_tasks(vault, folder, &index) else {
        return Ok(SendWatchPlan::default());
    };
    let Ok(enrollment) = EnrollmentFold::load(vault) else {
        return Ok(SendWatchPlan::default());
    };
    if !enrollment.is_enrolled(&sent) || !watchers.is_watched(&sent) {
        return Ok(SendWatchPlan::default());
    }
    let Ok(journal) = WatchJournal::load(vault) else {
        return Ok(SendWatchPlan::default());
    };
    let pending = file::load_send_watch_pending(vault, account)?;
    let mut prepared = vec![watch::observation_only(
        sent.clone(),
        sent_thread,
        account,
        &journal,
        &index,
    )];
    for task in watchers.watchers(&sent) {
        for target in &task.targets {
            if target.key == sent {
                continue;
            }
            let Some(thread) = siblings.get(&target.key) else {
                continue;
            };
            if prepared
                .iter()
                .any(|watch| watch.observation.key == target.key)
            {
                continue;
            }
            prepared.push(watch::observation_only(
                target.key.clone(),
                thread,
                account,
                &journal,
                &index,
            ));
        }
    }
    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    let today = bob_env::current_datetime().date();
    let running = hold::load_running_tasks(vault);
    let plan = watch_apply::plan_for_send(
        vault,
        timezone,
        config.reply_wait_days(),
        today,
        running,
        &watchers,
        &prepared,
        &journal,
        &pending,
    );
    let mail_path = vault.join(mail_relative);
    let mut result = SendWatchPlan::default();
    for edit in plan.edits {
        if edit.target == mail_path {
            continue;
        }
        let Ok(relative) = edit.target.strip_prefix(vault) else {
            continue;
        };
        let relative = relative.to_path_buf();
        let slashes = save::path_to_slashes(&relative);
        result.receipts.push(PathReceipt {
            path: slashes,
            sha256: save::sha256(edit.updated.as_bytes()),
        });
        result.extra_paths.push(relative);
        result.staged.push(StagedTextFile {
            target: edit.target,
            target_existed: edit.existed,
            original_target: edit.original,
            updated_target: edit.updated,
        });
    }
    result.turns = plan
        .receipts
        .turns
        .into_iter()
        .map(|turn| SendWatchTurn {
            account: turn.key.account().to_string(),
            thread_id: turn.key.thread_id().to_string(),
            message_id: turn.message_id,
            internal_date: turn.internal_date,
            turn: turn.turn.as_str().to_string(),
            effect: turn.effect.as_str().to_string(),
        })
        .collect();
    result.pending = plan.pending;
    Ok(result)
}

/// Append stored watch turns if they are not already the folded marker.
/// Duplicate appends fold to the same identity and never resend.
pub(super) fn append_turns(
    vault: &Path,
    turns: &[SendWatchTurn],
) -> Result<(), GmailError> {
    if turns.is_empty() {
        return Ok(());
    }
    let host = vault_sync::hostname_slug();
    let journal = WatchJournal::load(vault).unwrap_or_default();
    for turn in turns {
        let Some(applied) = applied_turn(turn) else {
            continue;
        };
        if let Some(marker) = journal.marker(&applied.key)
            && marker.message_id.eq_ignore_ascii_case(&applied.message_id)
            && marker.internal_date == applied.internal_date
        {
            continue;
        }
        WatchJournal::append_turn(vault, &host, &applied).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not record the Gmail watch turn: {error}"),
            )
            .with_hint(
                "the send is acknowledged; retry `bob gmail send NOTE` to finish local finalization",
            )
        })?;
    }
    Ok(())
}

fn applied_turn(turn: &SendWatchTurn) -> Option<AppliedTurn> {
    let key = ThreadKey::new(&turn.account, &turn.thread_id)?;
    let parsed_turn = match turn.turn.as_str() {
        "yours" => turn::Turn::Yours,
        "theirs" => turn::Turn::Theirs,
        _ => return None,
    };
    let effect = AppliedEffect::parse(&turn.effect)?;
    Some(AppliedTurn {
        key,
        message_id: turn.message_id.to_ascii_lowercase(),
        internal_date: turn.internal_date,
        turn: parsed_turn,
        effect,
    })
}

/// SHA-256 of a vault-relative path's current bytes, when the file exists.
pub(super) fn file_digest(vault: &Path, relative: &str) -> Option<String> {
    fs::read(vault.join(relative))
        .ok()
        .map(|bytes| save::sha256(&bytes))
}

/// Whether this extra path still matches the receipt send wrote.
pub(super) fn receipt_matches(
    vault: &Path,
    receipts: &[PathReceipt],
    relative: &str,
) -> bool {
    receipts.iter().any(|receipt| {
        receipt.path == relative
            && file_digest(vault, relative).as_deref() == Some(&receipt.sha256)
    })
}

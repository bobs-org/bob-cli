//! Metadata-first preparation and deterministic task effects for Gmail watches.
//!
//! This module performs remote reads and prepares mail refreshes only. It
//! never writes the vault; the following transaction phase consumes its
//! typed results while holding the maintenance lock.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use chrono::{DateTime, Utc};

use super::{
    enroll::EnrollmentFold,
    retry::{is_auth_abort, should_retry},
    scan::{WatchScan, WatchingTask},
    watch_state::WatchJournal,
    ThreadKey,
};
use crate::native::{
    config::GmailBodyMode,
    gmail::{
        body::{importable_messages, inert_single_line},
        model::{GmailThread, HeaderKind},
        note::{self, index::MailIndex},
        policy::NeverSavePolicy,
        resolve::{GmailRead, ResolvedIdentity},
        save::{self, PreparedSave},
        turn::{self, Net, ThreadTurn, Turn},
        GmailError,
    },
};

/// The fetched turn state for one currently watched thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadObservation {
    pub(crate) key: ThreadKey,
    pub(crate) folded: ThreadTurn,
}

/// A transaction-ready Bob-owned turn-field update. The source note is
/// reread and transformed under the maintenance lock by the apply phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnMetadataUpdate {
    pub(crate) path: String,
    pub(crate) turn: Option<String>,
    pub(crate) turn_since: Option<String>,
}

/// All remote preparation for one watched thread. `refresh` may contain
/// email bodies in memory and deliberately has no Debug/serde surface.
pub(crate) struct PreparedWatch {
    pub(crate) observation: ThreadObservation,
    pub(crate) note_path: Option<String>,
    /// Sanitized mail metadata used only for human watch rows.
    pub(crate) title: Option<String>,
    pub(crate) who: Option<String>,
    pub(crate) refresh: Option<PreparedSave>,
    pub(crate) turn_update: Option<TurnMetadataUpdate>,
    /// True only on the first run that observed the missing/trashed thread.
    pub(crate) report_gone: bool,
    /// A note refresh can fail independently of the metadata-based turn.
    pub(crate) refresh_failure: Option<WatchFailure>,
    /// Union of every message's Gmail label ids, for health routing.
    pub(crate) label_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchFailure {
    pub(crate) key: ThreadKey,
    pub(crate) kind: String,
    pub(crate) message: String,
}

/// Isolated preparation results. An auth abort is present when the caller
/// must discard every staged write; other failures leave independent
/// successes available to the transaction.
pub(crate) struct WatchBatch {
    pub(crate) prepared: Vec<PreparedWatch>,
    pub(crate) failures: Vec<WatchFailure>,
    pub(crate) deferred: Vec<ThreadKey>,
    pub(crate) aborted: Option<GmailError>,
    pub(crate) warnings: Vec<String>,
}

/// Task-level action after all of a task's watched threads have been folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskEffect {
    /// Every linked watched thread is known to be waiting on Bryan.
    Hold { nudge_at: DateTime<Utc> },
    /// A new incoming message has precedence over outstanding sends.
    Lifted,
    /// The newest decisive incoming move is a delivery failure.
    Bounced,
}

/// One task assessment, keyed by its pre-transaction location and stable block
/// id. `deferred_unknown` tells the transaction to retain any new outgoing
/// marker until every watched sibling can be evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskPlan {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: Option<String>,
    pub(crate) effect: Option<TaskEffect>,
    pub(crate) has_new_move: bool,
    pub(crate) deferred_unknown: bool,
}

/// Select currently watched enrollments, plus explicitly supplied rows filed
/// or adopted in this run. Enrollments without an open linking task are never
/// fetched, and removing a Gmail label does not remove an enrollment.
pub(crate) fn watched_keys(
    account: &str,
    enrollment: &EnrollmentFold,
    scan: &WatchScan,
    additional: &[ThreadKey],
) -> Vec<ThreadKey> {
    let mut keys = BTreeSet::new();
    for key in enrollment.threads() {
        if key.is_account(account) && scan.is_watched(key) {
            keys.insert(key.clone());
        }
    }
    for key in additional {
        // These are this run's prepared filing candidates. The transaction
        // projects their new task rows before applying the watch effect.
        if key.is_account(account) {
            keys.insert(key.clone());
        }
    }
    keys.into_iter().collect()
}

/// Prepare enrolled thread metadata through the caller's coordinated read.
/// `known_metadata` lets sync reuse discovery's successful metadata for newly
/// filed candidates instead of fetching the thread again.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_watches<R: GmailRead>(
    reader: &R,
    vault: &Path,
    account: &str,
    enrollment: &EnrollmentFold,
    scan: &WatchScan,
    additional: &[ThreadKey],
    known_metadata: &BTreeMap<ThreadKey, GmailThread>,
    journal: &WatchJournal,
    index: &MailIndex,
    body_mode: GmailBodyMode,
    never_save: &NeverSavePolicy,
    timezone: chrono_tz::Tz,
) -> WatchBatch {
    let keys = watched_keys(account, enrollment, scan, additional);
    let mut batch = WatchBatch {
        prepared: Vec::new(),
        failures: Vec::new(),
        deferred: Vec::new(),
        aborted: None,
        warnings: journal.warnings().to_vec(),
    };

    for (position, key) in keys.iter().enumerate() {
        let thread = match known_metadata.get(key) {
            Some(thread) => Ok(thread.clone()),
            None => reader.thread_metadata(key.account(), key.thread_id()),
        };
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) if is_auth_abort(&error) => {
                batch.warnings.push(format!(
                    "stopped Gmail watch reads when the grant was revoked or expired: {}: {}",
                    error.kind(), error.message()
                ));
                batch.aborted = Some(error);
                batch.deferred.extend(keys[position + 1..].iter().cloned());
                break;
            }
            Err(error)
                if should_retry(&error) || error.kind() == "deferred" =>
            {
                batch.warnings.push(format!(
                    "deferred remaining Gmail watch reads when the retry budget ran out: {}: {}",
                    error.kind(), error.message()
                ));
                batch.deferred.extend(keys[position..].iter().cloned());
                break;
            }
            Err(error) if error.kind() == "not_found" => {
                if !journal.was_reported_gone(key) {
                    batch.prepared.push(gone_prepared(key, index));
                }
                continue;
            }
            Err(error) => {
                batch.failures.push(WatchFailure {
                    key: key.clone(),
                    kind: error.kind().to_string(),
                    message: error.message().to_string(),
                });
                continue;
            }
        };

        if thread.id != key.thread_id()
            || thread
                .messages
                .iter()
                .any(|message| message.thread_id != key.thread_id())
        {
            batch.failures.push(WatchFailure {
                key: key.clone(),
                kind: "unresolvable".to_string(),
                message: "Gmail returned a different thread identity"
                    .to_string(),
            });
            continue;
        }
        if is_trashed(&thread) {
            if !journal.was_reported_gone(key) {
                batch.prepared.push(gone_prepared(key, index));
            }
            continue;
        }

        let marker = journal.marker(key);
        let marker = marker.map(|marker| turn::Marker {
            message_id: &marker.message_id,
            internal_date: marker.internal_date,
        });
        let folded = turn::fold(&thread.messages, key.account(), marker);
        if let Net::Unknown(reason) = folded.net {
            batch.warnings.push(format!(
                "Gmail thread {} has an unknown reply turn: {}",
                key.thread_id(),
                reason.message()
            ));
        }
        let mut prepared = PreparedWatch {
            observation: ThreadObservation {
                key: key.clone(),
                folded: folded.clone(),
            },
            note_path: None,
            title: display_metadata(&thread, key.account())
                .map(|value| value.0),
            who: display_metadata(&thread, key.account()).map(|value| value.1),
            refresh: None,
            turn_update: None,
            report_gone: false,
            refresh_failure: None,
            label_ids: thread_label_ids(&thread),
        };

        let Some(entry) = index.by_identity.get(&key.index_key()) else {
            batch.prepared.push(prepared);
            continue;
        };
        prepared.note_path = Some(entry.path.clone());
        prepared.title = Some(entry.title.clone());
        prepared.who = Some(entry.who.clone());
        let source_path = vault.join(&entry.path);
        let existing = match fs::read_to_string(&source_path) {
            Ok(contents) => contents,
            Err(error) => {
                prepared.refresh_failure = Some(WatchFailure {
                    key: key.clone(),
                    kind: "note_read".to_string(),
                    message: format!(
                        "could not read the saved mail note: {error}"
                    ),
                });
                batch.prepared.push(prepared);
                continue;
            }
        };
        let parsed = match note::parse::parse(&existing) {
            Ok(parsed)
                if parsed.account.eq_ignore_ascii_case(key.account())
                    && parsed.thread_id == key.thread_id() =>
            {
                parsed
            }
            Ok(_) => {
                prepared.refresh_failure = Some(WatchFailure {
                    key: key.clone(),
                    kind: "unresolvable".to_string(),
                    message: "saved mail note identity changed during watch preparation".to_string(),
                });
                batch.prepared.push(prepared);
                continue;
            }
            Err(error) => {
                prepared.refresh_failure = Some(WatchFailure {
                    key: key.clone(),
                    kind: "note_damaged".to_string(),
                    message: error.to_string(),
                });
                batch.prepared.push(prepared);
                continue;
            }
        };
        let remote_ids: Vec<_> = importable_messages(&thread)
            .map(|message| message.id.to_ascii_lowercase())
            .collect();
        let saved_ids: Vec<_> = parsed
            .cards
            .iter()
            .map(|card| card.id.to_ascii_lowercase())
            .collect();

        if remote_ids != saved_ids {
            let identity = ResolvedIdentity {
                account: key.account().to_string(),
                thread_id: key.thread_id().to_string(),
                known_path: Some(entry.path.clone()),
            };
            match save::prepare_sync_candidate_from_thread(
                reader, account, identity, thread, body_mode, never_save,
            ) {
                Ok(refresh) => prepared.refresh = Some(refresh),
                Err(error) => {
                    prepared.refresh_failure = Some(WatchFailure {
                        key: key.clone(),
                        kind: error.kind().to_string(),
                        message: error.message().to_string(),
                    });
                }
            }
        } else if !matches!(folded.net, Net::Unknown(_))
            && let Some(turn) = folded.turn
        {
            let turn_since = folded.turn_since.map(|time| {
                time.with_timezone(&timezone)
                    .format("%Y-%m-%dT%H:%M:%S%:z")
                    .to_string()
            });
            let turn = Some(turn.as_str().to_string());
            if note::parse::update_turn_fields(
                &existing,
                turn.as_deref(),
                turn_since.as_deref(),
            )
            .is_ok_and(|updated| updated.is_some())
            {
                prepared.turn_update = Some(TurnMetadataUpdate {
                    path: entry.path.clone(),
                    turn,
                    turn_since,
                });
            }
        } else if !matches!(folded.net, Net::Unknown(_))
            && note::parse::update_turn_fields(&existing, None, None)
                .is_ok_and(|updated| updated.is_some())
        {
            prepared.turn_update = Some(TurnMetadataUpdate {
                path: entry.path.clone(),
                turn: None,
                turn_since: None,
            });
        }
        batch.prepared.push(prepared);
    }
    batch
}

fn gone_prepared(key: &ThreadKey, index: &MailIndex) -> PreparedWatch {
    let entry = index.by_identity.get(&key.index_key());
    PreparedWatch {
        observation: ThreadObservation {
            key: key.clone(),
            folded: ThreadTurn {
                net: Net::Nothing,
                turn: None,
                turn_since: None,
                newest_counted_id: None,
                newest_internal_date: None,
                last_send_at: None,
            },
        },
        note_path: entry.map(|entry| entry.path.clone()),
        title: entry.map(|entry| entry.title.clone()),
        who: entry.map(|entry| entry.who.clone()),
        refresh: None,
        turn_update: None,
        report_gone: true,
        refresh_failure: None,
        label_ids: BTreeSet::new(),
    }
}

fn thread_label_ids(thread: &GmailThread) -> BTreeSet<String> {
    thread
        .messages
        .iter()
        .flat_map(|message| message.label_ids.iter().cloned())
        .collect()
}

fn is_trashed(thread: &GmailThread) -> bool {
    !thread.messages.is_empty()
        && thread.messages.iter().all(|message| {
            message
                .label_ids
                .iter()
                .any(|label| label.eq_ignore_ascii_case("TRASH"))
        })
}

fn display_metadata(
    thread: &GmailThread,
    account: &str,
) -> Option<(String, String)> {
    let first = importable_messages(thread).next()?;
    let subject = crate::native::gmail::note::subject_without_prefixes(
        first.first_header(HeaderKind::Subject).unwrap_or_default(),
    );
    let title = if subject.is_empty() {
        "(no subject)".to_string()
    } else {
        subject
    };
    Some((
        inert_single_line(&title),
        inert_single_line(&note::thread_who(first, account)),
    ))
}

/// Observation-only preparation for send finalization. The sent-note write
/// owns the mail note; siblings are fetched only so multi-thread aggregation
/// can see them. Neither refresh nor turn-field updates are produced.
pub(crate) fn observation_only(
    key: ThreadKey,
    thread: &GmailThread,
    account: &str,
    journal: &WatchJournal,
    index: &MailIndex,
) -> PreparedWatch {
    let marker = journal.marker(&key).map(|marker| turn::Marker {
        message_id: &marker.message_id,
        internal_date: marker.internal_date,
    });
    let folded = turn::fold(&thread.messages, account, marker);
    let entry = index.by_identity.get(&key.index_key());
    let display = display_metadata(thread, account);
    PreparedWatch {
        observation: ThreadObservation { key, folded },
        note_path: entry.map(|entry| entry.path.clone()),
        title: entry
            .map(|entry| entry.title.clone())
            .or_else(|| display.as_ref().map(|value| value.0.clone())),
        who: entry
            .map(|entry| entry.who.clone())
            .or_else(|| display.as_ref().map(|value| value.1.clone())),
        refresh: None,
        turn_update: None,
        report_gone: false,
        refresh_failure: None,
        label_ids: thread_label_ids(thread),
    }
}

/// Aggregate one effect per task, independent of thread iteration order.
/// Incoming moves and bounces win over outgoing moves; an outgoing move only
/// holds the task when every watched sibling is known to be awaiting a reply.
pub(crate) fn plan_tasks(
    scan: &WatchScan,
    prepared: &[PreparedWatch],
) -> Vec<TaskPlan> {
    let observations: BTreeMap<_, _> = prepared
        .iter()
        .filter(|watch| !watch.report_gone)
        .map(|watch| {
            (
                watch.observation.key.clone(),
                watch.observation.folded.clone(),
            )
        })
        .collect();
    scan.tasks()
        .iter()
        .map(|task| plan_task(task, &observations))
        .collect()
}

fn plan_task(
    task: &WatchingTask,
    observations: &BTreeMap<ThreadKey, ThreadTurn>,
) -> TaskPlan {
    let mut incoming: Option<(i64, String, ThreadKey, TaskEffect)> = None;
    let mut has_new_hold = false;
    let mut has_new_move = false;
    let mut all_waiting = true;
    let mut has_unknown = false;
    let mut newest_send: Option<DateTime<Utc>> = None;

    for target in &task.targets {
        let Some(folded) = observations.get(&target.key) else {
            all_waiting = false;
            has_unknown = true;
            continue;
        };
        match folded.net {
            Net::Lift | Net::Bounced => {
                has_new_move = true;
                let date = folded.newest_internal_date.unwrap_or(i64::MIN);
                let effect = if folded.net == Net::Bounced {
                    TaskEffect::Bounced
                } else {
                    TaskEffect::Lifted
                };
                let message_id = folded
                    .newest_counted_id
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let candidate = (date, message_id, target.key.clone(), effect);
                if incoming.as_ref().is_none_or(|current| {
                    (&candidate.0, &candidate.1, &candidate.2)
                        > (&current.0, &current.1, &current.2)
                }) {
                    incoming = Some(candidate);
                }
            }
            Net::Hold => {
                has_new_hold = true;
                has_new_move = true;
            }
            Net::Nothing => {}
            Net::Unknown(_) => {
                all_waiting = false;
                has_unknown = true;
            }
        }
        if folded.turn != Some(Turn::Theirs) {
            all_waiting = false;
        }
        if folded.turn == Some(Turn::Theirs)
            && let Some(send) = folded.last_send_at
            && newest_send.is_none_or(|current| send > current)
        {
            newest_send = Some(send);
        }
    }

    let effect = if let Some((_, _, _, effect)) = incoming {
        Some(effect)
    } else if has_new_hold && all_waiting {
        newest_send.map(|nudge_at| TaskEffect::Hold { nudge_at })
    } else {
        None
    };
    let deferred_unknown = effect.is_none() && has_new_hold && has_unknown;
    TaskPlan {
        path: task.path.clone(),
        line: task.line,
        block_id: task.block_id.clone(),
        effect,
        has_new_move,
        deferred_unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, fs};

    use super::*;
    use crate::native::gmail::sync::scan::{LinkPlacement, WatchTarget};
    use chrono::TimeZone;

    fn key(id: &str) -> ThreadKey {
        ThreadKey::new("me@example.com", id).unwrap()
    }

    fn task(keys: &[ThreadKey]) -> WatchingTask {
        WatchingTask {
            path: "projects/example.md".to_string(),
            line: 7,
            text: "- [ ] #task Test".to_string(),
            block_id: Some("task1".to_string()),
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

    fn folded(
        net: Net,
        turn: Option<Turn>,
        date: i64,
        send: Option<DateTime<Utc>>,
    ) -> ThreadTurn {
        ThreadTurn {
            net,
            turn,
            turn_since: None,
            newest_counted_id: Some("aa01".to_string()),
            newest_internal_date: Some(date),
            last_send_at: send,
        }
    }

    fn observations(
        entries: Vec<(ThreadKey, ThreadTurn)>,
    ) -> BTreeMap<ThreadKey, ThreadTurn> {
        entries.into_iter().collect()
    }

    #[test]
    fn hold_requires_every_sibling_to_be_known_waiting_and_uses_newest_send() {
        let a = key("aa01");
        let b = key("bb02");
        let send_a = Utc.timestamp_millis_opt(10).unwrap();
        let send_b = Utc.timestamp_millis_opt(20).unwrap();
        let values = observations(vec![
            (
                a.clone(),
                folded(Net::Hold, Some(Turn::Theirs), 10, Some(send_a)),
            ),
            (
                b.clone(),
                folded(Net::Nothing, Some(Turn::Theirs), 20, Some(send_b)),
            ),
        ]);
        let plan = plan_task(&task(&[a.clone(), b.clone()]), &values);
        assert_eq!(plan.effect, Some(TaskEffect::Hold { nudge_at: send_b }));
        assert!(!plan.deferred_unknown);

        let unknown = observations(vec![
            (
                a.clone(),
                folded(Net::Hold, Some(Turn::Theirs), 10, Some(send_a)),
            ),
            (
                b.clone(),
                folded(
                    Net::Unknown(turn::UnknownReason::UnprovenDirection),
                    None,
                    20,
                    None,
                ),
            ),
        ]);
        let plan = plan_task(&task(&[a, b]), &unknown);
        assert_eq!(plan.effect, None);
        assert!(plan.deferred_unknown);
    }

    #[test]
    fn incoming_move_wins_over_sibling_send_in_either_order() {
        let a = key("aa01");
        let b = key("bb02");
        let send = folded(Net::Hold, Some(Turn::Theirs), 10, None);
        let reply = folded(Net::Lift, Some(Turn::Yours), 11, None);
        let first = observations(vec![
            (a.clone(), send.clone()),
            (b.clone(), reply.clone()),
        ]);
        let second = observations(vec![(b.clone(), reply), (a.clone(), send)]);
        assert_eq!(
            plan_task(&task(&[a.clone(), b.clone()]), &first).effect,
            Some(TaskEffect::Lifted)
        );
        assert_eq!(
            plan_task(&task(&[b, a]), &second).effect,
            Some(TaskEffect::Lifted)
        );
    }

    #[test]
    fn bounce_is_kept_as_the_newest_incoming_reason() {
        let a = key("aa01");
        let values = observations(vec![(
            a.clone(),
            folded(Net::Bounced, Some(Turn::Yours), 10, None),
        )]);
        assert_eq!(
            plan_task(&task(&[a]), &values).effect,
            Some(TaskEffect::Bounced)
        );
    }

    #[test]
    fn no_new_counted_move_never_reasserts_a_manual_task_edit() {
        let a = key("aa01");
        let values = observations(vec![(
            a.clone(),
            folded(Net::Nothing, Some(Turn::Theirs), 10, None),
        )]);
        let plan = plan_task(&task(&[a]), &values);
        assert_eq!(plan.effect, None);
        assert!(!plan.has_new_move);
        assert!(!plan.deferred_unknown);
    }

    #[test]
    fn task_plans_keep_scan_order() {
        let a = key("aa01");
        let task = task(std::slice::from_ref(&a));
        let scan = WatchScan::from_tasks_for_test(vec![task.clone(), task]);
        let prepared = Vec::new();
        let plans = plan_tasks(&scan, &prepared);
        assert_eq!(plans.len(), 2);
        assert!(plans.iter().all(|plan| plan.effect.is_none()));
    }

    struct Reader {
        threads: RefCell<BTreeMap<ThreadKey, Result<GmailThread, GmailError>>>,
        metadata_calls: RefCell<Vec<ThreadKey>>,
        full_calls: RefCell<Vec<ThreadKey>>,
    }

    impl Reader {
        fn new(
            entries: impl IntoIterator<
                Item = (ThreadKey, Result<GmailThread, GmailError>),
            >,
        ) -> Self {
            Self {
                threads: RefCell::new(entries.into_iter().collect()),
                metadata_calls: RefCell::new(Vec::new()),
                full_calls: RefCell::new(Vec::new()),
            }
        }

        fn get(
            &self,
            account: &str,
            id: &str,
        ) -> Result<GmailThread, GmailError> {
            let key = ThreadKey::new(account, id).unwrap();
            self.threads.borrow().get(&key).cloned().unwrap_or_else(|| {
                Err(GmailError::runtime("not_found", "missing".into()))
            })
        }
    }

    impl GmailRead for Reader {
        fn attachment_bytes(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, GmailError> {
            unreachable!("test bodies are inline")
        }
        fn thread_get(
            &self,
            account: &str,
            id: &str,
        ) -> Result<GmailThread, GmailError> {
            self.full_calls
                .borrow_mut()
                .push(ThreadKey::new(account, id).unwrap());
            self.get(account, id)
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
        fn labels_list(
            &self,
            _: &str,
        ) -> Result<Vec<crate::native::gmail::gog::GmailLabel>, GmailError>
        {
            Ok(Vec::new())
        }
        fn thread_metadata(
            &self,
            account: &str,
            id: &str,
        ) -> Result<GmailThread, GmailError> {
            self.metadata_calls
                .borrow_mut()
                .push(ThreadKey::new(account, id).unwrap());
            self.get(account, id)
        }
    }

    fn synthetic_thread(
        id: &str,
        messages: &[(&str, &str, &str)],
    ) -> GmailThread {
        let messages = messages
            .iter()
            .map(|(message_id, date, from)| {
                let to = if from.to_ascii_lowercase().contains("me@example.com")
                {
                    "Dana <dana@example.com>"
                } else {
                    "me@example.com"
                };
                serde_json::json!({
                    "id": message_id,
                    "threadId": id,
                    "labelIds": ["INBOX"],
                    "internalDate": date,
                    "snippet": "private preview",
                    "payload": {
                        "mimeType": "text/plain",
                        "headers": [
                            {"name":"From", "value":from},
                            {"name":"To", "value":to},
                            {"name":"Subject", "value":"Example"},
                            {"name":"Content-Type", "value":"text/plain"}
                        ],
                        "body": {"data":"SGVsbG8="}
                    }
                })
            })
            .collect::<Vec<_>>();
        GmailThread::from_gog_json(
            serde_json::json!({"thread":{"id":id,"messages":messages}})
                .to_string()
                .as_bytes(),
        )
        .unwrap()
    }

    fn saved_note(id: &str, cards: &[&str], fields: &str) -> String {
        let cards = cards.iter().map(|message_id| format!(
            "<!-- bob:gmail:msg {message_id} - -->\n> [!mail+] Dana · Fri, Sep 19 · 9:46 AM to you\n> preserved body\n\n^m-{message_id}\n"
        )).collect::<Vec<_>>().join("\n");
        format!(
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: {id}\n{fields}---\n\n# Example\n\n· Dana ↔ you · {} messages · Sep 19 → Sep 19\n\n## Notes\nmanual notes\n\n## Reply\nreply text\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n{cards}\n<!-- bob:gmail:end -->\n",
            cards.lines().filter(|line| line.starts_with("<!-- bob:gmail:msg")).count()
        )
    }

    fn indexed_note(path: &str, key: &ThreadKey) -> MailIndex {
        let mut index = MailIndex::default();
        index.by_identity.insert(
            key.index_key(),
            note::index::MailIndexEntry {
                account: key.account().to_string(),
                thread_id: key.thread_id().to_string(),
                path: path.to_string(),
                title: "Example".to_string(),
                who: "Dana".to_string(),
                label: "Example".to_string(),
                started: String::new(),
                updated: String::new(),
                messages: 1,
                turn: None,
                turn_since: None,
            },
        );
        index
    }

    fn prepare_test(
        reader: &Reader,
        vault: &Path,
        keys: &[ThreadKey],
        index: &MailIndex,
        journal: &WatchJournal,
        policy: &NeverSavePolicy,
        known: &BTreeMap<ThreadKey, GmailThread>,
    ) -> WatchBatch {
        let enrollment = EnrollmentFold::load(vault).unwrap();
        let scan = WatchScan::from_tasks_for_test(vec![task(keys)]);
        prepare_watches(
            reader,
            vault,
            "me@example.com",
            &enrollment,
            &scan,
            keys,
            known,
            journal,
            index,
            GmailBodyMode::Full,
            policy,
            chrono_tz::UTC,
        )
    }

    #[test]
    fn metadata_only_poll_is_byte_noop_and_first_watch_sees_outgoing_last() {
        let dir = tempfile::tempdir().unwrap();
        let key = key("aa01");
        let path = "mail/aa01.md";
        let note = saved_note(
            "aa01",
            &["aa01"],
            "turn: theirs\nturn_since: 1970-01-01T00:00:01+00:00\n",
        );
        fs::create_dir_all(dir.path().join("mail")).unwrap();
        fs::write(dir.path().join(path), &note).unwrap();
        let thread = synthetic_thread(
            "aa01",
            &[("aa01", "1000", "Me <me@example.com>")],
        );
        let reader = Reader::new([(key.clone(), Ok(thread.clone()))]);
        let batch = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &indexed_note(path, &key),
            &WatchJournal::default(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert_eq!(batch.prepared.len(), 1);
        assert_eq!(batch.prepared[0].observation.folded.net, Net::Hold);
        assert!(batch.prepared[0].refresh.is_none());
        assert!(batch.prepared[0].turn_update.is_none());
        assert_eq!(fs::read_to_string(dir.path().join(path)).unwrap(), note);
        assert_eq!(reader.metadata_calls.borrow().as_slice(), &[key]);
        assert!(reader.full_calls.borrow().is_empty());
    }

    #[test]
    fn metadata_only_turn_change_stages_only_the_owned_frontmatter_fields() {
        let dir = tempfile::tempdir().unwrap();
        let key = key("aa01");
        let path = "mail/aa01.md";
        let note = saved_note(
            "aa01",
            &["aa01"],
            "turn: yours\nturn_since: 1969-12-31T23:59:59+00:00\n",
        );
        fs::create_dir_all(dir.path().join("mail")).unwrap();
        fs::write(dir.path().join(path), &note).unwrap();
        let thread = synthetic_thread(
            "aa01",
            &[("aa01", "1000", "Me <me@example.com>")],
        );
        let reader = Reader::new([(key.clone(), Ok(thread))]);
        let batch = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &indexed_note(path, &key),
            &WatchJournal::default(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        let update = batch.prepared[0].turn_update.as_ref().unwrap();
        assert_eq!(update.path, path);
        assert_eq!(update.turn.as_deref(), Some("theirs"));
        assert_eq!(
            update.turn_since.as_deref(),
            Some("1970-01-01T00:00:01+00:00")
        );
        assert!(batch.prepared[0].refresh.is_none());
        assert_eq!(fs::read_to_string(dir.path().join(path)).unwrap(), note);
    }

    #[test]
    fn changed_card_ids_prepare_refresh_from_cached_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let key = key("aa01");
        let path = "mail/aa01.md";
        fs::create_dir_all(dir.path().join("mail")).unwrap();
        fs::write(dir.path().join(path), saved_note("aa01", &["aa01"], ""))
            .unwrap();
        let thread = synthetic_thread(
            "aa01",
            &[
                ("aa01", "1000", "Dana <dana@example.com>"),
                ("aa02", "2000", "Dana <dana@example.com>"),
            ],
        );
        let reader = Reader::new([(key.clone(), Ok(thread.clone()))]);
        let batch = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &indexed_note(path, &key),
            &WatchJournal::default(),
            &NeverSavePolicy::default(),
            &BTreeMap::from([(key.clone(), thread)]),
        );
        assert!(batch.prepared[0].refresh.is_some());
        assert!(reader.metadata_calls.borrow().is_empty());
        assert!(reader.full_calls.borrow().is_empty());
    }

    #[test]
    fn never_save_blocks_refresh_without_losing_metadata_turn() {
        let dir = tempfile::tempdir().unwrap();
        let key = key("aa01");
        let path = "mail/aa01.md";
        fs::create_dir_all(dir.path().join("mail")).unwrap();
        fs::write(dir.path().join(path), saved_note("aa01", &["aa01"], ""))
            .unwrap();
        let thread = synthetic_thread(
            "aa01",
            &[
                ("aa01", "1000", "Dana <blocked@example.com>"),
                ("aa02", "2000", "Dana <blocked@example.com>"),
            ],
        );
        let reader = Reader::new([(key.clone(), Ok(thread.clone()))]);
        let batch = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &indexed_note(path, &key),
            &WatchJournal::default(),
            &NeverSavePolicy::for_test(&["blocked@example.com"], &[]),
            &BTreeMap::from([(key.clone(), thread)]),
        );
        assert!(batch.prepared[0].refresh.is_none());
        assert_eq!(
            batch.prepared[0]
                .refresh_failure
                .as_ref()
                .map(|f| f.kind.as_str()),
            Some("never_save")
        );
        assert_eq!(batch.prepared[0].observation.folded.net, Net::Lift);
    }

    #[test]
    fn auth_abort_stops_reads_and_transient_exhaustion_defers_remaining_keys() {
        let dir = tempfile::tempdir().unwrap();
        let a = key("aa01");
        let b = key("bb02");
        let reader = Reader::new([(
            a.clone(),
            Err(GmailError::runtime("invalid_grant", "expired".into())),
        )]);
        let auth_batch = prepare_test(
            &reader,
            dir.path(),
            &[a.clone(), b.clone()],
            &MailIndex::default(),
            &WatchJournal::default(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert_eq!(
            auth_batch.aborted.as_ref().map(GmailError::kind),
            Some("invalid_grant")
        );
        assert_eq!(auth_batch.deferred, vec![b.clone()]);
        assert_eq!(
            reader.metadata_calls.borrow().as_slice(),
            std::slice::from_ref(&a)
        );

        let reader = Reader::new([
            (
                a.clone(),
                Ok(synthetic_thread(
                    "aa01",
                    &[("aa01", "1000", "Dana <dana@example.com>")],
                )),
            ),
            (
                b.clone(),
                Err(GmailError::runtime("network", "offline".into())),
            ),
        ]);
        let batch = prepare_test(
            &reader,
            dir.path(),
            &[a.clone(), b.clone()],
            &MailIndex::default(),
            &WatchJournal::default(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert_eq!(batch.deferred, vec![b]);
        assert_eq!(batch.prepared.len(), 1);
        assert_eq!(batch.prepared[0].observation.key, a);
    }

    #[test]
    fn permanent_error_keeps_independent_prepared_thread() {
        let dir = tempfile::tempdir().unwrap();
        let a = key("aa01");
        let b = key("bb02");
        let reader = Reader::new([
            (
                a.clone(),
                Err(GmailError::runtime("bad_json", "invalid".into())),
            ),
            (
                b.clone(),
                Ok(synthetic_thread(
                    "bb02",
                    &[("bb02", "2000", "Dana <dana@example.com>")],
                )),
            ),
        ]);
        let batch = prepare_test(
            &reader,
            dir.path(),
            &[a.clone(), b.clone()],
            &MailIndex::default(),
            &WatchJournal::default(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert_eq!(batch.failures.len(), 1);
        assert_eq!(batch.failures[0].key, a);
        assert_eq!(batch.prepared.len(), 1);
        assert_eq!(batch.prepared[0].observation.key, b);
    }

    #[test]
    fn missing_thread_is_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let key = key("aa01");
        let reader = Reader::new([(
            key.clone(),
            Err(GmailError::runtime("not_found", "gone".into())),
        )]);
        let first = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &MailIndex::default(),
            &WatchJournal::load(dir.path()).unwrap(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert!(first.prepared[0].report_gone);
        WatchJournal::append_gone(dir.path(), "testhost", &key).unwrap();
        let second = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &MailIndex::default(),
            &WatchJournal::load(dir.path()).unwrap(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert!(second.prepared.is_empty());
    }

    #[test]
    fn trashed_thread_is_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let key = key("aa01");
        let mut thread = synthetic_thread(
            "aa01",
            &[("aa01", "1000", "Dana <dana@example.com>")],
        );
        for message in &mut thread.messages {
            message.label_ids = vec!["TRASH".to_string()];
        }
        let reader = Reader::new([(key.clone(), Ok(thread))]);
        let first = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &MailIndex::default(),
            &WatchJournal::load(dir.path()).unwrap(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert!(first.prepared[0].report_gone);
        WatchJournal::append_gone(dir.path(), "testhost", &key).unwrap();
        let second = prepare_test(
            &reader,
            dir.path(),
            std::slice::from_ref(&key),
            &MailIndex::default(),
            &WatchJournal::load(dir.path()).unwrap(),
            &NeverSavePolicy::default(),
            &BTreeMap::new(),
        );
        assert!(second.prepared.is_empty());
    }
}

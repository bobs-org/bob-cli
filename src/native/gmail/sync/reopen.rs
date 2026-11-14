//! Metadata-first preparation for counted activity on closed Gmail threads.
//!
//! This phase only discovers and prepares reply candidates. It does not
//! mutate the vault, append markers, or persist activity cursors; the later
//! transaction phase consumes [`ReopenBatch`] and settles its proposed state.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Duration, SecondsFormat, Utc};

use super::{
    archive_scan::{ArchiveScan, ArchiveThreadEvidence},
    enroll::EnrollmentFold,
    health::ReopenActivityState,
    retry::is_auth_abort,
    watch::is_trashed,
    watch_state::WatchJournal,
    ThreadKey,
};
use crate::native::{
    config::GmailBodyMode,
    gmail::{
        body::inert_single_line,
        note::{self, index::MailIndex},
        policy::NeverSavePolicy,
        resolve::{GmailRead, ResolvedIdentity},
        save::{self, PreparedSave},
        turn::{self, Net},
        GmailError,
    },
};

const ACTIVITY_PAGE_SIZE: u32 = 500;
const ACTIVITY_MAX_PAGES: usize = 10;
const ACTIVITY_OVERLAP_SECONDS: i64 = 15 * 60;
const ACTIVITY_BOOTSTRAP_DAYS: i64 = 14;

/// One reply whose mail note has been hydrated and is ready for local task
/// planning. `PreparedSave` deliberately keeps message bodies in memory only.
#[allow(dead_code)] // Consumed by the local reopen planner in the next phase.
pub(crate) struct PreparedReopenCandidate {
    pub(crate) key: ThreadKey,
    /// The exact newest counted message that caused the lift or bounce.
    pub(crate) message_id: String,
    pub(crate) message_at: DateTime<Utc>,
    /// Inert single-line sender text for the eventual reopen log.
    pub(crate) sender: String,
    /// True when the counted activity was a bounce rather than a reply.
    pub(crate) bounced: bool,
    /// Frozen effective-linker evidence for the local planner to recheck.
    pub(crate) evidence: ArchiveThreadEvidence,
    /// Body-bearing saved-note preparation; never serialize or log this.
    pub(crate) save: PreparedSave,
}

/// A body-free failure while preparing one eligible closed thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReopenFailure {
    pub(crate) key: ThreadKey,
    pub(crate) kind: String,
    pub(crate) message: String,
}

/// Prepared replies plus all state needed by the later transaction phase.
/// Cursor writes are intentionally deferred to live transaction settlement.
pub(crate) struct ReopenBatch {
    pub(crate) candidates: Vec<PreparedReopenCandidate>,
    pub(crate) failures: Vec<ReopenFailure>,
    pub(crate) deferred: BTreeSet<ThreadKey>,
    pub(crate) warnings: Vec<String>,
    pub(crate) search_complete: bool,
    pub(crate) pages_fetched: usize,
    pub(crate) aborted: bool,
    pub(crate) proposed_activity: ReopenActivityState,
}

struct ActivitySearch {
    ids: BTreeSet<String>,
    warnings: Vec<String>,
    complete: bool,
    pages_fetched: usize,
    aborted: bool,
}

/// The Gmail `after:` timestamp to query from. Missing or invalid state
/// bootstraps from 14 days before the fixed run-start instant.
pub(crate) fn activity_since(
    state: &ReopenActivityState,
    run_started_at: DateTime<Utc>,
) -> DateTime<Utc> {
    state
        .high_water_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|| {
            run_started_at - Duration::days(ACTIVITY_BOOTSTRAP_DAYS)
        })
}

/// One account-scoped, ids-only activity query with the indexing overlap.
pub(crate) fn activity_query(
    state: &ReopenActivityState,
    run_started_at: DateTime<Utc>,
) -> String {
    let after = activity_after(state, run_started_at);
    format!("after:{after} -from:me")
}

fn activity_after(
    state: &ReopenActivityState,
    run_started_at: DateTime<Utc>,
) -> i64 {
    activity_since(state, run_started_at)
        .timestamp()
        .saturating_sub(ACTIVITY_OVERLAP_SECONDS)
}

/// Eligible threads from this account's enrollment and a complete archive
/// scan. Archive scan's strict proof requires nonempty, positively closed
/// linkers and rejects open/unknown rows, unresolved merges, identity doubts,
/// damaged mail notes, and incomplete vault reads.
pub(crate) fn eligible_closed_threads(
    account: &str,
    enrollment: &EnrollmentFold,
    archive: &ArchiveScan,
) -> BTreeMap<String, ArchiveThreadEvidence> {
    if !archive.scan_complete() {
        return BTreeMap::new();
    }
    enrollment
        .threads()
        .filter(|key| key.is_account(account))
        .filter_map(|key| {
            let evidence = archive.evidence(key)?;
            archive
                .archivable(evidence)
                .then(|| (key.thread_id().to_string(), evidence.clone()))
        })
        .collect()
}

/// Discover, fetch, fold, and hydrate counted replies for eligible closed
/// threads. `reader` should be the run's shared `CoordinatedRead`, so every
/// page and metadata/hydration read uses the same budget.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_reopens<R: GmailRead>(
    reader: &R,
    account: &str,
    enrollment: &EnrollmentFold,
    archive: &ArchiveScan,
    activity: &ReopenActivityState,
    index: &MailIndex,
    journal: &WatchJournal,
    body_mode: GmailBodyMode,
    never_save: &NeverSavePolicy,
    run_started_at: DateTime<Utc>,
) -> ReopenBatch {
    let mut batch = ReopenBatch {
        candidates: Vec::new(),
        failures: Vec::new(),
        deferred: BTreeSet::new(),
        warnings: Vec::new(),
        search_complete: false,
        pages_fetched: 0,
        aborted: false,
        proposed_activity: normalized_activity(activity),
    };

    // An incomplete vault scan could hide an open linker. Do not search or
    // advance the cursor until a later run can prove the complete closed set.
    if !archive.scan_complete() {
        batch.warnings.push(
            "skipped closed-thread activity search because the archive scan was incomplete".to_string(),
        );
        return batch;
    }

    let eligible = eligible_closed_threads(account, enrollment, archive);
    let search = search_activity(reader, account, activity, run_started_at);
    batch.warnings.extend(search.warnings);
    batch.search_complete = search.complete;
    batch.pages_fetched = search.pages_fetched;
    batch.aborted = search.aborted;

    let eligible_ids: BTreeSet<_> = eligible.keys().cloned().collect();
    let old_retries = normalized_activity(activity).retry_thread_ids;
    let mut selected_ids: BTreeSet<String> =
        search.ids.intersection(&eligible_ids).cloned().collect();
    selected_ids.extend(old_retries.intersection(&eligible_ids).cloned());

    // A revoked grant stops all later reads. Existing retries remain queued;
    // the unchanged cursor will also rediscover any activity hits next run.
    if search.aborted {
        for thread_id in old_retries.intersection(&eligible_ids) {
            let key = ThreadKey::new(account, thread_id)
                .expect("eligible thread IDs were validated by the vault scan");
            batch.deferred.insert(key);
        }
        batch.proposed_activity.retry_thread_ids = old_retries;
        return batch;
    }

    let ordered: Vec<_> = selected_ids
        .iter()
        .filter_map(|thread_id| {
            eligible.get(thread_id).map(|evidence| {
                (
                    ThreadKey::new(account, thread_id)
                        .expect("archive scan thread IDs are valid"),
                    evidence.clone(),
                    old_retries.contains(thread_id),
                )
            })
        })
        .collect();
    let after = activity_after(activity, run_started_at);

    if search.complete {
        batch.proposed_activity.high_water_at =
            Some(run_started_at.to_rfc3339_opts(SecondsFormat::Secs, true));
        batch.proposed_activity.retry_thread_ids.clear();
    }

    for (position, (key, evidence, is_retry)) in ordered.iter().enumerate() {
        match prepare_one(
            reader, account, key, evidence, after, *is_retry, index, journal,
            body_mode, never_save,
        ) {
            PreparedOne::Candidate(candidate) => {
                batch.candidates.push(candidate);
                batch
                    .proposed_activity
                    .retry_thread_ids
                    .remove(key.thread_id());
            }
            PreparedOne::NoChange => {
                batch
                    .proposed_activity
                    .retry_thread_ids
                    .remove(key.thread_id());
            }
            PreparedOne::Gone => {
                batch
                    .proposed_activity
                    .retry_thread_ids
                    .remove(key.thread_id());
            }
            PreparedOne::Failure(failure, retry) => {
                if failure.kind == "unknown_turn" {
                    batch.warnings.push(format!(
                        "Gmail thread {} has an unknown reply turn: {}",
                        key.thread_id(),
                        failure.message
                    ));
                }
                if retry {
                    batch
                        .proposed_activity
                        .retry_thread_ids
                        .insert(key.thread_id().to_string());
                } else {
                    batch
                        .proposed_activity
                        .retry_thread_ids
                        .remove(key.thread_id());
                }
                batch.failures.push(failure);
            }
            PreparedOne::AuthAbort(failure) => {
                batch.failures.push(failure);
                batch.aborted = true;
                for (deferred_key, _, _) in &ordered[position..] {
                    batch.deferred.insert(deferred_key.clone());
                    batch
                        .proposed_activity
                        .retry_thread_ids
                        .insert(deferred_key.thread_id().to_string());
                }
                break;
            }
        }
    }

    batch
}

fn normalized_activity(activity: &ReopenActivityState) -> ReopenActivityState {
    let high_water_at = activity
        .high_water_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| {
            value
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        });
    let retry_thread_ids = activity
        .retry_thread_ids
        .iter()
        .filter_map(|thread_id| normalize_thread_id(thread_id))
        .collect();
    ReopenActivityState {
        high_water_at,
        retry_thread_ids,
    }
}

fn search_activity<R: GmailRead>(
    reader: &R,
    account: &str,
    activity: &ReopenActivityState,
    run_started_at: DateTime<Utc>,
) -> ActivitySearch {
    let query = activity_query(activity, run_started_at);
    let mut ids = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut invalid_ids = 0usize;
    let mut page: Option<String> = None;
    let mut seen_pages = HashSet::new();
    let mut pages_fetched = 0;

    loop {
        let response = match reader.search_page(
            account,
            &query,
            ACTIVITY_PAGE_SIZE,
            page.as_deref(),
        ) {
            Ok(response) => response,
            Err(error) => {
                let aborted = is_auth_abort(&error);
                warnings.push(if aborted {
                    format!(
                        "stopped closed-thread activity search when the grant was revoked or expired: {}: {}",
                        error.kind(), error.message()
                    )
                } else {
                    format!(
                        "closed-thread activity search failed; preserving its cursor: {}: {}",
                        error.kind(), error.message()
                    )
                });
                return ActivitySearch {
                    ids,
                    warnings,
                    complete: false,
                    pages_fetched,
                    aborted,
                };
            }
        };
        pages_fetched += 1;

        let Some(threads) =
            response.get("threads").and_then(|value| value.as_array())
        else {
            warnings.push(
                "closed-thread activity search returned no thread list; preserving its cursor".to_string(),
            );
            return ActivitySearch {
                ids,
                warnings,
                complete: false,
                pages_fetched,
                aborted: false,
            };
        };
        for thread in threads {
            match thread
                .get("id")
                .and_then(|value| value.as_str())
                .and_then(normalize_thread_id)
            {
                Some(thread_id) => {
                    ids.insert(thread_id);
                }
                None => invalid_ids += 1,
            }
        }
        let next = response
            .get("nextPageToken")
            .and_then(|value| value.as_str())
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        let Some(next) = next else {
            if invalid_ids > 0 {
                warnings.push(format!(
                    "skipped {invalid_ids} invalid thread IDs in closed-thread activity results"
                ));
            }
            return ActivitySearch {
                ids,
                warnings,
                complete: true,
                pages_fetched,
                aborted: false,
            };
        };
        if !seen_pages.insert(next.clone()) {
            warnings.push(
                "closed-thread activity search repeated a page token; preserving its cursor".to_string(),
            );
            return ActivitySearch {
                ids,
                warnings,
                complete: false,
                pages_fetched,
                aborted: false,
            };
        }
        if pages_fetched >= ACTIVITY_MAX_PAGES {
            warnings.push(format!(
                "closed-thread activity search exceeded {ACTIVITY_MAX_PAGES} pages; preserving its cursor"
            ));
            return ActivitySearch {
                ids,
                warnings,
                complete: false,
                pages_fetched,
                aborted: false,
            };
        }
        page = Some(next);
    }
}

fn normalize_thread_id(thread_id: &str) -> Option<String> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty()
        || thread_id.len() > 128
        || !thread_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(thread_id.to_ascii_lowercase())
}

enum PreparedOne {
    Candidate(PreparedReopenCandidate),
    NoChange,
    Gone,
    Failure(ReopenFailure, bool),
    AuthAbort(ReopenFailure),
}

#[allow(clippy::too_many_arguments)]
fn prepare_one<R: GmailRead>(
    reader: &R,
    account: &str,
    key: &ThreadKey,
    evidence: &ArchiveThreadEvidence,
    activity_after: i64,
    is_retry: bool,
    index: &MailIndex,
    journal: &WatchJournal,
    body_mode: GmailBodyMode,
    never_save: &NeverSavePolicy,
) -> PreparedOne {
    let thread = match reader.thread_metadata(key.account(), key.thread_id()) {
        Ok(thread) => thread,
        Err(error) if is_auth_abort(&error) => {
            return PreparedOne::AuthAbort(failure(key, &error));
        }
        Err(error) if error.kind() == "not_found" => {
            return PreparedOne::Gone;
        }
        Err(error) => return PreparedOne::Failure(failure(key, &error), true),
    };
    if normalize_thread_id(&thread.id).as_deref() != Some(key.thread_id())
        || thread.messages.iter().any(|message| {
            normalize_thread_id(&message.thread_id).as_deref()
                != Some(key.thread_id())
        })
    {
        return PreparedOne::Failure(
            ReopenFailure {
                key: key.clone(),
                kind: "unresolvable".into(),
                message: "Gmail returned a different thread identity".into(),
            },
            true,
        );
    }
    if is_trashed(&thread) {
        return PreparedOne::Gone;
    }

    let marker = journal.marker(key).map(|marker| turn::Marker {
        message_id: &marker.message_id,
        internal_date: marker.internal_date,
    });
    let folded = turn::fold(&thread.messages, key.account(), marker);
    match folded.net {
        Net::Unknown(reason) => {
            return PreparedOne::Failure(
                ReopenFailure {
                    key: key.clone(),
                    kind: "unknown_turn".into(),
                    message: reason.message().to_string(),
                },
                true,
            );
        }
        Net::Hold | Net::Nothing => return PreparedOne::NoChange,
        Net::Lift | Net::Bounced => {}
    }

    let Some(message_id) = folded.newest_counted_id else {
        return PreparedOne::Failure(
            ReopenFailure {
                key: key.clone(),
                kind: "unresolvable".into(),
                message: "counted reply has no message identity".into(),
            },
            true,
        );
    };
    let Some(message) = thread
        .messages
        .iter()
        .find(|message| message.id.eq_ignore_ascii_case(&message_id))
    else {
        return PreparedOne::Failure(
            ReopenFailure {
                key: key.clone(),
                kind: "unresolvable".into(),
                message: "counted reply metadata is missing from the thread"
                    .into(),
            },
            true,
        );
    };
    let Some(message_at) = message
        .internal_date
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
    else {
        return PreparedOne::Failure(
            ReopenFailure {
                key: key.clone(),
                kind: "unknown_turn".into(),
                message: "counted reply has no valid Gmail internalDate".into(),
            },
            true,
        );
    };
    // Gmail activity search returns thread IDs, so an ignored or automated
    // new message can surface a thread whose last counted reply is old. Do
    // not reinterpret that older reply as the recent trigger. Explicit retry
    // IDs bypass this fence because their preparation was already deferred.
    if !is_retry && message_at.timestamp() < activity_after {
        return PreparedOne::NoChange;
    }
    let sender = inert_single_line(&note::thread_who(message, account));
    let Some(entry) = index.by_identity.get(&key.index_key()) else {
        return PreparedOne::Failure(
            ReopenFailure {
                key: key.clone(),
                kind: "unresolvable".into(),
                message: "closed thread has no indexed mail note".into(),
            },
            true,
        );
    };
    let identity = ResolvedIdentity {
        account: key.account().to_string(),
        thread_id: key.thread_id().to_string(),
        known_path: Some(entry.path.clone()),
    };
    let save = match save::prepare_sync_candidate_from_thread(
        reader, account, identity, thread, body_mode, never_save,
    ) {
        Ok(save) => save,
        Err(error) if is_auth_abort(&error) => {
            return PreparedOne::AuthAbort(failure(key, &error));
        }
        Err(error) => {
            let retry = error.kind() != "never_save";
            return PreparedOne::Failure(failure(key, &error), retry);
        }
    };

    PreparedOne::Candidate(PreparedReopenCandidate {
        key: key.clone(),
        message_id,
        message_at,
        sender,
        bounced: matches!(folded.net, Net::Bounced),
        evidence: evidence.clone(),
        save,
    })
}

fn failure(key: &ThreadKey, error: &GmailError) -> ReopenFailure {
    ReopenFailure {
        key: key.clone(),
        kind: error.kind().to_string(),
        message: error.message().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::BTreeMap};

    use super::*;
    use crate::native::gmail::{
        gog::GmailLabel,
        model::{GmailMessage, GmailThread, Header, MimeBody, MimePart},
        note::index::MailIndexEntry,
        resolve::GmailRead,
        sync::{
            archive_scan::{ArchiveIssue, ArchiveLinkTask},
            scan::LinkPlacement,
        },
    };

    const ACCOUNT: &str = "me@example.com";
    const THREAD: &str = "aabbccdd";

    fn key(thread_id: &str) -> ThreadKey {
        ThreadKey::new(ACCOUNT, thread_id).unwrap()
    }

    fn evidence(thread_id: &str, status: char) -> ArchiveThreadEvidence {
        let task_class =
            super::super::archive_scan::classify_archive_status(status);
        ArchiveThreadEvidence {
            key: key(thread_id),
            note: format!("mail/{thread_id}.md"),
            tasks: vec![ArchiveLinkTask {
                path: "job.md".into(),
                line: 2,
                text: format!("- [{status}] #task [[mail/{thread_id}]]"),
                block_id: Some("closed-task".into()),
                status,
                class: task_class,
                placement: LinkPlacement::TaskLine,
                in_done: false,
                merge_retired: false,
                via_merge: false,
                from_duplicate_note: false,
            }],
            saved_ids: vec!["saved-message".into()],
            note_fingerprint: Some("sha256:fixture".into()),
            merge_unresolved: None,
            identity_uncertain: false,
            issues: Vec::new(),
        }
    }

    fn enrollment(thread_id: &str) -> EnrollmentFold {
        EnrollmentFold::test_with_keys([key(thread_id)])
    }

    fn empty_archive(
        thread_id: &str,
        status: char,
        complete: bool,
    ) -> ArchiveScan {
        ArchiveScan::test_scan(evidence(thread_id, status), complete)
    }

    fn started() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[derive(Default)]
    struct MockRead {
        pages: Vec<serde_json::Value>,
        search_error: Option<(String, String)>,
        attachment_error: Option<(String, String)>,
        calls: RefCell<Vec<(String, u32, Option<String>)>>,
        threads: BTreeMap<String, GmailThread>,
        thread_errors: BTreeMap<String, (String, String)>,
        metadata_calls: RefCell<Vec<String>>,
    }

    impl GmailRead for MockRead {
        fn attachment_bytes(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, GmailError> {
            match &self.attachment_error {
                Some((kind, message)) => {
                    Err(GmailError::runtime(kind, message.clone()))
                }
                None => Ok(Vec::new()),
            }
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
            Ok(serde_json::json!({}))
        }

        fn search_page(
            &self,
            _: &str,
            query: &str,
            max: u32,
            page: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            self.calls.borrow_mut().push((
                query.to_string(),
                max,
                page.map(str::to_string),
            ));
            if let Some((kind, message)) = &self.search_error {
                return Err(GmailError::runtime(kind, message.clone()));
            }
            let index = self.calls.borrow().len() - 1;
            Ok(self
                .pages
                .get(index)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"threads": []})))
        }

        fn labels_list(&self, _: &str) -> Result<Vec<GmailLabel>, GmailError> {
            Ok(Vec::new())
        }

        fn thread_metadata(
            &self,
            _: &str,
            thread_id: &str,
        ) -> Result<GmailThread, GmailError> {
            self.metadata_calls.borrow_mut().push(thread_id.to_string());
            if let Some((kind, message)) = self.thread_errors.get(thread_id) {
                return Err(GmailError::runtime(kind, message.clone()));
            }
            self.threads.get(thread_id).cloned().ok_or_else(|| {
                GmailError::runtime("not_found", "thread is gone".into())
            })
        }
    }

    fn indexed_mail(thread_id: &str) -> MailIndex {
        let mut index = MailIndex::default();
        index.by_identity.insert(
            (ACCOUNT.to_string(), thread_id.to_string()),
            MailIndexEntry {
                account: ACCOUNT.to_string(),
                thread_id: thread_id.to_string(),
                path: format!("mail/{thread_id}.md"),
                title: "A subject".into(),
                who: "Alice".into(),
                label: "✉️ A subject · Alice".into(),
                started: String::new(),
                updated: String::new(),
                messages: 1,
                turn: None,
                turn_since: None,
            },
        );
        index
    }

    fn incoming_message(
        thread_id: &str,
        message_id: &str,
        from: &str,
        internal_date: i64,
        labels: &[&str],
        extra_headers: &[(&str, &str)],
    ) -> GmailMessage {
        let mut headers = vec![Header {
            name: "From".into(),
            value: from.into(),
        }];
        headers.extend(extra_headers.iter().map(|(name, value)| Header {
            name: (*name).into(),
            value: (*value).into(),
        }));
        GmailMessage {
            id: message_id.into(),
            thread_id: thread_id.into(),
            label_ids: labels.iter().map(|label| (*label).into()).collect(),
            internal_date: Some(internal_date.to_string()),
            snippet: None,
            headers,
            payload: None,
        }
    }

    fn thread(thread_id: &str, messages: Vec<GmailMessage>) -> GmailThread {
        GmailThread {
            id: thread_id.into(),
            history_id: None,
            messages,
        }
    }

    fn search_page(ids: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "threads": ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>()
        })
    }

    fn run_batch(
        reader: &MockRead,
        thread_id: &str,
        activity: &ReopenActivityState,
        archive_complete: bool,
    ) -> ReopenBatch {
        let enrollment = enrollment(thread_id);
        let archive = empty_archive(thread_id, 'x', archive_complete);
        prepare_reopens(
            reader,
            ACCOUNT,
            &enrollment,
            &archive,
            activity,
            &indexed_mail(thread_id),
            &WatchJournal::default(),
            GmailBodyMode::Headers,
            &NeverSavePolicy::default(),
            started(),
        )
    }

    #[test]
    fn bootstrap_and_saved_cursor_use_one_fifteen_minute_overlap() {
        let bootstrap = ReopenActivityState::default();
        let since = started() - Duration::days(14);
        assert_eq!(activity_since(&bootstrap, started()), since);
        assert_eq!(
            activity_query(&bootstrap, started()),
            format!("after:{} -from:me", since.timestamp() - 900)
        );

        let saved = ReopenActivityState {
            high_water_at: Some("2026-11-12T12:34:56-05:00".into()),
            retry_thread_ids: BTreeSet::new(),
        };
        let saved_since =
            DateTime::parse_from_rfc3339("2026-11-12T12:34:56-05:00")
                .unwrap()
                .with_timezone(&Utc);
        assert_eq!(activity_since(&saved, started()), saved_since);
        assert_eq!(
            activity_query(&saved, started()),
            format!("after:{} -from:me", saved_since.timestamp() - 900)
        );
    }

    #[test]
    fn activity_search_deduplicates_pages_and_stops_at_exact_tenth_page() {
        let mut pages = Vec::new();
        for number in 0..10 {
            let next = (number < 9).then(|| format!("p{}", number + 1));
            pages.push(serde_json::json!({
                "threads": if number == 0 {
                    vec![serde_json::json!({"id": "AABB"}), serde_json::json!({"id": "bad-id"})]
                } else if number == 1 {
                    vec![serde_json::json!({"id": "aabb"}), serde_json::json!({"id": "ccdd"})]
                } else {
                    Vec::new()
                },
                "nextPageToken": next
            }));
        }
        let reader = MockRead {
            pages,
            ..Default::default()
        };
        let result = search_activity(
            &reader,
            ACCOUNT,
            &ReopenActivityState::default(),
            started(),
        );
        assert!(result.complete);
        assert_eq!(result.pages_fetched, 10);
        assert_eq!(result.ids, BTreeSet::from(["aabb".into(), "ccdd".into()]));
        assert_eq!(reader.calls.borrow().len(), 10);
        assert!(reader.calls.borrow().iter().all(|(query, max, _)| {
            query == &activity_query(&ReopenActivityState::default(), started())
                && *max == ACTIVITY_PAGE_SIZE
        }));

        let mut more_pages = Vec::new();
        for number in 0..10 {
            more_pages.push(serde_json::json!({
                "threads": [],
                "nextPageToken": format!("p{}", number + 1)
            }));
        }
        let reader = MockRead {
            pages: more_pages,
            ..Default::default()
        };
        let result = search_activity(
            &reader,
            ACCOUNT,
            &ReopenActivityState::default(),
            started(),
        );
        assert!(!result.complete, "a next page after page 10 must fail");
        assert_eq!(result.pages_fetched, 10);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("10 pages")));
    }

    #[test]
    fn only_complete_positive_closed_evidence_is_eligible() {
        let enrollment = enrollment(THREAD);
        assert!(eligible_closed_threads(
            ACCOUNT,
            &enrollment,
            &empty_archive(THREAD, 'x', true)
        )
        .contains_key(THREAD));
        for status in [' ', '?', 'z'] {
            assert!(eligible_closed_threads(
                ACCOUNT,
                &enrollment,
                &empty_archive(THREAD, status, true)
            )
            .is_empty());
        }
        assert!(eligible_closed_threads(
            ACCOUNT,
            &enrollment,
            &empty_archive(THREAD, 'x', false)
        )
        .is_empty());
        assert!(eligible_closed_threads(
            "other@example.com",
            &enrollment,
            &empty_archive(THREAD, 'x', true)
        )
        .is_empty());

        let mut ambiguous = evidence(THREAD, 'x');
        ambiguous.identity_uncertain = true;
        assert!(eligible_closed_threads(
            ACCOUNT,
            &EnrollmentFold::test_with_keys([key(THREAD)]),
            &ArchiveScan::test_scan(ambiguous, true)
        )
        .is_empty());
        let mut unresolved = evidence(THREAD, 'x');
        unresolved.merge_unresolved = Some(
            super::super::archive_scan::MergeUnresolved {
                source_path: "job.md".into(),
                source_line: 2,
                source_block_id: Some("closed-task".into()),
                target: "[[job#^missing]]".into(),
                kind: super::super::archive_scan::MergeUnresolvedKind::MissingBlock,
            },
        );
        assert!(eligible_closed_threads(
            ACCOUNT,
            &EnrollmentFold::test_with_keys([key(THREAD)]),
            &ArchiveScan::test_scan(unresolved, true)
        )
        .is_empty());
        let mut damaged = evidence(THREAD, 'x');
        damaged.issues.push(ArchiveIssue::DamagedMailNote {
            note: damaged.note.clone(),
        });
        assert!(eligible_closed_threads(
            ACCOUNT,
            &EnrollmentFold::test_with_keys([key(THREAD)]),
            &ArchiveScan::test_scan(damaged, true)
        )
        .is_empty());
    }

    #[test]
    fn retries_are_fetched_even_when_the_activity_search_misses_them() {
        let reader = MockRead {
            pages: vec![search_page(&[])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(THREAD, Vec::new()),
            )]),
            ..Default::default()
        };
        let state = ReopenActivityState {
            high_water_at: Some("2026-11-13T00:00:00Z".into()),
            retry_thread_ids: BTreeSet::from([THREAD.into()]),
        };
        let batch = run_batch(&reader, THREAD, &state, true);
        assert_eq!(reader.metadata_calls.borrow().as_slice(), [THREAD]);
        assert!(batch.candidates.is_empty());
        assert!(batch.proposed_activity.retry_thread_ids.is_empty());
        assert!(batch.search_complete);
        assert_eq!(
            batch.proposed_activity.high_water_at.as_deref(),
            Some("2026-11-14T12:00:00Z")
        );
    }

    #[test]
    fn search_auth_failure_preserves_cursor_and_existing_retries() {
        let reader = MockRead {
            search_error: Some((
                "invalid_grant".into(),
                "grant expired".into(),
            )),
            ..Default::default()
        };
        let state = ReopenActivityState {
            high_water_at: Some("2026-11-13T00:00:00Z".into()),
            retry_thread_ids: BTreeSet::from([THREAD.into()]),
        };
        let batch = run_batch(&reader, THREAD, &state, true);
        assert!(batch.aborted);
        assert!(!batch.search_complete);
        assert_eq!(batch.proposed_activity.high_water_at, state.high_water_at);
        assert_eq!(
            batch.proposed_activity.retry_thread_ids,
            state.retry_thread_ids
        );
        assert!(reader.metadata_calls.borrow().is_empty());
    }

    #[test]
    fn failed_search_and_page_overflow_preserve_the_cursor() {
        let cursor = "2026-11-13T00:00:00Z";
        let activity = ReopenActivityState {
            high_water_at: Some(cursor.into()),
            retry_thread_ids: BTreeSet::new(),
        };
        let failed = MockRead {
            search_error: Some(("network".into(), "offline".into())),
            ..Default::default()
        };
        let batch = run_batch(&failed, THREAD, &activity, true);
        assert_eq!(
            batch.proposed_activity.high_water_at.as_deref(),
            Some(cursor)
        );
        assert!(!batch.search_complete);

        let pages = (0..ACTIVITY_MAX_PAGES)
            .map(|number| {
                serde_json::json!({
                    "threads": [],
                    "nextPageToken": format!("p{}", number + 1)
                })
            })
            .collect();
        let overflow = MockRead {
            pages,
            ..Default::default()
        };
        let batch = run_batch(&overflow, THREAD, &activity, true);
        assert_eq!(batch.pages_fetched, ACTIVITY_MAX_PAGES);
        assert_eq!(
            batch.proposed_activity.high_water_at.as_deref(),
            Some(cursor)
        );
        assert!(!batch.search_complete);
    }

    #[test]
    fn missing_trashed_and_mismatched_threads_follow_watch_identity_rules() {
        let missing = MockRead {
            pages: vec![search_page(&[THREAD])],
            thread_errors: BTreeMap::from([(
                THREAD.into(),
                ("not_found".into(), "gone".into()),
            )]),
            ..Default::default()
        };
        let batch =
            run_batch(&missing, THREAD, &ReopenActivityState::default(), true);
        assert!(batch.candidates.is_empty());
        assert!(batch.failures.is_empty());

        let trashed = MockRead {
            pages: vec![search_page(&[THREAD])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(
                    THREAD,
                    vec![incoming_message(
                        THREAD,
                        "reply1",
                        "Alice <alice@example.com>",
                        started().timestamp_millis(),
                        &["TRASH"],
                        &[],
                    )],
                ),
            )]),
            ..Default::default()
        };
        let batch =
            run_batch(&trashed, THREAD, &ReopenActivityState::default(), true);
        assert!(batch.candidates.is_empty());
        assert!(batch.failures.is_empty());

        let mismatch = MockRead {
            pages: vec![search_page(&[THREAD])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread("11223344", Vec::new()),
            )]),
            ..Default::default()
        };
        let batch =
            run_batch(&mismatch, THREAD, &ReopenActivityState::default(), true);
        assert_eq!(batch.failures[0].kind, "unresolvable");
        assert_eq!(
            batch.proposed_activity.retry_thread_ids,
            BTreeSet::from([THREAD.into()])
        );
    }

    #[test]
    fn only_recent_lift_or_bounce_hydrates_a_candidate() {
        let now = started().timestamp_millis();
        let reply = incoming_message(
            THREAD,
            "reply1",
            "Alice <alice@example.com>",
            now,
            &[],
            &[],
        );
        let reader = MockRead {
            pages: vec![search_page(&[THREAD])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(THREAD, vec![reply]),
            )]),
            ..Default::default()
        };
        let batch = prepare_reopens(
            &reader,
            ACCOUNT,
            &enrollment(THREAD),
            &empty_archive(THREAD, 'x', true),
            &ReopenActivityState::default(),
            &indexed_mail(THREAD),
            &WatchJournal::default(),
            GmailBodyMode::Headers,
            &NeverSavePolicy::default(),
            started(),
        );
        assert_eq!(batch.candidates.len(), 1);
        assert_eq!(batch.candidates[0].message_id, "reply1");
        assert_eq!(batch.candidates[0].message_at.timestamp_millis(), now);
        assert!(!batch.candidates[0].sender.contains('\n'));

        let bounce = incoming_message(
            THREAD,
            "bounce1",
            "Mail Delivery Subsystem <mailer-daemon@googlemail.com>",
            now,
            &[],
            &[],
        );
        let reader = MockRead {
            pages: vec![search_page(&[THREAD])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(THREAD, vec![bounce]),
            )]),
            ..Default::default()
        };
        let batch = prepare_reopens(
            &reader,
            ACCOUNT,
            &enrollment(THREAD),
            &empty_archive(THREAD, 'x', true),
            &ReopenActivityState::default(),
            &indexed_mail(THREAD),
            &WatchJournal::default(),
            GmailBodyMode::Headers,
            &NeverSavePolicy::default(),
            started(),
        );
        assert_eq!(batch.candidates.len(), 1);
        assert_eq!(batch.candidates[0].message_id, "bounce1");

        let blocked_reply = incoming_message(
            THREAD,
            "blocked-reply",
            "Blocked <blocked@example.com>",
            now,
            &[],
            &[],
        );
        let reader = MockRead {
            pages: vec![search_page(&[THREAD])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(THREAD, vec![blocked_reply]),
            )]),
            ..Default::default()
        };
        let batch = prepare_reopens(
            &reader,
            ACCOUNT,
            &enrollment(THREAD),
            &empty_archive(THREAD, 'x', true),
            &ReopenActivityState::default(),
            &indexed_mail(THREAD),
            &WatchJournal::default(),
            GmailBodyMode::Headers,
            &NeverSavePolicy::for_test(&["blocked@example.com"], &[]),
            started(),
        );
        assert!(batch.candidates.is_empty());
        assert_eq!(batch.failures[0].kind, "never_save");
        assert!(batch.proposed_activity.retry_thread_ids.is_empty());

        let mut attachment_reply = incoming_message(
            THREAD,
            "attachment-reply",
            "Alice <alice@example.com>",
            now,
            &[],
            &[],
        );
        attachment_reply.payload = Some(MimePart {
            part_id: None,
            mime_type: "text/plain".into(),
            filename: None,
            headers: Vec::new(),
            body: Some(MimeBody {
                size: Some(5),
                data: None,
                attachment_id: Some("body-part".into()),
            }),
            parts: Vec::new(),
        });
        let reader = MockRead {
            pages: vec![search_page(&[THREAD])],
            attachment_error: Some(("network".into(), "offline".into())),
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(THREAD, vec![attachment_reply]),
            )]),
            ..Default::default()
        };
        let batch = prepare_reopens(
            &reader,
            ACCOUNT,
            &enrollment(THREAD),
            &empty_archive(THREAD, 'x', true),
            &ReopenActivityState::default(),
            &indexed_mail(THREAD),
            &WatchJournal::default(),
            GmailBodyMode::Full,
            &NeverSavePolicy::default(),
            started(),
        );
        assert!(batch.candidates.is_empty());
        assert_eq!(batch.failures[0].kind, "network");
        assert_eq!(
            batch.proposed_activity.retry_thread_ids,
            BTreeSet::from([THREAD.into()])
        );

        for message in [
            incoming_message(
                THREAD,
                "sent",
                "Me <me@example.com>",
                now,
                &["SENT"],
                &[("To", "Alice <alice@example.com>")],
            ),
            incoming_message(
                THREAD,
                "draft",
                "Alice <alice@example.com>",
                now,
                &["DRAFT"],
                &[],
            ),
            incoming_message(
                THREAD,
                "auto",
                "Robot <robot@example.com>",
                now,
                &[],
                &[("Precedence", "auto_reply")],
            ),
        ] {
            let reader = MockRead {
                pages: vec![search_page(&[THREAD])],
                threads: BTreeMap::from([(
                    THREAD.into(),
                    thread(THREAD, vec![message]),
                )]),
                ..Default::default()
            };
            let batch = run_batch(
                &reader,
                THREAD,
                &ReopenActivityState::default(),
                true,
            );
            assert!(batch.candidates.is_empty());
            assert!(batch.failures.is_empty());
        }

        let old_reply = incoming_message(
            THREAD,
            "old-reply",
            "Alice <alice@example.com>",
            (started() - Duration::days(4)).timestamp_millis(),
            &[],
            &[],
        );
        let new_auto = incoming_message(
            THREAD,
            "auto-new",
            "Robot <robot@example.com>",
            started().timestamp_millis(),
            &[],
            &[("Precedence", "auto_reply")],
        );
        let activity = ReopenActivityState {
            high_water_at: Some(
                (started() - Duration::days(2))
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
            ),
            retry_thread_ids: BTreeSet::new(),
        };
        let reader = MockRead {
            pages: vec![search_page(&[THREAD])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(THREAD, vec![old_reply, new_auto]),
            )]),
            ..Default::default()
        };
        let batch = run_batch(&reader, THREAD, &activity, true);
        assert!(batch.candidates.is_empty());
        assert!(batch.failures.is_empty());

        let unknown = incoming_message(
            THREAD,
            "unknown",
            "",
            started().timestamp_millis(),
            &[],
            &[],
        );
        let reader = MockRead {
            pages: vec![search_page(&[THREAD])],
            threads: BTreeMap::from([(
                THREAD.into(),
                thread(THREAD, vec![unknown]),
            )]),
            ..Default::default()
        };
        let batch =
            run_batch(&reader, THREAD, &ReopenActivityState::default(), true);
        assert!(batch.candidates.is_empty());
        assert_eq!(batch.failures[0].kind, "unknown_turn");
        assert!(batch
            .warnings
            .iter()
            .any(|warning| warning.contains("unknown reply turn")));
        assert_eq!(
            batch.proposed_activity.retry_thread_ids,
            BTreeSet::from([THREAD.into()])
        );
    }
}

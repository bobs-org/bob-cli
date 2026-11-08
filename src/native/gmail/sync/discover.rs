//! Paged ids-only discovery for `bob gmail sync`.
//!
//! One run resolves config, the writer pin, gog, and the account, then
//! discovers with paged `label:<enrollment label>` `threads.list` reads
//! through the last page. Already enrolled ids need no candidate metadata
//! or hydration; unseen ids proceed in stable discovery order. Local
//! adoption from the
//! vault scan is independent of search membership so a lost journal append
//! is repaired even after the label is gone.
//!
//! `-l` caps new filings only (default 50, 0 through 500). Watchers and
//! adoptions never consume filing slots, and route-problem rows count as
//! filings. At 0, the run skips new-candidate search, fetch, and filing
//! while still scanning, adopting, and reporting watchers normally.

use std::collections::{BTreeSet, HashSet};

use super::{
    enroll::EnrollmentFold,
    retry::{is_auth_abort, should_retry},
    ThreadKey,
};
use crate::native::gmail::{resolve::GmailRead, GmailError};

/// Hard safety cap for one discovery run.
pub(crate) const MAX_DISCOVERY_IDS: usize = 5_000;
/// Thread ids requested per search page.
pub(crate) const PAGE_SIZE: u32 = 500;
/// Largest `-l` value: at most 500 new filings per run.
pub(crate) const MAX_LIMIT: u32 = 500;
/// Default `-l` when the flag is absent.
#[allow(dead_code)] // bob-cli-in.4 wires this shared default into the CLI.
pub(crate) const DEFAULT_LIMIT: u32 = 50;

/// Validate `-l`: 0 through 500. Zero means watch only.
pub(crate) fn validate_limit(limit: u32) -> Result<u32, GmailError> {
    if limit > MAX_LIMIT {
        return Err(GmailError::setup(
            "limit",
            format!("sync limit must be at most {MAX_LIMIT}"),
        ));
    }
    Ok(limit)
}

/// Gmail `label:` query for the enrollment label, with pull's escaping.
///
/// A label with whitespace is quoted and backslash/quote-escaped; other
/// labels pass through as `label:<label>`.
pub(crate) fn label_query(label: &str) -> String {
    if label.bytes().any(|byte| byte.is_ascii_whitespace()) {
        format!(
            "label:\"{}\"",
            label.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("label:{label}")
    }
}

/// Ids-only discovery result in stable first-seen order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DiscoveryOutcome {
    /// Deduplicated, validated, lowercased thread ids in search order.
    pub(crate) ids: Vec<String>,
    /// Body-free warnings (invalid ids skipped, cap, repeated token).
    pub(crate) warnings: Vec<String>,
    /// Whether the 5,000-id cap stopped paging with more results remaining.
    pub(crate) capped: bool,
    /// Whether transient search exhaustion deferred later pages.
    pub(crate) deferred: bool,
    /// Whether a non-auth search failure left the run in failure state
    /// without discarding already-discovered ids.
    pub(crate) failed: bool,
    /// Search pages fetched.
    pub(crate) pages_fetched: usize,
}

/// Discover thread ids with paged `threads.list` reads to the last page.
///
/// Deduplicates while preserving first-seen order, skips invalid ids with a
/// bounded warning, stops at [`MAX_DISCOVERY_IDS`] with a cap warning only
/// when more results remain, and stops on a repeated page token instead of
/// looping. Callers pass a [`super::retry::CoordinatedRead`] so every page
/// is paced and retried; a non-auth search failure keeps already-discovered
/// ids (including an empty first page) as a body-free failure on the
/// outcome so local adoption and recovery still run. Only a revoked or
/// expired grant returns `Err`. Returns the raw ids; callers join with
/// enrollment (see [`select_candidates`]) and never hydrate enrolled ids.
pub(crate) fn discover_ids<R: GmailRead>(
    reader: &R,
    account: &str,
    label: &str,
) -> Result<DiscoveryOutcome, GmailError> {
    let query = label_query(label);
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut ordered: Vec<String> = Vec::new();
    let mut warnings = Vec::new();
    let mut invalid = 0usize;
    let mut seen_tokens: BTreeSet<String> = BTreeSet::new();
    let mut page: Option<String> = None;
    let mut pages_fetched = 0usize;
    let mut capped = false;
    let mut deferred_search = false;
    let mut failed = false;

    loop {
        let response = match reader.search_page(
            account,
            &query,
            PAGE_SIZE,
            page.as_deref(),
        ) {
            Ok(response) => response,
            Err(error) if is_auth_abort(&error) => return Err(error),
            Err(error)
                if should_retry(&error) || error.kind() == "deferred" =>
            {
                warnings.push(format!(
                    "deferred remaining Gmail search pages when the retry budget ran out: {}: {}",
                    error.kind(),
                    error.message()
                ));
                deferred_search = true;
                failed = true;
                break;
            }
            Err(error) => {
                warnings.push(format!(
                    "Gmail search failed: {}: {}",
                    error.kind(),
                    error.message()
                ));
                failed = true;
                break;
            }
        };
        pages_fetched += 1;

        let threads =
            response.get("threads").and_then(|value| value.as_array());
        match threads {
            Some(threads) => {
                let mut cap_has_more = false;
                for (thread_index, thread) in threads.iter().enumerate() {
                    let Some(id) =
                        thread.get("id").and_then(|value| value.as_str())
                    else {
                        invalid += 1;
                        continue;
                    };
                    let trimmed = id.trim();
                    if trimmed.is_empty()
                        || trimmed.len() > 128
                        || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
                    {
                        invalid += 1;
                        continue;
                    }
                    let normalized = trimmed.to_ascii_lowercase();
                    if seen_ids.insert(normalized.clone()) {
                        ordered.push(normalized);
                        if ordered.len() >= MAX_DISCOVERY_IDS {
                            capped = true;
                            // More results remain when this page still holds
                            // unprocessed threads after the cap.
                            cap_has_more = thread_index + 1 < threads.len();
                            break;
                        }
                    }
                }
                if capped {
                    // A next page token also means more results remain
                    // beyond this page.
                    let has_next = matches!(
                        response.get("nextPageToken"),
                        Some(serde_json::Value::String(token))
                            if !token.is_empty()
                    );
                    if has_next {
                        cap_has_more = true;
                    }
                    if cap_has_more {
                        warnings.push(format!(
                            "stopped at the {MAX_DISCOVERY_IDS} thread safety cap; more results remain"
                        ));
                    }
                }
            }
            None => {
                // The transport inserts `"threads": []` when Gmail omits
                // the key, so a present non-array value is a malformed
                // page. Keep ids already discovered and fail the run
                // instead of aborting adoption and recovery.
                if response.get("nextPageToken").is_some()
                    || response.get("resultSizeEstimate").is_some()
                {
                    warnings.push(
                        "Gmail search failed: bad_json: gog label search returned no thread list"
                            .to_string(),
                    );
                    failed = true;
                }
                break;
            }
        }

        if capped {
            break;
        }

        let next = match response.get("nextPageToken") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(token)) if token.is_empty() => None,
            Some(serde_json::Value::String(token)) => Some(token.clone()),
            Some(_) => {
                warnings.push(
                    "Gmail search failed: bad_json: gog returned an invalid Gmail search page token"
                        .to_string(),
                );
                failed = true;
                break;
            }
        };
        let Some(next) = next else {
            break;
        };
        if !seen_tokens.insert(next.clone()) {
            warnings.push(
                "stopped paging on a repeated Gmail search page token"
                    .to_string(),
            );
            break;
        }
        // Guard against a runaway server that mints fresh tokens forever:
        // the unique-id cap bounds useful work, and 100 pages bound empty
        // or single-id pages well beyond the 10 full pages 5,000 ids need.
        if pages_fetched >= 100 {
            warnings.push(
                "stopped paging after 100 Gmail search pages".to_string(),
            );
            break;
        }
        page = Some(next);
    }

    if invalid > 0 {
        warnings.push(format!("skipped {invalid} invalid Gmail thread ids"));
    }
    Ok(DiscoveryOutcome {
        ids: ordered,
        warnings,
        capped,
        deferred: deferred_search,
        failed,
        pages_fetched,
    })
}

/// Select at most `limit` unseen filing candidates in discovery order.
///
/// Already enrolled ids and locally adoptable threads need no candidate
/// metadata and never consume filing slots, so they are filtered before the
/// limit applies. Callers pass the already computed adoption keys (from
/// [`super::enroll::Membership::adoptions`]) so discovery order stays stable
/// and adoptable threads never receive metadata or body fetches. The
/// returned keys are account-qualified ([`ThreadKey`]) for per-account
/// exclusion: ids from another account's search never match this fold. A
/// zero limit selects nothing so the run skips search, fetch, and filing
/// while watchers and adoptions proceed normally.
pub(crate) fn select_candidates(
    ids: &[String],
    account: &str,
    fold: &EnrollmentFold,
    adopted: &std::collections::BTreeSet<ThreadKey>,
    limit: u32,
) -> Result<Vec<ThreadKey>, GmailError> {
    validate_limit(limit)?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut selected = Vec::new();
    for id in ids {
        let Some(key) = ThreadKey::new(account, id) else {
            continue;
        };
        if fold.is_enrolled(&key) {
            continue;
        }
        if adopted.contains(&key) {
            continue;
        }
        selected.push(key);
        if selected.len() >= limit as usize {
            break;
        }
    }
    Ok(selected)
}

/// One metadata fetch result: successes keep their threads, failures keep
/// their body-free errors, and deferred ids catch up next run.
pub(crate) struct FetchOutcome {
    /// Threads fetched in the requested key order.
    pub(crate) successes:
        Vec<(ThreadKey, crate::native::gmail::model::GmailThread)>,
    /// Per-thread failures in the requested key order (isolation).
    pub(crate) failures: Vec<(ThreadKey, GmailError)>,
    /// Body-free warnings, including the auth-abort note when set.
    pub(crate) warnings: Vec<String>,
    /// Unprocessed keys when the shared budget exhausted.
    pub(crate) deferred: Vec<ThreadKey>,
    /// The revoked/expired grant that aborted the batch, if any.
    pub(crate) aborted: Option<GmailError>,
}

/// Fetch one label list.
///
/// Called once per run; the caller maps the union of every message's label
/// ids through this response and never routes by a label-name search.
/// Callers pass a [`super::retry::CoordinatedRead`] so this read shares the
/// run's pacing slot and wait budget.
pub(crate) fn fetch_labels_once<R: GmailRead>(
    reader: &R,
    account: &str,
) -> Result<Vec<crate::native::gmail::gog::GmailLabel>, GmailError> {
    reader.labels_list(account)
}

/// Fetch candidate metadata with isolation.
///
/// Callers pass a [`super::retry::CoordinatedRead`] so requests share the
/// run's pacing slot and wait budget. A revoked or expired grant aborts
/// the batch before any vault mutation: successes so far are retained, the
/// rest defer, and `aborted` carries the grant error. Permanent per-thread
/// failures are recorded with the thread and the batch continues; budget
/// exhaustion defers this key and every remaining key for the next run.
pub(crate) fn fetch_candidate_metadata<R: GmailRead>(
    reader: &R,
    keys: &[ThreadKey],
) -> FetchOutcome {
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    let mut deferred = Vec::new();
    let mut aborted = None;

    for (index, key) in keys.iter().enumerate() {
        match reader.thread_metadata(key.account(), key.thread_id()) {
            Ok(thread) => successes.push((key.clone(), thread)),
            Err(error) => {
                if is_auth_abort(&error) {
                    warnings.push(format!(
                        "stopped Gmail fetches when the grant was revoked or expired: {}: {}",
                        error.kind(),
                        error.message()
                    ));
                    aborted = Some(error);
                    deferred.extend(keys[index + 1..].iter().cloned());
                    break;
                }
                if should_retry(&error) || error.kind() == "deferred" {
                    deferred.push(key.clone());
                    deferred.extend(keys[index + 1..].iter().cloned());
                    warnings.push(format!(
                        "deferred remaining Gmail fetches when the retry budget ran out: {}: {}",
                        error.kind(),
                        error.message()
                    ));
                    break;
                }
                warnings.push(format!(
                    "skipped Gmail thread {}: {}: {}",
                    key.thread_id(),
                    error.kind(),
                    error.message()
                ));
                failures.push((key.clone(), error));
            }
        }
    }

    FetchOutcome {
        successes,
        failures,
        warnings,
        deferred,
        aborted,
    }
}

#[cfg(test)]
mod tests {
    use super::super::retry::{
        Clock, CoordinatedRead, SharedBudget, Sleeper, MIN_GAP,
    };
    use super::*;
    use std::{cell::RefCell, collections::VecDeque, time::Duration};

    struct MockRead {
        pages: RefCell<VecDeque<serde_json::Value>>,
        page_calls: RefCell<Vec<Option<String>>>,
        page_error: Option<GmailError>,
        metadata_queue: RefCell<
            VecDeque<
                Result<crate::native::gmail::model::GmailThread, GmailError>,
            >,
        >,
    }

    impl MockRead {
        fn pages(values: Vec<serde_json::Value>) -> Self {
            Self {
                pages: RefCell::new(values.into()),
                page_calls: RefCell::new(Vec::new()),
                page_error: None,
                metadata_queue: RefCell::new(VecDeque::new()),
            }
        }

        fn threads(ids: &[&str], next: Option<&str>) -> serde_json::Value {
            let mut value = serde_json::json!({
                "threads": ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>(),
                "resultSizeEstimate": ids.len(),
            });
            if let Some(token) = next {
                value["nextPageToken"] = serde_json::json!(token);
            }
            value
        }

        fn thread(id: &str) -> crate::native::gmail::model::GmailThread {
            crate::native::gmail::model::GmailThread::from_gog_json(
                format!(r#"{{"thread":{{"id":"{id}","messages":[]}}}}"#)
                    .as_bytes(),
            )
            .expect("synthetic thread")
        }

        fn with_metadata(
            mut self,
            results: Vec<
                Result<crate::native::gmail::model::GmailThread, GmailError>,
            >,
        ) -> Self {
            self.metadata_queue = RefCell::new(results.into());
            self
        }
    }

    impl GmailRead for MockRead {
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
            _: &str,
            _: &str,
        ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
        {
            unreachable!()
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
            page: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            self.page_calls.borrow_mut().push(page.map(str::to_string));
            if let Some(error) = &self.page_error {
                return Err(error.clone());
            }
            self.pages.borrow_mut().pop_front().ok_or_else(|| {
                GmailError::runtime("bad_json", "missing mock page".to_string())
            })
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
            _: &str,
            thread_id: &str,
        ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
        {
            self.metadata_queue
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Ok(MockRead::thread(thread_id)))
        }
    }

    struct TestClock {
        now: RefCell<std::time::Instant>,
        sleeps: RefCell<Vec<Duration>>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                now: RefCell::new(std::time::Instant::now()),
                sleeps: RefCell::new(Vec::new()),
            }
        }

        fn advance(&self, duration: Duration) {
            *self.now.borrow_mut() += duration;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> std::time::Instant {
            *self.now.borrow()
        }
    }

    impl Sleeper for TestClock {
        fn sleep(&self, duration: Duration) {
            self.sleeps.borrow_mut().push(duration);
            self.advance(duration);
        }
    }

    fn keys(account: &str, ids: &[&str]) -> Vec<ThreadKey> {
        ids.iter()
            .map(|id| ThreadKey::new(account, id).unwrap())
            .collect()
    }

    #[test]
    fn label_queries_escape_whitespace() {
        assert_eq!(label_query("bob"), "label:bob");
        assert_eq!(label_query("my label"), "label:\"my label\"");
        assert_eq!(
            label_query("a\"b\\c"),
            "label:a\"b\\c",
            "no whitespace means no quoting"
        );
    }

    #[test]
    fn two_pages_deduplicate_and_keep_discovery_order() {
        let reader = MockRead::pages(vec![
            MockRead::threads(&["AA01", "bb02"], Some("token-2")),
            MockRead::threads(&["bb02", "cc03"], None),
        ]);
        let outcome =
            discover_ids(&reader, "me@example.com", "bob").expect("pages");
        assert_eq!(outcome.ids, vec!["aa01", "bb02", "cc03"]);
        assert_eq!(outcome.pages_fetched, 2);
        assert!(!outcome.capped);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        assert_eq!(
            *reader.page_calls.borrow(),
            vec![None, Some("token-2".to_string())]
        );
    }

    #[test]
    fn invalid_ids_skip_with_warning() {
        let reader = MockRead::pages(vec![MockRead::threads(
            &["aa01", "", "xyz", "bb02"],
            None,
        )]);
        let outcome = discover_ids(&reader, "me@example.com", "bob")
            .expect("invalid skipped");
        assert_eq!(outcome.ids, vec!["aa01", "bb02"]);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("invalid"));
    }

    #[test]
    fn repeated_page_token_stops_instead_of_looping() {
        let reader = MockRead::pages(vec![
            MockRead::threads(&["aa01"], Some("repeat")),
            MockRead::threads(&["bb02"], Some("repeat")),
            MockRead::threads(&["cc03"], None),
        ]);
        let outcome =
            discover_ids(&reader, "me@example.com", "bob").expect("repeat");
        assert_eq!(outcome.ids, vec!["aa01", "bb02"]);
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("repeated")));
        assert_eq!(outcome.pages_fetched, 2);
    }

    #[test]
    fn cap_warning_at_five_thousand_with_more_remaining() {
        let first: Vec<String> =
            (0..3000).map(|i| format!("{i:08x}")).collect();
        let second: Vec<String> =
            (3000..6000).map(|i| format!("{i:08x}")).collect();
        let first_refs: Vec<&str> = first.iter().map(String::as_str).collect();
        let second_refs: Vec<&str> =
            second.iter().map(String::as_str).collect();
        let reader = MockRead::pages(vec![
            MockRead::threads(&first_refs, Some("token-2")),
            MockRead::threads(&second_refs, Some("token-3")),
        ]);
        let outcome =
            discover_ids(&reader, "me@example.com", "bob").expect("capped");
        assert_eq!(outcome.ids.len(), MAX_DISCOVERY_IDS);
        assert!(outcome.capped);
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("5") || warning.contains("cap")));
    }

    #[test]
    fn already_enrolled_pages_precede_unseen_ids() {
        use std::fs;
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let dir = vault.join(".bob/gmail");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("athena.jsonl"),
            "{\"v\":1,\"ts\":\"2026-11-01T09:00:00-04:00\",\"host\":\"athena\",\
             \"kind\":\"gmail.filed\",\"value\":{\"account\":\"me@example.com\",\
             \"thread_id\":\"aa01\",\"note\":\"mail/2026/a.md\",\
             \"task_file\":\"job.md\",\"via\":\"route\",\"route\":\"job\"}}\n",
        )
        .unwrap();
        let fold = EnrollmentFold::load(vault).unwrap();
        let reader = MockRead::pages(vec![MockRead::threads(
            &["aa01", "bb02", "cc03"],
            None,
        )]);
        let outcome =
            discover_ids(&reader, "me@example.com", "bob").expect("pages");
        assert_eq!(outcome.ids, vec!["aa01", "bb02", "cc03"]);
        let selected = select_candidates(
            &outcome.ids,
            "me@example.com",
            &fold,
            &std::collections::BTreeSet::new(),
            50,
        )
        .expect("select");
        let ids: Vec<&str> =
            selected.iter().map(|key| key.thread_id()).collect();
        assert_eq!(ids, vec!["bb02", "cc03"], "enrolled needs no hydration");
    }

    #[test]
    fn limit_zero_selects_nothing_and_bad_limits_are_setup_errors() {
        let temp = tempfile::tempdir().unwrap();
        let fold = EnrollmentFold::load(temp.path()).unwrap();
        let empty = std::collections::BTreeSet::new();
        let ids = vec!["aa01".to_string(), "bb02".to_string()];
        assert!(select_candidates(&ids, "me@example.com", &fold, &empty, 0)
            .expect("zero")
            .is_empty());
        let error =
            select_candidates(&ids, "me@example.com", &fold, &empty, 501)
                .expect_err("too large");
        assert_eq!(error.kind(), "limit");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn limit_caps_unseen_but_never_enrolled() {
        let temp = tempfile::tempdir().unwrap();
        let fold = EnrollmentFold::load(temp.path()).unwrap();
        let empty = std::collections::BTreeSet::new();
        let ids =
            vec!["aa01".to_string(), "bb02".to_string(), "cc03".to_string()];
        let selected =
            select_candidates(&ids, "me@example.com", &fold, &empty, 2)
                .expect("limit");
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].thread_id(), "aa01");
        assert_eq!(selected[1].thread_id(), "bb02");
    }

    #[test]
    fn fetch_isolates_per_thread_failures_and_keeps_successes() {
        let reader = MockRead::pages(vec![]).with_metadata(vec![
            Ok(MockRead::thread("aa01")),
            Err(GmailError::runtime("not_found", "gone".to_string())),
            Ok(MockRead::thread("cc03")),
        ]);
        let outcome = fetch_candidate_metadata(
            &reader,
            &keys("me@example.com", &["aa01", "bb02", "cc03"]),
        );
        assert_eq!(outcome.successes.len(), 2);
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].0.thread_id(), "bb02");
        assert!(outcome.deferred.is_empty());
        assert!(outcome.aborted.is_none());
        assert!(!outcome.warnings.is_empty());
    }

    #[test]
    fn fetch_auth_abort_retains_successes_and_defers_rest() {
        let reader = MockRead::pages(vec![]).with_metadata(vec![
            Ok(MockRead::thread("aa01")),
            Err(GmailError::runtime("invalid_grant", "expired".to_string())),
            Ok(MockRead::thread("cc03")),
        ]);
        let outcome = fetch_candidate_metadata(
            &reader,
            &keys("me@example.com", &["aa01", "bb02", "cc03"]),
        );
        assert_eq!(outcome.successes.len(), 1);
        assert!(outcome.failures.is_empty());
        assert_eq!(
            outcome.aborted.as_ref().map(|error| error.kind()),
            Some("invalid_grant")
        );
        assert_eq!(outcome.deferred.len(), 1);
        assert_eq!(outcome.deferred[0].thread_id(), "cc03");
    }

    #[test]
    fn zero_budget_allows_healthy_two_thread_run_with_pacing() {
        let reader = MockRead::pages(vec![]);
        let clock = TestClock::new();
        let budget = SharedBudget::new(Duration::ZERO);
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&reader, &clock, &budget, &clock, &last);
        let outcome = fetch_candidate_metadata(
            &coordinated,
            &keys("me@example.com", &["aa01", "bb02"]),
        );
        // `-r 0` still paces; a healthy two-thread run is not capped.
        assert_eq!(outcome.successes.len(), 2);
        assert!(outcome.deferred.is_empty());
        assert!(outcome.aborted.is_none());
        assert_eq!(clock.sleeps.borrow().len(), 1);
        assert_eq!(clock.sleeps.borrow()[0], MIN_GAP);
    }

    #[test]
    fn exhausted_wait_budget_defers_remaining_candidates() {
        struct FirstThenTransient {
            calls: RefCell<usize>,
        }
        impl GmailRead for FirstThenTransient {
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
                _: &str,
                thread_id: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                self.thread_metadata("", thread_id)
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
                _: &str,
                thread_id: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                let mut calls = self.calls.borrow_mut();
                *calls += 1;
                if *calls == 1 {
                    return Ok(MockRead::thread(thread_id));
                }
                Err(GmailError::runtime("rate_limited", "quota".to_string()))
            }
        }
        let reader = FirstThenTransient {
            calls: RefCell::new(0),
        };
        let clock = TestClock::new();
        let budget = SharedBudget::new(Duration::from_millis(50));
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&reader, &clock, &budget, &clock, &last);
        let outcome = fetch_candidate_metadata(
            &coordinated,
            &keys("me@example.com", &["aa01", "bb02", "cc03"]),
        );
        assert_eq!(outcome.successes.len(), 1);
        assert_eq!(outcome.successes[0].0.thread_id(), "aa01");
        assert!(
            outcome
                .deferred
                .iter()
                .map(|key| key.thread_id())
                .collect::<Vec<_>>()
                == vec!["bb02", "cc03"]
        );
        assert!(coordinated.gave_up());
    }

    #[test]
    fn default_limit_is_fifty_and_within_max() {
        assert_eq!(DEFAULT_LIMIT, 50);
        assert!(DEFAULT_LIMIT <= MAX_LIMIT);
        let temp = tempfile::tempdir().unwrap();
        let fold = EnrollmentFold::load(temp.path()).unwrap();
        let empty = std::collections::BTreeSet::new();
        let ids = vec!["aa01".to_string(), "bb02".to_string()];
        let selected = select_candidates(
            &ids,
            "me@example.com",
            &fold,
            &empty,
            DEFAULT_LIMIT,
        )
        .expect("default");
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn adoptable_threads_never_consume_filing_slots() {
        let temp = tempfile::tempdir().unwrap();
        let fold = EnrollmentFold::load(temp.path()).unwrap();
        let adopted = std::collections::BTreeSet::from([ThreadKey::new(
            "me@example.com",
            "aa01",
        )
        .unwrap()]);
        let ids =
            vec!["aa01".to_string(), "bb02".to_string(), "cc03".to_string()];
        let selected =
            select_candidates(&ids, "me@example.com", &fold, &adopted, 1)
                .expect("adopted skipped before limit");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].thread_id(), "bb02");
        let all =
            select_candidates(&ids, "me@example.com", &fold, &adopted, 50)
                .expect("all");
        assert!(
            all.iter().all(|key| key.thread_id() != "aa01"),
            "adoptable threads receive no candidate fetch"
        );
    }

    #[test]
    fn fetch_labels_once_returns_empty_without_network_writes() {
        let reader = MockRead::pages(vec![]);
        let labels =
            fetch_labels_once(&reader, "me@example.com").expect("labels");
        assert!(labels.is_empty());
    }

    #[test]
    fn transient_search_retry_keeps_first_page_and_defers_rest() {
        struct Flaky {
            calls: RefCell<usize>,
        }

        impl GmailRead for Flaky {
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
                _: &str,
                _: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                unreachable!()
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
                page: Option<&str>,
            ) -> Result<serde_json::Value, GmailError> {
                let mut calls = self.calls.borrow_mut();
                *calls += 1;
                if page.is_none() {
                    return Ok(MockRead::threads(&["aa01"], Some("next")));
                }
                Err(GmailError::runtime("rate_limited", "quota".to_string()))
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
                _: &str,
                thread_id: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                Ok(MockRead::thread(thread_id))
            }
        }

        let reader = Flaky {
            calls: RefCell::new(0),
        };
        let clock = TestClock::new();
        // Small wait budget: the first page succeeds, the second page keeps
        // failing transiently until retries give up, so the run keeps the
        // first page and defers the rest.
        let budget = SharedBudget::new(Duration::from_millis(50));
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&reader, &clock, &budget, &clock, &last);
        let outcome = discover_ids(&coordinated, "me@example.com", "bob")
            .expect("partial search");
        assert_eq!(outcome.ids, vec!["aa01"]);
        assert!(outcome.deferred);
        assert!(outcome.failed);
        assert!(
            outcome.warnings.iter().any(|w| w.contains("deferred")),
            "{:?}",
            outcome.warnings
        );
        assert!(*reader.calls.borrow() > 1, "transient page is retried");
    }

    #[test]
    fn cap_without_more_results_stays_quiet() {
        // Exactly one full page of 5,000 ids with no next token: no cap
        // warning because nothing remains outstanding.
        let ids: Vec<String> =
            (0..MAX_DISCOVERY_IDS).map(|i| format!("{i:08x}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reader = MockRead::pages(vec![MockRead::threads(&refs, None)]);
        let outcome =
            discover_ids(&reader, "me@example.com", "bob").expect("exact cap");
        assert_eq!(outcome.ids.len(), MAX_DISCOVERY_IDS);
        assert!(outcome.capped);
        assert!(
            !outcome.warnings.iter().any(|w| w.contains("more results")),
            "quiet when cap leaves nothing outstanding: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn permanent_first_page_failure_keeps_empty_ids_and_fails() {
        let mut reader = MockRead::pages(vec![]);
        reader.page_error = Some(GmailError::runtime(
            "not_found",
            "label missing".to_string(),
        ));
        let outcome = discover_ids(&reader, "me@example.com", "bob")
            .expect("non-auth search stays an outcome");
        assert!(outcome.ids.is_empty());
        assert!(outcome.failed);
        assert!(!outcome.deferred);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("not_found")),
            "{:?}",
            outcome.warnings
        );
    }

    #[test]
    fn later_page_failure_keeps_earlier_ids_and_fails() {
        struct LaterFail {
            calls: RefCell<usize>,
        }
        impl GmailRead for LaterFail {
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
                _: &str,
                _: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                unreachable!()
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
                page: Option<&str>,
            ) -> Result<serde_json::Value, GmailError> {
                let mut calls = self.calls.borrow_mut();
                *calls += 1;
                if page.is_none() {
                    return Ok(MockRead::threads(
                        &["aa01", "bb02"],
                        Some("next"),
                    ));
                }
                Err(GmailError::runtime("not_found", "gone".to_string()))
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
                _: &str,
                _: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                unreachable!()
            }
        }
        let outcome = discover_ids(
            &LaterFail {
                calls: RefCell::new(0),
            },
            "me@example.com",
            "bob",
        )
        .expect("later page");
        assert_eq!(outcome.ids, vec!["aa01", "bb02"]);
        assert!(outcome.failed);
        assert!(!outcome.deferred);
    }

    #[test]
    fn malformed_later_page_keeps_earlier_ids_and_fails() {
        let reader = MockRead::pages(vec![
            MockRead::threads(&["aa01"], Some("token-2")),
            serde_json::json!({"nextPageToken": "token-3", "resultSizeEstimate": 1}),
        ]);
        let outcome = discover_ids(&reader, "me@example.com", "bob")
            .expect("malformed later page");
        assert_eq!(outcome.ids, vec!["aa01"]);
        assert!(outcome.failed);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("bad_json")),
            "{:?}",
            outcome.warnings
        );
    }

    #[test]
    fn transient_search_page_retries_then_recovers() {
        struct Recovering {
            calls: RefCell<usize>,
        }
        impl GmailRead for Recovering {
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
                _: &str,
                _: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                unreachable!()
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
                let mut calls = self.calls.borrow_mut();
                *calls += 1;
                if *calls == 1 {
                    return Err(GmailError::runtime(
                        "rate_limited",
                        "quota".to_string(),
                    ));
                }
                Ok(MockRead::threads(&["aa01"], None))
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
                _: &str,
                _: &str,
            ) -> Result<crate::native::gmail::model::GmailThread, GmailError>
            {
                unreachable!()
            }
        }
        let reader = Recovering {
            calls: RefCell::new(0),
        };
        let clock = TestClock::new();
        let budget = SharedBudget::new(Duration::from_secs(120));
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&reader, &clock, &budget, &clock, &last);
        let outcome = discover_ids(&coordinated, "me@example.com", "bob")
            .expect("recovered");
        assert_eq!(outcome.ids, vec!["aa01"]);
        assert!(!outcome.failed);
        assert!(!outcome.deferred);
        assert_eq!(*reader.calls.borrow(), 2, "one retry then success");
        assert_eq!(clock.sleeps.borrow().len(), 1);
    }
}

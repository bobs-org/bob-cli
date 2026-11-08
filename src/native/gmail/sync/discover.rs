//! Paged ids-only discovery for `bob gmail sync`.
//!
//! One run resolves config, the writer pin, gog, and the account, then
//! discovers with paged `label:<enrollment label>` searches through the
//! last page. Already enrolled ids need no candidate metadata or hydration;
//! unseen ids proceed in stable discovery order. Local adoption from the
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
    retry::{
        execute_with_retry, is_auth_abort, pace_before_request, should_retry,
        Clock, SharedBudget, Sleeper,
    },
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` defaults `-l`.
pub(crate) const DEFAULT_LIMIT: u32 = 50;

/// Validate `-l`: 0 through 500. Zero means watch only.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` validates limits.
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` searches labels.
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
    /// Search pages fetched.
    pub(crate) pages_fetched: usize,
}

/// Discover thread ids with paged `--ids-only` searches to the last page.
///
/// Deduplicates while preserving first-seen order, skips invalid ids with a
/// bounded warning, stops at [`MAX_DISCOVERY_IDS`] with a cap warning only
/// when more results remain, and stops on a repeated page token instead of
/// looping. Every search page is paced and retried within the shared budget
/// (see [`super::retry`]); a transient page failure after budget exhaustion
/// keeps earlier pages and defers the rest with a body-free warning, while a
/// revoked or expired grant aborts before any vault mutation. Returns the raw
/// ids; callers join with enrollment (see [`select_candidates`]) and never
/// hydrate enrolled ids.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` discovers ids.
pub(crate) fn discover_ids<R: GmailRead, S: Sleeper, C: Clock>(
    reader: &R,
    account: &str,
    label: &str,
    budget: &SharedBudget,
    sleeper: &S,
    clock: &C,
    last_request: &mut Option<std::time::Instant>,
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

    loop {
        if !pace_before_request(sleeper, budget, clock, last_request) {
            warnings.push(
                "deferred remaining Gmail search pages when the retry budget ran out"
                    .to_string(),
            );
            deferred_search = true;
            break;
        }
        let response = match execute_with_retry(sleeper, budget, clock, || {
            reader.search_page(account, &query, PAGE_SIZE, page.as_deref())
        }) {
            Ok(response) => response,
            Err(error) if is_auth_abort(&error) => return Err(error),
            Err(error) if should_retry(&error) => {
                warnings.push(
                    "deferred remaining Gmail search pages when the retry budget ran out"
                        .to_string(),
                );
                deferred_search = true;
                break;
            }
            Err(error) => {
                if ordered.is_empty() {
                    return Err(error);
                }
                warnings.push(format!(
                    "kept {} discovered Gmail threads after a search failure: {}",
                    ordered.len(),
                    error.kind()
                ));
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
                // No `threads` key means gog returned no thread list. An
                // empty result set is the end of paging; a non-empty
                // response without the key is a malformed page.
                if response.get("nextPageToken").is_some()
                    || response.get("resultSizeEstimate").is_some()
                {
                    return Err(GmailError::runtime(
                        "bad_json",
                        "gog label search returned no thread list".to_string(),
                    ));
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
                return Err(GmailError::runtime(
                    "bad_json",
                    "gog returned an invalid Gmail search page token"
                        .to_string(),
                ));
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` selects filings.
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` files from this.
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

/// Fetch one label list with bounded retries.
///
/// Called once per run; the caller maps the union of every message's label
/// ids through this response and never routes by a label-name search. The
/// shared `last_request` paces this read against every other Gmail request
/// in the run.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` lists labels.
pub(crate) fn fetch_labels_once<R: GmailRead, S: Sleeper, C: Clock>(
    reader: &R,
    account: &str,
    budget: &SharedBudget,
    sleeper: &S,
    clock: &C,
    last_request: &mut Option<std::time::Instant>,
) -> Result<Vec<crate::native::gmail::gog::GmailLabel>, GmailError> {
    if !pace_before_request(sleeper, budget, clock, last_request) {
        return Err(GmailError::runtime(
            "deferred",
            "deferred Gmail label fetch when the retry budget ran out"
                .to_string(),
        ));
    }
    execute_with_retry(sleeper, budget, clock, || reader.labels_list(account))
}

/// Fetch candidate metadata with pacing, bounded retries, and isolation.
///
/// Requests share the run's `last_request` pacing slot across stages and
/// each request retries transient failures within the shared budget. A
/// revoked or expired grant aborts the batch before any vault mutation:
/// successes so far are retained, the rest defer, and `aborted` carries the
/// grant error. Permanent per-thread failures are recorded with the thread
/// and the batch continues; budget exhaustion defers the remaining keys for
/// the next run.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` fetches candidates.
pub(crate) fn fetch_candidate_metadata<R: GmailRead, S: Sleeper, C: Clock>(
    reader: &R,
    keys: &[ThreadKey],
    budget: &SharedBudget,
    sleeper: &S,
    clock: &C,
    last_request: &mut Option<std::time::Instant>,
) -> FetchOutcome {
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    let mut deferred = Vec::new();
    let mut aborted = None;

    for (index, key) in keys.iter().enumerate() {
        if !pace_before_request(sleeper, budget, clock, last_request) {
            deferred.extend(keys[index..].iter().cloned());
            warnings.push(
                "deferred remaining Gmail fetches when the retry budget ran out"
                    .to_string(),
            );
            break;
        }
        match execute_with_retry(sleeper, budget, clock, || {
            reader.thread_metadata(key.account(), key.thread_id())
        }) {
            Ok(thread) => successes.push((key.clone(), thread)),
            Err(error) => {
                if is_auth_abort(&error) {
                    warnings.push(
                        "stopped Gmail fetches when the grant was revoked or expired"
                            .to_string(),
                    );
                    aborted = Some(error);
                    deferred.extend(keys[index + 1..].iter().cloned());
                    break;
                }
                if should_retry(&error) {
                    // execute_with_retry only returns a transient error when
                    // the budget exhausted mid-retry: defer this and the rest.
                    deferred.push(key.clone());
                    deferred.extend(keys[index + 1..].iter().cloned());
                    warnings.push(
                        "deferred remaining Gmail fetches when the retry budget ran out"
                            .to_string(),
                    );
                    break;
                }
                warnings.push(format!(
                    "skipped Gmail thread {}: {}",
                    key.thread_id(),
                    error.kind()
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

        fn budget(&self, secs: u64) -> SharedBudget {
            SharedBudget::new_with_now(
                Duration::from_secs(secs),
                *self.now.borrow(),
            )
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = discover_ids(
            &reader,
            "me@example.com",
            "bob",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
        .expect("pages");
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = discover_ids(
            &reader,
            "me@example.com",
            "bob",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = discover_ids(
            &reader,
            "me@example.com",
            "bob",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
        .expect("repeat");
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = discover_ids(
            &reader,
            "me@example.com",
            "bob",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
        .expect("capped");
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = discover_ids(
            &reader,
            "me@example.com",
            "bob",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
        .expect("pages");
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = fetch_candidate_metadata(
            &reader,
            &keys("me@example.com", &["aa01", "bb02", "cc03"]),
            &budget,
            &clock,
            &clock,
            &mut last,
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = fetch_candidate_metadata(
            &reader,
            &keys("me@example.com", &["aa01", "bb02", "cc03"]),
            &budget,
            &clock,
            &clock,
            &mut last,
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
    fn zero_budget_allows_healthy_threads_without_pacing_sleep() {
        let reader = MockRead::pages(vec![]);
        let clock = TestClock::new();
        let budget = SharedBudget::new_with_now(Duration::ZERO, clock.now());
        let mut last = None;
        let outcome = fetch_candidate_metadata(
            &reader,
            &keys("me@example.com", &["aa01", "bb02"]),
            &budget,
            &clock,
            &clock,
            &mut last,
        );
        // `-r 0` gives every needed operation one immediate attempt with no
        // pacing sleep, so a healthy two-thread run is not capped at one.
        assert_eq!(outcome.successes.len(), 2);
        assert!(outcome.deferred.is_empty());
        assert!(outcome.aborted.is_none());
        assert!(clock.sleeps.borrow().is_empty());
    }

    #[test]
    fn exhausted_nonzero_budget_defers_remaining_candidates() {
        let reader = MockRead::pages(vec![]);
        let clock = TestClock::new();
        let budget = clock.budget(120);
        // Exhaust the budget after the first fetch by advancing past it.
        let mut last = None;
        let outcome = fetch_candidate_metadata(
            &reader,
            &keys("me@example.com", &["aa01", "bb02"]),
            &budget,
            &clock,
            &clock,
            &mut last,
        );
        assert_eq!(outcome.successes.len(), 2);
        // Now advance past the deadline and prove the next paced request
        // defers instead of issuing another request.
        clock.advance(Duration::from_secs(121));
        let mut stale = Some(clock.now());
        assert!(
            !pace_before_request(&clock, &budget, &clock, &mut stale),
            "exhausted budget defers"
        );
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
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let labels = fetch_labels_once(
            &reader,
            "me@example.com",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
        .expect("labels");
        assert!(labels.is_empty());
        assert!(clock.sleeps.borrow().is_empty());
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
        // Small budget: the first page succeeds, the second page keeps
        // failing transiently until the shared budget exhausts, so the run
        // keeps the first page and defers the rest.
        let budget = clock.budget(1);
        let mut last = None;
        let outcome = discover_ids(
            &reader,
            "me@example.com",
            "bob",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
        .expect("partial search");
        assert_eq!(outcome.ids, vec!["aa01"]);
        assert!(outcome.deferred);
        assert!(
            outcome.warnings.iter().any(|w| w.contains("deferred")),
            "{:?}",
            outcome.warnings
        );
    }

    #[test]
    fn cap_without_more_results_stays_quiet() {
        // Exactly one full page of 5,000 ids with no next token: no cap
        // warning because nothing remains outstanding.
        let ids: Vec<String> =
            (0..MAX_DISCOVERY_IDS).map(|i| format!("{i:08x}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reader = MockRead::pages(vec![MockRead::threads(&refs, None)]);
        let clock = TestClock::new();
        let budget = clock.budget(120);
        let mut last = None;
        let outcome = discover_ids(
            &reader,
            "me@example.com",
            "bob",
            &budget,
            &clock,
            &clock,
            &mut last,
        )
        .expect("exact cap");
        assert_eq!(outcome.ids.len(), MAX_DISCOVERY_IDS);
        assert!(outcome.capped);
        assert!(
            !outcome.warnings.iter().any(|w| w.contains("more results")),
            "quiet when cap leaves nothing outstanding: {:?}",
            outcome.warnings
        );
    }
}

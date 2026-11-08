//! Bounded pacing and transient-retry helpers for `bob gmail sync`.
//!
//! All remote fetching finishes before the vault maintenance lock. Requests
//! are paced with a short minimum gap and retried with bounded exponential
//! backoff plus jitter for rate limits, network errors, and transient
//! service failures. A revoked or expired grant aborts before any vault
//! mutation; missing sign-in and configuration failures are setup errors and
//! are never retried.
//!
//! The helpers are reusable by the later watch loop, whose watched-thread
//! fetches must not share the new-filing `-l` limit. Callers share one
//! [`SharedBudget`] across the guard, the retry loop, and the vault lock so
//! `-r` bounds waiting for those three, never healthy request time. Tests
//! inject a [`Sleeper`] and [`Clock`] so backoff stays fast and
//! deterministic.

use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};

use crate::native::gmail::{
    gog::GmailLabel, model::GmailThread, resolve::GmailRead, GmailError,
};

/// Minimum gap between two Gmail requests.
pub(crate) const MIN_GAP: Duration = Duration::from_millis(200);
/// First backoff step for a transient failure.
pub(crate) const BASE_BACKOFF: Duration = Duration::from_millis(200);
/// Upper bound for any single backoff sleep.
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Whether a classified Gmail failure is worth retrying within the budget.
///
/// Only rate limiting, network reachability, timeouts, and transient
/// service failures retry. Authentication, setup, and permanent failures
/// return immediately so the caller can report or abort.
pub(crate) fn is_transient(error: &GmailError) -> bool {
    matches!(
        error.kind(),
        "network" | "timeout" | "rate_limited" | "transient"
    )
}

/// Whether the grant itself is revoked or expired.
///
/// A global `invalid_grant` aborts the run before any vault mutation. It is
/// a runtime failure (exit 1), never a quiet skip, and it is never retried:
/// another attempt with the same grant fails the same way.
pub(crate) fn is_auth_abort(error: &GmailError) -> bool {
    error.kind() == "invalid_grant"
}

/// Whether the caller should sleep and try the same request again.
pub(crate) fn should_retry(error: &GmailError) -> bool {
    is_transient(error) && !is_auth_abort(error)
}

/// Injectable sleep for tests.
///
/// Production uses [`SystemSleeper`]; tests record sleeps without waiting.
pub(crate) trait Sleeper {
    fn sleep(&self, duration: Duration);
}

/// Injectable clock for tests.
///
/// Production uses [`SystemClock`]; tests advance a manual clock on every
/// sleep so pacing and exhaustion stay deterministic without wall-clock
/// flakiness. The recording sleeper alone does not advance [`Instant`], so
/// it cannot reliably test exhaustion.
pub(crate) trait Clock {
    fn now(&self) -> Instant;
}

/// Production clock: the real wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Production sleeper: block the thread.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemSleeper;

impl Sleeper for SystemSleeper {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// One run's remaining-wait budget.
///
/// Created once from `-r` (default 120 s; 0 means one immediate attempt) and
/// shared by the guard wait, the retry loop, and the vault-lock wait so the
/// three do not each wait another full timeout. The budget is the remaining
/// allowance for **waiting** only: contention sleeps, transient-retry
/// backoff, and vault-lock waits, measured with the injectable [`Clock`].
/// Ordinary request pacing never consumes it, is never refused by it, and
/// never fails or defers an operation. A zero timeout never sleeps for
/// contention, backoff, or lock waiting; a healthy multi-thread run is
/// never capped.
#[derive(Debug)]
pub(crate) struct SharedBudget {
    timeout: Duration,
    remaining: Cell<Duration>,
}

impl SharedBudget {
    /// A budget of `timeout` for waiting. Zero means no waiting at all.
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            remaining: Cell::new(timeout),
        }
    }

    /// A budget of `timeout`, ignoring `now`. Kept so existing tests that
    /// pin a start instant keep compiling after the wait-only rewrite.
    #[cfg(test)]
    pub(crate) fn new_with_now(timeout: Duration, _now: Instant) -> Self {
        Self::new(timeout)
    }

    /// Whether `-r 0` was configured: every needed operation still gets one
    /// immediate attempt with no contention, backoff, or lock-wait sleep.
    pub(crate) fn is_zero_budget(&self) -> bool {
        self.timeout.is_zero()
    }

    /// Remaining wait allowance.
    pub(crate) fn remaining(&self) -> Duration {
        self.remaining.get()
    }

    /// Whether any wait remains.
    pub(crate) fn is_exhausted(&self) -> bool {
        self.remaining().is_zero()
    }

    /// Whether any wait remains. `now` is ignored.
    #[cfg(test)]
    pub(crate) fn is_exhausted_at(&self, _now: Instant) -> bool {
        self.is_exhausted()
    }

    /// Subtract a measured wait from the remaining allowance.
    pub(crate) fn consume_wait(&self, elapsed: Duration) {
        self.remaining
            .set(self.remaining.get().saturating_sub(elapsed));
    }
}

/// Exponential backoff for `attempt` (0-based), capped with jitter.
///
/// Doubling from [`BASE_BACKOFF`] to [`MAX_BACKOFF`], plus a small
/// deterministic jitter ((attempt * 37) % 100 ms) so concurrent runs spread
/// without needing an RNG in tests.
pub(crate) fn backoff_for_attempt(attempt: u32) -> Duration {
    let doubled = BASE_BACKOFF
        .as_millis()
        .saturating_mul(1_u128 << attempt.min(10));
    let capped = doubled.min(MAX_BACKOFF.as_millis());
    let jitter = u128::from(attempt.wrapping_mul(37) % 100);
    Duration::from_millis(
        (capped + jitter).min(MAX_BACKOFF.as_millis() + 99) as u64
    )
}

/// Sleep at most `desired` for a wait that consumes the shared budget.
///
/// Used for guard contention, transient-retry backoff, and vault-lock
/// waits. Returns the slept duration, or `None` when `-r 0` or the remaining
/// wait allowance cannot cover another wait, so the caller defers. Ordinary
/// request pacing must not call this. The injectable [`Clock`] measures the
/// wait so healthy request time never spends the budget.
pub(crate) fn sleep_within_budget<S: Sleeper, C: Clock>(
    sleeper: &S,
    budget: &SharedBudget,
    clock: &C,
    desired: Duration,
) -> Option<Duration> {
    if budget.is_zero_budget() || desired.is_zero() {
        return None;
    }
    let remaining = budget.remaining();
    if remaining.is_zero() {
        return None;
    }
    let sleep_for = desired.min(remaining);
    if sleep_for.is_zero() {
        return None;
    }
    let start = clock.now();
    sleeper.sleep(sleep_for);
    let elapsed = clock.now().saturating_duration_since(start);
    // A sleeper that does not advance the clock (tests) still accounts the
    // requested wait so the loop cannot spin forever.
    let accounted = if elapsed.is_zero() {
        sleep_for
    } else {
        elapsed
    };
    budget.consume_wait(accounted);
    Some(sleep_for)
}

/// Run `operation` once, retrying transient failures within the budget.
///
/// Each actual read attempt is the caller's `operation` (see
/// [`CoordinatedRead`], which paces and timestamps every attempt). This
/// helper only backs off between attempts of the same request. Auth aborts
/// and permanent failures return immediately without sleeping. When the
/// budget exhausts, the last transient error returns so the caller retains
/// prepared successes and defers the rest. A zero budget still makes one
/// immediate attempt with no backoff sleep.
pub(crate) fn execute_with_retry<S: Sleeper, C: Clock, T>(
    sleeper: &S,
    budget: &SharedBudget,
    clock: &C,
    mut operation: impl FnMut() -> Result<T, GmailError>,
) -> Result<T, GmailError> {
    let mut attempt: u32 = 0;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                if !should_retry(&error) {
                    return Err(error);
                }
                let backoff = backoff_for_attempt(attempt);
                if sleep_within_budget(sleeper, budget, clock, backoff)
                    .is_none()
                {
                    return Err(error);
                }
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// Enforce the minimum gap since `last_request` at `clock`.
///
/// Sleeps `MIN_GAP - elapsed` when the previous request was too recent.
/// Always applies, including at `-r 0`. Never consumes the wait budget,
/// never fails, and never defers an operation: a healthy multi-thread run
/// is never capped by pacing.
pub(crate) fn pace_before_request<S: Sleeper, C: Clock>(
    sleeper: &S,
    clock: &C,
    last_request: &mut Option<Instant>,
) {
    if let Some(last) = *last_request {
        let elapsed = clock.now().saturating_duration_since(last);
        if elapsed < MIN_GAP {
            sleeper.sleep(MIN_GAP - elapsed);
        }
    }
    *last_request = Some(clock.now());
}

/// Body-free deferred error when the shared wait budget cannot afford
/// another contention or backoff sleep. Callers treat this as "catch up
/// next run", not as a remote failure.
pub(crate) fn deferred_error() -> GmailError {
    GmailError::runtime(
        "deferred",
        "deferred remaining Gmail work when the retry budget ran out"
            .to_string(),
    )
}

/// One injectable coordinator for every Gmail read during preparation:
/// search pages, label list, candidate metadata, hydration, policy label
/// lookup, and attachment bytes.
///
/// Records two sticky latches from the **raw** read errors, before shared
/// save/policy code re-labels them (`invalid_grant` → `awaiting_sign_in`,
/// budget exhaustion → `refused`):
/// - **auth abort:** the first [`is_auth_abort`] error;
/// - **gave up:** the wait budget was exhausted, or a transient failure
///   ran out of retries.
///
/// Ordinary pacing always applies across stage boundaries and never
/// consumes the wait budget. After a give-up latch, further reads return
/// [`deferred_error`] without calling the inner reader so remaining
/// unprepared keys catch up next run.
pub(crate) struct CoordinatedRead<'a, R, S, C> {
    inner: &'a R,
    sleeper: &'a S,
    budget: &'a SharedBudget,
    clock: &'a C,
    last: &'a RefCell<Option<Instant>>,
    auth_abort: RefCell<Option<GmailError>>,
    gave_up: Cell<bool>,
}

impl<'a, R: GmailRead, S: Sleeper, C: Clock> CoordinatedRead<'a, R, S, C> {
    pub(crate) fn new(
        inner: &'a R,
        sleeper: &'a S,
        budget: &'a SharedBudget,
        clock: &'a C,
        last: &'a RefCell<Option<Instant>>,
    ) -> Self {
        Self {
            inner,
            sleeper,
            budget,
            clock,
            last,
            auth_abort: RefCell::new(None),
            gave_up: Cell::new(false),
        }
    }

    /// The first raw auth-abort error, if any read hit a revoked grant.
    pub(crate) fn auth_abort(&self) -> Option<GmailError> {
        self.auth_abort.borrow().clone()
    }

    /// Whether a wait-budget exhaustion or a transient retry give-up latched.
    pub(crate) fn gave_up(&self) -> bool {
        self.gave_up.get()
    }

    fn run<T>(
        &self,
        mut operation: impl FnMut() -> Result<T, GmailError>,
    ) -> Result<T, GmailError> {
        if let Some(error) = self.auth_abort() {
            return Err(error);
        }
        if self.gave_up.get() {
            return Err(deferred_error());
        }
        // Pace and stamp every actual read attempt, including retries, so
        // a recovered retry still leaves MIN_GAP before the next stage
        // and a budget-limited backoff shorter than MIN_GAP still waits
        // out the remainder. Backoff that already covers the gap is not
        // slept again: `pace_before_request` is a no-op when elapsed
        // already meets MIN_GAP. Ordinary pacing never consumes `-r`.
        match execute_with_retry(self.sleeper, self.budget, self.clock, || {
            pace_before_request(
                self.sleeper,
                self.clock,
                &mut self.last.borrow_mut(),
            );
            operation()
        }) {
            Ok(value) => Ok(value),
            Err(error) => {
                if is_auth_abort(&error) {
                    *self.auth_abort.borrow_mut() = Some(error.clone());
                } else if should_retry(&error) {
                    self.gave_up.set(true);
                }
                Err(error)
            }
        }
    }
}

impl<R: GmailRead, S: Sleeper, C: Clock> GmailRead
    for CoordinatedRead<'_, R, S, C>
{
    fn attachment_bytes(
        &self,
        account: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, GmailError> {
        self.run(|| {
            self.inner
                .attachment_bytes(account, message_id, attachment_id)
        })
    }

    fn thread_get(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<GmailThread, GmailError> {
        self.run(|| self.inner.thread_get(account, thread_id))
    }

    fn message_metadata(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<serde_json::Value, GmailError> {
        self.run(|| self.inner.message_metadata(account, message_id))
    }

    fn search_page(
        &self,
        account: &str,
        query: &str,
        max: u32,
        page: Option<&str>,
    ) -> Result<serde_json::Value, GmailError> {
        self.run(|| self.inner.search_page(account, query, max, page))
    }

    fn labels_list(
        &self,
        account: &str,
    ) -> Result<Vec<GmailLabel>, GmailError> {
        self.run(|| self.inner.labels_list(account))
    }

    fn thread_metadata(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<GmailThread, GmailError> {
        self.run(|| self.inner.thread_metadata(account, thread_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, sync::atomic::AtomicUsize};

    /// Manual clock that advances on every sleep, so pacing and exhaustion
    /// stay deterministic without wall-clock flakiness.
    struct ManualClock {
        now: RefCell<Instant>,
        sleeps: RefCell<Vec<Duration>>,
    }

    impl ManualClock {
        fn new() -> Self {
            Self {
                now: RefCell::new(Instant::now()),
                sleeps: RefCell::new(Vec::new()),
            }
        }

        fn advance(&self, duration: Duration) {
            *self.now.borrow_mut() += duration;
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> Instant {
            *self.now.borrow()
        }
    }

    impl Sleeper for ManualClock {
        fn sleep(&self, duration: Duration) {
            self.sleeps.borrow_mut().push(duration);
            self.advance(duration);
        }
    }

    fn transient(kind: &str) -> GmailError {
        GmailError::runtime(kind, "synthetic".to_string())
    }

    #[test]
    fn only_rate_network_timeout_and_transient_retry() {
        for kind in ["network", "timeout", "rate_limited", "transient"] {
            assert!(should_retry(&transient(kind)), "{kind}");
        }
        for kind in [
            "invalid_grant",
            "not_logged_in",
            "no_client",
            "config",
            "limit",
            "bad_json",
            "not_found",
            "gog",
            "api_disabled",
        ] {
            assert!(!should_retry(&transient(kind)), "{kind}");
        }
        assert!(is_auth_abort(&transient("invalid_grant")));
        assert!(!is_auth_abort(&transient("rate_limited")));
    }

    #[test]
    fn backoff_grows_with_cap_and_jitter() {
        let first = backoff_for_attempt(0);
        let second = backoff_for_attempt(1);
        let tenth = backoff_for_attempt(10);
        assert!(second > first, "{first:?} vs {second:?}");
        assert!(tenth <= MAX_BACKOFF + Duration::from_millis(99));
        // Deterministic jitter: same attempt always sleeps the same.
        assert_eq!(backoff_for_attempt(3), backoff_for_attempt(3));
        assert_ne!(backoff_for_attempt(3), backoff_for_attempt(4));
    }

    #[test]
    fn zero_budget_never_sleeps_and_returns_last_error() {
        let clock = ManualClock::new();
        let budget = SharedBudget::new_with_now(Duration::ZERO, clock.now());
        assert!(budget.is_exhausted_at(clock.now()));
        assert!(budget.is_zero_budget());
        let attempts = AtomicUsize::new(0);
        let error = execute_with_retry(&clock, &budget, &clock, || {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(transient("rate_limited"))
        })
        .expect_err("exhausted");
        assert_eq!(error.kind(), "rate_limited");
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "zero budget makes one immediate attempt"
        );
        assert!(clock.sleeps.borrow().is_empty());
    }

    #[test]
    fn transient_then_success_sleeps_once_with_backoff() {
        let clock = ManualClock::new();
        let budget =
            SharedBudget::new_with_now(Duration::from_secs(120), clock.now());
        let attempts = AtomicUsize::new(0);
        let value = execute_with_retry(&clock, &budget, &clock, || {
            let attempt =
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if attempt == 0 {
                Err(transient("rate_limited"))
            } else {
                Ok(7)
            }
        })
        .expect("recovers");
        assert_eq!(value, 7);
        assert_eq!(clock.sleeps.borrow().len(), 1);
        assert_eq!(clock.sleeps.borrow()[0], backoff_for_attempt(0));
    }

    #[test]
    fn auth_abort_returns_without_sleeping() {
        let clock = ManualClock::new();
        let budget =
            SharedBudget::new_with_now(Duration::from_secs(120), clock.now());
        let error = execute_with_retry(&clock, &budget, &clock, || {
            Err::<(), _>(transient("invalid_grant"))
        })
        .expect_err("aborts");
        assert_eq!(error.kind(), "invalid_grant");
        assert!(clock.sleeps.borrow().is_empty());
    }

    #[test]
    fn pacing_enforces_minimum_gap_without_consuming_budget() {
        let clock = ManualClock::new();
        let budget = SharedBudget::new(Duration::from_secs(120));
        let mut last = None;
        pace_before_request(&clock, &clock, &mut last);
        assert!(clock.sleeps.borrow().is_empty(), "first request is free");
        pace_before_request(&clock, &clock, &mut last);
        assert_eq!(clock.sleeps.borrow().len(), 1, "second request paces");
        assert_eq!(
            budget.remaining(),
            Duration::from_secs(120),
            "pacing never consumes the wait budget"
        );
        clock.advance(MIN_GAP + Duration::from_millis(1));
        pace_before_request(&clock, &clock, &mut last);
        assert_eq!(clock.sleeps.borrow().len(), 1);
        assert_eq!(budget.remaining(), Duration::from_secs(120));
    }

    #[test]
    fn zero_budget_pacing_still_sleeps_the_gap() {
        let clock = ManualClock::new();
        let budget = SharedBudget::new(Duration::ZERO);
        let mut last = None;
        pace_before_request(&clock, &clock, &mut last);
        assert!(clock.sleeps.borrow().is_empty());
        pace_before_request(&clock, &clock, &mut last);
        assert_eq!(
            clock.sleeps.borrow().as_slice(),
            &[MIN_GAP],
            "-r 0 still applies ordinary pacing"
        );
        assert!(budget.is_zero_budget());
        assert!(budget.remaining().is_zero());
    }

    #[test]
    fn wait_sleeps_consume_budget_request_time_does_not() {
        let clock = ManualClock::new();
        let budget = SharedBudget::new(Duration::from_millis(500));
        // Healthy request time advances the clock without a wait sleep.
        clock.advance(Duration::from_secs(30));
        assert_eq!(
            budget.remaining(),
            Duration::from_millis(500),
            "elapsed request time does not spend -r"
        );
        let slept = sleep_within_budget(
            &clock,
            &budget,
            &clock,
            Duration::from_millis(200),
        )
        .expect("wait");
        assert_eq!(slept, Duration::from_millis(200));
        assert_eq!(budget.remaining(), Duration::from_millis(300));
    }

    #[test]
    fn coordinator_latches_auth_abort_from_raw_error() {
        struct AuthOnce {
            calls: AtomicUsize,
        }
        impl GmailRead for AuthOnce {
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
            ) -> Result<GmailThread, GmailError> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(transient("invalid_grant"))
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
            ) -> Result<Vec<GmailLabel>, GmailError> {
                unreachable!()
            }
        }
        let inner = AuthOnce {
            calls: AtomicUsize::new(0),
        };
        let clock = ManualClock::new();
        let budget = SharedBudget::new(Duration::from_secs(120));
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&inner, &clock, &budget, &clock, &last);
        let error = coordinated
            .thread_get("me@example.com", "aa01")
            .err()
            .expect("auth");
        assert_eq!(error.kind(), "invalid_grant");
        assert!(is_auth_abort(&coordinated.auth_abort().unwrap()));
        // A later read returns the latched error without calling inner.
        let again = coordinated
            .thread_get("me@example.com", "bb02")
            .err()
            .expect("latched");
        assert_eq!(again.kind(), "invalid_grant");
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn coordinator_latches_give_up_and_skips_remaining_reads() {
        struct AlwaysTransient {
            calls: AtomicUsize,
        }
        impl GmailRead for AlwaysTransient {
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
            ) -> Result<GmailThread, GmailError> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(transient("rate_limited"))
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
            ) -> Result<Vec<GmailLabel>, GmailError> {
                unreachable!()
            }
        }
        let inner = AlwaysTransient {
            calls: AtomicUsize::new(0),
        };
        let clock = ManualClock::new();
        let budget = SharedBudget::new(Duration::from_millis(50));
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&inner, &clock, &budget, &clock, &last);
        let error = coordinated
            .thread_get("me@example.com", "aa01")
            .err()
            .expect("gave up");
        assert_eq!(error.kind(), "rate_limited");
        assert!(coordinated.gave_up());
        let first_calls = inner.calls.load(std::sync::atomic::Ordering::SeqCst);
        assert!(first_calls >= 1);
        let deferred = coordinated
            .thread_get("me@example.com", "bb02")
            .err()
            .expect("latched give-up");
        assert_eq!(deferred.kind(), "deferred");
        assert_eq!(
            inner.calls.load(std::sync::atomic::Ordering::SeqCst),
            first_calls,
            "give-up skips remaining inner reads"
        );
    }

    #[test]
    fn coordinator_paces_across_stage_boundaries() {
        struct CountingRead {
            labels: AtomicUsize,
            metadata: AtomicUsize,
            bodies: AtomicUsize,
        }
        impl GmailRead for CountingRead {
            fn attachment_bytes(
                &self,
                _: &str,
                _: &str,
                _: &str,
            ) -> Result<Vec<u8>, GmailError> {
                self.bodies
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(b"bytes".to_vec())
            }
            fn thread_get(
                &self,
                _: &str,
                thread_id: &str,
            ) -> Result<GmailThread, GmailError> {
                self.bodies
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(GmailThread::from_gog_json(
                    format!(
                        r#"{{"thread":{{"id":"{thread_id}","messages":[]}}}}"#
                    )
                    .as_bytes(),
                )
                .expect("thread"))
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
                Ok(serde_json::json!({"threads":[{"id":"aa01"}]}))
            }
            fn labels_list(
                &self,
                _: &str,
            ) -> Result<Vec<GmailLabel>, GmailError> {
                self.labels
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::new())
            }
            fn thread_metadata(
                &self,
                _: &str,
                thread_id: &str,
            ) -> Result<GmailThread, GmailError> {
                self.metadata
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(GmailThread::from_gog_json(
                    format!(
                        r#"{{"thread":{{"id":"{thread_id}","messages":[]}}}}"#
                    )
                    .as_bytes(),
                )
                .expect("thread"))
            }
        }
        let inner = CountingRead {
            labels: AtomicUsize::new(0),
            metadata: AtomicUsize::new(0),
            bodies: AtomicUsize::new(0),
        };
        let clock = ManualClock::new();
        let budget = SharedBudget::new(Duration::ZERO);
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&inner, &clock, &budget, &clock, &last);
        coordinated
            .search_page("me@example.com", "label:bob", 500, None)
            .unwrap();
        coordinated.labels_list("me@example.com").unwrap();
        coordinated
            .thread_metadata("me@example.com", "aa01")
            .unwrap();
        coordinated.thread_get("me@example.com", "aa01").unwrap();
        coordinated.thread_get("me@example.com", "bb02").unwrap();
        coordinated
            .attachment_bytes("me@example.com", "m1", "a1")
            .unwrap();
        // Six stage reads: first is free, the next five pace. Two
        // hydrations at `-r 0` both run.
        assert_eq!(clock.sleeps.borrow().len(), 5);
        assert!(clock.sleeps.borrow().iter().all(|sleep| *sleep == MIN_GAP));
        assert!(budget.remaining().is_zero(), "-r 0 wait budget stays empty");
    }

    #[test]
    fn retry_then_next_stage_paces_from_the_retry_attempt() {
        struct FlakyThenOk {
            search: AtomicUsize,
            labels: AtomicUsize,
        }
        impl GmailRead for FlakyThenOk {
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
            ) -> Result<GmailThread, GmailError> {
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
                let calls = self
                    .search
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if calls == 0 {
                    return Err(transient("rate_limited"));
                }
                Ok(serde_json::json!({"threads":[{"id":"aa01"}]}))
            }
            fn labels_list(
                &self,
                _: &str,
            ) -> Result<Vec<GmailLabel>, GmailError> {
                self.labels
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::new())
            }
        }
        let inner = FlakyThenOk {
            search: AtomicUsize::new(0),
            labels: AtomicUsize::new(0),
        };
        let clock = ManualClock::new();
        let budget = SharedBudget::new(Duration::from_secs(120));
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&inner, &clock, &budget, &clock, &last);
        coordinated
            .search_page("me@example.com", "label:bob", 500, None)
            .unwrap();
        coordinated.labels_list("me@example.com").unwrap();
        assert_eq!(inner.search.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(inner.labels.load(std::sync::atomic::Ordering::SeqCst), 1);
        let sleeps = clock.sleeps.borrow().clone();
        assert_eq!(sleeps.len(), 2, "{sleeps:?}");
        assert_eq!(sleeps[0], backoff_for_attempt(0));
        assert_eq!(
            sleeps[1], MIN_GAP,
            "the next stage paces from the successful retry, not the failed first attempt"
        );
        assert_eq!(
            budget.remaining(),
            Duration::from_secs(120) - backoff_for_attempt(0),
            "pacing never consumes the wait budget"
        );
    }

    #[test]
    fn short_remaining_backoff_still_paces_the_retry() {
        struct AlwaysOnce {
            calls: AtomicUsize,
        }
        impl GmailRead for AlwaysOnce {
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
            ) -> Result<GmailThread, GmailError> {
                let calls = self
                    .calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if calls == 0 {
                    return Err(transient("rate_limited"));
                }
                Ok(GmailThread::from_gog_json(
                    br#"{"thread":{"id":"aa01","messages":[]}}"#,
                )
                .expect("thread"))
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
            ) -> Result<Vec<GmailLabel>, GmailError> {
                unreachable!()
            }
        }
        let inner = AlwaysOnce {
            calls: AtomicUsize::new(0),
        };
        let clock = ManualClock::new();
        // Shorter than MIN_GAP: the budget-limited backoff cannot cover
        // the request gap on its own.
        let budget = SharedBudget::new(Duration::from_millis(50));
        let last = RefCell::new(None);
        let coordinated =
            CoordinatedRead::new(&inner, &clock, &budget, &clock, &last);
        coordinated
            .thread_get("me@example.com", "aa01")
            .expect("retry recovers after a short backoff");
        let sleeps = clock.sleeps.borrow().clone();
        assert_eq!(sleeps.len(), 2, "{sleeps:?}");
        assert_eq!(sleeps[0], Duration::from_millis(50));
        assert_eq!(
            sleeps[1],
            MIN_GAP - Duration::from_millis(50),
            "the retry still waits out the remainder of MIN_GAP"
        );
        assert!(budget.remaining().is_zero());
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}

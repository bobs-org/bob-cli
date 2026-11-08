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
//! `-r` bounds the whole run instead of each wait separately. Tests inject a
//! [`Sleeper`] so backoff stays fast and deterministic.

use std::time::{Duration, Instant};

use crate::native::gmail::GmailError;

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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` retries fetches.
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` aborts on it.
pub(crate) fn is_auth_abort(error: &GmailError) -> bool {
    error.kind() == "invalid_grant"
}

/// Whether the caller should sleep and try the same request again.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` retries fetches.
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
#[allow(dead_code)] // `bob gmail sync` paces live runs with this.
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
/// three do not each wait another full timeout. A zero timeout never sleeps
/// for contention or backoff, but ordinary request pacing still proceeds
/// without sleeping so a healthy multi-thread run is not capped at one
/// thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SharedBudget {
    timeout: Duration,
    deadline: Instant,
}

#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` shares one budget.
impl SharedBudget {
    /// A budget of `timeout` from now. Zero means no waiting at all.
    pub(crate) fn new(timeout: Duration) -> Self {
        Self::new_with_now(timeout, Instant::now())
    }

    /// A budget of `timeout` from `now`, for deterministic tests.
    pub(crate) fn new_with_now(timeout: Duration, now: Instant) -> Self {
        Self {
            timeout,
            deadline: now + timeout,
        }
    }

    /// Whether `-r 0` was configured: every needed operation still gets one
    /// immediate attempt with no contention or backoff sleep.
    pub(crate) fn is_zero_budget(&self) -> bool {
        self.timeout.is_zero()
    }

    /// Time left before the deadline at `now`, or zero when exhausted.
    pub(crate) fn remaining_at(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }

    /// Time left before the deadline, or zero when exhausted.
    pub(crate) fn remaining(&self) -> Duration {
        self.remaining_at(Instant::now())
    }

    /// Whether any wait remains at `now`.
    pub(crate) fn is_exhausted_at(&self, now: Instant) -> bool {
        self.remaining_at(now).is_zero()
    }
}

/// Exponential backoff for `attempt` (0-based), capped with jitter.
///
/// Doubling from [`BASE_BACKOFF`] to [`MAX_BACKOFF`], plus a small
/// deterministic jitter ((attempt * 37) % 100 ms) so concurrent runs spread
/// without needing an RNG in tests.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` backs off.
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

/// Sleep at most `desired`, bounded by the shared budget at `clock`.
///
/// Returns the slept duration, or `None` when the budget is already
/// exhausted so the caller defers the remaining work to the next run.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` paces fetches.
pub(crate) fn sleep_within_budget<S: Sleeper, C: Clock>(
    sleeper: &S,
    budget: &SharedBudget,
    clock: &C,
    desired: Duration,
) -> Option<Duration> {
    let remaining = budget.remaining_at(clock.now());
    if remaining.is_zero() || desired.is_zero() {
        return None;
    }
    let sleep_for = desired.min(remaining);
    // A zero budget never sleeps; a non-zero budget sleeps at least a
    // millisecond so pacing still applies on short budgets.
    if sleep_for.is_zero() {
        return None;
    }
    sleeper.sleep(sleep_for);
    Some(sleep_for)
}

/// Run `operation` once, retrying transient failures within the budget.
///
/// Pacing between distinct requests is the caller's job (see
/// [`pace_before_request`]); this helper only backs off between attempts of
/// the same request. Auth aborts and permanent failures return immediately
/// without sleeping. When the budget exhausts, the last transient error
/// returns so the caller retains prepared successes and defers the rest.
/// A zero budget still makes one immediate attempt with no backoff sleep.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` fetches with it.
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
/// Returns `false` when a non-zero budget is exhausted so the caller defers
/// the remaining candidates instead of issuing another request. A zero
/// `-r 0` budget never sleeps for pacing: every needed operation still gets
/// its one immediate attempt, so pacing alone never caps a healthy
/// multi-thread run at one thread.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` paces fetches.
pub(crate) fn pace_before_request<S: Sleeper, C: Clock>(
    sleeper: &S,
    budget: &SharedBudget,
    clock: &C,
    last_request: &mut Option<Instant>,
) -> bool {
    if budget.is_zero_budget() {
        *last_request = Some(clock.now());
        return true;
    }
    if let Some(last) = *last_request {
        let elapsed = clock.now().saturating_duration_since(last);
        if elapsed < MIN_GAP
            && sleep_within_budget(sleeper, budget, clock, MIN_GAP - elapsed)
                .is_none()
        {
            return false;
        }
    }
    *last_request = Some(clock.now());
    true
}

/// Body-free deferred error when the shared budget cannot afford another
/// paced request. Callers treat this as "catch up next run", not as a
/// remote failure.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` defers work.
pub(crate) fn deferred_error() -> GmailError {
    GmailError::runtime(
        "deferred",
        "deferred remaining Gmail work when the retry budget ran out"
            .to_string(),
    )
}

/// One run's Gmail stages share a single pacing slot and a single
/// remaining budget: the caller threads one `last_request` through paged
/// search, label enumeration, candidate metadata, hydration, policy
/// metadata, and attachment reads (see `command.rs` and `discover.rs`), so
/// `-r` bounds the whole run and ordinary pacing never requires a second
/// sleep for the same gap.

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
    fn pacing_enforces_minimum_gap_and_respects_budget() {
        let clock = ManualClock::new();
        let budget =
            SharedBudget::new_with_now(Duration::from_secs(120), clock.now());
        let mut last = None;
        assert!(pace_before_request(&clock, &budget, &clock, &mut last));
        assert!(clock.sleeps.borrow().is_empty(), "first request is free");
        assert!(pace_before_request(&clock, &budget, &clock, &mut last));
        assert_eq!(clock.sleeps.borrow().len(), 1, "second request paces");
        // Advancing past the gap needs no further sleep.
        clock.advance(MIN_GAP + Duration::from_millis(1));
        assert!(pace_before_request(&clock, &budget, &clock, &mut last));
        assert_eq!(clock.sleeps.borrow().len(), 1);
    }

    #[test]
    fn zero_budget_pacing_still_allows_healthy_requests() {
        let clock = ManualClock::new();
        let budget = SharedBudget::new_with_now(Duration::ZERO, clock.now());
        let mut last = None;
        assert!(pace_before_request(&clock, &budget, &clock, &mut last));
        assert!(clock.sleeps.borrow().is_empty());
        assert!(pace_before_request(&clock, &budget, &clock, &mut last));
        assert!(
            clock.sleeps.borrow().is_empty(),
            "-r 0 never sleeps for pacing"
        );
    }

    #[test]
    fn exhausted_nonzero_budget_defers_pacing() {
        let clock = ManualClock::new();
        let budget =
            SharedBudget::new_with_now(Duration::from_millis(10), clock.now());
        let mut last = None;
        assert!(pace_before_request(&clock, &budget, &clock, &mut last));
        // Exhaust the small budget with a sleep, then pacing must defer.
        clock.advance(Duration::from_secs(1));
        let mut stale = Some(clock.now());
        // Move the last request to now so pacing needs a sleep it cannot afford.
        // Budget is exhausted, so the next paced request defers.
        assert!(!pace_before_request(&clock, &budget, &clock, &mut stale));
    }
}

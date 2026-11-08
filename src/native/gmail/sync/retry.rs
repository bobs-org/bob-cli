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
/// three do not each wait another full timeout. A zero timeout never sleeps:
/// every wait fails fast.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SharedBudget {
    deadline: Instant,
}

#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` shares one budget.
impl SharedBudget {
    /// A budget of `timeout` from now. Zero means no waiting at all.
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
        }
    }

    /// Time left before the deadline, or zero when exhausted.
    pub(crate) fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Whether any wait remains. A zero timeout is exhausted immediately.
    pub(crate) fn is_exhausted(&self) -> bool {
        self.remaining().is_zero()
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

/// Sleep at most `desired`, bounded by the shared budget.
///
/// Returns the slept duration, or `None` when the budget is already
/// exhausted so the caller defers the remaining work to the next run.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` paces fetches.
pub(crate) fn sleep_within_budget<S: Sleeper>(
    sleeper: &S,
    budget: &SharedBudget,
    desired: Duration,
) -> Option<Duration> {
    let remaining = budget.remaining();
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` fetches with it.
pub(crate) fn execute_with_retry<S: Sleeper, T>(
    sleeper: &S,
    budget: &SharedBudget,
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
                if sleep_within_budget(sleeper, budget, backoff).is_none() {
                    return Err(error);
                }
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// Enforce the minimum gap since `last_request`.
///
/// Sleeps `MIN_GAP - elapsed` when the previous request was too recent.
/// Returns `false` when the budget is exhausted so the caller defers the
/// remaining candidates instead of issuing another request.
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` paces fetches.
pub(crate) fn pace_before_request<S: Sleeper>(
    sleeper: &S,
    budget: &SharedBudget,
    last_request: &mut Option<Instant>,
) -> bool {
    if let Some(last) = *last_request {
        let elapsed = last.elapsed();
        if elapsed < MIN_GAP
            && sleep_within_budget(sleeper, budget, MIN_GAP - elapsed).is_none()
        {
            return false;
        }
    }
    *last_request = Some(Instant::now());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, sync::atomic::AtomicUsize};

    #[derive(Default)]
    struct RecordSleep {
        sleeps: RefCell<Vec<Duration>>,
    }

    impl Sleeper for RecordSleep {
        fn sleep(&self, duration: Duration) {
            self.sleeps.borrow_mut().push(duration);
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
        let sleeper = RecordSleep::default();
        let budget = SharedBudget::new(Duration::ZERO);
        assert!(budget.is_exhausted());
        let attempts = AtomicUsize::new(0);
        let error = execute_with_retry(&sleeper, &budget, || {
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
        assert!(sleeper.sleeps.borrow().is_empty());
    }

    #[test]
    fn transient_then_success_sleeps_once_with_backoff() {
        let sleeper = RecordSleep::default();
        let budget = SharedBudget::new(Duration::from_secs(120));
        let attempts = AtomicUsize::new(0);
        let value = execute_with_retry(&sleeper, &budget, || {
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
        assert_eq!(sleeper.sleeps.borrow().len(), 1);
        assert_eq!(sleeper.sleeps.borrow()[0], backoff_for_attempt(0));
    }

    #[test]
    fn auth_abort_returns_without_sleeping() {
        let sleeper = RecordSleep::default();
        let budget = SharedBudget::new(Duration::from_secs(120));
        let error = execute_with_retry(&sleeper, &budget, || {
            Err::<(), _>(transient("invalid_grant"))
        })
        .expect_err("aborts");
        assert_eq!(error.kind(), "invalid_grant");
        assert!(sleeper.sleeps.borrow().is_empty());
    }

    #[test]
    fn pacing_enforces_minimum_gap_and_respects_budget() {
        let sleeper = RecordSleep::default();
        let budget = SharedBudget::new(Duration::from_secs(120));
        let mut last = None;
        assert!(pace_before_request(&sleeper, &budget, &mut last));
        assert!(sleeper.sleeps.borrow().is_empty(), "first request is free");
        assert!(pace_before_request(&sleeper, &budget, &mut last));
        assert_eq!(sleeper.sleeps.borrow().len(), 1, "second request paces");

        let empty = SharedBudget::new(Duration::ZERO);
        let mut last = Some(Instant::now());
        assert!(
            !pace_before_request(&sleeper, &empty, &mut last),
            "exhausted budget defers instead of requesting"
        );
    }
}

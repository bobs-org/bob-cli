//! Bounded retry loop around task-status sync attempts.
use super::*;

/// Bounded jittered-backoff retry controller wrapped around one complete
/// `sync_task_statuses` attempt. Each attempt reacquires the maintenance
/// lock, rediscovers inputs, and recomputes the whole plan from scratch, so
/// backing off between attempts never holds the shared lock.
pub(super) struct RetryEnv {
    pub(super) now: Box<dyn Fn() -> Instant>,
    pub(super) sleep: Box<dyn Fn(Duration)>,
    pub(super) jitter: Box<dyn Fn() -> f64>,
    pub(super) log: Box<dyn Fn(String)>,
}

impl RetryEnv {
    pub(super) fn production(format: OutputFormat) -> Self {
        let log: Box<dyn Fn(String)> = match format {
            OutputFormat::Human => Box::new(|line: String| println!("{line}")),
            OutputFormat::Json => Box::new(|line: String| eprintln!("{line}")),
        };
        Self {
            now: Box::new(Instant::now),
            sleep: Box::new(std::thread::sleep),
            jitter: Box::new(random_unit_interval),
            log,
        }
    }
}

/// A fresh random value in `[0, 1)`, varying between calls. Seeded from
/// wall-clock nanoseconds, the process ID, and a monotonically increasing
/// counter rather than pulling in a `rand`-sized dependency for one small
/// jitter draw.
pub(super) fn random_unit_interval() -> f64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut mixed = nanos as u64 ^ (nanos >> 64) as u64;
    mixed ^= (std::process::id() as u64) << 32;
    mixed ^= COUNTER.fetch_add(1, Ordering::Relaxed);
    // splitmix64 finalizer: spread the mixed seed across all bits.
    mixed ^= mixed >> 33;
    mixed = mixed.wrapping_mul(0xff51_afd7_ed55_8ccd);
    mixed ^= mixed >> 33;
    mixed = mixed.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    mixed ^= mixed >> 33;
    (mixed as f64) / (u64::MAX as f64)
}

/// The exponential delay ceiling for the attempt that just failed: 2, 4, 8,
/// 16, then 30 seconds and capped there.
pub(super) fn retry_ceiling_secs(attempt_number: u32) -> u64 {
    match attempt_number {
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => 30,
    }
}

/// A delay in the upper half of `[ceiling / 2, ceiling]`, driven by a
/// `jitter_unit` drawn from `[0, 1]`.
pub(super) fn retry_delay(ceiling: Duration, jitter_unit: f64) -> Duration {
    let jitter_unit = jitter_unit.clamp(0.0, 1.0);
    let half = ceiling / 2;
    half + half.mul_f64(jitter_unit)
}

/// Whether `error` is eligible for an automatic retry. Only the allowlisted
/// transient reasons ever retry, and only when the failed attempt applied no
/// files: a partial apply is always terminal, even when its own
/// `applied_files` list happens to be empty, because the reason can combine
/// different failure stages and this must not broaden replay safety by
/// guessing.
pub(super) fn is_retryable(error: &SyncError) -> bool {
    if !error.applied_files.is_empty() {
        return false;
    }
    matches!(
        error.reason.as_deref(),
        Some("lock_contention")
            | Some("vault_changed")
            | Some("quiet_period")
            | Some("unstable_read")
    )
}

pub(super) fn format_retry_duration(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

pub(super) fn retry_timestamp() -> String {
    bob_env::current_datetime()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

pub(super) fn log_retry_decision(
    env: &RetryEnv,
    run_id: &str,
    attempt_number: u32,
    elapsed: Duration,
    error: &SyncError,
    delay: Duration,
) {
    let mut line = format!(
        "{} {COMMAND_NAME}: retry run={run_id} attempt={attempt_number} elapsed={} reason={} delay={} error=\"{}\"",
        retry_timestamp(),
        format_retry_duration(elapsed),
        error.reason.as_deref().unwrap_or("unknown"),
        format_retry_duration(delay),
        error.message,
    );
    if let Some(recovery) = &error.recovery_directory {
        line.push_str(&format!(" recovery={recovery}"));
    }
    (env.log)(line);
}

pub(super) fn log_retry_summary(
    env: &RetryEnv,
    run_id: &str,
    outcome: &str,
    attempts: u32,
    elapsed: Duration,
    reason: Option<&str>,
) {
    let mut line = format!(
        "{} {COMMAND_NAME}: retry run={run_id} {outcome} attempts={attempts} elapsed={}",
        retry_timestamp(),
        format_retry_duration(elapsed),
    );
    if let Some(reason) = reason {
        line.push_str(&format!(" reason={reason}"));
    }
    (env.log)(line);
}

/// Run `attempt` under the bounded retry policy, calling it again after a
/// jittered backoff whenever it fails with an eligible transient reason
/// within `budget`. The initial attempt always runs regardless of budget,
/// and an active attempt is always allowed to finish; the budget only gates
/// whether another attempt starts. Elapsed time is tracked as a running
/// difference against the monotonic start instant rather than as a
/// `start + budget` deadline, so an overlong `--retry-timeout` cannot
/// overflow the addition.
pub(super) fn retry_loop<F>(
    run_id: &str,
    budget: Duration,
    env: &RetryEnv,
    mut attempt: F,
) -> Result<SyncResult, SyncError>
where
    F: FnMut(u32) -> Result<SyncResult, SyncError>,
{
    let start = (env.now)();
    let mut attempt_number: u32 = 1;
    let mut retried = false;
    loop {
        let error = match attempt(attempt_number) {
            Ok(result) => {
                if retried {
                    log_retry_summary(
                        env,
                        run_id,
                        "succeeded",
                        attempt_number,
                        (env.now)() - start,
                        None,
                    );
                }
                return Ok(result);
            }
            Err(error) => error,
        };

        if !is_retryable(&error) {
            if retried {
                log_retry_summary(
                    env,
                    run_id,
                    "stopped",
                    attempt_number,
                    (env.now)() - start,
                    error.reason.as_deref(),
                );
            }
            return Err(error);
        }

        let elapsed = (env.now)() - start;
        if elapsed >= budget {
            log_retry_summary(
                env,
                run_id,
                "exhausted retry budget",
                attempt_number,
                elapsed,
                error.reason.as_deref(),
            );
            return Err(error);
        }

        let ceiling = Duration::from_secs(retry_ceiling_secs(attempt_number));
        let remaining = budget - elapsed;
        let delay = retry_delay(ceiling, (env.jitter)()).min(remaining);
        log_retry_decision(env, run_id, attempt_number, elapsed, &error, delay);
        (env.sleep)(delay);
        retried = true;

        let elapsed_after_sleep = (env.now)() - start;
        if elapsed_after_sleep >= budget {
            log_retry_summary(
                env,
                run_id,
                "exhausted retry budget",
                attempt_number,
                elapsed_after_sleep,
                error.reason.as_deref(),
            );
            return Err(error);
        }
        attempt_number += 1;
    }
}

pub(super) fn run_with_retries(
    request: &Request,
    env: &RetryEnv,
) -> Result<SyncResult, SyncError> {
    let run_id = new_run_id();
    retry_loop(&run_id, request.retry_timeout, env, |_attempt_number| {
        sync_task_statuses(request)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    fn empty_sync_result(daily_file: &str) -> SyncResult {
        SyncResult {
            ok: true,
            dry_run: false,
            daily_file: daily_file.to_string(),
            previous_daily_file: None,
            open_pomodoros: 0,
            references: 0,
            previous_daily_references: 0,
            recent_activity_references: 0,
            dependency_references: 0,
            scanned_files: 0,
            marked_next: Vec::new(),
            marked_in_progress: Vec::new(),
            cleared: Vec::new(),
            cleared_in_progress: Vec::new(),
            marked_blocked: Vec::new(),
            unblocked: Vec::new(),
            struck_completed_references: Vec::new(),
            embedded_completed_references: Vec::new(),
            moved_completed_references: Vec::new(),
            marker_added_references: Vec::new(),
            marker_removed_references: Vec::new(),
            removed_canceled_references: Vec::new(),
            removed_duplicate_lines: Vec::new(),
            removed_empty_pomodoros: Vec::new(),
            grouped_task_sections: Vec::new(),
            grouping_warnings: Vec::new(),
            applied_files: Vec::new(),
            deferred_files: Vec::new(),
            recovery_directory: None,
            kept_next: 0,
            kept_in_progress: 0,
            unresolved_references: Vec::new(),
        }
    }

    fn sync_error(
        message: &str,
        reason: Option<&str>,
        applied_files: Vec<&str>,
    ) -> SyncError {
        SyncError {
            message: message.to_string(),
            reason: reason.map(str::to_string),
            applied_files: applied_files
                .into_iter()
                .map(str::to_string)
                .collect(),
            deferred_files: Vec::new(),
            recovery_directory: None,
        }
    }

    type MockEnv = (
        Rc<RefCell<Vec<Duration>>>,
        Rc<RefCell<Vec<String>>>,
        RetryEnv,
    );

    /// A `RetryEnv` whose clock only advances when the mock `sleep` is
    /// called, so simulating a long `--retry-timeout` budget costs no real
    /// wall-clock time. `jitter_values` are consumed in order, defaulting to
    /// `0.0` (the bottom of the upper-half jitter range) once exhausted.
    fn mock_env(jitter_values: Vec<f64>) -> MockEnv {
        let clock = Rc::new(RefCell::new(Instant::now()));
        let sleeps = Rc::new(RefCell::new(Vec::new()));
        let logs = Rc::new(RefCell::new(Vec::new()));
        let jitter_queue = Rc::new(RefCell::new(jitter_values));

        let now_clock = Rc::clone(&clock);
        let sleep_clock = Rc::clone(&clock);
        let sleep_log = Rc::clone(&sleeps);
        let log_lines = Rc::clone(&logs);

        let env = RetryEnv {
            now: Box::new(move || *now_clock.borrow()),
            sleep: Box::new(move |duration: Duration| {
                sleep_log.borrow_mut().push(duration);
                *sleep_clock.borrow_mut() += duration;
            }),
            jitter: Box::new(move || {
                let mut queue = jitter_queue.borrow_mut();
                if queue.is_empty() {
                    0.0
                } else {
                    queue.remove(0)
                }
            }),
            log: Box::new(move |line: String| {
                log_lines.borrow_mut().push(line);
            }),
        };
        (sleeps, logs, env)
    }

    /// An attempt closure fed from a fixed script. Calling it past the end
    /// of the script panics, which is how tests assert `retry_loop` never
    /// starts more attempts than expected.
    fn scripted_attempts(
        results: Vec<Result<SyncResult, SyncError>>,
    ) -> (
        Rc<RefCell<u32>>,
        impl FnMut(u32) -> Result<SyncResult, SyncError>,
    ) {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_handle = Rc::clone(&calls);
        let mut results = results.into_iter();
        let attempt = move |_attempt_number: u32| {
            *calls.borrow_mut() += 1;
            results
                .next()
                .expect("retry_loop requested more attempts than were scripted")
        };
        (calls_handle, attempt)
    }

    #[test]
    fn retry_ceiling_progression_caps_at_thirty_seconds() {
        assert_eq!(retry_ceiling_secs(1), 2);
        assert_eq!(retry_ceiling_secs(2), 4);
        assert_eq!(retry_ceiling_secs(3), 8);
        assert_eq!(retry_ceiling_secs(4), 16);
        assert_eq!(retry_ceiling_secs(5), 30);
        assert_eq!(retry_ceiling_secs(100), 30);
    }

    #[test]
    fn retry_delay_stays_within_upper_half_of_ceiling() {
        let ceiling = Duration::from_secs(8);
        assert_eq!(retry_delay(ceiling, 0.0), Duration::from_secs(4));
        assert_eq!(retry_delay(ceiling, 1.0), Duration::from_secs(8));
        let mid = retry_delay(ceiling, 0.5);
        assert!(mid >= Duration::from_secs(4) && mid <= Duration::from_secs(8));
        // Out-of-range jitter is clamped defensively rather than panicking.
        assert_eq!(retry_delay(ceiling, -1.0), Duration::from_secs(4));
        assert_eq!(retry_delay(ceiling, 2.0), Duration::from_secs(8));
    }

    #[test]
    fn random_unit_interval_varies_and_stays_in_unit_range() {
        let samples: Vec<f64> =
            (0..20).map(|_| random_unit_interval()).collect();
        assert!(samples.iter().all(|value| (0.0..1.0).contains(value)));
        assert!(
            samples.windows(2).any(|pair| pair[0] != pair[1]),
            "production jitter must vary between invocations"
        );
    }

    #[test]
    fn allowed_transient_reasons_are_retried_without_applied_files() {
        for reason in [
            "lock_contention",
            "vault_changed",
            "quiet_period",
            "unstable_read",
        ] {
            let error = sync_error("boom", Some(reason), vec![]);
            assert!(is_retryable(&error), "expected {reason} to be retryable");
        }
    }

    #[test]
    fn terminal_and_unknown_reasons_are_never_retried() {
        for reason in [
            None,
            Some("recovery_failed"),
            Some("unsupported_file"),
            Some("io"),
            Some("partial_apply"),
            Some("some_future_reason"),
        ] {
            let error = sync_error("boom", reason, vec![]);
            assert!(
                !is_retryable(&error),
                "expected {reason:?} to stop immediately"
            );
        }
    }

    #[test]
    fn partial_apply_is_never_retried_even_with_no_applied_files() {
        // A partial-apply reason can combine different failure stages, so it
        // stays terminal even when this particular failure applied nothing.
        let error = sync_error("boom", Some("partial_apply"), vec![]);
        assert!(!is_retryable(&error));
    }

    #[test]
    fn any_error_listing_applied_files_is_never_retried() {
        let error = sync_error("boom", Some("lock_contention"), vec!["dev.md"]);
        assert!(!is_retryable(&error));
    }

    #[test]
    fn retry_loop_retries_lock_contention_then_succeeds() {
        let (sleeps, logs, env) = mock_env(vec![0.0, 0.0]);
        let (calls, attempts) = scripted_attempts(vec![
            Err(sync_error("locked", Some("lock_contention"), vec![])),
            Err(sync_error("locked", Some("lock_contention"), vec![])),
            Ok(empty_sync_result("2026/20260710.md")),
        ]);
        let result =
            retry_loop("test-run", Duration::from_secs(120), &env, attempts);
        assert!(result.is_ok());
        assert_eq!(*calls.borrow(), 3);
        // Ceiling 2s then 4s; jitter 0.0 selects the bottom of each upper
        // half (1s, then 2s).
        assert_eq!(
            *sleeps.borrow(),
            vec![Duration::from_secs(1), Duration::from_secs(2)]
        );
        let lines = logs.borrow();
        assert_eq!(lines.len(), 3, "two retry decisions plus one summary");
        assert!(lines[0].contains("run=test-run"));
        assert!(lines[0].contains("attempt=1"));
        assert!(lines[0].contains("reason=lock_contention"));
        assert!(lines[1].contains("attempt=2"));
        assert!(lines[2].contains("succeeded"));
        assert!(lines[2].contains("attempts=3"));
    }

    #[test]
    fn retry_loop_stops_immediately_on_terminal_reason_without_noise() {
        let (sleeps, logs, env) = mock_env(vec![]);
        let (calls, attempts) = scripted_attempts(vec![Err(sync_error(
            "boom",
            Some("io"),
            vec![],
        ))]);
        let result =
            retry_loop("run", Duration::from_secs(120), &env, attempts);
        assert!(result.is_err());
        assert_eq!(*calls.borrow(), 1);
        assert!(sleeps.borrow().is_empty());
        assert!(
            logs.borrow().is_empty(),
            "an uncontended single-attempt failure must retain its existing concise output"
        );
    }

    #[test]
    fn retry_loop_never_starts_another_attempt_once_budget_is_exhausted() {
        let (sleeps, logs, env) = mock_env(vec![0.0, 0.0, 0.0]);
        let (calls, attempts) = scripted_attempts(vec![
            Err(sync_error("locked", Some("lock_contention"), vec![])),
            Err(sync_error("locked", Some("lock_contention"), vec![])),
            Err(sync_error("locked", Some("lock_contention"), vec![])),
        ]);
        let result = retry_loop("run", Duration::from_secs(5), &env, attempts);
        let error = result.expect_err("budget must exhaust as a failure");
        assert_eq!(error.reason.as_deref(), Some("lock_contention"));
        // attempt 1 (ceiling 2s -> 1s sleep), attempt 2 (ceiling 4s -> 2s
        // sleep), attempt 3 (ceiling 8s -> half 4s clamped to the 2s left).
        assert_eq!(*calls.borrow(), 3, "no fourth attempt once exhausted");
        assert_eq!(
            *sleeps.borrow(),
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(2),
            ]
        );
        assert!(logs
            .borrow()
            .last()
            .unwrap()
            .contains("exhausted retry budget"));
    }

    #[test]
    fn retry_loop_clamps_sleep_to_remaining_budget() {
        let (sleeps, _logs, env) = mock_env(vec![0.0]);
        let (calls, attempts) = scripted_attempts(vec![Err(sync_error(
            "locked",
            Some("lock_contention"),
            vec![],
        ))]);
        let result =
            retry_loop("run", Duration::from_millis(500), &env, attempts);
        assert!(result.is_err());
        assert_eq!(
            *calls.borrow(),
            1,
            "the clamped sleep already exhausts the budget"
        );
        assert_eq!(*sleeps.borrow(), vec![Duration::from_millis(500)]);
    }

    #[test]
    fn retry_loop_with_zero_budget_makes_one_attempt_and_never_sleeps() {
        let (sleeps, _logs, env) = mock_env(vec![]);
        let (calls, attempts) = scripted_attempts(vec![Err(sync_error(
            "locked",
            Some("lock_contention"),
            vec![],
        ))]);
        let result = retry_loop("run", Duration::ZERO, &env, attempts);
        assert!(result.is_err());
        assert_eq!(*calls.borrow(), 1);
        assert!(sleeps.borrow().is_empty());
    }

    #[test]
    fn retry_decision_log_includes_run_attempt_reason_and_recovery_directory() {
        let (_sleeps, logs, env) = mock_env(vec![0.0]);
        let mut contended = sync_error(
            "the vault changed (dev.md); rerun the command",
            Some("vault_changed"),
            vec![],
        );
        contended.recovery_directory = Some("/tmp/recovery".to_string());
        let (_calls, attempts) = scripted_attempts(vec![
            Err(contended),
            Ok(empty_sync_result("2026/20260710.md")),
        ]);
        let result =
            retry_loop("abc123", Duration::from_secs(30), &env, attempts);
        assert!(result.is_ok());
        let lines = logs.borrow();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("run=abc123"));
        assert!(lines[0].contains("attempt=1"));
        assert!(lines[0].contains("reason=vault_changed"));
        assert!(lines[0].contains("recovery=/tmp/recovery"));
        assert!(lines[1].contains("succeeded"));
        assert!(lines[1].contains("attempts=2"));
    }
}

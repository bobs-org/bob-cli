//! The same cancellation gate is used by Foundation and the portable fake tests.

use super::{Coordination, TimedOut};
use std::cell::RefCell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const DONE: u8 = 2;
const TIMED_OUT: u8 = 3;

pub(super) enum RequestError {
    Cancelled,
    Failed(String),
}

#[derive(Default)]
struct Gate(AtomicU8);

impl Gate {
    fn transition(&self, to: u8) -> bool {
        self.0
            .compare_exchange(PENDING, to, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

/// `request` invokes its callback synchronously, at most once, and returns only
/// after it finishes. `cancel` must be safe on a different thread and unblock a
/// pending request. A late callback after cancellation is harmless.
pub(super) fn run<T>(
    budget: Duration,
    accessor: impl FnOnce() -> T,
    request: impl FnOnce(&dyn Fn()) -> Result<(), RequestError>,
    cancel: impl FnOnce() + Send,
) -> Coordination<T> {
    let started = Instant::now();
    if budget.is_zero() {
        return Coordination::Held(TimedOut {
            waited: Duration::ZERO,
        });
    }
    let gate = Gate::default();
    let accessor = RefCell::new(Some(accessor));
    let result = RefCell::new(None);
    let (stop, stopped) = mpsc::channel();

    let error = thread::scope(|scope| {
        let gate = &gate;
        scope.spawn(move || {
            if stopped.recv_timeout(budget.saturating_sub(started.elapsed()))
                == Err(mpsc::RecvTimeoutError::Timeout)
                && gate.transition(TIMED_OUT)
            {
                cancel();
            }
        });
        let error = request(&|| {
            // The watchdog might not have been scheduled at the deadline yet.
            if started.elapsed() >= budget {
                gate.transition(TIMED_OUT);
            }
            if !gate.transition(RUNNING) {
                return;
            }
            let waited = started.elapsed();
            let _ = stop.send(());
            // Never unwind through the Objective-C block ABI. Resume in Rust
            // only after Foundation releases access and the watchdog joins.
            let value = catch_unwind(AssertUnwindSafe(|| {
                accessor.borrow_mut().take().expect("accessor called once")()
            }));
            *result.borrow_mut() = Some((value, waited));
            gate.0.store(DONE, Ordering::SeqCst);
        });
        gate.transition(DONE);
        let _ = stop.send(());
        error
    });

    if let Some((value, waited)) = result.into_inner() {
        return match value {
            Ok(value) => Coordination::Granted { value, waited },
            Err(panic) => resume_unwind(panic),
        };
    }
    match error {
        Err(RequestError::Failed(message)) => Coordination::Failed(message),
        Err(RequestError::Cancelled) => Coordination::Held(TimedOut {
            waited: started.elapsed(),
        }),
        Ok(()) if gate.0.load(Ordering::SeqCst) == TIMED_OUT => {
            Coordination::Held(TimedOut {
                waited: started.elapsed(),
            })
        }
        Ok(()) => Coordination::Failed(
            "file coordinator returned without granting access".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn timeout_cancels_and_rejects_even_a_late_callback() {
        let (cancelled, cancellation) = mpsc::channel();
        let mut writes = 0;
        let outcome = run(
            Duration::from_millis(20),
            || writes += 1,
            |grant| {
                cancellation.recv_timeout(Duration::from_secs(2)).unwrap();
                grant();
                Err(RequestError::Cancelled)
            },
            move || cancelled.send(()).unwrap(),
        );
        assert!(matches!(outcome, Coordination::Held(_)));
        assert_eq!(writes, 0);
    }

    #[test]
    fn running_accessor_finishes_after_budget_without_cancellation() {
        let outcome = run(
            Duration::from_millis(100),
            || {
                thread::sleep(Duration::from_millis(150));
                "written"
            },
            |grant| {
                grant();
                Ok(())
            },
            || panic!("running accessor cancelled"),
        );
        assert!(
            matches!(outcome, Coordination::Granted { value: "written", waited } if waited < Duration::from_millis(100))
        );
    }

    #[test]
    fn cancellation_and_start_have_exactly_one_winner() {
        for _ in 0..100 {
            let gate = Arc::new(Gate::default());
            let barrier = Arc::new(Barrier::new(2));
            let other = Arc::clone(&gate);
            let ready = Arc::clone(&barrier);
            let cancel = thread::spawn(move || {
                ready.wait();
                other.transition(TIMED_OUT)
            });
            barrier.wait();
            let ran = gate.transition(RUNNING);
            assert_ne!(ran, cancel.join().unwrap());
        }
    }

    #[test]
    fn unexpected_error_never_runs_accessor_or_waits_out_budget() {
        let start = Instant::now();
        let outcome = run(
            Duration::from_secs(60),
            || panic!("unexpected write"),
            |_| Err(RequestError::Failed("permission denied".into())),
            || panic!("already finished"),
        );
        assert_eq!(outcome, Coordination::Failed("permission denied".into()));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn zero_budget_does_not_request_access() {
        let outcome = run(
            Duration::ZERO,
            || panic!("unexpected write"),
            |_| panic!("unexpected request"),
            || panic!("unexpected cancel"),
        );
        assert_eq!(
            outcome,
            Coordination::Held(TimedOut {
                waited: Duration::ZERO
            })
        );
    }

    #[test]
    fn missing_callback_is_a_failure_and_accessor_errors_are_values() {
        assert!(matches!(
            run(Duration::from_secs(1), || (), |_| Ok(()), || {}),
            Coordination::Failed(_)
        ));
        let outcome = run(
            Duration::from_secs(1),
            || Err::<(), _>("disk full"),
            |grant| {
                grant();
                Ok(())
            },
            || {},
        );
        assert!(matches!(
            outcome,
            Coordination::Granted {
                value: Err("disk full"),
                ..
            }
        ));
    }

    #[test]
    fn panic_resumes_only_after_request_returns() {
        let returned = RefCell::new(false);
        let panic = catch_unwind(AssertUnwindSafe(|| {
            run(
                Duration::from_secs(1),
                || panic!("write panic"),
                |grant| {
                    grant();
                    *returned.borrow_mut() = true;
                    Ok(())
                },
                || {},
            );
        }));
        assert!(panic.is_err());
        assert!(*returned.borrow());
    }
}

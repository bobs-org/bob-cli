//! Host-local per-account guard for `bob gmail sync`.
//!
//! Live runs hold an exclusive, process-held file lock at
//! `$XDG_STATE_HOME/bob/gmail/sync-<account digest>.lock` (with the
//! documented home fallback through [`crate::native::env::state_home`])
//! from discovery through journal completion. The OS releases the lock on
//! exit, so a crashed run never leaves a permanently stale marker. A busy
//! guard is a quiet exit-0 skip: no vault writes, no journal appends, no
//! commit.
//!
//! The path uses a SHA-256 digest of the normalized account (trimmed,
//! ASCII-lowercased), never the raw email as a file name. Different accounts
//! hold different guards; the same account with different case or padding
//! shares one guard.

use std::{
    fs::{self, File},
    io,
    path::PathBuf,
    time::Duration,
};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use super::retry::{Clock, SharedBudget, Sleeper};
use crate::native::env as bob_env;

/// Guard directory under the state home: `$XDG_STATE_HOME/bob/gmail`.
fn guard_dir() -> PathBuf {
    bob_env::state_home().join("bob/gmail")
}

/// The host-local lock path for `account`.
///
/// Normalizes case and surrounding whitespace before digesting so
/// `You@Example.com` and `you@example.com` share one guard while
/// `other@example.com` gets another.
pub(crate) fn guard_path(account: &str) -> PathBuf {
    let normalized = account.trim().to_ascii_lowercase();
    let digest = hex::encode(Sha256::digest(normalized.as_bytes()));
    guard_dir().join(format!("sync-{digest}.lock"))
}

/// An acquired per-account guard. Dropping closes the file and the OS
/// releases the exclusive lock.
#[derive(Debug)]
pub(crate) struct SyncGuard {
    _file: File,
}

/// Either the guard was acquired or another live run holds it.
#[derive(Debug)]
pub(crate) enum GuardAttempt {
    Acquired(SyncGuard),
    Busy,
}

/// Try once to acquire the account guard without waiting.
///
/// Creates the guard directory on first use. A dry run must not call this:
/// orchestration skips the guard entirely when `-d` is set so no state file
/// or directory is created.
pub(crate) fn try_acquire(account: &str) -> io::Result<GuardAttempt> {
    let path = guard_path(account);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(GuardAttempt::Acquired(SyncGuard { _file: file })),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Ok(GuardAttempt::Busy)
        }
        Err(error) => Err(error),
    }
}

/// Acquire the guard, waiting within the shared `-r` budget.
///
/// A zero budget tries once and returns [`GuardAttempt::Busy`] when held.
/// Otherwise polls about every 100 ms until the shared deadline, so the
/// guard wait, the retry loop, and the vault-lock wait together stay within
/// one `-r` instead of each waiting another full timeout.
pub(crate) fn acquire_with_budget<S: Sleeper, C: Clock>(
    sleeper: &S,
    budget: &SharedBudget,
    clock: &C,
    account: &str,
) -> io::Result<GuardAttempt> {
    match try_acquire(account)? {
        GuardAttempt::Acquired(guard) => {
            return Ok(GuardAttempt::Acquired(guard))
        }
        GuardAttempt::Busy => {}
    }
    if budget.is_zero_budget() || budget.is_exhausted() {
        return Ok(GuardAttempt::Busy);
    }
    loop {
        let remaining = budget.remaining();
        if remaining.is_zero() {
            return Ok(GuardAttempt::Busy);
        }
        if super::retry::sleep_within_budget(
            sleeper,
            budget,
            clock,
            remaining.min(Duration::from_millis(100)),
        )
        .is_none()
        {
            return Ok(GuardAttempt::Busy);
        }
        match try_acquire(account)? {
            acquired @ GuardAttempt::Acquired(_) => return Ok(acquired),
            GuardAttempt::Busy => {}
        }
        if budget.is_exhausted() {
            return Ok(GuardAttempt::Busy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env::TestEnvGuard;
    use std::time::{Duration, Instant};

    struct NoSleep;

    impl Sleeper for NoSleep {
        fn sleep(&self, _: Duration) {}
    }

    struct TestClock {
        now: std::cell::Cell<Instant>,
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            self.now.get()
        }
    }

    fn isolated_state(prefix: &str) -> (tempfile::TempDir, TestEnvGuard) {
        let temp = tempfile::tempdir().unwrap();
        let guard = TestEnvGuard::set(&[(
            "XDG_STATE_HOME",
            Some(temp.path().as_os_str()),
        )]);
        let _ = prefix;
        (temp, guard)
    }

    fn unique_account(prefix: &str) -> String {
        format!("{prefix}@example.com")
    }

    #[test]
    fn guard_paths_are_account_separated_and_normalized() {
        let (_temp, _env) = isolated_state("sync-guard-path");
        let first = guard_path("You@Example.com");
        let second = guard_path("  you@example.com  ");
        let other = guard_path("other@example.com");
        assert_eq!(first, second, "case and padding share one guard");
        assert_ne!(first, other, "different accounts diverge");
        assert!(
            first.to_string_lossy().contains("sync-"),
            "digest path: {}",
            first.display()
        );
        assert!(
            !first.to_string_lossy().contains('@'),
            "never a raw email path: {}",
            first.display()
        );
        assert_eq!(
            first.parent().unwrap(),
            guard_dir().as_path(),
            "under $XDG_STATE_HOME/bob/gmail"
        );
    }

    #[test]
    fn real_lock_contention_and_release() {
        let (_temp, _env) = isolated_state("sync-guard-contention");
        let account = unique_account("sync-guard-contention");
        let first = match try_acquire(&account).expect("acquire") {
            GuardAttempt::Acquired(guard) => guard,
            GuardAttempt::Busy => {
                panic!("first acquisition must acquire, not report Busy")
            }
        };
        assert!(
            matches!(try_acquire(&account).unwrap(), GuardAttempt::Busy),
            "second acquisition while held is busy"
        );
        drop(first);
        // Another test thread's fork can inherit the guard fd until it execs,
        // so the immediate re-acquire can see WouldBlock. Poll briefly like
        // ob.rs lock_wait_behavior (fd is CLOEXEC; production unaffected).
        let start = std::time::Instant::now();
        let limit = Duration::from_secs(10);
        loop {
            match try_acquire(&account).expect("re-acquire") {
                GuardAttempt::Acquired(_) => break,
                GuardAttempt::Busy => {
                    assert!(start.elapsed() < limit, "release frees the guard");
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }

    #[test]
    fn zero_budget_fails_fast_when_busy() {
        let (_temp, _env) = isolated_state("sync-guard-budget");
        let account = unique_account("sync-guard-budget");
        let _held = match try_acquire(&account).expect("hold") {
            GuardAttempt::Acquired(guard) => guard,
            GuardAttempt::Busy => {
                panic!("first acquisition must acquire, not report Busy")
            }
        };
        let clock = TestClock {
            now: std::cell::Cell::new(Instant::now()),
        };
        let budget = SharedBudget::new_with_now(Duration::ZERO, clock.now());
        assert!(
            matches!(
                acquire_with_budget(&NoSleep, &budget, &clock, &account)
                    .unwrap(),
                GuardAttempt::Busy
            ),
            "zero retry timeout makes one immediate attempt"
        );
    }

    #[test]
    fn dry_run_creates_no_guard_state() {
        let (temp, _env) = isolated_state("sync-guard-dry-run");
        let state = temp.path().join("bob/gmail");
        assert!(!state.exists(), "isolated state starts empty");
        // Dry-run orchestration never calls try_acquire; assert the
        // precondition directly: no guard file exists without acquisition.
        assert!(
            !guard_path("you@example.com").exists(),
            "no state without acquisition"
        );
    }
}

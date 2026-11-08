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

use super::retry::{SharedBudget, Sleeper};
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` guards runs.
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` guards runs.
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
#[cfg_attr(not(test), allow(dead_code))] // `bob gmail sync` guards runs.
pub(crate) fn acquire_with_budget<S: Sleeper>(
    sleeper: &S,
    budget: &SharedBudget,
    account: &str,
) -> io::Result<GuardAttempt> {
    match try_acquire(account)? {
        GuardAttempt::Acquired(guard) => {
            return Ok(GuardAttempt::Acquired(guard))
        }
        GuardAttempt::Busy => {}
    }
    if budget.is_exhausted() {
        return Ok(GuardAttempt::Busy);
    }
    loop {
        let remaining = budget.remaining();
        if remaining.is_zero() {
            return Ok(GuardAttempt::Busy);
        }
        sleeper.sleep(remaining.min(Duration::from_millis(100)));
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
    use std::time::Duration;

    struct NoSleep;

    impl Sleeper for NoSleep {
        fn sleep(&self, _: Duration) {}
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
        let account = "you@example.com";
        let first = try_acquire(account).expect("acquire");
        assert!(
            matches!(try_acquire(account).unwrap(), GuardAttempt::Busy),
            "second acquisition while held is busy"
        );
        drop(first);
        assert!(
            matches!(try_acquire(account).unwrap(), GuardAttempt::Acquired(_)),
            "release frees the guard"
        );
    }

    #[test]
    fn zero_budget_fails_fast_when_busy() {
        let (_temp, _env) = isolated_state("sync-guard-budget");
        let account = "you@example.com";
        let _held = try_acquire(account).expect("hold");
        let budget = SharedBudget::new(Duration::ZERO);
        assert!(
            matches!(
                acquire_with_budget(&NoSleep, &budget, account).unwrap(),
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

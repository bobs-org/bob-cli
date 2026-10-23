//! Save-before-write coordination for files that may be open in an editor.
//!
//! The budget bounds waiting for access, not the accessor's execution. A timeout
//! never interrupts an accessor that has started. Callers must read the current
//! bytes inside the accessor: a presenter may have saved while we were waiting.

use std::path::Path;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Coordination<T> {
    Granted { value: T, waited: Duration },
    Held(TimedOut),
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TimedOut {
    pub waited: Duration,
}

/// Whether this platform's implementation asks file presenters to save/reload.
pub(crate) const IS_COORDINATED: bool = cfg!(target_os = "macos");

/// Run `accessor` once, synchronously, under a `.forMerging` write on macOS.
///
/// `Held` and `Failed` guarantee the accessor did not run. An accessor's own
/// error belongs in `T` (usually a `Result`), and is still reported as `Granted`.
/// Other platforms call the accessor directly, even with a zero budget.
pub(crate) fn coordinate_merging_write<T>(
    path: &Path,
    budget: Duration,
    accessor: impl FnOnce() -> T,
) -> Coordination<T> {
    #[cfg(target_os = "macos")]
    {
        macos::coordinate(path, budget, accessor)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, budget);
        Coordination::Granted {
            value: accessor(),
            waited: Duration::ZERO,
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
mod attempt;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(test, target_os = "macos"))]
mod macos_tests;

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn direct_write_is_once_synchronous_and_keeps_accessor_errors() {
        let mut calls = 0;
        let result = coordinate_merging_write(
            Path::new("unused"),
            Duration::ZERO,
            || {
                calls += 1;
                Err::<(), _>("write failed")
            },
        );
        assert_eq!(calls, 1);
        assert_eq!(
            result,
            Coordination::Granted {
                value: Err("write failed"),
                waited: Duration::ZERO,
            }
        );
        assert!(!IS_COORDINATED);
    }
}

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

/// Notifications paired with a physical move while its coordinated write is
/// still active. The macOS implementation keeps its Foundation objects behind
/// this interface; portable callers use the same filesystem operation with a
/// no-op notifier.
pub(crate) trait MoveNotifier {
    fn will_move(&self, from: &Path, to: &Path);
    fn did_move(&self, from: &Path, to: &Path);
}

/// The only move token exposed to the synchronous accessor. It can move the
/// planned source to the planned destination once, and move it back once, but
/// cannot be retained as proof of access after the accessor returns.
pub(crate) struct MoveContext<'a> {
    from: &'a Path,
    to: &'a Path,
    at_destination: bool,
    notifier: &'a dyn MoveNotifier,
}

impl MoveContext<'_> {
    pub(crate) fn move_forward(&mut self) -> std::io::Result<()> {
        if self.at_destination {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "planned file move already completed",
            ));
        }
        self.move_no_replace(self.from, self.to)?;
        self.at_destination = true;
        Ok(())
    }

    pub(crate) fn move_back(&mut self) -> std::io::Result<()> {
        if !self.at_destination {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "planned file move has not completed",
            ));
        }
        self.move_no_replace(self.to, self.from)?;
        self.at_destination = false;
        Ok(())
    }

    fn move_no_replace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        if from == to {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "source and destination paths are identical",
            ));
        }
        self.notifier.will_move(from, to);
        match crate::native::highlights_ref::rename_no_replace(from, to) {
            Ok(()) => {
                self.notifier.did_move(from, to);
                Ok(())
            }
            Err(error) => {
                // `didMoveToURL` balances `willMoveToURL`. Report the item at
                // its unchanged URL so presenters are never told that a move
                // succeeded when the no-clobber filesystem operation failed.
                self.notifier.did_move(from, from);
                Err(error)
            }
        }
    }
}

struct NoopMoveNotifier;

impl MoveNotifier for NoopMoveNotifier {
    fn will_move(&self, _from: &Path, _to: &Path) {}
    fn did_move(&self, _from: &Path, _to: &Path) {}
}

static NOOP_MOVE_NOTIFIER: NoopMoveNotifier = NoopMoveNotifier;

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

/// Run a no-clobber move under a single `.forMerging | .forMoving` write on
/// macOS. Both the forward move and a possible inverse move must happen inside
/// the accessor. Other platforms run the same filesystem operation directly.
pub(crate) fn coordinate_moving_write<T>(
    from: &Path,
    to: &Path,
    budget: Duration,
    accessor: impl FnOnce(&mut MoveContext<'_>) -> T,
) -> Coordination<T> {
    #[cfg(target_os = "macos")]
    {
        macos::coordinate_move(from, to, budget, accessor)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = budget;
        let mut context = MoveContext {
            from,
            to,
            at_destination: false,
            notifier: &NOOP_MOVE_NOTIFIER,
        };
        Coordination::Granted {
            value: accessor(&mut context),
            waited: Duration::ZERO,
        }
    }
}

/// Perform a planned move without OS coordination. Used when the feature is
/// disabled or an enclosing transaction already owns the coordinated scope.
pub(crate) fn move_directly<T>(
    from: &Path,
    to: &Path,
    accessor: impl FnOnce(&mut MoveContext<'_>) -> T,
) -> Coordination<T> {
    let mut context = MoveContext {
        from,
        to,
        at_destination: false,
        notifier: &NOOP_MOVE_NOTIFIER,
    };
    Coordination::Granted {
        value: accessor(&mut context),
        waited: Duration::ZERO,
    }
}

#[cfg(test)]
pub(crate) fn move_with_notifier_for_test<T>(
    from: &Path,
    to: &Path,
    notifier: &dyn MoveNotifier,
    accessor: impl FnOnce(&mut MoveContext<'_>) -> T,
) -> T {
    let mut context = MoveContext {
        from,
        to,
        at_destination: false,
        notifier,
    };
    accessor(&mut context)
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

    #[test]
    fn direct_move_scope_performs_forward_and_inverse_no_clobber_moves() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("source.pdf");
        let to = dir.path().join("destination.pdf");
        std::fs::write(&from, b"saved before access").unwrap();

        let result =
            coordinate_moving_write(&from, &to, Duration::ZERO, |scope| {
                scope.move_forward().unwrap();
                assert!(!from.exists());
                assert_eq!(std::fs::read(&to).unwrap(), b"saved before access");
                scope.move_back().unwrap();
                assert!(!to.exists());
                std::fs::read(&from).unwrap()
            });

        assert_eq!(
            result,
            Coordination::Granted {
                value: b"saved before access".to_vec(),
                waited: Duration::ZERO,
            }
        );
    }

    #[test]
    fn direct_move_scope_never_replaces_destination() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("source.pdf");
        let to = dir.path().join("destination.pdf");
        std::fs::write(&from, b"source").unwrap();
        std::fs::write(&to, b"occupant").unwrap();

        let outcome = move_directly(&from, &to, |scope| scope.move_forward());

        let Coordination::Granted { value, .. } = outcome else {
            panic!("direct scope must run its accessor")
        };
        assert_eq!(
            value.unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&from).unwrap(), b"source");
        assert_eq!(std::fs::read(&to).unwrap(), b"occupant");
    }
}

use super::attempt::{self, RequestError};
use super::{Coordination, MoveContext, MoveNotifier};
use block2::StackBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::AnyThread;
use objc2_foundation::{
    NSFileCoordinator, NSFileCoordinatorWritingOptions, NSURL,
};
use std::cell::Cell;
use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

// NSFileCoordinator is deliberately !Send/!Sync. Only cancel is documented as
// callable from another thread. Do not expose the coordinator through this type.
struct CancelHandle(Retained<NSFileCoordinator>);

// SAFETY: The retained reference keeps the coordinator alive until the watchdog
// joins. The only operation available on that thread is Apple's thread-safe
// cancel(); all other coordinator operations stay on the requesting thread.
unsafe impl Send for CancelHandle {}

impl CancelHandle {
    fn cancel(self) {
        autoreleasepool(|_| self.0.cancel());
    }
}

struct CoordinatedMoveNotifier<'a> {
    coordinator: &'a NSFileCoordinator,
    from: &'a Path,
    to: &'a Path,
    from_url: &'a NSURL,
    to_url: &'a NSURL,
}

impl CoordinatedMoveNotifier<'_> {
    fn url_for(&self, path: &Path) -> &NSURL {
        if path == self.from {
            self.from_url
        } else {
            debug_assert_eq!(path, self.to);
            self.to_url
        }
    }
}

impl MoveNotifier for CoordinatedMoveNotifier<'_> {
    fn will_move(&self, from: &Path, to: &Path) {
        self.coordinator
            .itemAtURL_willMoveToURL(self.url_for(from), self.url_for(to));
    }

    fn did_move(&self, from: &Path, to: &Path) {
        self.coordinator
            .itemAtURL_didMoveToURL(self.url_for(from), self.url_for(to));
    }
}

pub(super) fn coordinate<T>(
    path: &Path,
    budget: Duration,
    accessor: impl FnOnce() -> T,
) -> Coordination<T> {
    autoreleasepool(|_| {
        let Some(url) = NSURL::from_file_path(path) else {
            return Coordination::Failed(
                "cannot form a file URL for coordination".into(),
            );
        };
        let coordinator = NSFileCoordinator::initWithFilePresenter(
            NSFileCoordinator::alloc(),
            None,
        );
        let cancel = CancelHandle(coordinator.clone());
        attempt::run(
            budget,
            accessor,
            |grant| {
                let moved = Cell::new(false);
                let writer = StackBlock::new(|actual_url: NonNull<NSURL>| {
                    // SAFETY: Foundation supplies a live, nonnull NSURL for the
                    // duration of this synchronous accessor.
                    let actual_url = unsafe { actual_url.as_ref() };
                    if actual_url.to_file_path() != url.to_file_path() {
                        // This API's closure captures its path. Never let it write
                        // a stale path when a presenter moved the coordinated file.
                        moved.set(true);
                        return;
                    }
                    grant();
                });
                let mut error = None;
                coordinator
                    .coordinateWritingItemAtURL_options_error_byAccessor(
                        &url,
                        NSFileCoordinatorWritingOptions::ForMerging,
                        Some(&mut error),
                        &writer,
                    );
                if let Some(error) = error {
                    if error.domain().to_string() == "NSCocoaErrorDomain"
                        && error.code() == 3072
                    {
                        Err(RequestError::Cancelled)
                    } else {
                        Err(RequestError::Failed(format!(
                            "{} ({} {})",
                            error.localizedDescription(),
                            error.domain(),
                            error.code()
                        )))
                    }
                } else if moved.get() {
                    Err(RequestError::Failed("file moved during coordination; retry with its current path".into()))
                } else {
                    Ok(())
                }
            },
            move || cancel.cancel(),
        )
    })
}

pub(super) fn coordinate_move<T>(
    from: &Path,
    to: &Path,
    budget: Duration,
    accessor: impl FnOnce(&mut MoveContext<'_>) -> T,
) -> Coordination<T> {
    autoreleasepool(|_| {
        if from == to {
            return Coordination::Failed(
                "source and destination paths are identical".into(),
            );
        }
        let Some(from_url) = NSURL::from_file_path(from) else {
            return Coordination::Failed(
                "cannot form a file URL for move coordination source".into(),
            );
        };
        let Some(to_url) = NSURL::from_file_path(to) else {
            return Coordination::Failed(
                "cannot form a file URL for move coordination destination"
                    .into(),
            );
        };
        let coordinator = NSFileCoordinator::initWithFilePresenter(
            NSFileCoordinator::alloc(),
            None,
        );
        let cancel = CancelHandle(coordinator.clone());
        let notifications = CoordinatedMoveNotifier {
            coordinator: &coordinator,
            from,
            to,
            from_url: &from_url,
            to_url: &to_url,
        };
        attempt::run(
            budget,
            || {
                let mut context = MoveContext {
                    from,
                    to,
                    at_destination: false,
                    notifier: &notifications,
                };
                accessor(&mut context)
            },
            |grant| {
                let actual_url_changed = Cell::new(false);
                let writer = StackBlock::new(|actual_url: NonNull<NSURL>| {
                    // SAFETY: Foundation supplies a live, nonnull NSURL for
                    // the duration of this synchronous accessor.
                    let actual_url = unsafe { actual_url.as_ref() };
                    if !start_accessor_if_planned(actual_url, &from_url, grant)
                    {
                        // Never let a caller move the planned paths based on
                        // a presenter relocation it did not prepare for.
                        actual_url_changed.set(true);
                    }
                });
                let mut error = None;
                coordinator
                    .coordinateWritingItemAtURL_options_error_byAccessor(
                        &from_url,
                        NSFileCoordinatorWritingOptions::ForMerging
                            | NSFileCoordinatorWritingOptions::ForMoving,
                        Some(&mut error),
                        &writer,
                    );
                if let Some(error) = error {
                    if error.domain().to_string() == "NSCocoaErrorDomain"
                        && error.code() == 3072
                    {
                        Err(RequestError::Cancelled)
                    } else {
                        Err(RequestError::Failed(format!(
                            "{} ({} {})",
                            error.localizedDescription(),
                            error.domain(),
                            error.code()
                        )))
                    }
                } else if actual_url_changed.get() {
                    Err(RequestError::Failed(
                        "file moved during coordination; retry with its current path"
                            .into(),
                    ))
                } else {
                    Ok(())
                }
            },
            move || cancel.cancel(),
        )
    })
}

fn actual_url_matches(actual: &NSURL, planned: &NSURL) -> bool {
    actual.to_file_path() == planned.to_file_path()
}

fn start_accessor_if_planned(
    actual: &NSURL,
    planned: &NSURL,
    grant: impl FnOnce(),
) -> bool {
    if !actual_url_matches(actual, planned) {
        return false;
    }
    grant();
    true
}

#[cfg(test)]
pub(super) fn start_accessor_for_test(
    actual: &Path,
    planned: &Path,
    grant: impl FnOnce(),
) -> bool {
    let (Some(actual), Some(planned)) = (
        NSURL::from_file_path(actual),
        NSURL::from_file_path(planned),
    ) else {
        return false;
    };
    start_accessor_if_planned(&actual, &planned, grant)
}

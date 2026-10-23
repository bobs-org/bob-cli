use super::attempt::{self, RequestError};
use super::Coordination;
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

//! One worker-count source for every thread pool sized from the host.
//!
//! Pools that would otherwise start one thread per logical CPU ask
//! [`available`] instead. `BOB_MAX_THREADS` caps that count for the
//! whole `bob` process, so a test harness that spawns many `bob`
//! processes at once (or a busy shared host) can keep each one from
//! fanning out to every CPU. Per-pool clamps and job-size bounds still
//! apply on top of this value. Unset, the count is exactly
//! `available_parallelism()`.

use std::thread;

use super::env;

/// Environment variable that caps every host-sized worker pool.
pub(crate) const MAX_THREADS_ENV: &str = "BOB_MAX_THREADS";

/// Logical CPUs available to this process, capped by `BOB_MAX_THREADS`.
///
/// `fallback` is the pool's own count when the host cannot report its
/// parallelism; the cap applies to it too. A missing, empty, zero, or
/// non-numeric `BOB_MAX_THREADS` is ignored.
pub(crate) fn available(fallback: usize) -> usize {
    let host = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(fallback);
    capped(host, env::var(MAX_THREADS_ENV).ok().as_deref())
}

fn capped(host: usize, raw_cap: Option<&str>) -> usize {
    match raw_cap.and_then(parse_cap) {
        Some(cap) => host.min(cap),
        None => host,
    }
}

fn parse_cap(raw: &str) -> Option<usize> {
    raw.trim().parse::<usize>().ok().filter(|cap| *cap > 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    use crate::native::env::TestEnvGuard;

    fn host() -> usize {
        thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(7)
    }

    #[test]
    fn unset_cap_keeps_the_host_count() {
        let _guard = TestEnvGuard::set(&[(MAX_THREADS_ENV, None)]);
        assert_eq!(available(7), host());
    }

    #[test]
    fn positive_cap_bounds_the_host_count() {
        let _guard =
            TestEnvGuard::set(&[(MAX_THREADS_ENV, Some(OsStr::new("1")))]);
        assert_eq!(available(7), 1);
    }

    #[test]
    fn cap_above_the_host_count_changes_nothing() {
        let _guard = TestEnvGuard::set(&[(
            MAX_THREADS_ENV,
            Some(OsStr::new("1000000")),
        )]);
        assert_eq!(available(7), host());
    }

    #[test]
    fn invalid_caps_are_ignored() {
        for raw in ["", "0", "-2", "two", "1.5", "99999999999999999999999"] {
            assert_eq!(capped(12, Some(raw)), 12, "cap {raw:?}");
        }
        assert_eq!(capped(12, None), 12);
    }

    #[test]
    fn caps_parse_with_surrounding_whitespace() {
        assert_eq!(capped(12, Some(" 3 ")), 3);
        assert_eq!(capped(2, Some("3")), 2);
    }
}

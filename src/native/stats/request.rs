use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::native::env as bob_env;

/// Which metrics a report should include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MetricSelection {
    Overview,
    AllVisible,
    Explicit(Vec<String>),
}

/// Requested grouping. `Auto` picks day/week/month from the resolved
/// complete-day count in the series phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grouping {
    Auto,
    Day,
    Week,
    Month,
}

/// Inputs to the stats engine. `now` is captured once so `BOB_NOW`
/// applies for the whole request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatsRequest {
    pub(crate) vault: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) selection: MetricSelection,
    pub(crate) range: Option<String>,
    pub(crate) grouping: Grouping,
    pub(crate) offset: u32,
    pub(crate) compare: bool,
    pub(crate) now: NaiveDateTime,
}

impl StatsRequest {
    pub(crate) fn new(vault: PathBuf, config: PathBuf) -> Self {
        Self::at(vault, config, bob_env::current_datetime())
    }

    /// Pure constructor for unit tests that pin the clock.
    pub(crate) fn at(
        vault: PathBuf,
        config: PathBuf,
        now: NaiveDateTime,
    ) -> Self {
        Self {
            vault,
            config,
            selection: MetricSelection::Overview,
            range: None,
            grouping: Grouping::Auto,
            offset: 0,
            compare: false,
            now,
        }
    }

    pub(crate) fn with_now(mut self, now: NaiveDateTime) -> Self {
        self.now = now;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use chrono::NaiveDate;

    use super::*;
    use crate::native::env::TestEnvGuard;

    #[test]
    fn new_reads_bob_now_once() {
        let _guard = TestEnvGuard::set(&[(
            "BOB_NOW",
            Some(OsStr::new("2026-10-11 09:15:00")),
        )]);
        let request = StatsRequest::new(
            PathBuf::from("/vault"),
            PathBuf::from("/config.yml"),
        );
        assert_eq!(
            request.now,
            NaiveDate::from_ymd_opt(2026, 10, 11)
                .unwrap()
                .and_hms_opt(9, 15, 0)
                .unwrap()
        );
        assert_eq!(request.selection, MetricSelection::Overview);
        assert_eq!(request.grouping, Grouping::Auto);
        assert!(!request.compare);
    }

    #[test]
    fn at_ignores_environment() {
        let _guard = TestEnvGuard::set(&[(
            "BOB_NOW",
            Some(OsStr::new("2026-01-01 00:00:00")),
        )]);
        let now = NaiveDate::from_ymd_opt(2026, 10, 11)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let request = StatsRequest::at(
            PathBuf::from("/vault"),
            PathBuf::from("/config.yml"),
            now,
        );
        assert_eq!(request.now, now);
        let shifted = request.with_now(
            NaiveDate::from_ymd_opt(2026, 10, 12)
                .unwrap()
                .and_hms_opt(8, 0, 0)
                .unwrap(),
        );
        assert_eq!(
            shifted.now.date(),
            NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
        );
        let _ = MetricSelection::AllVisible;
        let _ = [Grouping::Day, Grouping::Week, Grouping::Month];
    }
}

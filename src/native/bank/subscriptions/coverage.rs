//! Per-account import coverage windows for subscription ledgers.
//!
//! Windows come from journal import events. Overlapping and adjacent windows
//! on the same account merge; a gap between windows stays a gap. Coverage is
//! never inferred from min/max dates across those gaps, and one account never
//! covers another. An interval is spanned only when every day from its first
//! date through its due date lies inside one merged window.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::native::bank::journal::JournalSnapshot;

/// Merged inclusive import windows keyed by account alias.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CoverageIndex {
    accounts: BTreeMap<String, Vec<(NaiveDate, NaiveDate)>>,
}

impl CoverageIndex {
    pub(crate) fn from_journal(journal: &JournalSnapshot) -> Self {
        let mut grouped =
            BTreeMap::<String, Vec<(NaiveDate, NaiveDate)>>::new();
        for event in &journal.imports {
            grouped
                .entry(event.account.clone())
                .or_default()
                .push((event.start, event.end));
        }
        let accounts = grouped
            .into_iter()
            .map(|(account, windows)| (account, merge_windows(windows)))
            .collect();
        Self { accounts }
    }

    /// True when `date` falls in any merged window for `account`.
    pub(crate) fn contains(&self, account: &str, date: NaiveDate) -> bool {
        self.accounts.get(account).is_some_and(|windows| {
            windows
                .iter()
                .any(|(start, end)| *start <= date && date <= *end)
        })
    }

    /// True when every day in `start..=end` is covered for `account`.
    pub(crate) fn spans(
        &self,
        account: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> bool {
        if start > end {
            return true;
        }
        self.accounts.get(account).is_some_and(|windows| {
            windows
                .iter()
                .any(|(from, to)| *from <= start && end <= *to)
        })
    }

    /// True when every relevant account spans `start..=end`.
    ///
    /// No relevant accounts means there is no historical account evidence, so
    /// a missing charge must not be declared missed.
    pub(crate) fn spans_all(
        &self,
        accounts: &[String],
        start: NaiveDate,
        end: NaiveDate,
    ) -> bool {
        !accounts.is_empty()
            && accounts
                .iter()
                .all(|account| self.spans(account, start, end))
    }
}

pub(crate) fn merge_windows(
    mut windows: Vec<(NaiveDate, NaiveDate)>,
) -> Vec<(NaiveDate, NaiveDate)> {
    windows.retain(|(start, end)| start <= end);
    windows.sort_unstable();
    let mut merged = Vec::new();
    for (start, end) in windows {
        match merged.last_mut() {
            Some((_, cur_end))
                if start <= *cur_end || cur_end.succ_opt() == Some(start) =>
            {
                if end > *cur_end {
                    *cur_end = end;
                }
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, NaiveDate};

    use crate::native::bank::journal::{ImportCounts, ImportEvent};
    use crate::native::bank::source::SourceKind;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("date")
    }

    fn import(account: &str, start: NaiveDate, end: NaiveDate) -> ImportEvent {
        ImportEvent {
            ts: DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
                .expect("ts"),
            host: "test".to_owned(),
            path: ".bob/bank/test.jsonl".to_owned(),
            line: 1,
            account: account.to_owned(),
            source: SourceKind::Ofx,
            file_sha256: "a".repeat(64),
            start,
            end,
            balance_cents: None,
            balance_asof: None,
            counts: ImportCounts::default(),
        }
    }

    #[test]
    fn merges_overlapping_and_adjacent_windows_and_keeps_gaps() {
        let merged = merge_windows(vec![
            (date(2026, 1, 10), date(2026, 1, 20)),
            (date(2026, 1, 1), date(2026, 1, 10)),
            (date(2026, 1, 21), date(2026, 1, 31)),
            (date(2026, 3, 1), date(2026, 3, 15)),
        ]);
        assert_eq!(
            merged,
            [
                (date(2026, 1, 1), date(2026, 1, 31)),
                (date(2026, 3, 1), date(2026, 3, 15)),
            ]
        );
    }

    #[test]
    fn does_not_cover_another_account_or_a_gap() {
        let mut journal = JournalSnapshot::default();
        journal.imports = vec![
            import("checking", date(2026, 1, 1), date(2026, 1, 31)),
            import("savings", date(2026, 2, 1), date(2026, 2, 28)),
        ];
        let coverage = CoverageIndex::from_journal(&journal);
        assert!(coverage.contains("checking", date(2026, 1, 15)));
        assert!(!coverage.contains("checking", date(2026, 2, 15)));
        assert!(!coverage.contains("savings", date(2026, 1, 15)));
        assert!(coverage.spans(
            "checking",
            date(2026, 1, 1),
            date(2026, 1, 31)
        ));
        assert!(!coverage.spans(
            "checking",
            date(2026, 1, 15),
            date(2026, 2, 1)
        ));
        assert!(!coverage.spans_all(
            &["checking".to_owned(), "savings".to_owned()],
            date(2026, 1, 1),
            date(2026, 1, 31)
        ));
        assert!(!coverage.spans_all(&[], date(2026, 1, 1), date(2026, 1, 31)));
    }
}

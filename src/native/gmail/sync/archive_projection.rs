//! Body-free reader projections over the Gmail archive journal.
//!
//! Ownership and history have separate result types: list consumers receive
//! only currently owned receipts, while sync's recent archive snapshot keeps
//! released events as historical actions.

use std::collections::BTreeMap;

use chrono::{
    DateTime, Duration, FixedOffset, LocalResult, NaiveDateTime, SecondsFormat,
    TimeZone, Utc,
};
use chrono_tz::Tz;

use super::{
    archive_state::{
        ArchiveHistoryRecord, ArchiveHistoryState, ArchiveJournal,
        ArchiveReceipt,
    },
    ThreadKey,
};
use crate::native::gmail::note::index::MailIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiptLabelView {
    pub(crate) name: String,
    pub(crate) id: Option<String>,
}

/// One currently owned receipt in `bob gmail list` JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveReceiptView {
    pub(crate) archive_event_id: String,
    pub(crate) archived_at: String,
    pub(crate) message_count: usize,
    pub(crate) label: ReceiptLabelView,
}

/// A complete current-ownership projection keyed by exact mail identity.
pub(crate) type ListReceiptProjection =
    BTreeMap<(String, String), Vec<ArchiveReceiptView>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentArchiveItem {
    pub(crate) thread_id: String,
    pub(crate) note: String,
    pub(crate) archived_at: String,
    pub(crate) message_count: usize,
    pub(crate) label: ReceiptLabelView,
    pub(crate) archive_event_id: String,
    pub(crate) state: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentArchiveSnapshot {
    pub(crate) since: String,
    pub(crate) count: usize,
    pub(crate) items: Vec<RecentArchiveItem>,
}

/// Project current ownership for rows in the offline mail index. Every
/// result is tied to the index's exact `(account, thread_id)` identity; no
/// subject, label, or note-text matching is used.
pub(crate) fn list_receipts(
    journal: &ArchiveJournal,
    index: &MailIndex,
    account_scope: Option<&str>,
) -> Option<ListReceiptProjection> {
    if !journal.scope_is_safe(account_scope) {
        return None;
    }

    let mut receipts: BTreeMap<_, Vec<&ArchiveReceipt>> = BTreeMap::new();
    for receipt in journal.owned_receipts() {
        let identity = receipt.key.index_key();
        if index.by_identity.contains_key(&identity) {
            receipts.entry(identity).or_default().push(receipt);
        }
    }

    Some(
        receipts
            .into_iter()
            .map(|(identity, mut values)| {
                values.sort_by(|left, right| {
                    right.archived_at.cmp(&left.archived_at).then_with(|| {
                        left.archive_event_id.cmp(&right.archive_event_id)
                    })
                });
                (identity, values.into_iter().map(receipt_view).collect())
            })
            .collect(),
    )
}

/// Project one recent archive action per thread for the requested account.
/// Released receipts remain in history and report their current terminal
/// state; only accepted archive events inside the interval are considered.
pub(crate) fn recent_archive(
    journal: &ArchiveJournal,
    account: &str,
    timezone: Tz,
    now: DateTime<Utc>,
) -> Result<RecentArchiveSnapshot, &'static str> {
    if !journal.scope_is_safe(Some(account)) {
        return Err("Gmail archive history is unsafe for this account");
    }
    let since = previous_local_day_start(now, timezone)
        .ok_or("could not resolve the previous local calendar day")?;

    let mut newest_by_thread: BTreeMap<ThreadKey, ArchiveHistoryRecord> =
        BTreeMap::new();
    for record in journal.archive_history() {
        let receipt = &record.receipt;
        if !receipt.key.is_account(account)
            || receipt.archived_at < since
            || receipt.archived_at.with_timezone(&Utc) > now
        {
            continue;
        }
        let replace = newest_by_thread.get(&receipt.key).is_none_or(|prior| {
            receipt.archived_at > prior.receipt.archived_at
                || (receipt.archived_at == prior.receipt.archived_at
                    && receipt.archive_event_id
                        > prior.receipt.archive_event_id)
        });
        if replace {
            newest_by_thread.insert(receipt.key.clone(), record);
        }
    }

    let mut records: Vec<_> = newest_by_thread.into_values().collect();
    records.sort_by(|left, right| {
        right
            .receipt
            .archived_at
            .cmp(&left.receipt.archived_at)
            .then_with(|| left.receipt.key.cmp(&right.receipt.key))
            .then_with(|| {
                left.receipt
                    .archive_event_id
                    .cmp(&right.receipt.archive_event_id)
            })
    });
    let items: Vec<_> = records.into_iter().map(history_view).collect();
    Ok(RecentArchiveSnapshot {
        since: since.to_rfc3339_opts(SecondsFormat::Secs, false),
        count: items.len(),
        items,
    })
}

fn receipt_view(receipt: &ArchiveReceipt) -> ArchiveReceiptView {
    ArchiveReceiptView {
        archive_event_id: receipt.archive_event_id.clone(),
        archived_at: receipt
            .archived_at
            .to_rfc3339_opts(SecondsFormat::Secs, false),
        message_count: receipt.message_ids.len(),
        label: ReceiptLabelView {
            name: receipt.label.name.clone(),
            id: receipt.label.id.clone(),
        },
    }
}

fn history_view(record: ArchiveHistoryRecord) -> RecentArchiveItem {
    let receipt = record.receipt;
    let state = match record.state {
        ArchiveHistoryState::Archived => "archived",
        ArchiveHistoryState::LetGo => "let_go",
        ArchiveHistoryState::Restored => "restored",
    };
    RecentArchiveItem {
        thread_id: receipt.key.thread_id().to_string(),
        note: record.note,
        archived_at: receipt
            .archived_at
            .to_rfc3339_opts(SecondsFormat::Secs, false),
        message_count: receipt.message_ids.len(),
        label: ReceiptLabelView {
            name: receipt.label.name,
            id: receipt.label.id,
        },
        archive_event_id: receipt.archive_event_id,
        state,
    }
}

/// Previous local calendar day's first valid instant, not `now - 24h`.
/// Ambiguous midnight chooses the earlier instant; a midnight gap advances
/// to the first valid local minute, keeping the calendar boundary stable.
fn previous_local_day_start(
    now: DateTime<Utc>,
    timezone: Tz,
) -> Option<DateTime<FixedOffset>> {
    let previous_day = now.with_timezone(&timezone).date_naive().pred_opt()?;
    let midnight = previous_day.and_hms_opt(0, 0, 0)?;
    resolve_local_start(timezone, midnight).map(|value| value.fixed_offset())
}

fn resolve_local_start(
    timezone: Tz,
    midnight: NaiveDateTime,
) -> Option<DateTime<Tz>> {
    for minute in 0..=24 * 60 {
        let local = midnight.checked_add_signed(Duration::minutes(minute))?;
        match timezone.from_local_datetime(&local) {
            LocalResult::Single(value) => return Some(value),
            LocalResult::Ambiguous(left, right) => {
                return Some(
                    if left.with_timezone(&Utc) <= right.with_timezone(&Utc) {
                        left
                    } else {
                        right
                    },
                );
            }
            LocalResult::None => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::previous_local_day_start;

    #[test]
    fn previous_calendar_day_starts_at_local_midnight_across_dst() {
        let spring_now = Utc.with_ymd_and_hms(2026, 3, 9, 16, 0, 0).unwrap();
        let spring =
            previous_local_day_start(spring_now, chrono_tz::America::New_York)
                .unwrap();
        assert_eq!(spring.to_rfc3339(), "2026-03-08T00:00:00-05:00");

        let fall_now = Utc.with_ymd_and_hms(2026, 11, 2, 17, 0, 0).unwrap();
        let fall =
            previous_local_day_start(fall_now, chrono_tz::America::New_York)
                .unwrap();
        assert_eq!(fall.to_rfc3339(), "2026-11-01T00:00:00-04:00");

        let spring_day_start =
            Utc.with_ymd_and_hms(2026, 3, 9, 4, 0, 0).unwrap();
        let fall_day_start =
            Utc.with_ymd_and_hms(2026, 11, 2, 5, 0, 0).unwrap();
        assert_eq!(
            spring_day_start.signed_duration_since(spring.with_timezone(&Utc)),
            chrono::Duration::hours(23)
        );
        assert_eq!(
            fall_day_start.signed_duration_since(fall.with_timezone(&Utc)),
            chrono::Duration::hours(25)
        );
    }
}

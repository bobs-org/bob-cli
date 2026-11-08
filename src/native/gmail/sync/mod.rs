//! `bob gmail sync`: label-routed filing, enrollment, and the watch set.
//!
//! This module is being built in phases. Membership lives here now:
//!
//! - [`enroll`] folds every host's `gmail.pulled` / `gmail.filed` /
//!   `gmail.adopted` journal events into an [`enroll::EnrollmentFold`],
//!   appends new body-free enrollment events, and turns the fold plus a
//!   watch scan into per-thread [`enroll::CandidateDecision`]s and
//!   label-free [`enroll::AdoptionIntent`]s.
//! - [`scan`] walks the vault (with `bob gmail link -a`'s exclusions) for
//!   open `#task` rows that link an indexed mail note on their own line or
//!   in a direct link-only child bullet, returning a [`scan::WatchScan`].
//! - [`discover`] pages `label:<enrollment label>` through Gmail's
//!   IDs-only `threads.list` Discovery call through the last page,
//!   deduplicates and validates ids, stops at the 5,000-id cap, and
//!   selects at most `-l` unseen filing candidates.
//! - [`guard`] holds the host-local per-account file guard for a live run.
//! - [`retry`] shares one `-r` budget across guard, retry, and vault-lock
//!   waits with pacing and bounded backoff.
//! - [`command`] is `bob gmail sync` (hidden `pull` alias) and [`report`]
//!   prints its human and JSON documents.
//!
//! Consumers (discovery, filing, the report, and the later watch loop) use
//! these structs directly; none of them parses a rendered report. Scanning
//! and deciding never write: the apply step owns durability and appends
//! enrollment events only after the vault writes they describe are durable.

pub(crate) mod command;
pub(crate) mod discover;
pub(crate) mod enroll;
pub(crate) mod file;
pub(crate) mod guard;
pub(crate) mod hold;
pub(crate) mod report;
pub(crate) mod retry;
pub(crate) mod scan;

pub(crate) use command::run;

/// One Gmail conversation's identity: the lowercase account plus the
/// lowercase hex thread id. Every map keyed by thread in sync uses this
/// key, and [`ThreadKey::index_key`] matches
/// [`super::note::index::MailIndex::by_identity`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ThreadKey {
    account: String,
    thread_id: String,
}

impl ThreadKey {
    /// Normalize and validate an identity: a non-empty account (trimmed,
    /// ASCII-lowercased) and a non-empty all-hex thread id (lowercased).
    pub(crate) fn new(account: &str, thread_id: &str) -> Option<Self> {
        let account = account.trim();
        let thread_id = thread_id.trim();
        if account.is_empty()
            || thread_id.is_empty()
            || !thread_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self {
            account: account.to_ascii_lowercase(),
            thread_id: thread_id.to_ascii_lowercase(),
        })
    }

    pub(crate) fn account(&self) -> &str {
        &self.account
    }

    pub(crate) fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// The `(account, thread_id)` tuple the mail-note index is keyed by.
    pub(crate) fn index_key(&self) -> (String, String) {
        (self.account.clone(), self.thread_id.clone())
    }

    /// Whether this thread belongs to `account` (compared normalized).
    pub(crate) fn is_account(&self, account: &str) -> bool {
        self.account == account.trim().to_ascii_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_keys_normalize_account_and_hex_and_reject_bad_ids() {
        let key = ThreadKey::new(" Me@Example.COM ", "AABBcc01").unwrap();
        assert_eq!(key.account(), "me@example.com");
        assert_eq!(key.thread_id(), "aabbcc01");
        assert_eq!(
            key.index_key(),
            ("me@example.com".to_string(), "aabbcc01".to_string())
        );
        assert!(key.is_account("ME@example.com"));
        assert!(!key.is_account("other@example.com"));
        assert_eq!(
            ThreadKey::new("me@example.com", "aabbcc01"),
            Some(key.clone())
        );
        assert!(ThreadKey::new("", "aabb").is_none());
        assert!(ThreadKey::new("me@example.com", "").is_none());
        assert!(ThreadKey::new("me@example.com", "xyz").is_none());
        assert!(ThreadKey::new("me@example.com", "aa bb").is_none());
    }
}

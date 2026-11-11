//! `bank::status` — account status for list/report/check.
//!
//! In this phase every importable account is `unverified` with the hint
//! `No import yet — bob bank import FILE`, and `cash` accounts are
//! `untracked`. `bank-balance` replaces only the body of [`accounts`].

use super::config::{account_statuses, AccountStatus, BankConfig};

pub(crate) fn accounts(config: &BankConfig) -> Vec<AccountStatus> {
    account_statuses(config)
}

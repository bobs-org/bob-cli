//! Shared task-completion engine (phase `engine`).
//!
//! The single home for "what completing a task writes": the embedded tree
//! close, the scoped ledger retirement, and the Blocked-dependent recovery.
//! No user-visible behavior changes here; later phases call into this
//! module. Keep everything `pub(crate)`.

pub(crate) mod recovery;
pub(crate) mod retirement;
pub(crate) mod tree;

pub(crate) use recovery::{
    recover_blocked_dependents, DependentRecovery, RecoveredDependent,
};
pub(crate) use retirement::{
    retire_completed_links, LedgerMove, LedgerRetirement, LinkStatus,
};
pub(crate) use tree::{
    close_policy_allows, close_traversal_gate, complete_task_tree, ClosedTask,
    CompleteTreeOutcome, LeftOpenReason, LeftOpenTask, RootPolicy,
};

/// Shared recurring-task rule for the engine, the `execute` phase, and the
/// completable-task catalog: a line with `[repeat:: …]`, `(repeat:: …)`, or
/// `🔁` is recurring, matching the Cancel picker's refusal.
pub(crate) fn is_recurring_task_line(line: &str) -> bool {
    line.contains("[repeat::")
        || line.contains("(repeat::")
        || line.contains('🔁')
}

#[cfg(test)]
mod tests;

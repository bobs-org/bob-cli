//! Shared task-completion engine (phase `engine`).
//!
//! The single home for "what completing a task writes": the embedded tree
//! close, the scoped ledger retirement, and the successor planner.
//! No user-visible behavior changes here; later phases call into this
//! module. Keep everything `pub(crate)`.

pub(crate) mod project_close;
pub(crate) mod project_pull_in;
#[cfg(test)]
pub(crate) mod recovery;
pub(crate) mod retirement;
pub(crate) mod successors;
pub(crate) mod tree;

pub(crate) use project_pull_in::{
    closed_prj_may_have_parent, inspect_pull_ins,
};
pub(crate) use retirement::{
    retire_completed_links, LedgerRetirement, LinkStatus,
};
pub(crate) use successors::{
    is_inbox_note, live_link_keys, plan_successors, ClosingContext,
    ClosingContinuation, CompletedTask, OfferGroup, OfferOrderKey, OfferReport,
    ProjectFold, SuccessorInput, SuccessorPlan, SuccessorSelection,
};
pub(crate) use tree::{
    complete_embedded_trees, complete_task_tree, CompleteTreeOutcome,
    EmbeddedTreeRoot, EmbeddedVisitOutcome, RootPolicy,
};

/// Shared completable-status rule for the `!` picker catalog and the
/// `execute` phase: only Ready, Blocked, Next, and In Progress tasks can
/// be completed. Custom open statuses such as `[>]` default to an open
/// type but are refused here, so the picker never offers them.
pub(crate) fn is_completable_status(symbol: char) -> bool {
    matches!(symbol, ' ' | '?' | '*' | '/')
}

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

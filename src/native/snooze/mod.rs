//! Snoozed tasks: later-today Task Links waiting in `## Snoozed`.
//!
//! The contract lives in `docs/snooze.md`. This module owns the row-model
//! layer ([`parse`], [`when`], [`state`]), the shared conformance-vector
//! loader ([`vectors`]), and the pure mutation planners ([`plan`]).
//!
//! The guarded CLI (`cli.rs`, `shell.rs`), receipts, and tick delivery are
//! separate parts of the same child epic.

pub(crate) mod cli;
pub(crate) mod parse;
pub(crate) mod plan;
pub(crate) mod shell;
pub(crate) mod state;
pub(crate) mod vectors;
pub(crate) mod when;

// Re-exports are the later CLI/capture surface; this phase's tests import
// through the submodule paths, so rustc would otherwise warn.
#[allow(unused_imports)]
pub(crate) use parse::{
    duplicate_snoozed_headers, snoozed_section_range, SnoozedSection,
};
#[allow(unused_imports)]
pub(crate) use plan::{
    plan_add, plan_cleanup, plan_drop, plan_wake, AddInput, AddRefusal,
    CleanupInput, DropInput, DropRefusal, FilePostimage, TaskFact, WakeInput,
};
#[allow(unused_imports)]
pub(crate) use state::{
    classify_rows, live_targets, ClassifiedRows, ClassifyInput, ClassifyRow,
    IgnoreReason, LiveTargets, ResolutionKind, RowState, TaskIdentity,
    TaskState,
};
#[allow(unused_imports)]
pub(crate) use when::{
    format_hhmm, parse_snooze_when, SnoozeClock, WhenRefusal,
};

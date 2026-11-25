//! Snoozed tasks: later-today Task Links waiting in `## Snoozed`.
//!
//! The contract lives in `docs/snooze.md`. This module owns the row-model
//! layer only: section bounds ([`parse`]), the injected zoned time grammar
//! ([`when`]), pure resolved-state classification ([`state`]), and the
//! shared conformance-vector loader ([`vectors`]).
//!
//! Mutation planners (`plan.rs`), the CLI (`cli.rs`), receipts, and tick
//! integration belong to later phases of the same child epic and must not
//! be added here.

pub(crate) mod parse;
pub(crate) mod state;
pub(crate) mod vectors;
pub(crate) mod when;

// Stable module exports for downstream phases (mutation planners, CLI,
// readers). Unused until those phases land; the allowance is scoped to
// this child epic's row-model phase and goes away at integration.
#[allow(unused_imports)]
pub(crate) use parse::{
    duplicate_snoozed_headers, snoozed_section_range, SnoozedSection,
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

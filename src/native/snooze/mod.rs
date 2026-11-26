//! Snoozed tasks: later-today Task Links waiting in `## Snoozed`.
//!
//! The contract lives in `docs/snooze.md`. This module owns the row-model
//! layer ([`parse`], [`when`], [`state`]), the shared conformance-vector
//! loader ([`vectors`]), and the pure mutation planners ([`plan`]).
//!
//! The guarded CLI (`cli.rs`, `shell.rs`) writes through the vault lock.
//! Receipts, Darwin, and desktop arbitration live in `receipt`/`notify`.
//!
//! Outer consumers import through the submodule paths:
//! `state::{classify_rows, live_targets}` for planned-today readers,
//! `when::parse_snooze_when` for the time grammar, and
//! `plan::{plan_add, plan_drop, plan_wake, plan_cleanup}` for capture's
//! in-memory batch.

pub(crate) mod cli;
pub(crate) mod notify;
pub(crate) mod parse;
pub(crate) mod plan;
pub(crate) mod read;
pub(crate) mod receipt;
pub(crate) mod shell;
pub(crate) mod state;
pub(crate) mod vectors;
pub(crate) mod when;

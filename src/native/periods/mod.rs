//! Period notes that add up: ISO weeks, quarters, years, and all time.
//!
//! Core owns the calendar, labels, deterministic scaffolds, the
//! note-anatomy parser, create-only `ensure`, and basic `list`. Later
//! phases add the report, migration, brief/draft, and the strips.

pub(crate) mod calendar;
pub(crate) mod cli;
pub(crate) mod ensure;
pub(crate) mod instructions;
pub(crate) mod labels;
pub(crate) mod list;
pub(crate) mod note;
pub(crate) mod scaffold;

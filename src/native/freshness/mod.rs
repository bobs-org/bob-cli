//! Task freshness: a rolling review lease for Ready tasks.
//!
//! The contract lives in `docs/freshness.md`. [`placement`] stamps
//! lines in canonical position; [`state`] evaluates them at read time.
//! Both sides (this crate and bob-ledger-tools) run that doc's
//! conformance vectors verbatim.

pub(crate) mod cli;
pub(crate) mod placement;
pub(crate) mod scan;
pub(crate) mod seed;
pub(crate) mod state;

pub(crate) use placement::{
    read_freshness, set_refresh, stamp_fresh, tasks_suffix_start, FreshRead,
    Refusal, Stamp,
};
pub(crate) use state::{
    collect_lints, counts, evaluate, queue, Counts, Evaluated, FreshState,
    FreshnessRow, IntervalSource, QueueEntry,
};

//! Guarded note writes for `bob task reconcile`.
//!
//! Snapshot the planning read-set, stage replacements as uniquely created
//! temporaries, retain recoverable originals, and refuse to overwrite a vault
//! that changed underfoot. Scoped to this command; other writers are unchanged.

#![allow(clippy::result_large_err, clippy::type_complexity)]

mod apply;
mod model;
mod preflight;
mod recovery;
mod snapshot;
mod staging;
#[cfg(test)]
mod tests;

pub(crate) use apply::{acquire_maintenance_lock, apply_plan};
pub(crate) use model::{
    new_run_id, ApplyError, ApplyOutcome, ApplySession, CaptureError,
    InputKind, InputSnapshot, ReasonCode, WritePlan,
};
pub(crate) use snapshot::{
    capture_optional, capture_required, planned_write, snapshot_for_path,
};

//! Pure task-merge planning primitives.
//!
//! The payload phase intentionally exposes only its internal preparation
//! result. Later phases compose dependency retirement, cancellation, and
//! ledger offers before adding the complete `plan_task_merge` entry point.

mod containers;
mod payload;
mod retirement;
mod types;

#[cfg(test)]
mod tests;

// The planner integration phase will consume this preparation entry point.
#[allow(unused_imports)]
pub(crate) use payload::prepare_task_merge;
// Later planner phases consume these crate-internal preparation records.
#[allow(unused_imports)]
pub(crate) use retirement::{
    apply_retirement, classify_source, find_open_dependents,
    normalize_merge_label, overlapping_effect_keys, repoint_dependents,
    section_provenance_reason, validate_repoint_cycles, OpenDependent,
    RetirementReports, SourceOutcome,
};
#[allow(unused_imports)]
pub(crate) use types::{
    DailyNoteReadState, MergeContainer, MergeContext, MergeError, MergeGesture,
    MergeRelationPolicy, MergeRequest, PreparedContainer,
    PreparedContainerKind, PreparedMerge, PreparedPayload, PreparedSource,
    SourceDisposition,
};

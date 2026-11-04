//! Pure task-merge planning primitives.
//!
//! The module exposes a complete pure planner alongside the internal helpers
//! that prepare payloads, retire sources, and compose ledger offers.

mod command;
mod containers;
mod payload;
mod plan;
mod retirement;
mod types;

#[cfg(test)]
mod tests;

// The planner integration phase will consume this preparation entry point.
pub(crate) use command::{build_cli, run};
#[allow(unused_imports)]
pub(crate) use payload::prepare_task_merge;
// The parent merge-label phase will call the planner through this module.
#[allow(unused_imports)]
pub(crate) use plan::plan_task_merge;
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
    MergeOffer, MergeOutcome, MergePlan, MergeRecord, MergeRelationPolicy,
    MergeRequest, MergeTargetDescriptor, MergeTaskDescriptor,
    PreparedContainer, PreparedContainerKind, PreparedMerge, PreparedPayload,
    PreparedSource, SourceDisposition,
};

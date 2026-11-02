//! Shared policy-adjacent types and copy for relation-successor planning.
//!
//! The classifier phase consumes these types; this phase establishes its
//! target/event/held vocabulary and the single presentation API.

use std::path::PathBuf;

pub(crate) mod classify;
pub(crate) mod copy;

// Public to the remaining relation-successor phases through this module.
#[allow(unused_imports)]
pub(crate) use classify::*;
#[allow(unused_imports)]
pub(crate) use copy::*;

pub(crate) use copy::{
    EventKind as RelationEventKind, FoldCopy as RelationFoldCopy,
    GroupCopy as RelationGroupCopy, HeldNoticeCopy as RelationHeldCopy,
    HeldReason as RelationHeldCopyReason,
    PickedNoticeCopy as RelationPickedNoticeCopy, ReleaseState,
    StructuredWarning,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RelationTarget {
    pub(crate) note_path: PathBuf,
    pub(crate) block_id: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationSlotAnchor {
    pub(crate) note_path: PathBuf,
    pub(crate) line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationEvent {
    pub(crate) target: RelationTarget,
    pub(crate) kind: RelationEventKind,
    /// Explicit only for gestures that name a placement slot (for example,
    /// a future unlink gesture); ordinary close gestures use their context.
    pub(crate) slot: Option<RelationSlotAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RelationHeldReason {
    AlreadyPlanned,
    AlreadyStarted,
    WaitsFor(Vec<RelationTarget>),
    Blocked {
        waits_on: Vec<RelationTarget>,
        scheduled: Option<String>,
        project: Option<PathBuf>,
    },
    #[allow(dead_code)] // Copy builders still match these held reasons.
    Scheduled(String),
    #[allow(dead_code)] // Copy builders still match these held reasons.
    Project,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationHeldRow {
    pub(crate) source: RelationTarget,
    pub(crate) reason: RelationHeldReason,
    pub(crate) triggering_targets: Vec<RelationTarget>,
}

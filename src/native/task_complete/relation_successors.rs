//! Shared policy-adjacent types and copy for relation-successor planning.
//!
//! The classifier phase consumes these types; this phase establishes its
//! target/event/held vocabulary and the single presentation API.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::native::config::relations::CancelLabel;

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
/// A successor insertion coordinate in the post-gesture day text.
/// `line` is a 0-based logical line, normally the position where the
/// parked target's first removed Task Link bullet stood. `note_path`
/// identifies that day note and is checked against the planner's day.
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

/// Optional relation-successor engine input for [`plan_successors`].
///
/// Production capture callers currently pass `None` so capture output
/// stays byte-identical; the integrate tests drive `Some` without any
/// filesystem I/O. Source texts and the narrow target reader are derived
/// from the planner's held snapshot (`snapshot` + `note_texts`); this
/// struct supplies the remaining relation facts: effective labels,
/// parked-only events (Done events are derived from `completed`),
/// relation-snapshot completeness, previous-daily identity, and any
/// targets to report as explicitly unavailable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RelationSuccessorInput {
    pub(crate) labels: Vec<CancelLabel>,
    pub(crate) parked: Vec<RelationEvent>,
    pub(crate) relation_snapshot_complete: bool,
    /// Completeness of the dependency proof included for relation-source
    /// R4. It remains independent of `SuccessorInput.snapshot_complete`,
    /// which controls normal recovery and pull-ins.
    pub(crate) dependency_snapshot_complete: bool,
    pub(crate) previous_daily: Option<PathBuf>,
    pub(crate) unavailable_targets: BTreeSet<PathBuf>,
}

/// Combine Done events derived from `completed` with caller-supplied
/// parked events. Callers supply parked-only gestures; the planner derives
/// Done events from every completed task with a block ID.
pub(crate) fn relation_events_for_planner(
    completed: &[crate::native::task_complete::successors::CompletedTask],
    parked: &[RelationEvent],
) -> Vec<RelationEvent> {
    let mut events = Vec::new();
    for task in completed {
        if task.block_id.is_empty() {
            continue;
        }
        events.push(RelationEvent {
            target: RelationTarget {
                note_path: task.relative_path.clone(),
                block_id: task.block_id.clone(),
                text: task.text.clone(),
            },
            kind: copy::EventKind::Done,
            slot: None,
        });
    }
    events.extend(parked.iter().cloned());
    events
}

/// Event-time helper for SA20, reusable by the later capture phase: the
/// shared `relation_successor_already_released` warning to emit when an
/// edge is authored after its target was already parked or done. Planning
/// the event with its then-current texts (no edge) yields no offer; this
/// warning is the only output for the late edge.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn edge_added_after_release_warning(
    label: &str,
    source_text: &str,
    target_text: &str,
) -> StructuredWarning {
    copy::already_released_warning(label, source_text, target_text)
}

/// Net-revalidation helper for SA21/SL20, reusable by the later capture
/// phase: whether a previously offered relation row survives the final
/// state after later items ran.
///
/// A row survives only when its source is still offerable (Ready or Next,
/// no live link today, not blocked or hidden) and its event still holds:
/// Done events survive (the target stays terminal) while parked events
/// require the target to still be Pending with no live link today.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn relation_offer_survives_final_state(
    source_status: char,
    source_live: bool,
    source_blocked: bool,
    source_hidden: bool,
    event: copy::EventKind,
    target_status: char,
    target_live: bool,
) -> bool {
    if !matches!(source_status, ' ' | '*') {
        return false;
    }
    if source_live || source_blocked || source_hidden {
        return false;
    }
    match event {
        copy::EventKind::Done => true,
        copy::EventKind::Parked => target_status == '/' && !target_live,
    }
}

/// Narrow in-memory target reader over held source texts plus an explicit
/// unavailable set. Staged texts win; unavailable paths report
/// `Unavailable` so R6 holds instead of releasing; unknown paths report
/// `Missing` so dangling edges are ignored.
pub(crate) struct MapTargetReader {
    staged: BTreeMap<PathBuf, String>,
    known: BTreeSet<PathBuf>,
    unavailable: BTreeSet<PathBuf>,
}

impl MapTargetReader {
    pub(crate) fn new(
        staged: BTreeMap<PathBuf, String>,
        known: BTreeSet<PathBuf>,
        unavailable: BTreeSet<PathBuf>,
    ) -> Self {
        Self {
            staged,
            known,
            unavailable,
        }
    }
}

impl classify::TargetNoteReader for MapTargetReader {
    fn read_target(&mut self, path: &Path) -> classify::TargetNoteRead {
        if let Some(text) = self.staged.get(path) {
            return classify::TargetNoteRead::Text(text.clone());
        }
        if self.unavailable.contains(path) {
            return classify::TargetNoteRead::Unavailable;
        }
        if self.known.contains(path) {
            classify::TargetNoteRead::Unavailable
        } else {
            classify::TargetNoteRead::Missing
        }
    }
}

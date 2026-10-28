//! Complete-label policy and source-task verdicts
//! (`docs/task-relations.md` §18 CP5).

use crate::native::{
    task_cancel::verdicts::{
        cancel_verdict, relation_policy_candidate, CancelSourceVerdict,
        ItemActionConflict, RelationPolicyCandidate, RelationPolicyInput,
    },
    task_status_hooks::TasksSettings,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompleteSourceVerdict {
    Completes {
        previous_status: char,
        project: bool,
    },
    AlreadyClosed {
        status: char,
    },
    Recurring,
    NewTask,
}

/// The managed Done eligible set and recurrence rules match Task Card
/// Cancel. Keep source classification in one place while presenting the
/// effect-specific name required by the complete-label contract.
pub(crate) fn complete_verdict(
    source_line: &str,
    tasks_settings: &TasksSettings,
    is_new_task: bool,
) -> CompleteSourceVerdict {
    match cancel_verdict(source_line, tasks_settings, is_new_task) {
        CancelSourceVerdict::Cancels {
            previous_status,
            project,
        } => CompleteSourceVerdict::Completes {
            previous_status,
            project,
        },
        CancelSourceVerdict::AlreadyClosed { status } => {
            CompleteSourceVerdict::AlreadyClosed { status }
        }
        CancelSourceVerdict::Recurring => CompleteSourceVerdict::Recurring,
        CancelSourceVerdict::NewTask => CompleteSourceVerdict::NewTask,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompleteRelationPolicyVerdict {
    Completes {
        previous_status: char,
        project: bool,
    },
    AlreadyClosed {
        status: char,
    },
    Recurring,
    NewTask,
    Conflict(Vec<ItemActionConflict>),
    CloseEffectsConflict,
    None,
}

impl CompleteRelationPolicyVerdict {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Completes { .. } => "completes",
            Self::AlreadyClosed { .. } => "already_closed",
            Self::Recurring => "recurring",
            Self::NewTask => "new_task",
            Self::Conflict(_) | Self::CloseEffectsConflict => "conflict",
            Self::None => "none",
        }
    }

    pub(crate) fn effect(&self) -> &'static str {
        if matches!(self, Self::Completes { .. }) {
            "complete"
        } else {
            "none"
        }
    }

    pub(crate) fn refusal(&self) -> Option<&'static str> {
        match self {
            Self::Recurring => Some("relation_completes_recurring_task"),
            Self::NewTask => Some("relation_completes_new_task"),
            Self::Conflict(_) => {
                Some("relation_complete_conflicts_with_item_action")
            }
            Self::CloseEffectsConflict => {
                Some("relation_close_effects_conflict")
            }
            Self::Completes { .. }
            | Self::AlreadyClosed { .. }
            | Self::None => None,
        }
    }
}

/// Apply the shared explicit-edge gate using the effective complete-label
/// list. `opposite_close_effect` is set by a caller that has found a cancel
/// intent for this same source in the current commit.
pub(crate) fn relation_complete_policy_verdict(
    input: RelationPolicyInput<'_>,
    opposite_close_effect: bool,
) -> CompleteRelationPolicyVerdict {
    match relation_policy_candidate(&input) {
        RelationPolicyCandidate::None => {
            return CompleteRelationPolicyVerdict::None;
        }
        RelationPolicyCandidate::Conflict(conflicts) => {
            return CompleteRelationPolicyVerdict::Conflict(conflicts);
        }
        RelationPolicyCandidate::Eligible => {}
    }
    if opposite_close_effect {
        return CompleteRelationPolicyVerdict::CloseEffectsConflict;
    }
    match complete_verdict(
        input.source_line,
        input.tasks_settings,
        input.source_is_new,
    ) {
        CompleteSourceVerdict::Completes {
            previous_status,
            project,
        } => CompleteRelationPolicyVerdict::Completes {
            previous_status,
            project,
        },
        CompleteSourceVerdict::AlreadyClosed { status } => {
            CompleteRelationPolicyVerdict::AlreadyClosed { status }
        }
        CompleteSourceVerdict::Recurring => {
            CompleteRelationPolicyVerdict::Recurring
        }
        CompleteSourceVerdict::NewTask => {
            CompleteRelationPolicyVerdict::NewTask
        }
    }
}

//! Cancel-label policy and source-task verdicts (`docs/task-relations.md` §17).

use crate::native::{
    relation_labels::relation_label_key,
    task_complete::is_recurring_task_line,
    task_status_hooks::{parse_tasks, TasksSettings},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelSourceVerdict {
    Cancels {
        previous_status: char,
        project: bool,
    },
    AlreadyClosed {
        status: char,
    },
    Recurring,
    NewTask,
}

impl CancelSourceVerdict {
    pub(crate) fn is_cancelled(self) -> bool {
        matches!(self, Self::Cancels { .. })
    }
}

/// The Task Card Cancel open set is intentionally fixed. A custom Tasks
/// status can be open in Obsidian without being eligible for this writer.
pub(crate) fn cancel_verdict(
    source_line: &str,
    tasks_settings: &TasksSettings,
    is_new_task: bool,
) -> CancelSourceVerdict {
    let status = parse_tasks(source_line, tasks_settings)
        .first()
        .map(|task| task.status)
        .or_else(|| task_status(source_line))
        .unwrap_or('?');
    if is_new_task {
        return CancelSourceVerdict::NewTask;
    }
    if !matches!(status, ' ' | '*' | '/' | '?') {
        return CancelSourceVerdict::AlreadyClosed { status };
    }
    if is_recurring_task_line(source_line) {
        return CancelSourceVerdict::Recurring;
    }
    CancelSourceVerdict::Cancels {
        previous_status: status,
        project: source_block_id(source_line) == Some("prj"),
    }
}

fn task_status(line: &str) -> Option<char> {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let rest = line.get(indent..)?.strip_prefix("- [")?;
    let mut chars = rest.chars();
    let status = chars.next()?;
    rest.get(status.len_utf8()..)?.strip_prefix(']')?;
    Some(status)
}

fn source_block_id(line: &str) -> Option<&str> {
    let trimmed = line.trim_end();
    let caret = trimmed.rfind('^')?;
    if caret == 0 || !trimmed.as_bytes()[caret - 1].is_ascii_whitespace() {
        return None;
    }
    let id = &trimmed[caret + 1..];
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    .then_some(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationAction {
    Added,
    Relabeled,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationDirection {
    LinksTo,
    LinkedFrom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationTrigger {
    Explicit,
    RelabelAway,
    Unlink,
    Reconcile,
    Rename,
    HandEdit,
    AgentEdit,
    Mirror,
    Sync,
    ConfigChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemActionConflict {
    Complete,
    Start,
    PomodoroLink,
    DependsOn,
    Blocks,
}

impl ItemActionConflict {
    pub(crate) fn from_modifier(modifier: &str) -> Option<Self> {
        match modifier {
            "!" => Some(Self::Complete),
            "=" => Some(Self::Start),
            ":" => Some(Self::PomodoroLink),
            "&" => Some(Self::DependsOn),
            "*" => Some(Self::Blocks),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RelationPolicyVerdict {
    Cancels(CancelSourceVerdict),
    AlreadyClosed { status: char },
    Recurring,
    NewTask,
    Conflict(Vec<ItemActionConflict>),
    None,
}

impl RelationPolicyVerdict {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Cancels(_) => "cancels",
            Self::AlreadyClosed { .. } => "already_closed",
            Self::Recurring => "recurring",
            Self::NewTask => "new_task",
            Self::Conflict(_) => "conflict",
            Self::None => "none",
        }
    }

    pub(crate) fn cancels(&self) -> bool {
        matches!(self, Self::Cancels(_))
    }
}

pub(crate) struct RelationPolicyInput<'a> {
    pub(crate) configured_labels: &'a [String],
    pub(crate) label: &'a str,
    pub(crate) previous_label: Option<&'a str>,
    pub(crate) action: RelationAction,
    pub(crate) direction: RelationDirection,
    pub(crate) trigger: RelationTrigger,
    pub(crate) source_line: &'a str,
    pub(crate) tasks_settings: &'a TasksSettings,
    pub(crate) source_is_new: bool,
    pub(crate) conflicts: Vec<ItemActionConflict>,
}

/// Only explicit Added/Relabeled edges reach the source verdict. The
/// caller resolves which task is the relation source before calling this
/// function; direction never swaps source and target here.
pub(crate) fn relation_policy_verdict(
    input: RelationPolicyInput<'_>,
) -> RelationPolicyVerdict {
    let _ = (input.previous_label, input.direction);
    if input.trigger != RelationTrigger::Explicit
        || !matches!(
            input.action,
            RelationAction::Added | RelationAction::Relabeled
        )
    {
        return RelationPolicyVerdict::None;
    }
    let key = relation_label_key(input.label);
    if !input
        .configured_labels
        .iter()
        .any(|configured| relation_label_key(configured) == key)
    {
        return RelationPolicyVerdict::None;
    }
    if !input.conflicts.is_empty() {
        return RelationPolicyVerdict::Conflict(input.conflicts);
    }
    match cancel_verdict(
        input.source_line,
        input.tasks_settings,
        input.source_is_new,
    ) {
        CancelSourceVerdict::Cancels {
            previous_status,
            project,
        } => RelationPolicyVerdict::Cancels(CancelSourceVerdict::Cancels {
            previous_status,
            project,
        }),
        CancelSourceVerdict::AlreadyClosed { status } => {
            RelationPolicyVerdict::AlreadyClosed { status }
        }
        CancelSourceVerdict::Recurring => RelationPolicyVerdict::Recurring,
        CancelSourceVerdict::NewTask => RelationPolicyVerdict::NewTask,
    }
}

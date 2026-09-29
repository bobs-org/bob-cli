//! Pure Pomodoro-close planner: rewrite the running ledger entry, apply linked
//! task effects, and compose dated Work Log entries without writing files.

#![allow(dead_code)]

/// Task text without the `#task` tag. Embedded and subtask rows are looked
/// up with the global filter cleared (resolution does not require `#task`),
/// so their description keeps the tag; every other row filters it out.
pub(crate) fn close_task_text(description: &str) -> String {
    description
        .split_whitespace()
        .filter(|token| *token != "#task")
        .collect::<Vec<_>>()
        .join(" ")
}

mod ledger;
mod linked_tasks;
mod links;
mod selection;

#[cfg(test)]
mod linked_task_tests;
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod tests;

pub(crate) use ledger::{
    find_running_pomodoro, plan_ledger_close, sub_bullet_range,
    BlockLinkTarget, FindRunningError, LedgerClosePlan, LedgerLinkRole,
    RunningPomodoro, WorkLogNode,
};
pub(crate) use linked_tasks::{
    lookup_task, plan_pomodoro_close, CloseVault, PomodoroClosePlan,
    PomodoroClosePlanError,
};
pub(crate) use links::{
    bare_embedded_link, bare_plain_link, range_is_struck,
    strikethrough_inner_spans, strip_pomodoro_markers, wikilink_tokens,
    WikiToken,
};
pub(crate) use selection::CloseSelection;

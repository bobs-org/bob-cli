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
mod names;
mod reset;
mod selection;

#[cfg(test)]
mod linked_task_tests;
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use ledger::plan_ledger_close;
#[allow(unused_imports)]
pub(crate) use ledger::{
    find_running_pomodoro, plan_ledger_close_with_parked,
    plan_ledger_close_with_policy, signed_minute_delta, sub_bullet_range,
    target_from_token, BlockLinkTarget, FindRunningError, LedgerClosePlan,
    LedgerClosePolicy, LedgerLinkRole, RunningPomodoro, WorkLogNode,
};
#[allow(unused_imports)]
pub(crate) use linked_tasks::{
    clock_resolver, lookup_task, plan_pomodoro_close,
    plan_pomodoro_close_for_entry, plan_pomodoro_close_outcome, CloseVault,
    CloseVaultHomeSource, PomodoroCloseOutcome, PomodoroClosePlan,
    PomodoroClosePlanError,
};
pub(crate) use links::{
    bare_embedded_link, bare_plain_link, dropped_plain_link, range_is_struck,
    strikethrough_inner_spans, strip_pomodoro_markers, wikilink_tokens,
    WikiToken,
};
pub(crate) use names::{close_names, CloseNaming};
#[allow(unused_imports)]
pub(crate) use reset::{
    has_standalone_note, is_reset_selection, is_reset_spec, plan_reset,
    reset_if_eligible, ResetPlan,
};
pub(crate) use selection::{
    join_numbers, number_task_links, number_task_links_in_range,
    CloseSelection, NumberedTaskLink, TaskLinkMarker, TaskLinkOutcome,
};

//! Pure planners for the `@route+block-id!` link-presence toggle and the
//! Ensure Next relocation (link policy once shared with the
//! `<ctrl+shift+enter>` keymap).
//!
//! Read `plugins/block-id-prompt/main.js` in the `bob-plugins` repo first --
//! `planTargetTaskUpdate`, `planTargetTaskOpenUpdate`,
//! `planPomodoroLinkInsertion`, `planFuturePomodoroLinkCleanup`,
//! `planAllOpenPomodoroLinkCleanup`, `planPomodoroLinkCleanupForRanges`,
//! `isDedicatedLinkBullet`, and `listItemSubtreeEdit` are the reference this
//! module mirrors. Everything here operates on `&str` note contents and
//! returns a full postimage; nothing touches disk.
//!
//! `bob capture` wires these planners into its staged batch planner, while
//! keeping the note-level mutation rules here as pure functions so the route
//! note and daily ledger behavior can be tested directly.
#![allow(dead_code)]

mod ledger;
mod links;
mod relocation;
mod task_update;
#[cfg(test)]
mod tests;
mod text;

pub(crate) use ledger::{
    list_open_entry_links, plan_pomodoro_link_ledger, PomodoroLinkLedgerAction,
};
pub(crate) use links::{
    find_movable_task_links, plan_link_insertion, plan_link_removal,
    LinkInsertionOutcome, LinkPlacement, LinkPlanError,
};
pub(crate) use relocation::{
    endpoint_from_entry, insert_named_placeholder, move_subtree_to_entry,
    plan_link_relocation, LinkRelocationAction, LinkRelocationError,
    PomodoroEndpoint,
};
pub(crate) use task_update::{plan_task_link, set_task_line_status};
pub(crate) use text::child_block_end_line;

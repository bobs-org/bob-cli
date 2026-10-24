//! Pure snapshot planner for `bob task move`.
//!
//! Selection, block movement, identity validation, collision renaming, and
//! vault-wide link repair run against in-memory note postimages. Filesystem
//! writes belong to the command adapter.

mod command;
pub(crate) mod plan;
#[cfg(test)]
mod tests;

pub(crate) use command::{build_cli, run};
pub(crate) use plan::{
    list_task_move_destinations, plan_task_move, MovePlan, MoveRequest,
    MoveSelector, TaskMoveDestination, VaultSnapshot,
};
#[cfg(test)]
pub(crate) use plan::{load_vault_snapshot, PlanError};

//! Pure snapshot planner for `bob task move`.
//!
//! Selection, block movement, identity validation, collision renaming, and
//! vault-wide link repair run against in-memory note postimages. Filesystem
//! writes belong to the command adapter.

pub(crate) mod plan;
#[cfg(test)]
mod tests;

pub(crate) use plan::{
    load_vault_snapshot, plan_task_move, MovePlan, MoveRequest, MoveSelector,
    PlanError, VaultSnapshot,
};

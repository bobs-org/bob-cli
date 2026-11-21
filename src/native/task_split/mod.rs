//! Pure planner and guarded command adapter for promoting task notes into
//! tasks, and for carrying a task requirement into its own task.

pub(crate) mod carry;
mod command;
pub(crate) mod plan;
#[cfg(test)]
mod tests;

pub(crate) use command::{build_cli, run};

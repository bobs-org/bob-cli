//! Pure planner and guarded command adapter for promoting task notes into tasks.

mod command;
pub(crate) mod plan;
#[cfg(test)]
mod tests;

pub(crate) use command::{build_cli, run};

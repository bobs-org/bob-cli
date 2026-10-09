//! `bob ref migrate-tasks` dry-run planner, report, and `--write`.
//!
//! Moves open v1 ref tasks into parent notes and rewrites every link
//! and dependency id that pointed at them. Modeled on `migrate-zorg`.

pub(crate) mod cli;
mod line;
mod plan;
mod report;
mod rewrite;
mod write;

pub(crate) use cli::{migrate_tasks_command, run_migrate_tasks};

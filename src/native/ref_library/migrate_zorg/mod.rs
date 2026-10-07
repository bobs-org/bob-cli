//! `bob ref migrate-zorg` dry-run planner and report (phase `planner`).
//!
//! The bare command plans the whole migration without writing anything:
//! one legacy note per unmirrored record with books folding their
//! chapters, collision-free stems, and a human or JSON report.

pub(crate) mod cli;
mod plan;
mod render;
mod report;

pub(crate) use cli::{migrate_zorg_command, run_migrate_zorg};

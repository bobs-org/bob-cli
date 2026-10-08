//! `bob ref migrate-zorg` dry-run planner, report, and `--write`
//! (phases `planner` and `writer`).
//!
//! The bare command plans the whole migration without writing anything:
//! one legacy note per unmirrored record with books folding their
//! chapters, collision-free stems, and a human or JSON report.
//! `--write` applies exactly that plan as one revertible commit.

pub(crate) mod cli;
mod plan;
mod render;
mod report;
mod write;

pub(crate) use cli::{migrate_zorg_command, run_migrate_zorg};

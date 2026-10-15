//! Read-only planning and dry-run reporting for `bob ref rename`.

mod cli;
pub(crate) mod links;
mod plan;
mod report;

pub(crate) use cli::{command, run};

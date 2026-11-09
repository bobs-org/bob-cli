//! Notebook recognition, list, and check.

pub(crate) mod cli;
pub(crate) mod model;
pub(crate) mod output;
pub(crate) mod rules;
pub(crate) mod scan;

pub(crate) use cli::{build_cli_check, build_cli_list};

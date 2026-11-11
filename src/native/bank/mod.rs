//! Bank hub, month notes, row grammar, and `bob bank`.

pub(crate) mod ack;
pub(crate) mod check;
pub(crate) mod cli;
pub(crate) mod config;
pub(crate) mod dates;
pub(crate) mod ensure;
pub(crate) mod grammar;
pub(crate) mod list;
pub(crate) mod model;
pub(crate) mod month;
pub(crate) mod output;
pub(crate) mod report;
pub(crate) mod scan;
pub(crate) mod status;

pub(crate) use cli::{
    build_cli_ack, build_cli_check, build_cli_ensure, build_cli_list,
    build_cli_report,
};

#[cfg(test)]
mod tests;

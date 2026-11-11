//! Bank hub, month notes, row grammar, and `bob bank check`.

pub(crate) mod check;
pub(crate) mod cli;
pub(crate) mod config;
pub(crate) mod grammar;
pub(crate) mod month;
pub(crate) mod output;
pub(crate) mod scan;

pub(crate) use cli::build_cli_check;

#[cfg(test)]
mod tests;

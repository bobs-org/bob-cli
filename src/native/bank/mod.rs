//! Bank hub, month notes, row grammar, and `bob bank`.

pub(crate) mod ack;
pub(crate) mod balance;
pub(crate) mod charts;
pub(crate) mod check;
pub(crate) mod check_subscriptions;
pub(crate) mod cli;
pub(crate) mod config;
pub(crate) mod config_report;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod config_spans;
pub(crate) mod config_update;
pub(crate) mod dates;
pub(crate) mod ensure;
pub(crate) mod grammar;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod identity;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod import_delete;
pub(crate) mod import_plan;
pub(crate) mod import_run;
pub(crate) mod journal;
pub(crate) mod list;
pub(crate) mod model;
pub(crate) mod month;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod naming;
pub(crate) mod output;
pub(crate) mod report;
pub(crate) mod scan;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod source;
pub(crate) mod status;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod subscriptions;
pub(crate) mod subscriptions_report;
pub(crate) mod suggest;

pub(crate) use cli::{
    build_cli_ack, build_cli_check, build_cli_config, build_cli_ensure,
    build_cli_import, build_cli_list, build_cli_report,
    build_cli_subscriptions,
};

#[cfg(test)]
mod tests;

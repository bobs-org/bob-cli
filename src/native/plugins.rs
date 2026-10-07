//! Bob Obsidian plugin management from the bob-plugins repo.
//!
//! Small facade over focused child modules. Production dependencies flow
//! `cli -> {git, guard, scan, sync, render, model}`, `sync -> {model, diff,
//! git, scan}`, `scan/diff/git -> model`, and `render -> model`.

mod cli;
mod diff;
mod git;
mod guard;
mod model;
mod render;
mod scan;
mod sync;
#[cfg(test)]
mod tests;

pub(crate) use cli::{build_cli, run};

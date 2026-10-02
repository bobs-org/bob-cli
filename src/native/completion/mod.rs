//! Completion-only command tree.
//!
//! This module owns the exhaustive clap tree used only for shell
//! completion. Runtime dispatch in `runner` and each module's `run`
//! stays untouched.

mod tree;

#[allow(unused_imports)]
pub(crate) use tree::tree;

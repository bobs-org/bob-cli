//! Attachment origin journal, resolution, and commands
//! (`docs/attachments.md`).
//!
//! `%c` and `bob attachments import` copy files into the vault; this
//! module records where each copy came from (the per-host
//! `.bob/attachments/<host>.jsonl` journal) and resolves a vault copy
//! back to its recorded source. Bob is the only writer.

pub(crate) mod cli;
pub(crate) mod journal;
pub(crate) mod origin;
#[cfg(test)]
mod tests;

pub(crate) use cli::{run_import, run_origin};

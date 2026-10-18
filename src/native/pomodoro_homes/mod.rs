//! Derived Pomodoro homes shared by the CLI, stats, and ledger integrations.
//!
//! Several public-to-the-crate entry points are staged for the dependent Stats
//! and live-surface phases, so allow unused declarations until those callers
//! land.

#![allow(unused)]

mod facts;
mod json;
mod label;
mod palette;
mod resolve;
mod session;

#[cfg(test)]
mod tests;

pub(crate) use facts::{NoteFacts, NoteKind, Rollup};
pub(crate) use json::{HomeKind, HomePart, Homes, SessionKind, SessionState};
pub(crate) use palette::{fallback_hue, fnv1a32, Hue};
pub(crate) use resolve::{
    path_string, rollup_value, unfiled_home, unlinked_home, DiskHomeSource,
    Home, HomeResolver, HomeSource, IndexedHomeSource, ResolverHealth,
};
pub(crate) use session::{
    attribute_ledger_entry, attribute_session, session_links, split_minutes,
    SessionHomes, SessionLink,
};

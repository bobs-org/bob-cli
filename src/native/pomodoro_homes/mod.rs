//! Derived Pomodoro homes shared by the CLI, stats, and ledger integrations.
//!
mod facts;
mod json;
mod label;
mod palette;
mod resolve;
mod session;

#[cfg(test)]
mod tests;

pub(crate) use facts::{NoteKind, Rollup};
pub(crate) use json::{HomeKind, Homes, SessionKind, SessionState};
pub(crate) use palette::{fallback_hue, Hue};
pub(crate) use resolve::{
    path_string, rollup_value, unfiled_home, unlinked_home, DiskHomeSource,
    Home, HomeResolver, HomeSource, IndexedHomeSource, ResolverHealth,
};
pub(crate) use session::{
    attribute_ledger_entry, attribute_session, session_links, split_minutes,
    SessionHomes, SessionLink,
};

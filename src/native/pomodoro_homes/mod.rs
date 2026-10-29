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

pub(crate) use facts::{NoteFacts, NoteKind, NoteTheme};
pub(crate) use json::{HomeKind, Homes, SessionKind, SessionState};
pub(crate) use palette::{fallback_hue, Hue};
pub(crate) use resolve::{
    path_string, unfiled_home, unlinked_home, DiskHomeSource, Home,
    HomeResolver, HomeSource, IndexedHomeSource, ResolverHealth,
};
#[cfg(test)]
pub(crate) use session::session_links;
pub(crate) use session::{
    attribute_ledger_entry, attribute_session, prepare_session_scan,
    session_links_from_scan, split_minutes, SessionHomes, SessionLink,
};

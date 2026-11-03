//! Derived Pomodoro homes shared by the CLI, stats, and ledger integrations.
//!
mod clock;
mod facts;
mod json;
mod label;
mod palette;
mod resolve;
mod session;

#[cfg(test)]
mod tests;

pub(crate) use clock::{
    effective_clock, off_clock_via_label, via_clock_note, ClockLookup,
    ClockResolution, ClockSource,
};
pub(crate) use facts::{NoteFacts, NoteKind, NoteOffClock, NoteTheme};
pub(crate) use json::{HomeKind, Homes, SessionKind, SessionState};
pub(crate) use palette::{fallback_hue, Hue};
pub(crate) use resolve::{
    path_string, root_label, unfiled_home, unlinked_home, DiskHomeSource, Home,
    HomeResolver, HomeSource, IndexedHomeSource, ResolverHealth,
};
#[cfg(test)]
pub(crate) use session::session_links;
pub(crate) use session::{
    attribute_ledger_entry, attribute_session, counted_session_links_from_scan,
    prepare_session_scan, split_minutes, SessionHomes, SessionLink,
};

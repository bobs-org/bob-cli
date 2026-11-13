//! Pure mail-note layout, labels, identity parsing, and refresh merging.
//!
//! This module does not perform network or vault I/O. In particular, message
//! bodies arrive through the inert body renderer and are embedded verbatim
//! only inside the renderer-owned callout structure here.

pub(crate) mod hub;
pub(crate) mod index;
pub(crate) mod label;
pub(crate) mod layout;
pub(crate) mod merge;
pub(crate) mod parse;
pub(crate) mod path;
pub(crate) mod receipt;
mod text;

pub(crate) use layout::{message_time, thread_who};
pub(crate) use text::subject_without_prefixes;

/// Extract mailbox addresses with the parser used by mail-note rendering.
/// Display names without an address are deliberately omitted.
pub(crate) fn mailbox_addresses(raw: &str) -> Vec<String> {
    text::parse_mailboxes(raw)
        .into_iter()
        .filter_map(|mailbox| mailbox.address)
        .collect()
}

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteError {
    InvalidIdentity,
    MissingMessageDate,
    MissingRenderedBody,
    Damaged { reason: &'static str },
    InvalidFolder,
    Io,
}

impl fmt::Display for NoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity => {
                f.write_str("mail thread identity is invalid")
            }
            Self::MissingMessageDate => {
                f.write_str("mail thread has a message without a valid date")
            }
            Self::MissingRenderedBody => {
                f.write_str("mail message has no rendered body")
            }
            Self::Damaged { reason } => {
                write!(f, "saved mail note is damaged: {reason}")
            }
            Self::InvalidFolder => {
                f.write_str("mail folder must be a relative path")
            }
            Self::Io => f.write_str("could not read mail note index entry"),
        }
    }
}

impl std::error::Error for NoteError {}

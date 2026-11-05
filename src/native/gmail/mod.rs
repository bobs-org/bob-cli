//! Offline Gmail identity: URL parsing, token decoding, and scanning.
//!
//! Pure module with no I/O. Later phases (gmail-save, gmail-link) consume
//! these verdicts and always confirm a decode against Gmail before writing.

pub(crate) mod ids;

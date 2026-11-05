//! Pure Gmail identity, message data, and body decoding shared by mail consumers.
//!
//! The identity helpers do no I/O; later save/link consumers confirm decoded IDs
//! against Gmail before writing.

pub(crate) mod body;
pub(crate) mod ids;
pub(crate) mod model;

//! Staged in-place overwrite of unannotated references.
//!
//! `recovery` owns the durable overwrite-intent lifecycle consumed by the
//! later transaction phase. The `prepare` phase adds planning and archive
//! transforms beside it.

pub(crate) mod recovery;

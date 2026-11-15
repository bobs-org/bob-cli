//! Guarded `bob bank config update`: envelope, category/group ops, and txn.

pub(crate) mod ops;
pub(crate) mod plan;
pub(crate) mod protocol;
pub(crate) mod run;

#[allow(unused_imports)]
pub(crate) use protocol::{parse_envelope, Envelope, Op, SCHEMA_VERSION};
#[allow(unused_imports)]
pub(crate) use run::{
    dry_run, failed_from_read, print_human, read_envelope_text, run_standalone,
    UpdateFailure, UpdateReport,
};

#[cfg(test)]
mod tests;

//! Internal read-only preparation and report models for `bob ref meet`.
//!
//! The public command stays unregistered until the command-and-docs phase.

mod model;
mod output;
mod prepare;

#[allow(unused_imports)]
pub(super) use model::{
    GuardedRootEdit, MeetingListReport, MeetingPrepareReport, PrepareError,
    PreparedFiling,
};
#[allow(unused_imports)]
pub(super) use output::{
    error_human, error_json, list_human, list_json, prepare_human,
    prepare_json, prepare_report,
};
#[allow(unused_imports)]
pub(super) use prepare::{
    prepare_filing, prepare_filing_from_source, prepare_list,
    MeetingPrepareOptions, MeetingSelection,
};

#[cfg(test)]
mod tests;

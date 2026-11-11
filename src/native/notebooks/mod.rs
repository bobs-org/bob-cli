//! Notebook recognition, list, check, catalogs, sync, and creation.
//!
//! The owned-callout region, first-write placement, and guarded catalog
//! transaction live in [`crate::native::host_catalog`], parameterized by a
//! [`crate::native::host_catalog::RegionSpec`]. Notebooks use the
//! [`crate::native::host_catalog::NOTEBOOKS`] spec; albums and the records
//! index reuse the same engine.

pub(crate) mod catalog;
pub(crate) mod cli;
pub(crate) mod create;
pub(crate) mod create_run;
pub(crate) mod model;
pub(crate) mod output;
pub(crate) mod region;
pub(crate) mod rules;
pub(crate) mod scan;
pub(crate) mod snapshot;
pub(crate) mod sync;
#[cfg(test)]
mod tests;
pub(crate) mod txn;

#[allow(unused_imports)]
pub(crate) use catalog::{plan_catalogs, CatalogGraph, CatalogPlan, FileEdit};
pub(crate) use cli::{
    build_cli_check, build_cli_create, build_cli_list, build_cli_sync,
};
#[allow(unused_imports)]
pub(crate) use create::{
    plan_create, render_new_notebook, CreatePlan, CreateRefusal, CreateRequest,
    NewNotebookInput,
};
#[allow(unused_imports)]
pub(crate) use snapshot::VaultSnapshot;
#[allow(unused_imports)]
pub(crate) use txn::{apply_and_commit, PlannedTxn, ReadDependencies};

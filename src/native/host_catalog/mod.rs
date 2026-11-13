//! Shared host-catalog engine for bob-owned callout regions in host notes.
//!
//! Notebooks, albums, and the records index all plan one owned callout
//! through [`RegionSpec`]. The spec names the callout, heading, optional
//! head title, first-write placement, and whether legacy v1 markers exist.
//! Byte-span edits keep a UTF-8 BOM, LF or CRLF endings, and every
//! surrounding byte. The guarded install lives in [`txn`]: lock-free
//! [`txn::apply_and_commit`] plus standalone [`txn::run_locked`].

pub(crate) mod diff;
pub(crate) mod region;
pub(crate) mod txn;

use std::path::PathBuf;

pub(crate) use diff::{color_diff, unified_diff};
#[allow(unused_imports)]
pub(crate) use region::{
    bare_text, canonical_callout, canonical_head, owned_regions, plan_host,
    plan_host_region, EditState, HostOutcome, HostRegionPlan, LegacySpec,
    OwnedRegion, Placement, RegionDamage, RegionSpec, ALBUMS,
    GENERATED_CATALOGS, LEGACY_BEGIN_MARKER, LEGACY_END_MARKER, NOTEBOOKS,
    PROJECTS,
};
pub(crate) use txn::{
    apply_and_commit, run_locked, staged_rewrites, CommitInfo, PlannedAppend,
    PlannedTxn, ReadDependencies, TxnResult,
};

/// One host's before/after bytes when its owned region must change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileEdit {
    pub(crate) path: PathBuf,
    pub(crate) before: String,
    pub(crate) after: String,
}

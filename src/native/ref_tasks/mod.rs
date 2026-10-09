//! The done/-aware ref-task locator and read-side contracts (phase `ref-locator`).
//!
//! Read-only: [`RefTaskIndex::build`] walks the vault once and locates every
//! `#ref` reading task (v2) plus follow-up `🔖` links. [`RefTaskIndex::select`]
//! picks the live task for one ref note. Later phases (`ref-sync-v2`,
//! `migrate-tasks`) reuse this API; nothing here writes the vault.

mod dates;
mod embed;
pub(crate) mod insert;
mod line;
mod select;
mod v1;
mod walk;

#[cfg(test)]
mod tests;

pub(crate) use dates::close_date;
pub(crate) use embed::{find_managed_embed, ManagedEmbed};
pub(crate) use insert::{insert_ref_task, InsertedRefTask};
pub(crate) use line::{
    allocate_ref_block_id, managed_embed_line, render_ref_task_line,
    sanitize_title_alias, slug_ref_stem, stamp_close_date_any_id,
    strip_blockquote_prefix, OrphanRefTask, RefFollowUp, REF_BLOCK_ID_MAX_LEN,
};
pub(crate) use select::{
    select_for_ref, RefTaskDiagnostic, RefTaskSelection, Selected,
    REF_TASK_DIAGNOSTIC_CODES,
};
pub(crate) use v1::{
    find_trackers, managed_region_line_range, parse_tracker_line, TrackerHit,
};
pub(crate) use walk::{is_conflict_copy_name, LocatedRefTask, RefTaskIndex};

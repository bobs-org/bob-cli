//! The done/-aware ref-task locator and read-side contracts (phase `ref-locator`).
//!
//! Read-only: [`RefTaskIndex::build`] walks the vault once and locates every
//! `#ref` reading task (v2) plus follow-up `🔖` links. [`RefTaskIndex::select`]
//! picks the live task for one ref note. Later phases (`ref-sync-v2`,
//! `migrate-tasks`) reuse this API; nothing here writes the vault.

mod dates;
mod edit;
mod embed;
pub(crate) mod insert;
mod line;
mod select;
mod v1;
mod walk;

#[cfg(test)]
mod tests;

pub(crate) use dates::close_date;
pub(crate) use edit::{
    edit_reading_task_checkbox, locate_original_line, READING_TASK_CHANGED,
};
pub(crate) use embed::find_managed_embed;
pub(crate) use insert::{insert_ref_task, insert_ref_task_with_preferred_id};
pub(crate) use line::{
    allocate_birth_block_id, allocate_ref_block_id, allocate_unique_block_id,
    birth_preview_id, is_embed_task_line, managed_embed_line,
    overwrite_task_alias, render_ref_task_line, sanitize_title_alias,
    slug_ref_stem, strip_blockquote_prefix, task_mark, ReadingIdentity,
};
pub(crate) use select::{
    is_open_mark, select_for_ref, RefTaskDiagnostic, RefTaskSelection,
    Selected, REF_TASK_DIAGNOSTIC_CODES,
};
pub(crate) use v1::{find_trackers, parse_tracker_line, TrackerHit};
pub(crate) use walk::{is_conflict_copy_name, LocatedRefTask, RefTaskIndex};

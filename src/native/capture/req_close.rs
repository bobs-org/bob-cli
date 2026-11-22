//! Capture adapter for pre-close requirement forests and the atomic
//! carry/drop write pass.
//!
//! Tree close already binds each managed Done owner to its pre-close
//! snapshot. This adapter copies those owners onto capture close
//! results so successor pre-gates cannot drop a req-only audit, and
//! so req-less closes still add no extra vault scan.
//!
//! The write pass beside the existing offer-verdict pass loads the guarded
//! staged snapshot, plans every structural requirement verdict atomically,
//! and stages each split postimage through the batch planner. It never
//! writes directly to disk and adds no separate commit path: the single
//! `write_staged_files` commit still guards external edits.

use std::path::Path;

use chrono::NaiveDate;

use super::{
    batch::CaptureBatchPlanner, output::CaptureError,
    relations_merge::load_staged_snapshot,
};
use crate::native::{
    note_tasks,
    task_complete::{
        req_close::{groups_from_owners, ReqCloseGroup, ReqCloseOwner},
        req_verdicts::ReqVerdictSelection,
        req_writes::{plan_req_writes, ReqWritePlan},
    },
    task_split::plan::SplitSettings,
};

/// Requirement groups for a closing set, in the owners' close order.
pub(super) fn groups_for_close(owners: &[ReqCloseOwner]) -> Vec<ReqCloseGroup> {
    groups_from_owners(owners)
}

/// Plan every structural requirement verdict on the staged snapshot.
///
/// Met-only selections write no bytes and perform no vault scan. Otherwise
/// the guarded staged snapshot is loaded (file-set and read guards
/// included; the full vault loads only because a carry may need link
/// repair), the pure plan runs, and each returned postimage is staged
/// through the planner bound to the exact snapshot it was planned from.
/// Any refusal stages nothing new for this pass, so every vault file keeps
/// its original bytes.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn plan_req_write_pass(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owners: &[ReqCloseOwner],
    selection: &ReqVerdictSelection,
    today: &str,
    date: NaiveDate,
) -> Result<ReqWritePlan, CaptureError> {
    use crate::native::task_complete::req_verdicts::ReqVerdict;

    let structural = selection.verdicts.iter().any(|verdict| {
        matches!(
            verdict.verdict,
            ReqVerdict::Drop | ReqVerdict::Carry | ReqVerdict::CarryPulled
        )
    });
    if !structural {
        let empty_snapshot = crate::native::task_move::VaultSnapshot {
            notes: std::collections::BTreeMap::new(),
        };
        let task_settings = note_tasks::read_settings(bob_dir);
        let plan = plan_req_writes(
            &empty_snapshot,
            owners,
            selection,
            today,
            date,
            &SplitSettings::default(),
            &task_settings,
        )
        .map_err(|error| {
            CaptureError::io_code(error.to_string(), error.code())
        })?;
        debug_assert!(plan.postimages.is_empty());
        return Ok(plan);
    }
    let snapshot = load_staged_snapshot(planner, bob_dir)?;
    let task_settings = note_tasks::read_settings(bob_dir);
    let plan = plan_req_writes(
        &snapshot,
        owners,
        selection,
        today,
        date,
        &SplitSettings::default(),
        &task_settings,
    )
    .map_err(|error| {
        CaptureError::io_code(
            format!(
                "requirement write refused: {}",
                error_code_message(&error)
            ),
            error.code(),
        )
        .with_typed(
            crate::native::capture::output::CaptureFailureKind::NoteChanged,
        )
    })?;
    for (relative, postimage) in &plan.postimages {
        let absolute = bob_dir.join(relative);
        let consumed = snapshot.notes.get(relative).cloned();
        planner.stage_snapshot_postimage(
            &absolute,
            consumed,
            postimage.clone(),
        )?;
    }
    return Ok(plan);

    fn error_code_message(
        error: &crate::native::task_complete::req_writes::ReqWriteError,
    ) -> String {
        match error {
            crate::native::task_complete::req_writes::ReqWriteError::Stale { selectors } => {
                format!("stale requirement pick: {}", selectors.join(", "))
            }
            crate::native::task_complete::req_writes::ReqWriteError::Refused { code, message } => {
                format!("{code}: {message}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn met_only_write_pass_needs_no_vault_scan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bob_dir = dir.path();
        let mut planner = CaptureBatchPlanner::default();
        let owners = Vec::new();
        let selection = ReqVerdictSelection {
            groups: Vec::new(),
            numbering: crate::native::task_complete::req_verdicts::ReqCardNumbering::default(),
            verdicts: Vec::new(),
            jump: None,
        };
        let plan = plan_req_write_pass(
            &mut planner,
            bob_dir,
            &owners,
            &selection,
            "2026-11-22",
            NaiveDate::from_ymd_opt(2026, 11, 22).expect("date"),
        )
        .expect("met-only passes");
        assert!(plan.postimages.is_empty());
        assert!(plan.outcomes.is_empty());
    }
}

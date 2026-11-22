//! Atomic carry and drop planning for the requirement close card.
//!
//! This module adapts [`crate::native::task_split::carry_req`] and
//! [`crate::native::task_block_layout::writers::drop_req`] to guarded
//! staged capture snapshots. It validates every structural choice against
//! the frozen pre-close forest, applies disjoint carries bottom-to-top and
//! drops next, composes coordinates after each edit, and returns exact
//! outcome receipts without adding another commit path.
//!
//! The caller owns atomicity: this function mutates only its in-memory
//! `working` copy and returns postimages. The capture adapter stages those
//! postimages through `CaptureBatchPlanner`; on any refusal the caller
//! stages nothing, so every vault file keeps its original bytes.
//!
//! Staleness policy: the owner's Done transition (`[ ]` to `[x]` plus the
//! completion field) is the one known internal transformation and is
//! tolerated. Every requirement row still needs its exact frozen identity
//! (sibling path, depth, clean text, and row-key suffix) as pending in the
//! current snapshot. Anything else — a moved or re-indented row, a changed
//! sibling order, or a missing file — refuses as `req_close_stale`. Row
//! lookup is by exact text and path, never by fuzzy label matching.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::req_close::{copy, ReqCloseOwner};
use super::req_verdicts::{ReqVerdict, ReqVerdictSelection};
use crate::native::{
    note_tasks::{self, NoteTaskSettings},
    task_block_layout::{drop_req, ReqTarget},
    task_move::VaultSnapshot,
    task_reqs::{self, EffectiveReqState},
    task_split::{
        carry::{LineMove, ReqRef},
        plan::SplitSettings,
    },
};

/// A task created by one carried requirement branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqCarriedTask {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: String,
}

/// One structural write outcome for placement and reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqWriteOutcome {
    pub(crate) owner_task_key: String,
    pub(crate) owner_key: String,
    pub(crate) row_key: String,
    pub(crate) path: String,
    pub(crate) depth: usize,
    pub(crate) text: String,
    pub(crate) verdict: ReqVerdict,
    pub(crate) reason: Option<String>,
    pub(crate) pulled: bool,
    pub(crate) receipt: String,
    pub(crate) carried: Option<ReqCarriedTask>,
    /// Descendant line moves reported by the underlying carry, shifted to
    /// final postimage coordinates (see [`ReqWritePlan::moves_hint`]).
    pub(crate) moves: Vec<LineMove>,
}

/// Aggregated Met receipt for one owner. Explicit Met rows and untouched
/// pending rows count here; descendants folded under a carried or dropped
/// ancestor never do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqOwnerMet {
    pub(crate) owner_task_key: String,
    pub(crate) owner_key: String,
    pub(crate) count: usize,
    pub(crate) receipt: String,
}

/// The atomic write plan: postimages plus per-row outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ReqWritePlan {
    pub(crate) outcomes: Vec<ReqWriteOutcome>,
    pub(crate) met: Vec<ReqOwnerMet>,
    pub(crate) postimages: BTreeMap<PathBuf, String>,
    /// Every `(old, new)` descendant move from all carries, in final
    /// coordinates where destination shifts could be resolved. Placement
    /// re-resolves pinned landings by block identity; this hint covers the
    /// common same-note case.
    pub(crate) moves_hint: Vec<LineMove>,
}

/// Refusal from the atomic write pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReqWriteError {
    Stale { selectors: Vec<String> },
    Refused { code: String, message: String },
}

impl std::fmt::Display for ReqWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale { selectors } => {
                write!(
                    formatter,
                    "stale requirement pick: {}",
                    selectors.join(", ")
                )
            }
            Self::Refused { code, message } => {
                write!(formatter, "{code}: {message}")
            }
        }
    }
}

impl ReqWriteError {
    pub(crate) fn code(&self) -> &str {
        match self {
            Self::Stale { .. } => "req_close_stale",
            Self::Refused { code, .. } => code.as_str(),
        }
    }

    fn stale(selector: impl Into<String>) -> Self {
        Self::Stale {
            selectors: vec![selector.into()],
        }
    }

    fn refused(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Refused {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Plan every structural requirement verdict atomically.
///
/// `snapshot` is the current staged snapshot (usually post-close: the owner
/// already reads `[x]` while its reqs are still open). `owners` is the
/// frozen pre-close forest in close order. `selection` holds the already
/// overlap-checked explicit verdicts. Met verdicts write no bytes; carries
/// run bottom-to-top, then drops, so earlier edits never shift a row that
/// is still waiting for its own write.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_req_writes(
    snapshot: &VaultSnapshot,
    owners: &[ReqCloseOwner],
    selection: &ReqVerdictSelection,
    today: &str,
    date: NaiveDate,
    split_settings: &SplitSettings,
    task_settings: &NoteTaskSettings,
) -> Result<ReqWritePlan, ReqWriteError> {
    let frozen_by_key: BTreeMap<&str, &ReqCloseOwner> = owners
        .iter()
        .map(|owner| (owner.key.as_str(), owner))
        .collect();
    // Structural verdicts only; Met writes no bytes.
    let mut carries: Vec<_> = selection
        .verdicts
        .iter()
        .filter(|verdict| {
            matches!(
                verdict.verdict,
                ReqVerdict::Carry | ReqVerdict::CarryPulled
            )
        })
        .collect();
    let mut drops: Vec<_> = selection
        .verdicts
        .iter()
        .filter(|verdict| verdict.verdict == ReqVerdict::Drop)
        .collect();
    // Bottom-to-top per note so earlier edits never shift a waiting row.
    carries.sort_by(|left, right| {
        (right.row.owner_path.clone(), right.row.line)
            .cmp(&(left.row.owner_path.clone(), left.row.line))
    });
    drops.sort_by(|left, right| {
        (right.row.owner_path.clone(), right.row.line)
            .cmp(&(left.row.owner_path.clone(), left.row.line))
    });

    let mut working = snapshot.notes.clone();
    let mut outcomes = Vec::new();
    // Intermediate carry records for final coordinate repair.
    struct PendingCarry {
        outcome_index: usize,
        dest_path: PathBuf,
        intermediate_line: usize,
        block_id: String,
    }
    let mut pending: Vec<PendingCarry> = Vec::new();

    for resolved in carries {
        let frozen_owner = frozen_by_key
            .get(resolved.row.owner_key.as_str())
            .copied()
            .ok_or_else(|| ReqWriteError::stale(resolved.row.key.clone()))?;
        let owner_path = PathBuf::from(&resolved.row.owner_path);
        let current = working
            .get(&owner_path)
            .ok_or_else(|| ReqWriteError::stale(resolved.row.key.clone()))?;
        let node = find_pending_node(
            current,
            task_settings,
            &resolved.row.key,
            &resolved.row.path,
            resolved.row.depth,
            &resolved.row.text,
        )
        .ok_or_else(|| ReqWriteError::stale(resolved.row.key.clone()))?;
        // The frozen owner identity must still name this note's owner:
        // same block when the owner carries one, else same task text.
        verify_owner_match(current, task_settings, frozen_owner, &node)
            .map_err(|_| ReqWriteError::stale(resolved.row.key.clone()))?;
        let owner_line = node.owner_line + 1;
        let working_snapshot = VaultSnapshot {
            notes: working.clone(),
        };
        let carry = crate::native::task_split::carry::carry_req(
            &working_snapshot,
            &owner_path,
            owner_line,
            ReqRef::RowKey(resolved.row.key.as_str()),
            today,
            split_settings,
            task_settings,
        )
        .map_err(|error| {
            ReqWriteError::refused(error.code.to_string(), error.message)
        })?;
        for (path, postimage) in &carry.plan.postimages {
            working.insert(path.clone(), postimage.clone());
        }
        let pulled = resolved.verdict == ReqVerdict::CarryPulled;
        let receipt = copy::carry_receipt(&resolved.row.text, pulled);
        outcomes.push(ReqWriteOutcome {
            owner_task_key: resolved.row.owner_task_key.clone(),
            owner_key: resolved.row.owner_key.clone(),
            row_key: resolved.row.key.clone(),
            path: resolved.row.path.clone(),
            depth: resolved.row.depth,
            text: resolved.row.text.clone(),
            verdict: resolved.verdict,
            reason: None,
            pulled,
            receipt,
            carried: Some(ReqCarriedTask {
                path: display_relative(&carry.path),
                line: carry.line,
                block_id: carry.block_id.clone(),
            }),
            moves: carry.moves.clone(),
        });
        let outcome_index = outcomes.len() - 1;
        pending.push(PendingCarry {
            outcome_index,
            dest_path: PathBuf::from(&carry.path),
            intermediate_line: carry.line,
            block_id: carry.block_id.clone(),
        });
    }

    for resolved in drops {
        let frozen_owner = frozen_by_key
            .get(resolved.row.owner_key.as_str())
            .copied()
            .ok_or_else(|| ReqWriteError::stale(resolved.row.key.clone()))?;
        let owner_path = PathBuf::from(&resolved.row.owner_path);
        let current = working
            .get(&owner_path)
            .ok_or_else(|| ReqWriteError::stale(resolved.row.key.clone()))?;
        let node = find_pending_node(
            current,
            task_settings,
            &resolved.row.key,
            &resolved.row.path,
            resolved.row.depth,
            &resolved.row.text,
        )
        .ok_or_else(|| ReqWriteError::stale(resolved.row.key.clone()))?;
        verify_owner_match(current, task_settings, frozen_owner, &node)
            .map_err(|_| ReqWriteError::stale(resolved.row.key.clone()))?;
        let spans = crate::native::capture::line_spans(current);
        let preimage = spans
            .get(node.line_index)
            .map(|span| span.text.to_string())
            .ok_or_else(|| ReqWriteError::stale(resolved.row.key.clone()))?;
        let reason = resolved.reason.clone().unwrap_or_default();
        let written = drop_req(
            current,
            ReqTarget {
                line_index: node.line_index,
                preimage: &preimage,
            },
            date,
            &reason,
            task_settings,
        )
        .map_err(|refusal| {
            ReqWriteError::refused(refusal.code.to_string(), refusal.message)
        })?;
        working.insert(owner_path.clone(), written.updated.clone());
        let reason = normalize_reason(resolved.reason.as_deref());
        let receipt = copy::drop_receipt(&resolved.row.text, reason.as_deref());
        outcomes.push(ReqWriteOutcome {
            owner_task_key: resolved.row.owner_task_key.clone(),
            owner_key: resolved.row.owner_key.clone(),
            row_key: resolved.row.key.clone(),
            path: resolved.row.path.clone(),
            depth: resolved.row.depth,
            text: resolved.row.text.clone(),
            verdict: ReqVerdict::Drop,
            reason,
            pulled: false,
            receipt,
            carried: None,
            moves: Vec::new(),
        });
    }

    // Repair created-task lines and descendant moves to final coordinates:
    // later inserts in the same destination file push earlier tasks down.
    // Re-locating by block identity is exact; label matching is never used.
    for carry in &pending {
        let Some(final_text) = working.get(&carry.dest_path) else {
            continue;
        };
        let scan = note_tasks::scan(final_text, task_settings);
        let Some(task) = scan.tasks().iter().find(|task| {
            task.block_id.as_deref() == Some(carry.block_id.as_str())
        }) else {
            continue;
        };
        let final_line = task.line_index + 1;
        let delta = final_line as isize - carry.intermediate_line as isize;
        if delta == 0 {
            continue;
        }
        if let Some(outcome) = outcomes.get_mut(carry.outcome_index) {
            if let Some(created) = outcome.carried.as_mut() {
                created.line = (created.line as isize + delta).max(1) as usize;
            }
            for line_move in &mut outcome.moves {
                line_move.to = (line_move.to as isize + delta).max(1) as usize;
            }
        }
    }
    let moves_hint = outcomes
        .iter()
        .flat_map(|outcome| outcome.moves.iter().copied())
        .collect();

    let met = met_receipts(owners, selection);
    let postimages = diff_postimages(&snapshot.notes, &working);
    Ok(ReqWritePlan {
        outcomes,
        met,
        postimages,
        moves_hint,
    })
}

/// Pending rows covered by structural verdicts never count as Met.
fn met_receipts(
    owners: &[ReqCloseOwner],
    selection: &ReqVerdictSelection,
) -> Vec<ReqOwnerMet> {
    let mut covered: BTreeMap<&str, usize> = BTreeMap::new();
    for verdict in &selection.verdicts {
        if matches!(
            verdict.verdict,
            ReqVerdict::Drop | ReqVerdict::Carry | ReqVerdict::CarryPulled
        ) {
            *covered.entry(verdict.row.owner_key.as_str()).or_default() +=
                1 + verdict.row.pending_descendants;
        }
    }
    // Explicit structural choices that name a folded descendant are refused
    // upstream; still, never let a descendant double-count through two
    // ancestors. Overlap is rejected, so a plain sum is exact here.
    let mut met = Vec::new();
    for owner in owners {
        let pending = owner.pending_count();
        let taken = covered.get(owner.key.as_str()).copied().unwrap_or(0);
        let count = pending.saturating_sub(taken);
        if count == 0 {
            continue;
        }
        // Owners without pending rows never reach here, and owners whose
        // every pending row was carried or dropped report no Met line.
        if owner.group().is_none() && pending == 0 {
            continue;
        }
        met.push(ReqOwnerMet {
            owner_task_key: owner.task_key.clone(),
            owner_key: owner.key.clone(),
            count,
            receipt: copy::met_receipt(count, &owner.text),
        });
    }
    met
}

fn diff_postimages(
    before: &BTreeMap<PathBuf, String>,
    after: &BTreeMap<PathBuf, String>,
) -> BTreeMap<PathBuf, String> {
    let mut changed = BTreeMap::new();
    let mut keys = BTreeSet::new();
    keys.extend(before.keys().cloned());
    keys.extend(after.keys().cloned());
    for key in keys {
        if before.get(&key) != after.get(&key)
            && let Some(postimage) = after.get(&key)
        {
            changed.insert(key, postimage.clone());
        }
    }
    changed
}

fn row_suffix(full_key: &str) -> &str {
    full_key
        .split_once("/req/")
        .map_or(full_key, |(_, rest)| rest)
}

fn find_pending_node(
    contents: &str,
    settings: &NoteTaskSettings,
    full_key: &str,
    path: &str,
    depth: usize,
    text: &str,
) -> Option<task_reqs::ReqNode> {
    let wanted = row_suffix(full_key);
    task_reqs::note_req_forest(contents, settings)
        .into_iter()
        .find(|node| {
            node.state == EffectiveReqState::Pending
                && node.text == text
                && node.depth == depth
                && task_reqs::req_path(node) == path
                && row_suffix(&task_reqs::req_row_key(
                    "", &node.path, &node.text,
                )) == wanted
        })
}

fn verify_owner_match(
    contents: &str,
    settings: &NoteTaskSettings,
    frozen: &ReqCloseOwner,
    node: &task_reqs::ReqNode,
) -> Result<(), ()> {
    let scan = note_tasks::scan(contents, settings);
    let Some(owner) = scan.task_at(node.owner_line) else {
        return Err(());
    };
    if !frozen.block_id.is_empty() {
        if owner.block_id.as_deref() == Some(frozen.block_id.as_str()) {
            return Ok(());
        }
        return Err(());
    }
    if owner.description == frozen.text {
        return Ok(());
    }
    Err(())
}

fn normalize_reason(reason: Option<&str>) -> Option<String> {
    reason.and_then(|reason| {
        let normalized =
            reason.split_whitespace().collect::<Vec<_>>().join(" ");
        (!normalized.is_empty()).then_some(normalized)
    })
}

fn display_relative(path: &str) -> String {
    Path::new(path)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod self_tests {
    use super::*;

    #[test]
    fn row_suffix_splits_on_first_req_marker() {
        assert_eq!(row_suffix("owner/req/1~abcdef12"), "1~abcdef12");
    }
}
